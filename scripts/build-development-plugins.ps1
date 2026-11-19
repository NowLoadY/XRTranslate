[CmdletBinding()]
param([string]$Source, [switch]$DebugBuild)
$ErrorActionPreference = 'Stop'
$checkout = Split-Path $PSScriptRoot -Parent
$sources = if ($Source) {
    Get-Item -LiteralPath $Source
} else {
    Get-ChildItem -LiteralPath (Split-Path $checkout -Parent) -Directory -Filter 'XRT-*-Plugin'
}
$target = Join-Path $checkout 'target'
$profile = if ($DebugBuild) { 'debug' } else { 'release' }
$builder = Join-Path $target "$profile/xrt-plugin.exe"
$sdkArgs = @('build', '--locked', '--manifest-path', (Join-Path $checkout 'Cargo.toml'),
    '--target-dir', $target, '-p', 'xrtranslate-plugin', '--features', 'host', '--bin', 'xrt-plugin')
if (-not $DebugBuild) { $sdkArgs += '--release' }
& cargo @sdkArgs
if ($LASTEXITCODE) { throw 'Could not build the development SDK tool.' }
$dataRoot = $checkout
$location = Join-Path $checkout 'data-directory.txt'
if ([IO.File]::Exists($location)) {
    $configured = [IO.File]::ReadAllText($location).Trim()
    if ($configured -match '^([a-zA-Z]:[\\/]|[\\/]{2})') { $dataRoot = $configured }
}
foreach ($directory in $sources) {
    $manifest = Join-Path $directory.FullName 'plugin.json'
    if (-not $Source -and -not [IO.File]::Exists($manifest)) { continue }
    $plugin = Get-Content -LiteralPath $manifest -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not $Source -and -not $plugin.pro_for) { continue }
    if ($plugin.id -cnotmatch '^[A-Za-z0-9_-]{1,64}$') { throw "Invalid plugin ID in $manifest" }
    $package = Join-Path $directory.FullName "dist/$($plugin.id)"
    $buildArgs = @('build', $directory.FullName, '--sdk', $checkout,
        '--output', $package, '--target-dir', $target, '--locked')
    if ($DebugBuild) { $buildArgs += '--debug' }
    & $builder @buildArgs
    if ($LASTEXITCODE) { throw "Could not build $($plugin.id)." }
    $built = Get-Content -LiteralPath (Join-Path $package 'plugin.json') -Raw -Encoding UTF8 | ConvertFrom-Json
    $destination = Join-Path $dataRoot "plugins/$($built.id)"
    & $builder stage $package (Join-Path $package $built.executable) $destination --replace
    if ($LASTEXITCODE) { throw "Could not update the development plugin at $destination." }
    Write-Host "Development plugin ready: $destination"
}
