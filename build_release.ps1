[CmdletBinding()]
param(
    # A new directory for the staged release. Relative paths use the project root.
    [string]$Output,
    # Add the verified Qwen3-ASR and Hy-MT2 GGUF packages for an offline release.
    [switch]$IncludeModels,
    # Verified ONNX Runtime 1.28 core DLL. GPU providers are downloaded later.
    [string]$OnnxRuntimeCpu,
    # Visual Studio VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT; auto-detected with vswhere.
    [string]$VcRuntimeDirectory,
    # Optional prebuilt plugin packages; their sources are built independently.
    [string]$PluginsDirectory,
    # Validate existing release inputs without building, downloading, or staging.
    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
$projectRoot = $PSScriptRoot
$workspaceManifest = Join-Path $projectRoot 'Cargo.toml'
$configPath = Join-Path $projectRoot 'config.json'
$vadModel = Join-Path $projectRoot 'models\silero-vad\src\silero_vad\data\silero_vad.onnx'
$speakerModel = Join-Path $projectRoot 'models\3D-Speaker-ERes2NetV2\speaker_embedding.onnx'
$seedDatabase = Join-Path $projectRoot 'XR-Corpus\corpora\default.sqlite'
$cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
$packagerPath = Join-Path $projectRoot 'target\release\xrtranslate-packager.exe'
$installerPath = Join-Path $projectRoot 'target\release\xrtranslate-installer.exe'
$runtimeAssetCache = Join-Path $projectRoot '.temp\runtime-assets'

$expectedOnnxBytes = 16277856
$expectedLicenseBytes = 1094
$expectedNoticesBytes = 331175
# Matches RuntimeLayout::ONNX_CPU_CORE_WIN_SOURCE_ARCHIVE.
$onnxCpuSourceArchive = 'onnxruntime-win-x64-gpu_cuda13-1.28.0.zip'

function Resolve-VcRuntimeDirectory {
    param([string]$Directory)

    $explicitDirectory = -not [string]::IsNullOrWhiteSpace($Directory)
    $candidateDirectories = @()
    if ($explicitDirectory) {
        $candidateDirectories = @([System.IO.Path]::GetFullPath($Directory))
    } else {
        $vswherePath = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
        if (-not (Test-Path -LiteralPath $vswherePath -PathType Leaf)) {
            throw 'Visual Studio discovery (vswhere.exe) was not found. Pass -VcRuntimeDirectory pointing to a Visual Studio VC/Redist/MSVC/<version>/x64/Microsoft.VC*.CRT directory. System32 DLLs are not release inputs.'
        }
        $installations = @(& $vswherePath -all -products '*' -version '[17.0,)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
        if ($LASTEXITCODE -ne 0) {
            throw 'Could not discover Visual Studio redistributable files. Pass -VcRuntimeDirectory explicitly.'
        }
        foreach ($installation in $installations) {
            $redistRoot = Join-Path $installation 'VC\Redist\MSVC'
            if (Test-Path -LiteralPath $redistRoot -PathType Container) {
                foreach ($versionDirectory in Get-ChildItem -LiteralPath $redistRoot -Directory) {
                    $x64Directory = Join-Path $versionDirectory.FullName 'x64'
                    if (Test-Path -LiteralPath $x64Directory -PathType Container) {
                        $candidateDirectories += Get-ChildItem -LiteralPath $x64Directory -Directory -Filter 'Microsoft.VC*.CRT' |
                            Select-Object -ExpandProperty FullName
                    }
                }
            }
        }
    }

    $available = @(
        foreach ($candidate in $candidateDirectories) {
            $layout = [regex]::Match($candidate, '[\\/]VC[\\/]Redist[\\/]MSVC[\\/](?<version>[^\\/]+)[\\/]x64[\\/]Microsoft\.VC[0-9]+\.CRT[\\/]?$', [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)
            if (-not $layout.Success) {
                throw 'VC runtime files must come from a Visual Studio VC/Redist/MSVC/<version>/x64/Microsoft.VC*.CRT directory. Do not use System32 or debug runtime files.'
            }
            if (-not (Test-Path -LiteralPath $candidate -PathType Container)) {
                throw "VC runtime redistributable directory was not found: $candidate"
            }
            # Directory discovery belongs here. Required DLL names, PE checks,
            # and integrity metadata are owned by the native release packager.
            $redistVersion = [version]'0.0'
            $parsedVersion = $null
            if ([version]::TryParse($layout.Groups['version'].Value, [ref]$parsedVersion)) {
                $redistVersion = $parsedVersion
            }
            [pscustomobject]@{
                Path = $candidate
                Version = $redistVersion
            }
        }
    )
    $selected = $available | Sort-Object Version -Descending | Select-Object -First 1
    if ($null -eq $selected) {
        throw 'No Visual Studio x64 redistributable CRT directory was found. Ensure the VS C++ redistributable files are present, or pass -VcRuntimeDirectory <VC/Redist/MSVC/version/x64/Microsoft.VC*.CRT>. No system dependencies will be installed.'
    }
    return $selected.Path
}

if (-not (Test-Path -LiteralPath $workspaceManifest -PathType Leaf)) {
    throw "Rust workspace manifest was not found: $workspaceManifest"
}
if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
    throw "Release configuration was not found: $configPath"
}
if (-not (Test-Path -LiteralPath $seedDatabase -PathType Leaf)) {
    throw "Default terminology database was not found: $seedDatabase. Run git submodule update --init XR-Corpus."
}

if ([string]::IsNullOrWhiteSpace($Output)) {
    $version = "0.1.0"
    $manifestContent = Get-Content -LiteralPath $workspaceManifest -Raw
    if ($manifestContent -match 'version\s*=\s*"([^"]+)"') {
        $version = $matches[1]
    }
    $Output = Join-Path $projectRoot "dist\XRTranslate-v$version-win-x64"
} elseif (-not [System.IO.Path]::IsPathRooted($Output)) {
    $Output = Join-Path $projectRoot $Output
}
$Output = [System.IO.Path]::GetFullPath($Output)
if (Test-Path -LiteralPath $Output) {
    throw "Release output already exists. Choose a new -Output path: $Output"
}

if ($ValidateOnly -and -not (Test-Path -LiteralPath $packagerPath -PathType Leaf)) {
    throw "Validation requires an existing native release packager: $packagerPath. -ValidateOnly never builds binaries."
}
if (-not $ValidateOnly) {
    if (Test-Path -LiteralPath $cargoPath) {
        $cargo = $cargoPath
    } elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
        $cargo = 'cargo'
    } else {
        throw 'Cargo was not found. Install Rust with rustup, then restart PowerShell.'
    }
}
$VcRuntimeDirectory = Resolve-VcRuntimeDirectory -Directory $VcRuntimeDirectory
Write-Host "Using app-local Visual C++ runtime from redistributable files: $VcRuntimeDirectory"


function Export-ZipEntry {
    param(
        [string]$ArchivePath,
        [string]$EntryPath,
        [string]$OutFile,
        [long]$ExpectedBytes
    )
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [System.IO.Compression.ZipFile]::OpenRead($ArchivePath)
    try {
        $entry = $archive.GetEntry($EntryPath)
        if ($null -eq $entry) {
            throw "Entry $EntryPath was not found in $ArchivePath"
        }
        if ($entry.Length -ne $ExpectedBytes) {
            throw "Size mismatch for archive entry $EntryPath : expected $ExpectedBytes, got $($entry.Length)"
        }
        $outDir = [System.IO.Path]::GetDirectoryName($OutFile)
        New-Item -ItemType Directory -Path $outDir -Force | Out-Null
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $OutFile, $true)
    } finally {
        $archive.Dispose()
    }
    $actualBytes = (Get-Item -LiteralPath $OutFile).Length
    if ($actualBytes -ne $ExpectedBytes) {
        throw "Size mismatch for extracted $EntryPath : expected $ExpectedBytes, got $actualBytes"
    }
}

