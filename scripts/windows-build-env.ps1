function Initialize-WindowsSpeechLibraries {
    param([Parameter(Mandatory)][string]$ProjectRoot)

    # Always select this checkout's /MD libraries, including after an
    # upgrade in the same PowerShell session. Cargo watches this environment path.
    $libraries = & python (Join-Path $ProjectRoot 'scripts/sherpa_libraries.py') --platform windows-x64
    if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the bundled speech libraries.' }
    $env:SHERPA_ONNX_LIB_DIR = $libraries.Trim()
}
