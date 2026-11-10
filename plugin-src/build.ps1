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
$profile = if ($DebugBuild) { 'debug' } else { 'release' }
$builder = @('xrt-plugin.exe', "target/$profile/xrt-plugin.exe", 'target/release/xrt-plugin.exe') |
    ForEach-Object { Join-Path $sdkRoot $_ } |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
    Select-Object -First 1
if (-not $builder) { throw "SDK build tool not found in: $sdkRoot" }
$arguments = @('build', $PSScriptRoot, '--sdk', $sdkRoot, '--output', $outputRoot)
$selected = @(Get-ChildItem -LiteralPath $PSScriptRoot -Directory | Where-Object {
    (Test-Path -LiteralPath (Join-Path $_.FullName 'plugin.json')) -and
    (-not $Plugin -or $_.Name -in $Plugin)
})
if ($selected.Count -eq 0) { throw 'No plugin sources selected.' }
foreach ($directory in $selected) { $arguments += @('--plugin', $directory.Name) }
if ($TargetDirectory) { $arguments += @('--target-dir', [System.IO.Path]::GetFullPath($TargetDirectory)) }
if ($DebugBuild) { $arguments += '--debug' }
if ($Offline) { $arguments += '--offline' }
if ($Locked) { $arguments += '--locked' }
& $builder @arguments
if ($LASTEXITCODE -ne 0) { throw 'Plugin build failed.' }
