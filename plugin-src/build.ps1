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
$previousBuildJobs = $env:CARGO_BUILD_JOBS
try {
    if ([string]::IsNullOrWhiteSpace($previousBuildJobs)) {
        # Each plugin loads the shared UI/media dependency graph. Parallel rustc
        # processes can exhaust Windows commit memory and report misleading E0463
        # errors. Budget against available commit, not the logical CPU count.
        $jobs = 1
        try {
            $availableBytes = (Get-CimInstance Win32_OperatingSystem -ErrorAction Stop).FreeVirtualMemory * 1KB
            $memoryJobs = [math]::Floor(($availableBytes - 2GB) / 4GB)
            $jobs = [math]::Max(1, [math]::Min($memoryJobs, [math]::Min(4, [Environment]::ProcessorCount)))
        } catch {
            Write-Verbose "Could not query available memory; using one build job: $_"
        }
        $env:CARGO_BUILD_JOBS = [string]$jobs
        Write-Host "Building plugins with $jobs concurrent job(s). Set CARGO_BUILD_JOBS to override."
    }
    & $builder @arguments
    if ($LASTEXITCODE -ne 0) { throw 'Plugin build failed.' }
} finally {
    $env:CARGO_BUILD_JOBS = $previousBuildJobs
}
