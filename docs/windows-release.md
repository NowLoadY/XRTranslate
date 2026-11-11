# Windows release

The [Windows release workflow](../.github/workflows/windows-release.yml) runs
only through **Actions → Windows release → Run workflow**. Pushing commits or
tags does not start a release build.

Create and push a tag such as `v0.3.5`, matching `workspace.package.version` in
`Cargo.toml`, and create its GitHub Release with the intended release notes.
Run the workflow from `main` and enter that existing tag. Leave `source_commit`
empty to build the tag, or enter a full commit SHA to replace an existing
Release's Windows package with that source revision. The source must match the
Release's version and include `build_release.ps1`; its recorded XR-Corpus
submodule commit must also be available on GitHub. The workflow checks out
the selected source and its exact submodule revisions.

The Windows Server 2025 runner uses Visual Studio 2026 and Rust 1.99.1. It invokes the
same default build as a local Windows release:

```powershell
.\build_release.ps1 -Output dist\XRTranslate-v0.3.5-win-x64
```

Release builds require Python 3 and the GitHub CLI (`gh`). The script prepares
resources once, reuses its download cache, and builds the application and current
public plugins. Locally installed and private plugins are not included. Optional
offline GGUF models are not included. The workflow creates
`XRTranslate-v0.3.5-win-x64-setup.exe`, using the runner's Inno Setup compiler.
With **compatibility_zip** enabled, it also creates
`XRTranslate-v0.3.5-win-x64.zip` with the same layout as previous Windows releases.
The selected packages are uploaded using GitHub's automatic workflow token.
The workflow does not create a Release or edit its title,
notes, or other platform assets.

The required speech libraries are included in `third_party/sherpa-onnx` and
are also used by the source launcher. Clearing the build cache does not
require downloading them again.

Before uploading, the Windows job runs the packaged backend, installer and
updater with `--version`, starts the packaged XR-Corpus service, and checks its
health/version and seeded vocabulary graph through local HTTP requests. The
smoke test uses a temporary configuration and database outside the package,
then stops its service. Every file in the ZIP is checked against the verified
release directory by size and SHA-256 before upload. It does not test GPU models
or full speech inference.

Publish the setup executable as the main Windows download. Keep **compatibility_zip**
enabled for a transition release newer than all previously published ZIP-only
versions. Retain that release and its ZIP permanently; later releases may disable
this option. Existing portable
copies upgrade in place and keep their settings, models and plugin data.

Rerunning replaces only the selected Windows ZIP and setup executable. Runs for the same
Release are serialized without cancelling an active build. The workflow logs
the exact source commit and leaves the published tag unchanged.

Successful runs cache Cargo registry/Git downloads, `target/release`, and the
downloaded model/runtime archives, the VAD and speaker ONNX models, and the
extracted ONNX CPU core and licenses.
GTCRN and its license are embedded in the packager and always written to the package.
Cached resources let subsequent runs skip those downloads. Configuration, credentials,
user databases and staged release packages are not cached.

Build caches are isolated by Windows runner image, architecture, Rust toolchain,
Cargo manifests/lockfiles and build script. Each source commit gets a new cache
entry and can restore compatible artifacts from an earlier commit. The workflow
always runs Cargo and package validation: Cargo rebuilds changed crates and any
dependencies whose inputs are no longer fresh. Resource caches have no fallback
when resource configuration or preparation code changes, so those changes fetch
the corresponding assets again. A first run or an evicted cache downloads and
builds normally; cache hits do not skip the package smoke tests.

## Windows distribution

Build the release directory once, then choose a distribution format:

```powershell
$package = 'dist\XRTranslate-v0.4.5-win-x64'
.\build_release.ps1 -Output $package
.\scripts\build-windows-distribution.ps1 -Package $package -Format Setup
```

Setup requires Inno Setup 6.6 or newer (`-Iscc` can specify its compiler).
It installs for the current Windows user, registers XRTranslate in Installed
Apps, and keeps upgrades at the registered location. The installer also offers
moving the installation or uninstalling it while keeping user data. New installations keep
settings, downloads and personal plugins in the Windows user profile; portable
copies upgraded in place retain their existing data location. The installer only
replaces its own files.

User data can be moved from either the installer or **Settings > General > Folders**.
Settings also shows both folders and opens them in the system file manager.

For Microsoft Store, supply the exact identity from Partner Center:

```powershell
.\scripts\build-windows-distribution.ps1 -Package $package -Format Msix `
  -IdentityName '<Partner Center identity name>' `
  -Publisher '<Partner Center publisher ID>' `
  -PublisherDisplayName '<Partner Center publisher name>'
```

This uses the Windows SDK's MakeAppx to validate and create an unsigned MSIX
for Store submission. Local sideloading requires signing with a trusted
certificate matching that identity. Store installation and updates remain
under Windows control.

Platform-managed distributions use their platform's updates and shortcuts.
Separately installed plugins use the same `plugins/<plugin-id>/` layout.

The standalone installer creates one Start Menu entry. Closing the window keeps
XRTranslate in the notification area; its menu provides **Exit**. Opening the
application again restores the running window.

Production standalone installers need trusted Authenticode signing before
publication. Building an installer alone does not remove SmartScreen warnings.

## Running from source

`start_rust_client.cmd` and `start_rust_client.ps1` build and run Release by default,
including the backend and plugins, matching the Linux source launcher. For an
explicit development build, use `.\start_rust_client.ps1 -Release:$false`.
Launchers always build current bundled plugin sources before starting, and stop
if a build fails. They do not download or update independently installed plugins.

## Manual cleanup

Run `clean.cmd` from the checkout to preview removable files and their sizes.
Nothing is deleted until you add `-Apply`:

```bat
clean.cmd
clean.cmd -Apply
clean.cmd -Scope All
clean.cmd -Scope All -Apply
```

The default keeps the Release build cache. `-Scope All` also clears it, so the
next build takes longer. `-Scope Build` selects build caches only;
`-Scope Packages` selects temporary staged packages only.

Cleanup preserves `dist`, project data, installed plugins, downloaded models,
tool/download caches, and unrecognized temporary files. Temporary package copies
include their bundled resources; copies with runtime data are kept. Close any
running builds and applications using the listed paths before applying cleanup.
