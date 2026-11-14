#requires -Version 7.0
[CmdletBinding()]
param([Parameter(Mandatory)][string]$Directory)

$ErrorActionPreference = 'Stop'
$binaries = (Resolve-Path -LiteralPath $Directory).Path
if (-not ('XRTranslate.ManifestResource' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
namespace XRTranslate {
    public static class ManifestResource {
        [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        static extern IntPtr LoadLibraryExW(string path, IntPtr file, uint flags);
        [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
        static extern IntPtr FindResourceW(IntPtr module, IntPtr name, IntPtr type);
        [DllImport("kernel32.dll")] static extern uint SizeofResource(IntPtr module, IntPtr resource);
        [DllImport("kernel32.dll")] static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
        [DllImport("kernel32.dll")] static extern IntPtr LockResource(IntPtr resource);
        [DllImport("kernel32.dll")] static extern bool FreeLibrary(IntPtr module);
        public static string Read(string path) {
            // Read resources without executing code or resolving the program's DLLs.
            IntPtr module = LoadLibraryExW(path, IntPtr.Zero, 0x22);
            if (module == IntPtr.Zero) throw new Win32Exception();
            try {
                IntPtr resource = FindResourceW(module, (IntPtr)1, (IntPtr)24);
                if (resource == IntPtr.Zero) throw new InvalidOperationException("Missing embedded application manifest: " + path);
                byte[] data = new byte[SizeofResource(module, resource)];
                Marshal.Copy(LockResource(LoadResource(module, resource)), data, 0, data.Length);
                return Encoding.UTF8.GetString(data).TrimStart('\uFEFF');
            } finally { FreeLibrary(module); }
        }
    }
}
'@
}

foreach ($name in 'xrtranslate-updater.exe', 'xrtranslate-installer.exe') {
    $path = Join-Path $binaries $name
    [xml]$manifest = [XRTranslate.ManifestResource]::Read($path)
    $level = $manifest.SelectSingleNode('//*[local-name()="requestedExecutionLevel"]')
    if (-not $level -or $level.level -cne 'asInvoker' -or $level.uiAccess -cne 'false') {
        throw "$name must embed requestedExecutionLevel=asInvoker with uiAccess=false. Rebuild the Windows helpers."
    }
}

function Invoke-MoveHelper([string[]]$Arguments) {
    $start = [Diagnostics.ProcessStartInfo]::new($helper)
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    try {
        if (-not $process.WaitForExit(10000)) {
            $process.Kill()
            throw 'User-data migration smoke test timed out.'
        }
        $output = $process.StandardOutput.ReadToEnd()
        $errorOutput = $process.StandardError.ReadToEnd()
        if ($process.ExitCode -ne 0) { throw "User-data migration failed: $errorOutput" }
        return $output.Trim()
    } finally { $process.Dispose() }
}

$temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$fixture = Join-Path $temporaryRoot "is-xrtranslate-move-$([guid]::NewGuid()).tmp"
New-Item -ItemType Directory -Path $fixture | Out-Null
try {
    $helper = Join-Path $fixture 'xrtranslate-updater.exe'
    Copy-Item -LiteralPath (Join-Path $binaries 'xrtranslate-updater.exe') -Destination $helper
    $root = Join-Path $fixture 'Application'
    $source = Join-Path $fixture '原始 data'
    $selected = Join-Path $fixture '目标 location'
    $location = Join-Path $fixture 'profile/data-directory.txt'
    New-Item -ItemType Directory -Path $root, (Join-Path $source 'runtime') | Out-Null
    $recording = Join-Path $source 'runtime/recording.wav'
    [IO.File]::WriteAllBytes($recording, [byte[]](1, 2, 3, 4))
    @{ recording = $recording; text = '保留内容' } | ConvertTo-Json |
        Set-Content -LiteralPath (Join-Path $source 'runtime/settings.json') -Encoding utf8NoBOM
    $arguments = @('move-data', '--root', (Join-Path $root '.'), '--source', (Join-Path $source '.'), '--location-file', $location)
    $target = Invoke-MoveHelper ($arguments + @('--target', (Join-Path $selected '.'), '--check'))
    if ($target -cne (Join-Path $selected 'XRTranslate-UserData') -or
        (Test-Path -LiteralPath $target) -or (Test-Path -LiteralPath $location)) {
        throw 'Checking a data location must resolve its destination without creating or moving files.'
    }
    Invoke-MoveHelper ($arguments + @('--target', (Join-Path $target '.'))) | Out-Null
    $moved = Get-Content -LiteralPath (Join-Path $target 'runtime/settings.json') -Raw | ConvertFrom-Json
    if ((Get-Content -LiteralPath $location -Raw).Trim() -cne $target -or
        [IO.Path]::GetFullPath($moved.recording) -cne (Join-Path $target 'runtime/recording.wav') -or
        $moved.text -cne '保留内容' -or (Test-Path -LiteralPath $recording) -or
        [Convert]::ToHexString([IO.File]::ReadAllBytes($moved.recording)) -cne '01020304') {
        throw 'User-data migration did not preserve data and update its location.'
    }
    Write-Host 'Windows helpers declare ordinary user permissions; temporary-folder migration passed.'
} finally {
    $resolved = (Resolve-Path -LiteralPath $fixture).Path
    if (-not $resolved.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or
        -not [StringComparer]::OrdinalIgnoreCase.Equals($resolved, [IO.Path]::GetFullPath($fixture))) {
        throw 'Unexpected smoke-test cleanup path.'
    }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