$explicitOnnxCore = -not [string]::IsNullOrWhiteSpace($OnnxRuntimeCpu)
if (-not $explicitOnnxCore) {
    $OnnxRuntimeCpu = Join-Path $runtimeAssetCache 'onnxruntime\cpu\onnxruntime.dll'
}
$OnnxRuntimeCpu = [System.IO.Path]::GetFullPath($OnnxRuntimeCpu)
if ($explicitOnnxCore -and (
    -not (Test-Path -LiteralPath $OnnxRuntimeCpu -PathType Leaf) -or
    (Get-Item -LiteralPath $OnnxRuntimeCpu).Length -ne $expectedOnnxBytes
)) {
    throw "The supplied ONNX Runtime core is missing or has an unexpected size: $OnnxRuntimeCpu"
}

if (-not $ValidateOnly) {
    & python (Join-Path $projectRoot 'scripts/prepare-release-cache.py') --target windows-x86_64 --cache $runtimeAssetCache --runtime-archive $onnxCpuSourceArchive
    if ($LASTEXITCODE -ne 0) { throw 'Release resource preparation failed.' }
}

$onnxRuntimeLicense = Join-Path $runtimeAssetCache 'licenses\onnxruntime\LICENSE'
$onnxRuntimeNotices = Join-Path $runtimeAssetCache 'licenses\onnxruntime\ThirdPartyNotices.txt'
$onnxArchiveRoot = [System.IO.Path]::GetFileNameWithoutExtension($onnxCpuSourceArchive)
$runtimeFiles = @(
    @{ Entry = 'lib/onnxruntime.dll'; Path = $OnnxRuntimeCpu; Bytes = $expectedOnnxBytes },
    @{ Entry = 'LICENSE'; Path = $onnxRuntimeLicense; Bytes = $expectedLicenseBytes },
    @{ Entry = 'ThirdPartyNotices.txt'; Path = $onnxRuntimeNotices; Bytes = $expectedNoticesBytes }
)
foreach ($file in $runtimeFiles) {
    if ((Test-Path -LiteralPath $file.Path -PathType Leaf) -and
        (Get-Item -LiteralPath $file.Path).Length -eq $file.Bytes) {
        continue
    }
    if ($ValidateOnly) {
        throw "Required release resource is missing or has an unexpected size: $($file.Path). -ValidateOnly never downloads or extracts files."
    }
    Export-ZipEntry -ArchivePath (Join-Path $runtimeAssetCache $onnxCpuSourceArchive) -EntryPath "$onnxArchiveRoot/$($file.Entry)" -OutFile $file.Path -ExpectedBytes $file.Bytes
}
Write-Host "Using verified ONNX Runtime core: $OnnxRuntimeCpu"

