# Windows release

The [Windows release workflow](../.github/workflows/windows-release.yml) runs
only through **Actions → Windows release → Run workflow**. Pushing commits or
tags does not start a release build.

Create and push a tag such as `v0.3.3`, matching `workspace.package.version` in
`Cargo.toml`, and create its GitHub Release with the intended release notes.
The tag must include the current `build_release.ps1`; its recorded XR-Corpus
submodule commit must also be available on GitHub. Then run the workflow from
`main` and enter that existing tag. The workflow checks out the tag and the
exact submodule revisions it records.

The Windows 2022 runner uses Visual Studio 2022 and Rust 1.95.0. It invokes the
same default build as a local Windows release:

```powershell
.\build_release.ps1 -Output dist\XRTranslate-v0.3.3-win-x64
```

The script prepares the standard native binaries and verified runtime/model
resources. Optional offline GGUF models are not included. The workflow creates
`XRTranslate-v0.3.3-win-x64.zip` with the directory's contents directly at the
ZIP root: `XRTranslate-v0.3.3.exe`, `bin/`, `config.json`, and the other package
files, matching previous Windows releases. It uploads this ZIP using GitHub's
automatic workflow token. It does not create a Release or edit its title,
notes, or other platform assets.

Before uploading, the Windows job runs the packaged backend, installer and
updater with `--version`, starts the packaged XR-Corpus service, and checks its
health/version and seeded vocabulary graph through local HTTP requests. The
smoke test uses a temporary configuration and database outside the package,
then stops its service. It does not test GPU models or full speech inference.

Rerun the workflow with the same tag to replace only the identically named
Windows ZIP. Runs for the same tag are serialized without cancelling an active
build. Changes to source or build inputs require a new tag; do not move an
already published tag to rebuild different code.
