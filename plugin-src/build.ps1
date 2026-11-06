[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Sdk,
    [Parameter(Mandatory)][string]$Output,
    [string]$TargetDirectory,
    [string[]]$Plugin,
    [switch]$DebugBuild,
    [switch]$Offline,
    [switch]$Locked
)
$ErrorActionPreference = 'Stop'
$sdkRoot = (Resolve-Path -LiteralPath $Sdk).Path
$outputRoot = [System.IO.Path]::GetFullPath($Output)
$builder = Join-Path $sdkRoot 'xrt-plugin.exe'
if (-not (Test-Path -LiteralPath $builder)) { throw "SDK build tool not found: $builder" }
foreach ($directory in Get-ChildItem -LiteralPath $PSScriptRoot -Directory) {
    if (-not (Test-Path -LiteralPath (Join-Path $directory.FullName 'plugin.json'))) { continue }
    if ($Plugin -and $directory.Name -notin $Plugin) { continue }
    $arguments = @('build', $directory.FullName, '--sdk', $sdkRoot, '--output', (Join-Path $outputRoot $directory.Name))
    if ($TargetDirectory) { $arguments += @('--target-dir', [System.IO.Path]::GetFullPath($TargetDirectory)) }
    if ($DebugBuild) { $arguments += '--debug' }
    if ($Offline) { $arguments += '--offline' }
    if ($Locked) { $arguments += '--locked' }
    & $builder @arguments
    if ($LASTEXITCODE -ne 0) { throw "Plugin build failed: $($directory.Name)" }
}
