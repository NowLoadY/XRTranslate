[CmdletBinding()]
param(
    [switch]$Release = $true
)

$ErrorActionPreference = 'Stop'
$projectRoot = $PSScriptRoot
$manifestPath = Join-Path $projectRoot 'rust-client\Cargo.toml'
$workspaceManifestPath = Join-Path $projectRoot 'Cargo.toml'
$corpusManifestPath = Join-Path $projectRoot 'XR-Corpus\Cargo.toml'
$cargoPath = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'

foreach ($path in @($manifestPath, $workspaceManifestPath, $corpusManifestPath)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Rust workspace manifest was not found: $path"
    }
}

if (Test-Path -LiteralPath $cargoPath) {
    $cargo = $cargoPath
} elseif (Get-Command cargo -ErrorAction SilentlyContinue) {
    $cargo = 'cargo'
} else {
    throw 'Cargo was not found. Install Rust with rustup, then restart PowerShell.'
}

# Keep the client and its child backend attached to this console while using
# the development launcher.  This makes capture and VAD diagnostics visible
# during reproduction without changing packaged-app behaviour.
if ([string]::IsNullOrWhiteSpace($env:RUST_LOG)) {
    $env:RUST_LOG = 'info'
}
$env:XRTRANSLATE_BACKEND_CONSOLE_LOG = '1'

$buildArguments = @(
    'build', '--locked', '--manifest-path', $workspaceManifestPath,
    '--target-dir', (Join-Path $projectRoot 'target'),
    '--package', 'rust-client', '--package', 'xrtranslate-backend',
    '--package', 'xrtranslate-account-login', '--package', 'xrtranslate-plugin',
    '--features', 'rust-client/mpv,xrtranslate-plugin/host'
)
$runtimeMarker = Join-Path $projectRoot 'runtime\native-runtime.json'
if (Test-Path -LiteralPath $runtimeMarker -PathType Leaf) {
    try {
        $runtimeSelection = Get-Content -LiteralPath $runtimeMarker -Raw | ConvertFrom-Json
        if (-not [string]::IsNullOrWhiteSpace($runtimeSelection.onnx_core_library)) {
            $managedCore = $runtimeSelection.onnx_core_library
            if (-not [System.IO.Path]::IsPathRooted($managedCore)) {
                $managedCore = Join-Path $projectRoot $managedCore
            }
            if (Test-Path -LiteralPath $managedCore -PathType Leaf) {
                $buildArguments += @('--features', 'xrtranslate-backend/managed-ort')
                Write-Host "Using managed ONNX Runtime core: $managedCore"
            }
        }
    } catch {
        Write-Warning "Ignoring invalid managed runtime marker: $($_.Exception.Message)"
    }
}
if ($Release) {
    $buildArguments += '--release'
}

Write-Host 'Preparing the application...'
. (Join-Path $projectRoot 'scripts/windows-build-env.ps1')
Initialize-WindowsSpeechLibraries -ProjectRoot $projectRoot
& $cargo @buildArguments
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$corpusBuildArguments = @(
    'build', '--locked', '--manifest-path', $corpusManifestPath,
    '--target-dir', (Join-Path $projectRoot 'target'),
    '--package', 'xr-corpus-server'
)
if ($Release) {
    $corpusBuildArguments += '--release'
}
& $cargo @corpusBuildArguments
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host 'Preparing development plugins...'
& (Join-Path $projectRoot 'plugin-src/build.ps1') -Sdk $projectRoot `
    -Output (Join-Path $projectRoot 'plugins') -TargetDirectory (Join-Path $projectRoot 'target') `
    -DebugBuild:(!$Release) -Locked

$targetSubdir = if ($Release) { 'release' } else { 'debug' }
$targetBinDir = Join-Path $projectRoot "target\$targetSubdir"
$resourceBinDir = Join-Path $projectRoot 'rust-client\resources\bin'

if (Test-Path -LiteralPath $resourceBinDir) {
    if (-not (Test-Path -LiteralPath $targetBinDir)) {
        New-Item -ItemType Directory -Path $targetBinDir -Force | Out-Null
    }
    Get-ChildItem -Path $resourceBinDir -Filter "*.dll" | ForEach-Object {
        $destFile = Join-Path $targetBinDir $_.Name
        if (-not (Test-Path -LiteralPath $destFile)) {
            Copy-Item -LiteralPath $_.FullName -Destination $destFile -Force
        }
    }
}

Write-Host 'Starting XRTranslate...'
Write-Host "Logging enabled (RUST_LOG=$env:RUST_LOG). Press Ctrl+C to stop."
Push-Location -LiteralPath $projectRoot
try {
    & (Join-Path $targetBinDir 'rust-client.exe')
    $clientExitCode = $LASTEXITCODE
} finally {
    Pop-Location
}
exit $clientExitCode
