#requires -Version 5.1
<#
.SYNOPSIS
Preview or remove reproducible files in this checkout.
.DESCRIPTION
Common: Debug and Python caches, plus unused packages directly staged in .temp.
Build: All Cargo and Python caches. Packages: Only temporary staged packages.
All: Build and Packages combined. Nothing is removed without -Apply.
#>
[CmdletBinding()]
param(
    [ValidateSet('Common', 'Build', 'Packages', 'All')]
    [string]$Scope = 'Common',
    [switch]$Apply
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path -LiteralPath (Split-Path $PSScriptRoot -Parent)).Path
$projectPrefix = $projectRoot.TrimEnd('\') + '\'
$candidates = [Collections.Generic.List[object]]::new()

function Assert-LocalPath([string]$Path) {
    $absolute = [IO.Path]::GetFullPath($Path)
    if (-not $absolute.StartsWith($projectPrefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Outside the checkout: $Path"
    }
    # Check ancestors as well: a directory junction can redirect a normal child.
    for ($cursor = $absolute; $cursor; $cursor = Split-Path $cursor -Parent) {
        $item = Get-Item -LiteralPath $cursor -Force
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Linked path: $cursor"
        }
        if ($cursor -eq $projectRoot) { break }
    }
}

function Measure-Candidate([string]$Path) {
    Assert-LocalPath $Path
    $tracked = @(& git -C (Split-Path $Path -Parent) ls-files -- (Split-Path $Path -Leaf))
    if ($LASTEXITCODE -ne 0) { throw "Cannot check tracked files: $Path" }
    if ($tracked.Count) { throw "Contains tracked files: $Path" }

    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($Path)
    [long]$bytes = 0
    while ($pending.Count) {
        $item = Get-Item -LiteralPath $pending.Pop() -Force
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
            throw "Contains a linked path: $($item.FullName)"
        }
        if ($item.PSIsContainer) {
            foreach ($child in Get-ChildItem -LiteralPath $item.FullName -Force) {
                $pending.Push($child.FullName)
            }
        } else {
            $bytes += $item.Length
        }
    }
    return $bytes
}

function Add-Candidate([string]$Path, [string]$Kind, [string]$PackageRoot = '') {
    if (-not (Test-Path -LiteralPath $Path)) { return }
    try {
        $bytes = Measure-Candidate $Path
        $candidates.Add([pscustomobject]@{
            Path = $Path; Kind = $Kind; Bytes = $bytes; PackageRoot = $PackageRoot
        })
    } catch {
        Write-Warning "Keeping $Path -- $($_.Exception.Message)"
    }
}

function Find-BuildCaches {
    $pending = [Collections.Generic.Stack[string]]::new()
    $pending.Push($projectRoot)
    # Do not traverse user data, installed plugins, downloads or dependencies.
    $excluded = @('.git', '.temp', 'dist', 'runtime', 'models', 'plugins',
        'third_party', 'assets', 'resources', 'node_modules', '.gradle', '.cxx')
    while ($pending.Count) {
        foreach ($directory in Get-ChildItem -LiteralPath $pending.Pop() -Directory -Force) {
            if ($directory.Name -in $excluded -or
                ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint)) { continue }
            if ($directory.Name -eq 'target') {
                if (Test-Path -LiteralPath (Join-Path $directory.Parent.FullName 'Cargo.toml')) {
                    if ($Scope -in @('Build', 'All')) {
                        Add-Candidate $directory.FullName 'Cargo cache'
                    } else {
                        Add-Candidate (Join-Path $directory.FullName 'debug') 'Debug cache'
                        # Cargo also uses target/<platform>/debug for cross builds.
                        foreach ($platform in Get-ChildItem -LiteralPath $directory.FullName -Directory) {
                            $debug = Join-Path $platform.FullName 'debug'
                            if (Test-Path -LiteralPath (Join-Path $debug '.fingerprint')) {
                                Add-Candidate $debug 'Debug cache'
                            }
                        }
                    }
                }
            } elseif ($directory.Name -eq '__pycache__') {
                Add-Candidate $directory.FullName 'Python cache'
            } else {
                $pending.Push($directory.FullName)
            }
        }
    }
}

function Test-StagedPackage([string]$Path) {
    $manifestPath = Join-Path $Path 'release-manifest.json'
    if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) { return $false }
    Assert-LocalPath $manifestPath
    try {
        $manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    } catch {
        Write-Warning "Keeping package with unreadable manifest: $Path"
        return $false
    }
    if (-not $manifest.layout_version -or -not $manifest.entrypoints.client) { return $false }
    foreach ($model in $manifest.models.packages) {
        if ($model.directory -and (Test-Path -LiteralPath (Join-Path $Path $model.directory))) {
            Write-Warning "Keeping package with downloaded models: $Path"
            return $false
        }
    }
    # A package used as a portable installation may contain irreplaceable data.
    $runtime = Join-Path $Path 'runtime'
    if (Test-Path -LiteralPath $runtime) {
        Assert-LocalPath $runtime
        if (@(Get-ChildItem -LiteralPath $runtime -Force |
                Where-Object Name -ne 'onnxruntime').Count) {
            Write-Warning "Keeping package with runtime data: $Path"
            return $false
        }
    }
    return $true
}

