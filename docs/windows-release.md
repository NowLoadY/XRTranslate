# Windows release

The [Windows release workflow](../.github/workflows/windows-release.yml) runs
only through **Actions → Windows release → Run workflow**. Pushing commits or
tags does not start a release build.

Create and push a tag such as `v0.3.3`, matching `workspace.package.version` in
`Cargo.toml`, and create its GitHub Release with the intended release notes.
Run the workflow from `main` and enter that existing tag. Leave `source_commit`
empty to build the tag, or enter a full commit SHA to replace an existing
Release's Windows package with that source revision. The source must match the
Release's version and include `build_release.ps1`; its recorded XR-Corpus
submodule commit must also be available on GitHub. The workflow checks out
the selected source and its exact submodule revisions.

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

Rerunning replaces only the identically named Windows ZIP. Runs for the same
Release are serialized without cancelling an active build. The workflow logs
the exact source commit and leaves the published tag unchanged.

Successful runs cache Cargo registry/Git downloads, `target/release`, and the
three bundled ONNX models plus the extracted ONNX CPU core and licenses.
Redundant model archives and the large ONNX GPU source ZIP are excluded; the
cached files let subsequent runs skip those downloads. Configuration, credentials,
user databases and staged release packages are not cached.

Build caches are isolated by Windows runner image, architecture, Rust toolchain,
Cargo manifests/lockfiles and build script. Each source commit gets a new cache
entry and can restore compatible artifacts from an earlier commit. The workflow
always runs Cargo and package validation: Cargo rebuilds changed crates and any
dependencies whose inputs are no longer fresh. Resource caches have no fallback
when resource configuration or preparation code changes, so those changes fetch
the corresponding assets again. A first run or an evicted cache downloads and
builds normally; cache hits do not skip the package smoke tests.
