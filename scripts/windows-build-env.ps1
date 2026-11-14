function Initialize-WindowsSpeechLibraries {
    param([Parameter(Mandatory)][string]$ProjectRoot)

    if ($env:SHERPA_ONNX_LIB_DIR -or $env:SHERPA_ONNX_ARCHIVE_DIR) { return }

    $lock = Get-Content (Join-Path $ProjectRoot 'Cargo.lock') -Raw
    $match = [regex]::Match($lock, '(?m)^name = "sherpa-onnx-sys"\r?\nversion = "([^"]+)"')
    if (-not $match.Success) { throw 'Cannot determine the locked speech library version.' }
    # Match Rust's dynamic MSVC runtime (/MD); Sherpa itself remains statically linked.
    $name = "sherpa-onnx-v$($match.Groups[1].Value)-win-x64-static-MD-Release-lib"
    $archive = Join-Path $ProjectRoot "third_party/sherpa-onnx/$name.tar.xz"
    if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
        throw "Missing bundled speech libraries. Restore this file from the repository: $archive"
    }

    $cache = Join-Path $ProjectRoot 'target/sherpa-onnx-prebuilt'
    $libraries = Join-Path $cache "$name/lib"
    $ready = Join-Path $libraries '.bundled'
    if (-not (Test-Path -LiteralPath $ready -PathType Leaf)) {
        New-Item -ItemType Directory -Force -Path $cache | Out-Null
        & tar -xJf $archive -C $cache
        if ($LASTEXITCODE -ne 0) { throw 'Could not unpack the bundled speech libraries.' }
        New-Item -ItemType File -Force -Path $ready | Out-Null
    }
    $env:SHERPA_ONNX_LIB_DIR = $libraries
}
