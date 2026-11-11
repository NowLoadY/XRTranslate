#requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Package,
    [ValidateSet('Setup', 'Msix', 'Steam')][string]$Format = 'Setup',
    [string]$OutputDirectory = 'dist',
    [string]$Iscc,
    [string]$MakeAppx,
    # MSIX identity values come from Partner Center, not the repository name.
    [string]$IdentityName,
    [string]$Publisher,
    [string]$PublisherDisplayName,
    [uint32]$SteamAppId,
    [uint32]$SteamDepotId
)

$ErrorActionPreference = 'Stop'
$packageRoot = (Resolve-Path -LiteralPath $Package).Path
$outputRoot = [IO.Path]::GetFullPath($OutputDirectory)
$sourceRoot = Split-Path $PSScriptRoot -Parent
$manifest = Get-Content -LiteralPath (Join-Path $packageRoot 'release-manifest.json') -Raw | ConvertFrom-Json
$entrypoint = $manifest.entrypoints.client
if ($entrypoint -cnotmatch '^XRTranslate-v(\d+\.\d+\.\d+)\.exe$') {
    throw 'Use a standard Windows release directory with a stable version.'
}
$version = $Matches[1]
if ($manifest.python -ne $false -or -not (Test-Path -LiteralPath (Join-Path $packageRoot $entrypoint))) {
    throw 'The Windows release package is incomplete.'
}
if ((Test-Path -LiteralPath (Join-Path $packageRoot 'runtime\user-config.json')) -or
    (Test-Path -LiteralPath (Join-Path $packageRoot 'data-directory.txt'))) {
    throw 'Use a fresh release package, not an installed application with user data.'
}
$bundledPlugins = @(Get-ChildItem -LiteralPath (Join-Path $packageRoot 'plugins') -Directory)
foreach ($plugin in $bundledPlugins) {
    $definition = Get-Content -LiteralPath (Join-Path $plugin.FullName 'plugin.json') -Raw | ConvertFrom-Json
    if ($definition.bundle_with_host -ne $true) { throw "Separately distributed plugin in package: $($plugin.Name)" }
}
if ($Format -eq 'Msix' -and (-not $IdentityName -or -not $Publisher -or -not $PublisherDisplayName)) {
    throw 'MSIX requires IdentityName, Publisher and PublisherDisplayName from Partner Center.'
}
if ($Format -eq 'Steam' -and (-not $SteamAppId -or -not $SteamDepotId)) {
    throw 'Steam requires the application and Windows depot IDs from Steamworks.'
}
New-Item -ItemType Directory -Path $outputRoot -Force | Out-Null
$work = Join-Path ([IO.Path]::GetTempPath()) "xrtranslate-distribution-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $work | Out-Null
try {
    $manifest.entrypoints.client = 'XRTranslate.exe'
    $manifest | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath (Join-Path $work 'release-manifest.json') -Encoding utf8
    $channel = @{ Setup = 'installer'; Msix = 'store'; Steam = 'steam' }[$Format]
    ConvertTo-Json $channel | Set-Content -LiteralPath (Join-Path $work 'distribution.json') -Encoding utf8
    $files = [Collections.Generic.List[object]]::new()
    foreach ($file in Get-ChildItem -LiteralPath $packageRoot -Recurse -File) {
        $relative = [IO.Path]::GetRelativePath($packageRoot, $file.FullName)
        if ($relative -in @('release-manifest.json', 'distribution.json')) { continue }
        if ($relative -eq $entrypoint) { $relative = 'XRTranslate.exe' }
        $files.Add(@{ Source = $file.FullName; Destination = $relative })
    }
    foreach ($name in 'release-manifest.json', 'distribution.json') {
        $files.Add(@{ Source = Join-Path $work $name; Destination = $name })
    }

    switch ($Format) {
        Setup {
            if (-not $Iscc) {
                $Iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
                if (-not $Iscc) {
                    $Iscc = @('C:\Program Files\Inno Setup 7\ISCC.exe', 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe') |
                        Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
                }
            }
            if (-not $Iscc) { throw 'Install Inno Setup 6.6 or newer, or pass -Iscc <ISCC.exe>.' }
            $files | ForEach-Object {
                $directory = [IO.Path]::GetDirectoryName($_.Destination)
                $destination = '{app}' + $(if ($directory) { "\$directory" })
                $check = if ($_.Destination -eq 'distribution.json') { '; Check: UseSharedData' } else { '' }
                'Source: "{0}"; DestDir: "{1}"; DestName: "{2}"; Flags: ignoreversion{3}' -f
                    $_.Source, $destination, [IO.Path]::GetFileName($_.Destination), $check
            } | Set-Content -LiteralPath (Join-Path $work 'files.iss') -Encoding utf8
            Set-Content -LiteralPath (Join-Path $work 'plugins.iss') -Encoding utf8 -Value @(
                $bundledPlugins | ForEach-Object {
                    'Type: filesandordirs; Name: "{{app}}\plugins\{0}"' -f $_.Name
                }
            )
            & $Iscc "/I$work" "/DPackageDir=$packageRoot" "/DAppVersion=$version" "/DOutputDir=$outputRoot" (Join-Path $sourceRoot 'packaging\windows\setup.iss')
            if ($LASTEXITCODE -ne 0) { throw 'Windows setup compilation failed.' }
        }
        Msix {
            if (-not $MakeAppx) {
                $sdk = Get-ChildItem -Path "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makeappx.exe" |
                    Sort-Object { [version]$_.Directory.Parent.Name } -Descending | Select-Object -First 1
                $MakeAppx = $sdk.FullName
            }
            if (-not $MakeAppx) { throw 'Install the Windows SDK, or pass -MakeAppx <makeappx.exe>.' }
            [xml]$xml = Get-Content -LiteralPath (Join-Path $sourceRoot 'packaging\windows\AppxManifest.xml') -Raw
            $xml.Package.Identity.SetAttribute('Name', $IdentityName)
            $xml.Package.Identity.SetAttribute('Publisher', $Publisher)
            $xml.Package.Identity.SetAttribute('Version', "$version.0")
            $xml.Package.Properties.PublisherDisplayName = $PublisherDisplayName
            $xml.Save((Join-Path $work 'AppxManifest.xml'))
            $files.Add(@{ Source = Join-Path $work 'AppxManifest.xml'; Destination = 'AppxManifest.xml' })
            Add-Type -AssemblyName System.Drawing
            $logo = [Drawing.Image]::FromFile((Join-Path $packageRoot 'resources\branding\xrtranslate-logo.png'))
            try {
                foreach ($asset in @(@('StoreLogo',50), @('Square44x44Logo',44), @('Square150x150Logo',150))) {
                    $bitmap = [Drawing.Bitmap]::new([int]$asset[1], [int]$asset[1])
                    $graphics = [Drawing.Graphics]::FromImage($bitmap)
                    try {
                        $graphics.Clear([Drawing.Color]::Transparent)
                        $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                        $graphics.DrawImage($logo, 0, 0, $bitmap.Width, $bitmap.Height)
                        $path = Join-Path $work "$($asset[0]).png"
                        $bitmap.Save($path, [Drawing.Imaging.ImageFormat]::Png)
                        $files.Add(@{ Source = $path; Destination = "Assets\$($asset[0]).png" })
                    } finally { $graphics.Dispose(); $bitmap.Dispose() }
                }
            } finally { $logo.Dispose() }
            @('[Files]') + @($files | ForEach-Object { '"{0}" "{1}"' -f $_.Source, $_.Destination }) |
                Set-Content -LiteralPath (Join-Path $work 'mapping.txt') -Encoding utf8
            & $MakeAppx pack /f (Join-Path $work 'mapping.txt') /p (Join-Path $outputRoot "XRTranslate-v$version-win-x64.msix") /o
            if ($LASTEXITCODE -ne 0) { throw 'MSIX validation or packaging failed.' }
        }
        Steam {
            $depot = Join-Path $outputRoot "XRTranslate-v$version-steam"
            if (Test-Path -LiteralPath $depot) { throw "Steam output already exists: $depot" }
            foreach ($file in $files) {
                $destination = Join-Path $depot "content\$($file.Destination)"
                New-Item -ItemType Directory -Path (Split-Path $destination -Parent) -Force | Out-Null
                Copy-Item -LiteralPath $file.Source -Destination $destination
            }
            . (Join-Path $PSScriptRoot 'steam-depot.ps1')
            New-SteamBuild -AppId $SteamAppId -DepotId $SteamDepotId `
                -ContentRoot (Join-Path $depot 'content') -OutputDirectory $depot
            Write-Host "Steam depot ready: $depot (launch XRTranslate.exe)"
        }
    }
} finally {
    $resolved = [IO.Path]::GetFullPath($work)
    $tempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (-not $resolved.StartsWith($tempRoot, [StringComparison]::OrdinalIgnoreCase) -or
        (Get-Item -LiteralPath $resolved).Attributes.HasFlag([IO.FileAttributes]::ReparsePoint)) {
        throw 'Refusing to remove an unexpected staging directory.'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
