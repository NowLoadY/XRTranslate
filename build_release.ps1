[CmdletBinding()]
param(
    # A new directory for the staged release. Relative paths use the project root.
    [string]$Output,
    # Add the verified Qwen3-ASR and Hy-MT2 GGUF packages for an offline release.
    [switch]$IncludeModels,
    # Verified ONNX Runtime 1.28 core DLL. GPU providers are downloaded later.
    [string]$OnnxRuntimeCpu,
    # VS 2022 VC/Redist/MSVC/*/x64/Microsoft.VC*.CRT; auto-detected with vswhere.
    [string]$VcRuntimeDirectory,
    # Validate existing release inputs without building, downloading, or staging.
    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
$projectRoot = $PSScriptRoot
$workspaceManifest = Join-Path $projectRoot 'Cargo.toml'
$configPath = Join-Path $projectRoot 'config.json'
$vadModel = Join-Path $projectRoot 'models\silero-vad\src\silero_vad\data\silero_vad.onnx'
$speakerModel = Join-Path $projectRoot 'models\3D-Speaker-ERes2NetV2\speaker_embedding.onnx'
$denoiseModel = Join-Path $projectRoot 'models\gtcrn\gtcrn_simple.onnx'
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
            throw 'Visual Studio discovery (vswhere.exe) was not found. Pass -VcRuntimeDirectory pointing to a VS 2022 VC/Redist/MSVC/<version>/x64/Microsoft.VC*.CRT directory. System32 DLLs are not release inputs.'
        }
        $installations = @(& $vswherePath -all -products '*' -version '[17.0,18.0)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath)
        if ($LASTEXITCODE -ne 0) {
            throw 'Could not discover Visual Studio 2022 redistributable files. Pass -VcRuntimeDirectory explicitly.'
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
                throw 'VC runtime files must come from a VS 2022 VC/Redist/MSVC/<version>/x64/Microsoft.VC*.CRT directory. Do not use System32 or debug runtime files.'
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
        throw 'No VS 2022 x64 redistributable CRT directory was found. Ensure the VS C++ redistributable files are present, or pass -VcRuntimeDirectory <VC/Redist/MSVC/version/x64/Microsoft.VC*.CRT>. No system dependencies will be installed.'
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

if (-not $ValidateOnly) {
    $buildArguments = @(
        'build', '--locked', '--manifest-path', $workspaceManifest, '--target-dir', (Join-Path $projectRoot 'target'), '--release',
        '--package', 'rust-client',
        '--package', 'xrtranslate-backend',
        '--package', 'xrtranslate-installer',
        '--package', 'xrtranslate-updater',
        '--package', 'xrtranslate-packager',
        '--features', 'rust-client/mpv,xrtranslate-backend/managed-ort'
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

function Get-OnnxCpuSourceArchive {
    $releaseConfig = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
    $matchingDownloads = @($releaseConfig.model_manager.onnxruntime.downloads | Where-Object {
        $_.name -eq $onnxCpuSourceArchive -and $_.target -eq 'windows-x86_64'
    })
    if ($matchingDownloads.Count -ne 1) {
        throw "Expected exactly one Windows ONNX core source archive in config.json: $onnxCpuSourceArchive"
    }
    $download = $matchingDownloads[0]
    $archivePath = Join-Path $runtimeAssetCache $download.name
    Write-Host 'Fetching the verified ONNX Runtime source archive for its CPU core and license files...'
    & $cargo run --locked --manifest-path $workspaceManifest --target-dir (Join-Path $projectRoot 'target') --release --package xrtranslate-download --example fetch -- $download.url $download.bytes $archivePath | Out-Host
    if ($LASTEXITCODE -ne 0) {
        throw "ONNX Runtime archive download failed with exit code $LASTEXITCODE"
    }
    return [pscustomobject]@{
        Path = $archivePath
        Root = [System.IO.Path]::GetFileNameWithoutExtension($download.name)
    }
}

$onnxArchive = $null
if ([string]::IsNullOrWhiteSpace($OnnxRuntimeCpu)) {
    $candidatePaths = @()

    # 1. Search in runtime asset cache (.temp/runtime-assets)
    if (Test-Path -LiteralPath $runtimeAssetCache -PathType Container) {
        $cachedCores = Get-ChildItem -LiteralPath $runtimeAssetCache -Filter 'onnxruntime.dll' -File -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Length -eq $expectedOnnxBytes }
        foreach ($c in $cachedCores) {
            $candidatePaths += $c.FullName
        }
    }

    # 2. Search in development runtime directory (runtime/onnxruntime/...)
    $devRuntimeDir = Join-Path $projectRoot 'runtime\onnxruntime'
    if (Test-Path -LiteralPath $devRuntimeDir -PathType Container) {
        $devCores = @(
            (Join-Path $devRuntimeDir 'cpu\onnxruntime.dll'),
            (Join-Path $devRuntimeDir 'cuda-13\onnxruntime.dll'),
            (Join-Path $devRuntimeDir 'cuda-12\onnxruntime.dll')
        )
        foreach ($p in $devCores) {
            if (Test-Path -LiteralPath $p -PathType Leaf) {
                $candidatePaths += $p
            }
        }
    }

    foreach ($cand in $candidatePaths) {
        if ((Get-Item -LiteralPath $cand).Length -eq $expectedOnnxBytes) {
            $OnnxRuntimeCpu = $cand
            break
        }
    }

    if ([string]::IsNullOrWhiteSpace($OnnxRuntimeCpu)) {
        if ($ValidateOnly) {
            throw 'Validation requires an existing verified ONNX Runtime CPU core. Pass -OnnxRuntimeCpu <onnxruntime.dll>. -ValidateOnly never downloads files.'
        }
        $onnxArchive = Get-OnnxCpuSourceArchive
        $OnnxRuntimeCpu = Join-Path $runtimeAssetCache 'onnxruntime\cpu\onnxruntime.dll'
        Export-ZipEntry -ArchivePath $onnxArchive.Path -EntryPath "$($onnxArchive.Root)/lib/onnxruntime.dll" -OutFile $OnnxRuntimeCpu -ExpectedBytes $expectedOnnxBytes
    }
    Write-Host "Using verified ONNX Runtime core (universal CPU engine): $OnnxRuntimeCpu"
}

$OnnxRuntimeCpu = [System.IO.Path]::GetFullPath($OnnxRuntimeCpu)
if (-not (Test-Path -LiteralPath $OnnxRuntimeCpu -PathType Leaf)) {
    throw "ONNX Runtime CPU core was not found: $OnnxRuntimeCpu"
}

# Resolve LICENSE and ThirdPartyNotices.txt
$onnxLicensesDir = Join-Path $projectRoot '.temp\runtime-assets\licenses\onnxruntime'
$candidateLicensePaths = @(
    (Join-Path (Split-Path (Split-Path $OnnxRuntimeCpu -Parent) -Parent) 'LICENSE'),
    (Join-Path (Split-Path $OnnxRuntimeCpu -Parent) 'LICENSE'),
    (Join-Path $onnxLicensesDir 'LICENSE')
)
$candidateNoticePaths = @(
    (Join-Path (Split-Path (Split-Path $OnnxRuntimeCpu -Parent) -Parent) 'ThirdPartyNotices.txt'),
    (Join-Path (Split-Path $OnnxRuntimeCpu -Parent) 'ThirdPartyNotices.txt'),
    (Join-Path $onnxLicensesDir 'ThirdPartyNotices.txt')
)

$onnxRuntimeLicense = $candidateLicensePaths | Where-Object {
    (Test-Path -LiteralPath $_ -PathType Leaf) -and
    ((Get-Item -LiteralPath $_).Length -eq $expectedLicenseBytes)
} | Select-Object -First 1

$onnxRuntimeNotices = $candidateNoticePaths | Where-Object {
    (Test-Path -LiteralPath $_ -PathType Leaf) -and
    ((Get-Item -LiteralPath $_).Length -eq $expectedNoticesBytes)
} | Select-Object -First 1

if ($null -eq $onnxRuntimeLicense -or $null -eq $onnxRuntimeNotices) {
    if ($ValidateOnly) {
        throw 'Validation requires verified ONNX Runtime LICENSE and ThirdPartyNotices.txt beside the core or in .temp/runtime-assets/licenses/onnxruntime. -ValidateOnly never downloads files.'
    }
    if ($null -eq $onnxArchive) {
        $onnxArchive = Get-OnnxCpuSourceArchive
    }
    if ($null -eq $onnxRuntimeLicense) {
        $targetLic = Join-Path $onnxLicensesDir 'LICENSE'
        Export-ZipEntry -ArchivePath $onnxArchive.Path -EntryPath "$($onnxArchive.Root)/LICENSE" -OutFile $targetLic -ExpectedBytes $expectedLicenseBytes
        $onnxRuntimeLicense = $targetLic
    }
    if ($null -eq $onnxRuntimeNotices) {
        $targetNot = Join-Path $onnxLicensesDir 'ThirdPartyNotices.txt'
        Export-ZipEntry -ArchivePath $onnxArchive.Path -EntryPath "$($onnxArchive.Root)/ThirdPartyNotices.txt" -OutFile $targetNot -ExpectedBytes $expectedNoticesBytes
        $onnxRuntimeNotices = $targetNot
    }
}


if (-not (Test-Path -LiteralPath $vadModel)) {
    throw "Silero VAD model was not found: $vadModel"
}
if (-not (Test-Path -LiteralPath $speakerModel)) {
    throw "ERes2NetV2 speaker ONNX model was not found: $speakerModel"
}
if (-not (Test-Path -LiteralPath $denoiseModel)) {
    throw "GTCRN denoise ONNX model was not found: $denoiseModel"
}

$packageArguments = @(
    '--rust-client-bin', (Join-Path $projectRoot 'target\release\rust-client.exe'),
    '--backend-bin', (Join-Path $projectRoot 'target\release\xrtranslate-backend.exe'),
    '--corpus-bin', (Join-Path $projectRoot 'target\release\xr-corpus-server.exe'),
    '--installer-bin', (Join-Path $projectRoot 'target\release\xrtranslate-installer.exe'),
    '--updater-bin', (Join-Path $projectRoot 'target\release\xrtranslate-updater.exe'),
    '--config', $configPath,
    '--resources-dir', (Join-Path $projectRoot 'rust-client\resources'),
    '--seed-database', $seedDatabase,
    '--vad-model', $vadModel,
    '--speaker-model', $speakerModel,
    '--denoise-model', $denoiseModel,
    '--onnx-runtime-cpu', $OnnxRuntimeCpu,
    '--onnx-runtime-license', $onnxRuntimeLicense,
    '--onnx-runtime-notices', $onnxRuntimeNotices,
    '--vc-runtime-dir', $VcRuntimeDirectory,
    '--output', $Output
)
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
