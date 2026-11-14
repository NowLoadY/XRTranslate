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

Release builds require Python 3.12+. The script builds the application and
current public plugins. Locally installed and private plugins are not included. Optional
offline GGUF models are not included. The workflow creates
`XRTranslate-v0.3.5-win-x64-setup.exe`, using Inno Setup.
With **compatibility_zip** enabled, it also creates
`XRTranslate-v0.3.5-win-x64.zip` for older clients. This ZIP opens the same installer
at the existing application location, preserving user data and registering the
installation. Cancelling Setup leaves the old application unchanged.
The selected packages are uploaded using GitHub's automatic workflow token.
The workflow does not create a Release or edit its title,
notes, or other platform assets.

Setup refreshes application files and bundled plugins. User settings, downloaded
models, GPU runtimes and independently installed plugins are preserved.

Before uploading, the Windows job runs the packaged backend, installer and
updater with `--version`, starts the packaged XR-Corpus service, and checks its
health/version and seeded vocabulary graph through local HTTP requests. The
smoke test uses a temporary configuration and database outside the package,
then stops its service. It does not test GPU models
or full speech inference.

Publish the setup executable as the main Windows download. Keep **compatibility_zip**
enabled for a transition release newer than all previously published ZIP-only
versions. Retain that release and its ZIP permanently; later releases may disable
this option. Existing portable
copies upgrade in place and keep their settings, models and plugin data.

Rerunning replaces only the selected Windows ZIP and setup executable. Runs for the same
Release are serialized without cancelling an active build. The workflow logs
the exact source commit and leaves the published tag unchanged.

## Windows distribution

Build the release directory once, then choose a distribution format:

```powershell
$package = 'dist\XRTranslate-v0.4.5-win-x64'
.\build_release.ps1 -Output $package
.\scripts\build-windows-distribution.ps1 -Package $package -Format Setup -CompatibilityZip
```

Setup uses the bundled Inno Setup compiler. Use `-Iscc` to select another compiler.
It installs for the current Windows user, registers XRTranslate in Installed
Apps, and keeps upgrades at the registered location. The installer also offers
moving the installation or uninstalling it while keeping user data. New installations keep
settings, downloads and personal plugins in the chosen data folder; portable
copies upgraded in place retain their existing data location. The installer only
replaces its own files.

User data can be moved from either the installer or **Settings > General > Folders**.
Settings also shows both folders and opens them in the system file manager.

For an offline Setup, use `-Offline -OfflineRuntime Vulkan` or
`-Offline -OfflineRuntime Cuda`. Supply the configured GGUF models in `models/`
and runtime ZIPs in `third_party/llama.cpp`, including the matching CUDA
redistributable for CUDA builds. Include their licenses. The command produces
an `-offline-vulkan-setup.exe` or `-offline-cuda-setup.exe` package.

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

## Cleanup

Run `clean.cmd` to preview the files selected for removal. Add `-Apply` to
confirm deletion. Review the preview and close programs using those paths first.

```bat
clean.cmd
clean.cmd -Apply
```
