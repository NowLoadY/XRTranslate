[CmdletBinding()]
param(
    # A new directory for the staged release. Relative paths use the project root.
    [string]$Output,
    # Add the verified Qwen3-ASR and Hy-MT2 GGUF packages for an offline release.
    [switch]$IncludeModels,
    # Verified ONNX Runtime 1.28 core DLL. GPU providers are downloaded later.
    [string]$OnnxRuntimeCpu,
    # Validate all release inputs without writing a release directory.
    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
$projectRoot = $PSScriptRoot
$workspaceManifest = Join-Path $projectRoot 'Cargo.toml'
$configPath = Join-Path $projectRoot 'config.json'
$vadModel = Join-Path $projectRoot 'models\silero-vad\src\silero_vad\data\silero_vad.onnx'
$speakerModel = Join-Path $projectRoot 'models\3D-Speaker-ERes2NetV2\speaker_embedding.onnx'
$denoiseModel = Join-Path $projectRoot 'models\gtcrn\gtcrn_simple.onnx'
$corporaDirectory = Join-Path $projectRoot 'XR-Corpus\corpora'
$cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'

$expectedOnnxSha256 = '2462fe2d64ce063babefda3d9b1998380ffa74e99acf5d24d520ee67daa9e0f1'
$expectedLicenseSha256 = 'c250d6278f0b47a6439fb7592b08b58a55eb9f535aa49a1db63211c3f982b674'
$expectedNoticesSha256 = 'fb0af774b4d7cffc5b9d046f2aaeade2f37df2f80abf8033c95dfffcc77a8866'

function Get-ZipEntryFromUrl {
    param(
        [string]$Url,
        [string]$EntryName,
        [string]$OutFile,
        [string]$ExpectedSha256
    )
    Add-Type -AssemblyName System.IO.Compression

    # 1. Get length of remote file
    $req = [System.Net.HttpWebRequest]::Create($Url)
    $req.Method = 'HEAD'
    $res = $req.GetResponse()
    $totalLen = $res.ContentLength
    $res.Close()

    # 2. Download the last 64KB to find the End of Central Directory Record (EOCD)
    $tailSize = [Math]::Min(65536, $totalLen)
    $tailStart = $totalLen - $tailSize
    $req = [System.Net.HttpWebRequest]::Create($Url)
    $req.AddRange($tailStart, $totalLen - 1)
    $res = $req.GetResponse()
    $ms = New-Object System.IO.MemoryStream
    $res.GetResponseStream().CopyTo($ms)
    $res.Close()
    $tailBytes = $ms.ToArray()

    # Find EOCD signature: 0x06054b50
    $eocdOffsetInTail = -1
    for ($i = $tailBytes.Length - 22; $i -ge 0; $i--) {
        if ($tailBytes[$i] -eq 0x50 -and $tailBytes[$i+1] -eq 0x4b -and $tailBytes[$i+2] -eq 0x05 -and $tailBytes[$i+3] -eq 0x06) {
            $eocdOffsetInTail = $i
            break
        }
    }
    if ($eocdOffsetInTail -eq -1) { throw "EOCD not found in remote zip archive: $Url" }

    $cdSize = [BitConverter]::ToUInt32($tailBytes, $eocdOffsetInTail + 12)
    $cdOffset = [BitConverter]::ToUInt32($tailBytes, $eocdOffsetInTail + 16)

    # 3. Download Central Directory
    $req = [System.Net.HttpWebRequest]::Create($Url)
    $req.AddRange($cdOffset, $cdOffset + $cdSize - 1)
    $res = $req.GetResponse()
    $msCd = New-Object System.IO.MemoryStream
    $res.GetResponseStream().CopyTo($msCd)
    $res.Close()
    $cdBytes = $msCd.ToArray()

    # 4. Parse Central Directory entries to find target entry
    $ptr = 0
    $foundLocalHeaderOffset = -1
    $compressedSize = 0
    $uncompressedSize = 0
    $compressionMethod = 0

    while ($ptr -lt $cdBytes.Length - 4) {
        $sig = [BitConverter]::ToUInt32($cdBytes, $ptr)
        if ($sig -ne 0x02014b50) { break }
        $method = [BitConverter]::ToUInt16($cdBytes, $ptr + 10)
        $cSize = [BitConverter]::ToUInt32($cdBytes, $ptr + 20)
        $uSize = [BitConverter]::ToUInt32($cdBytes, $ptr + 24)
        $fnLen = [BitConverter]::ToUInt16($cdBytes, $ptr + 28)
        $extraLen = [BitConverter]::ToUInt16($cdBytes, $ptr + 30)
        $commentLen = [BitConverter]::ToUInt16($cdBytes, $ptr + 32)
        $localHeaderOffset = [BitConverter]::ToUInt32($cdBytes, $ptr + 42)
        $fileName = [System.Text.Encoding]::UTF8.GetString($cdBytes, $ptr + 46, $fnLen)

        if ($fileName.EndsWith($EntryName, [System.StringComparison]::OrdinalIgnoreCase)) {
            $foundLocalHeaderOffset = $localHeaderOffset
            $compressedSize = $cSize
            $uncompressedSize = $uSize
            $compressionMethod = $method
            break
        }
        $ptr += 46 + $fnLen + $extraLen + $commentLen
    }

    if ($foundLocalHeaderOffset -eq -1) { throw "Entry $EntryName not found in remote archive $Url" }

    # 5. Read local header to get exact data offset
    $req = [System.Net.HttpWebRequest]::Create($Url)
    $req.AddRange($foundLocalHeaderOffset, $foundLocalHeaderOffset + 30 + 1024)
    $res = $req.GetResponse()
    $msLoc = New-Object System.IO.MemoryStream
    $res.GetResponseStream().CopyTo($msLoc)
    $res.Close()
    $locBytes = $msLoc.ToArray()

    $locFnLen = [BitConverter]::ToUInt16($locBytes, 26)
    $locExtraLen = [BitConverter]::ToUInt16($locBytes, 28)
    $dataOffset = $foundLocalHeaderOffset + 30 + $locFnLen + $locExtraLen

    # 6. Download file data
    $req = [System.Net.HttpWebRequest]::Create($Url)
    $req.AddRange($dataOffset, $dataOffset + $compressedSize - 1)
    $res = $req.GetResponse()
    $msData = New-Object System.IO.MemoryStream
    $res.GetResponseStream().CopyTo($msData)
    $res.Close()
    $dataBytes = $msData.ToArray()

    # 7. Decompress or save
    $outDir = [System.IO.Path]::GetDirectoryName($OutFile)
    if (-not [string]::IsNullOrEmpty($outDir) -and -not (Test-Path $outDir)) {
        New-Item -ItemType Directory -Path $outDir -Force | Out-Null
    }

    if ($compressionMethod -eq 0) {
        [System.IO.File]::WriteAllBytes($OutFile, $dataBytes)
    } elseif ($compressionMethod -eq 8) {
        $inMs = New-Object System.IO.MemoryStream(,$dataBytes)
        $deflate = New-Object System.IO.Compression.DeflateStream($inMs, [System.IO.Compression.CompressionMode]::Decompress)
        $outMs = New-Object System.IO.MemoryStream
        $deflate.CopyTo($outMs)
        $deflate.Close()
        [System.IO.File]::WriteAllBytes($OutFile, $outMs.ToArray())
    } else {
        throw "Unsupported compression method: $compressionMethod"
    }

    if (-not [string]::IsNullOrWhiteSpace($ExpectedSha256)) {
        $actualHash = (Get-FileHash -LiteralPath $OutFile -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actualHash -ne $ExpectedSha256.ToLowerInvariant()) {
            throw "SHA256 mismatch for extracted $EntryName : expected $ExpectedSha256, got $actualHash"
        }
    }
}

if ([string]::IsNullOrWhiteSpace($OnnxRuntimeCpu)) {
    if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
        throw "Release configuration was not found: $configPath"
    }

    $candidatePaths = @()

    # 1. Search in runtime asset cache (.temp/runtime-assets)
    $runtimeAssetCache = Join-Path $projectRoot '.temp\runtime-assets'
    if (Test-Path -LiteralPath $runtimeAssetCache -PathType Container) {
        $cachedCores = Get-ChildItem -LiteralPath $runtimeAssetCache -Filter 'onnxruntime.dll' -File -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Length -eq 16277856 }
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

    # Verify candidate SHA256
    foreach ($cand in $candidatePaths) {
        if ((Get-Item -LiteralPath $cand).Length -eq 16277856) {
            $hash = (Get-FileHash -LiteralPath $cand -Algorithm SHA256).Hash.ToLowerInvariant()
            if ($hash -eq $expectedOnnxSha256.ToLowerInvariant()) {
                $OnnxRuntimeCpu = $cand
                break
            }
        }
    }

    if ([string]::IsNullOrWhiteSpace($OnnxRuntimeCpu)) {
        throw "No verified ONNX Runtime 1.28 CPU core found in .temp\runtime-assets or runtime\onnxruntime. Pass -OnnxRuntimeCpu <onnxruntime.dll>."
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
    ((Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant() -eq $expectedLicenseSha256.ToLowerInvariant())
} | Select-Object -First 1

$onnxRuntimeNotices = $candidateNoticePaths | Where-Object {
    (Test-Path -LiteralPath $_ -PathType Leaf) -and
    ((Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash.ToLowerInvariant() -eq $expectedNoticesSha256.ToLowerInvariant())
} | Select-Object -First 1

if ($null -eq $onnxRuntimeLicense -or $null -eq $onnxRuntimeNotices) {
    if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) {
        throw "Release configuration was not found: $configPath"
    }
    $releaseConfig = Get-Content -LiteralPath $configPath -Raw | ConvertFrom-Json
    $downloadUrl = ($releaseConfig.model_manager.onnxruntime.downloads | Select-Object -First 1).url
    Write-Host "Fetching ONNX Runtime license files from official package..."
    if ($null -eq $onnxRuntimeLicense) {
        $targetLic = Join-Path $onnxLicensesDir 'LICENSE'
        Get-ZipEntryFromUrl -Url $downloadUrl -EntryName 'LICENSE' -OutFile $targetLic -ExpectedSha256 $expectedLicenseSha256
        $onnxRuntimeLicense = $targetLic
    }
    if ($null -eq $onnxRuntimeNotices) {
        $targetNot = Join-Path $onnxLicensesDir 'ThirdPartyNotices.txt'
        Get-ZipEntryFromUrl -Url $downloadUrl -EntryName 'ThirdPartyNotices.txt' -OutFile $targetNot -ExpectedSha256 $expectedNoticesSha256
        $onnxRuntimeNotices = $targetNot
    }
}


if (-not (Test-Path -LiteralPath $workspaceManifest)) {
    throw "Rust workspace manifest was not found: $workspaceManifest"
}
if (-not (Test-Path -LiteralPath $configPath)) {
    throw "Release configuration was not found: $configPath"
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
if (-not (Test-Path -LiteralPath $corporaDirectory)) {
    throw "Versioned Markdown corpora were not found: $corporaDirectory"
}

if (Test-Path -LiteralPath $cargoPath) {
    $cargo = $cargoPath
} elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
    $cargo = 'cargo'
} else {
    throw 'Cargo was not found. Install Rust with rustup, then restart PowerShell.'
}

if ([string]::IsNullOrWhiteSpace($Output)) {
    $version = "0.1.0"
    if (Test-Path -LiteralPath $workspaceManifest) {
        $manifestContent = Get-Content -LiteralPath $workspaceManifest -Raw
        if ($manifestContent -match 'version\s*=\s*"([^"]+)"') {
            $version = $matches[1]
        }
    }
    $Output = Join-Path $projectRoot "dist\XRTranslate-v$version-win-x64"
} elseif (-not [System.IO.Path]::IsPathRooted($Output)) {
    $Output = Join-Path $projectRoot $Output
}
$Output = [System.IO.Path]::GetFullPath($Output)

if (Test-Path -LiteralPath $Output) {
    throw "Release output already exists. Choose a new -Output path: $Output"
}

$buildArguments = @(
    'build', '--manifest-path', $workspaceManifest, '--release',
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

& $cargo build --manifest-path (Join-Path $projectRoot 'XR-Corpus\Cargo.toml') --target-dir (Join-Path $projectRoot 'target') --release --package xr-corpus-server
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$packageArguments = @(
    'run', '--manifest-path', $workspaceManifest, '--release', '--package', 'xrtranslate-packager', '--',
    '--rust-client-bin', (Join-Path $projectRoot 'target\release\rust-client.exe'),
    '--backend-bin', (Join-Path $projectRoot 'target\release\xrtranslate-backend.exe'),
    '--corpus-bin', (Join-Path $projectRoot 'target\release\xr-corpus-server.exe'),
    '--installer-bin', (Join-Path $projectRoot 'target\release\xrtranslate-installer.exe'),
    '--updater-bin', (Join-Path $projectRoot 'target\release\xrtranslate-updater.exe'),
    '--config', $configPath,
    '--resources-dir', (Join-Path $projectRoot 'rust-client\resources'),
    '--corpora-dir', $corporaDirectory,
    '--vad-model', $vadModel,
    '--speaker-model', $speakerModel,
    '--denoise-model', $denoiseModel,
    '--onnx-runtime-cpu', $OnnxRuntimeCpu,
    '--onnx-runtime-license', $onnxRuntimeLicense,
    '--onnx-runtime-notices', $onnxRuntimeNotices,
    '--output', $Output
)
if ($IncludeModels) {
    $packageArguments += '--include-models'
}
if ($ValidateOnly) {
    $packageArguments += '--check'
}

Write-Host 'Preparing the native release package...'
& $cargo @packageArguments
if ($LASTEXITCODE -ne 0 -or $ValidateOnly) {
    exit $LASTEXITCODE
}

Write-Host "Release directory is ready: $Output"