if (-not $ValidateOnly) {
    . (Join-Path $projectRoot 'scripts/windows-build-env.ps1')
    Initialize-WindowsSpeechLibraries -ProjectRoot $projectRoot
    $buildArguments = @(
        'build', '--locked', '--manifest-path', $workspaceManifest, '--target-dir', (Join-Path $projectRoot 'target'), '--release',
        '--package', 'rust-client',
        '--package', 'xrtranslate-backend',
        '--package', 'xrtranslate-installer',
        '--package', 'xrtranslate-updater',
        '--package', 'xrtranslate-account-login',
        '--package', 'xrtranslate-packager',
        '--package', 'xrtranslate-plugin',
        '--features', 'rust-client/mpv,xrtranslate-backend/managed-ort,xrtranslate-plugin/host'
    )

    Write-Host 'Building native release binaries...'
    & $cargo @buildArguments
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    & $cargo build --locked --manifest-path (Join-Path $projectRoot 'XR-Corpus\Cargo.toml') --target-dir (Join-Path $projectRoot 'target') --release --package xr-corpus-server
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }

    Write-Host 'Preparing verified base ONNX models from the shared resource catalogue...'
    & $installerPath --config $configPath prepare-resources --output $projectRoot --cache (Join-Path $runtimeAssetCache 'downloads')
    if ($LASTEXITCODE -ne 0) {
        exit $LASTEXITCODE
    }
}


if (-not (Test-Path -LiteralPath $vadModel)) {
    throw "Silero VAD model was not found: $vadModel"
}
if (-not (Test-Path -LiteralPath $speakerModel)) {
    throw "ERes2NetV2 speaker ONNX model was not found: $speakerModel"
}

$packageArguments = @(
    '--rust-client-bin', (Join-Path $projectRoot 'target\release\rust-client.exe'),
    '--backend-bin', (Join-Path $projectRoot 'target\release\xrtranslate-backend.exe'),
    '--corpus-bin', (Join-Path $projectRoot 'target\release\xr-corpus-server.exe'),
    '--installer-bin', (Join-Path $projectRoot 'target\release\xrtranslate-installer.exe'),
    '--updater-bin', (Join-Path $projectRoot 'target\release\xrtranslate-updater.exe'),
    '--account-login-bin', (Join-Path $projectRoot 'target\release\xrtranslate-account-login.exe'),
    '--config', $configPath,
    '--resources-dir', (Join-Path $projectRoot 'rust-client\resources'),
    '--seed-database', $seedDatabase,
    '--vad-model', $vadModel,
    '--speaker-model', $speakerModel,
    '--onnx-runtime-cpu', $OnnxRuntimeCpu,
    '--onnx-runtime-license', $onnxRuntimeLicense,
    '--onnx-runtime-notices', $onnxRuntimeNotices,
    '--vc-runtime-dir', $VcRuntimeDirectory,
    '--output', $Output
)
if ($PluginsDirectory) {
    foreach ($directory in Get-ChildItem -LiteralPath $PluginsDirectory -Directory) {
        if (Test-Path -LiteralPath (Join-Path $directory.FullName 'plugin.json')) {
            $packageArguments += @('--plugin-directory', $directory.FullName)
        }
    }
}
if ($IncludeModels) {
    $packageArguments += '--include-models'
}
if ($ValidateOnly) {
    $packageArguments += '--check'
}

if ($ValidateOnly) {
    Write-Host 'Validating existing native release inputs (no build or download)...'
} else {
    Write-Host 'Preparing the native release package...'
}
& $packagerPath @packageArguments
if ($LASTEXITCODE -ne 0 -or $ValidateOnly) {
    exit $LASTEXITCODE
}

Write-Host "Release directory is ready: $Output"
