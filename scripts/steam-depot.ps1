# Generate a SteamPipe file mapping for a prepared content directory.
function New-SteamBuild {
    param(
        [Parameter(Mandatory)][uint32]$AppId,
        [Parameter(Mandatory)][uint32]$DepotId,
        [Parameter(Mandatory)][string]$ContentRoot,
        [Parameter(Mandatory)][string]$OutputDirectory,
        [string]$DepotPath = '.'
    )
    if (-not $AppId -or -not $DepotId) { throw 'Steam application and depot IDs must be nonzero.' }
    $content = (Resolve-Path -LiteralPath $ContentRoot).Path.Replace('\', '/')
    if ($content.Contains('"') -or $DepotPath.Contains('"')) { throw 'Steam paths cannot contain quotes.' }
    New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
    @"
"AppBuild"
{
    "AppID" "$AppId"
    "BuildOutput" "output"
    "ContentRoot" "$content"
    "Depots"
    {
        "$DepotId"
        {
            "FileMapping"
            {
                "LocalPath" "*"
                "DepotPath" "$DepotPath"
                "recursive" "1"
            }
        }
    }
}
"@ | Set-Content -LiteralPath (Join-Path $OutputDirectory 'app_build.vdf') -Encoding utf8
}