function Find-TemporaryPackages {
    $temporaryRoot = Join-Path $projectRoot '.temp'
    if (-not (Test-Path -LiteralPath $temporaryRoot)) { return }
    Assert-LocalPath $temporaryRoot
    # Only inspect direct staging folders and packaged content directories.
    # Never walk into archived records, test profiles, tools or download caches.
    foreach ($directory in Get-ChildItem -LiteralPath $temporaryRoot -Directory) {
        if ($directory.Attributes -band [IO.FileAttributes]::ReparsePoint) { continue }
        if (Test-StagedPackage $directory.FullName) {
            Add-Candidate $directory.FullName 'Temporary package' $directory.FullName
            continue
        }
        foreach ($package in Get-ChildItem -LiteralPath $directory.FullName -Directory -Filter 'XRTranslate-*') {
            if (($package.Attributes -band [IO.FileAttributes]::ReparsePoint) -or
                -not (Test-Path -LiteralPath (Join-Path $package.FullName 'app_build.vdf'))) { continue }
            $extra = @(Get-ChildItem -LiteralPath $package.FullName -Force |
                Where-Object Name -notin @('content', 'app_build.vdf'))
            if (-not $extra.Count -and (Test-StagedPackage (Join-Path $package.FullName 'content'))) {
                Add-Candidate $package.FullName 'Temporary package' (Join-Path $package.FullName 'content')
            }
        }
    }
}

function Assert-NotInUse {
    # Do not interrupt programs or compete with a build that may recreate files.
    $builders = @('cargo.exe', 'rustc.exe', 'rustup.exe', 'cl.exe', 'link.exe',
        'cmake.exe', 'ninja.exe', 'MSBuild.exe', 'ISCC.exe',
        'xrtranslate-packager.exe', 'xrt-plugin.exe')
    foreach ($process in Get-CimInstance Win32_Process) {
        if ($process.Name -in $builders) {
            throw "Build tool is running: $($process.Name) (PID $($process.ProcessId)). Try again after it finishes."
        }
        foreach ($candidate in $candidates) {
            if ($process.ExecutablePath -and $process.ExecutablePath.StartsWith(
                    $candidate.Path.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
                throw "Package is running: $($process.Name) (PID $($process.ProcessId)). Close it before cleaning."
            }
        }
    }
}

function Confirm-Candidate($Candidate) {
    if ($Candidate.PackageRoot -and -not (Test-StagedPackage $Candidate.PackageRoot)) {
        throw "Package changed since preview: $($Candidate.Path)"
    }
    return Measure-Candidate $Candidate.Path
}

if (-not (Test-Path -LiteralPath (Join-Path $projectRoot 'Cargo.toml'))) {
    throw 'Run the cleanup script from an XRTranslate source checkout.'
}
if ($Scope -ne 'Packages') { Find-BuildCaches }
if ($Scope -ne 'Build') { Find-TemporaryPackages }

if (-not $candidates.Count) {
    Write-Host "No cleanup candidates for scope $Scope."
    return
}
$candidates | Sort-Object Path | Select-Object Kind,
    @{ Name = 'GiB'; Expression = { '{0:N3}' -f ($_.Bytes / 1GB) } },
    @{ Name = 'Path'; Expression = { $_.Path.Substring($projectPrefix.Length) } } |
    Format-Table -AutoSize | Out-Host
$total = ($candidates | Measure-Object Bytes -Sum).Sum
Write-Host ('Total: {0:N2} GiB in {1} locations.' -f ($total / 1GB), $candidates.Count)
if (-not $Apply) {
    Write-Host "Preview only. To remove these files: clean.cmd -Scope $Scope -Apply"
    return
}

Assert-NotInUse
# Validate the complete plan before removing anything, then check each path again.
foreach ($candidate in $candidates) { $null = Confirm-Candidate $candidate }
[long]$removed = 0
$failed = $false
foreach ($candidate in $candidates) {
    try {
        $bytes = Confirm-Candidate $candidate
        Remove-Item -LiteralPath $candidate.Path -Recurse -Force
        $removed += $bytes
        Write-Host "Removed: $($candidate.Path.Substring($projectPrefix.Length))"
    } catch {
        $failed = $true
        Write-Warning "Could not completely remove $($candidate.Path): $($_.Exception.Message)"
    }
}
Write-Host ('Removed {0:N2} GiB. Retained user data, models, downloads and dist.' -f ($removed / 1GB))
if ($failed) { exit 1 }
