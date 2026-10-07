//! Application update discovery, staging, and handoff to the updater process.
//!
//! The UI owns only the state machine. Network, archive validation, and
//! staging run on a worker thread; replacing the running application is handed
//! to `xrtranslate-updater` on desktop and the system package installer on Android.

use crate::client_settings::UpdateChannel;
use crossbeam_channel::{Receiver, TryRecvError, unbounded};
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, CONTENT_LENGTH, HeaderValue};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};
#[cfg(not(target_os = "android"))]
use std::{io, process::Command};
use xrtranslate_download::{DownloadClient, DownloadSpec};

const RELEASES_URL: &str = "https://api.github.com/repos/NowLoadY/XRTranslate/releases";
const RELEASES_PAGE: &str = "https://github.com/NowLoadY/XRTranslate/releases";
const RELEASE_DOWNLOAD_BASE: &str = "https://github.com/NowLoadY/XRTranslate/releases/download/";
const USER_AGENT: &str = concat!("XRTranslate updater/", env!("CARGO_PKG_VERSION"));
const GITHUB_API_VERSION: &str = "2022-11-28";

#[derive(Clone, Debug)]
pub struct AppUpdateInfo {
    pub version: String,
    pub asset_name: String,
    pub size: u64,
}

#[derive(Clone, Debug, Default)]
pub enum AppUpdateState {
    #[default]
    Idle,
    Checking,
    Current,
    Available(AppUpdateInfo),
    Downloading {
        info: AppUpdateInfo,
        downloaded: u64,
        total: u64,
    },
    Ready(AppUpdateInfo),
    Installing,
    Failed(String),
}

impl AppUpdateState {
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        matches!(
            self,
            Self::Checking | Self::Downloading { .. } | Self::Installing
        )
    }
}

#[derive(Clone, Debug)]
pub struct PreparedUpdate {
    source: PathBuf,
    #[cfg(not(target_os = "android"))]
    project_root: PathBuf,
    #[cfg(not(target_os = "android"))]
    updater_entrypoint: String,
    info: AppUpdateInfo,
}

#[cfg(not(target_os = "android"))]
#[derive(Debug)]
pub struct AppUpdateInstall {
    pub updater: PathBuf,
    pub source: PathBuf,
    pub target: PathBuf,
}

#[derive(Debug)]
enum Event {
    Checked(Result<Option<ReleaseAsset>, String>),
    Downloading { downloaded: u64, total: u64 },
    Prepared(Result<PreparedUpdate, String>),
}

#[derive(Default)]
pub struct AppUpdateManager {
    state: AppUpdateState,
    events: Option<Receiver<Event>>,
    available: Option<ReleaseAsset>,
    prepared: Option<PreparedUpdate>,
    proxy_url: Option<String>,
    channel: UpdateChannel,
}

impl AppUpdateManager {
    pub const fn is_supported() -> bool {
        cfg!(any(
            target_os = "windows",
            target_os = "linux",
            target_os = "android"
        ))
    }

    pub fn set_proxy_url(&mut self, proxy_url: &str) {
        self.proxy_url = (!proxy_url.trim().is_empty()).then(|| proxy_url.trim().to_owned());
    }
    pub fn set_channel(&mut self, channel: UpdateChannel) {
        if self.channel != channel {
            self.events = None;
            self.channel = channel;
            self.state = AppUpdateState::Idle;
            self.available = None;
            self.prepared = None;
        }
    }
    #[must_use]
    pub fn state(&self) -> &AppUpdateState {
        &self.state
    }

    #[must_use]
    pub fn is_busy(&self) -> bool {
        self.state.is_busy()
    }

    pub fn check(&mut self) -> Result<(), String> {
        if !Self::is_supported() || self.is_busy() {
            return Ok(());
        }
        let (sender, receiver) = unbounded();
        let proxy_url = self.proxy_url.clone();
        let channel = self.channel;
        thread::Builder::new()
            .name("app-update-checker".into())
            .spawn(move || {
                let result = run_async(|| check_latest_release(proxy_url.as_deref(), channel));
                let _ = sender.send(Event::Checked(result));
            })
            .map_err(|error| format!("Cannot start update checker: {error}"))?;
        self.state = AppUpdateState::Checking;
        self.events = Some(receiver);
        Ok(())
    }

    pub fn download(&mut self, project_root: PathBuf) -> Result<(), String> {
        if self.is_busy() {
            return Ok(());
        }
        let asset = self
            .available
            .clone()
            .ok_or("Check for updates before downloading.")?;
        let info = asset.info();
        let (sender, receiver) = unbounded();
        let proxy_url = self.proxy_url.clone();
        thread::Builder::new()
            .name("app-update-downloader".into())
            .spawn(move || {
                let progress = sender.clone();
                let result = run_async(|| {
                    download_and_stage(project_root, asset, progress, proxy_url.as_deref())
                });
                let _ = sender.send(Event::Prepared(result));
            })
            .map_err(|error| format!("Cannot start update download: {error}"))?;
        self.state = AppUpdateState::Downloading {
            downloaded: 0,
            total: info.size,
            info,
        };
        self.events = Some(receiver);
        Ok(())
    }

    #[cfg(not(target_os = "android"))]
    pub fn begin_install(&mut self) -> Result<AppUpdateInstall, String> {
        if self.is_busy() {
            return Err("An update task is already running.".into());
        }
        let prepared = self
            .prepared
            .clone()
            .ok_or("Download the update before installing.")?;
        let updater = prepared.source.join(&prepared.updater_entrypoint);
        if !updater.is_file() {
            return Err("The update installer is missing from the downloaded package.".into());
        }
        self.state = AppUpdateState::Installing;
        Ok(AppUpdateInstall {
            updater,
            source: prepared.source,
            target: prepared.project_root,
        })
    }

    #[cfg(target_os = "android")]
    pub fn begin_install(&mut self) -> Result<(), String> {
        if self.is_busy() {
            return Err("An update task is already running.".into());
        }
        let prepared = self
            .prepared
            .as_ref()
            .ok_or("Download the update before installing.")?;
        crate::android::request_update_install(&prepared.source)?;
        self.state = AppUpdateState::Installing;
        Ok(())
    }

    pub fn poll(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(result) = crate::android::take_update_install_result() {
            self.state = match result {
                Ok(()) => self
                    .prepared
                    .as_ref()
                    .map_or(AppUpdateState::Idle, |prepared| {
                        AppUpdateState::Ready(prepared.info.clone())
                    }),
                Err(error) => AppUpdateState::Failed(error),
            };
        }
        let Some(events) = &self.events else {
            return;
        };
        let mut finished = false;
        loop {
            match events.try_recv() {
                Ok(Event::Checked(result)) => {
                    match result {
                        Ok(Some(asset)) => {
                            self.state = AppUpdateState::Available(asset.info());
                            self.available = Some(asset);
                            self.prepared = None;
                        }
                        Ok(None) => {
                            self.state = AppUpdateState::Current;
                            self.available = None;
                            self.prepared = None;
                        }
                        Err(error) => self.state = AppUpdateState::Failed(error),
                    }
                    finished = true;
                    break;
                }
                Ok(Event::Downloading { downloaded, total }) => {
                    if let AppUpdateState::Downloading { info, .. } = &self.state {
                        self.state = AppUpdateState::Downloading {
                            info: info.clone(),
                            downloaded,
                            total,
                        };
                    }
                }
                Ok(Event::Prepared(result)) => {
                    match result {
                        Ok(prepared) => {
                            self.state = AppUpdateState::Ready(prepared.info.clone());
                            self.prepared = Some(prepared);
                        }
                        Err(error) => self.state = AppUpdateState::Failed(error),
                    }
                    finished = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.state =
                        AppUpdateState::Failed("The update worker stopped unexpectedly.".into());
                    finished = true;
                    break;
                }
            }
        }
        if finished {
            self.events = None;
        }
    }
}

#[derive(Clone, Debug)]
struct ReleaseAsset {
    version: String,
    name: String,
    download_url: String,
    size: u64,
    sha256: Option<String>,
}

impl ReleaseAsset {
    fn info(&self) -> AppUpdateInfo {
        AppUpdateInfo {
            version: self.version.clone(),
            asset_name: self.name.clone(),
            size: self.size,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<GitHubAsset>,
}

#[derive(Clone, Debug, Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

async fn check_latest_release(
    proxy_url: Option<&str>,
    channel: UpdateChannel,
) -> Result<Option<ReleaseAsset>, String> {
    if !AppUpdateManager::is_supported() {
        return Err("Updates are not supported on this platform.".into());
    }
    let client = http_client(proxy_url)?;
    match fetch_releases(&client).await {
        Ok(releases) => release_asset_from_catalogue(releases, channel),
        Err(api_error) => {
            fallback_catalogue_asset(&client, channel)
                .await
                .map_err(|fallback_error| {
                    format!(
                        "Cannot check for updates: GitHub API failed ({api_error}); \
                     release page fallback failed ({fallback_error})."
                    )
                })
        }
    }
}

async fn fetch_releases(client: &reqwest::Client) -> Result<Vec<GitHubRelease>, String> {
    let mut releases = Vec::new();
    for page in 1.. {
        let batch: Vec<GitHubRelease> =
            fetch_github_json(client, &format!("{RELEASES_URL}?per_page=100&page={page}")).await?;
        let complete = batch.len() < 100;
        releases.extend(batch);
        if complete {
            break;
        }
    }
    Ok(releases)
}

fn release_asset_from_catalogue(
    releases: Vec<GitHubRelease>,
    channel: UpdateChannel,
) -> Result<Option<ReleaseAsset>, String> {
    let current = parse_version(crate::version::APP_VERSION)?;
    Ok(releases
        .into_iter()
        .filter(|release| !release.draft)
        .filter_map(|release| {
            let version = parse_version(&release.tag_name).ok()?;
            if version <= current
                || (channel == UpdateChannel::Stable
                    && (release.prerelease || !version.is_stable()))
            {
                return None;
            }
            let asset = select_release_asset(&release.assets)?;
            Some((version, asset.clone()))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(version, asset)| ReleaseAsset {
            version: version.to_string(),
            name: asset.name,
            download_url: asset.browser_download_url,
            size: asset.size,
            sha256: asset.digest.as_deref().and_then(parse_sha256_digest),
        }))
}

async fn fallback_catalogue_asset(
    client: &reqwest::Client,
    channel: UpdateChannel,
) -> Result<Option<ReleaseAsset>, String> {
    let mut tags = Vec::new();
    for page in 1.. {
        let html = client
            .get(format!("{RELEASES_PAGE}?page={page}"))
            .send()
            .await
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .text()
            .await
            .map_err(|error| error.to_string())?;
        let page_tags = release_tags_from_html(&html);
        if page_tags.is_empty() {
            return Err("The release page did not list any release versions.".into());
        }
        tags.extend(page_tags);
        if !html.contains("rel=\"next\"") {
            break;
        }
    }
    let current = parse_version(crate::version::APP_VERSION)?;
    let mut candidates = tags
        .into_iter()
        .filter_map(|tag| parse_version(&tag).ok().map(|version| (version, tag)))
        .filter(|(version, _)| {
            version > &current && (channel == UpdateChannel::Beta || version.is_stable())
        })
        .collect::<Vec<_>>();
    candidates.sort_unstable_by(|(left, _), (right, _)| right.cmp(left));
    for (version, tag) in candidates {
        if let Some(asset) = fallback_release_asset(client, &tag, &version.to_string()).await? {
            return Ok(Some(asset));
        }
    }
    Ok(None)
}

fn release_tags_from_html(html: &str) -> Vec<String> {
    const MARKER: &str = "/releases/tag/";
    let mut tags = Vec::new();
    let mut remaining = html;
    while let Some(index) = remaining.find(MARKER) {
        remaining = &remaining[index + MARKER.len()..];
        let end = remaining
            .find(['\"', '\'', '<', '?', '#'])
            .unwrap_or(remaining.len());
        let tag = &remaining[..end];
        if !tag.is_empty() && !tags.iter().any(|existing| existing == tag) {
            tags.push(tag.to_owned());
        }
        remaining = &remaining[end..];
    }
    tags
}

async fn fetch_github_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, String> {
    let response = client
        .get(url)
        .header(ACCEPT, "application/vnd.github+json")
        .header(
            "X-GitHub-Api-Version",
            HeaderValue::from_static(GITHUB_API_VERSION),
        )
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(github_status_error(&response));
    }
    response
        .json::<T>()
        .await
        .map_err(|error| error.to_string())
}

fn github_status_error(response: &reqwest::Response) -> String {
    let status = response.status();
    let remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .and_then(|value| value.to_str().ok());
    let reset = response
        .headers()
        .get("x-ratelimit-reset")
        .and_then(|value| value.to_str().ok());
    if status == reqwest::StatusCode::FORBIDDEN && remaining == Some("0") {
        return reset.map_or_else(
            || "GitHub API rate limit exceeded".into(),
            |reset| format!("GitHub API rate limit exceeded; reset timestamp: {reset}"),
        );
    }
    format!("HTTP {status}")
}

async fn fallback_release_asset(
    client: &reqwest::Client,
    tag: &str,
    version: &str,
) -> Result<Option<ReleaseAsset>, String> {
    let mut names = vec![standard_release_asset_name(version)];
    if cfg!(target_os = "android") {
        names.push(format!("XRTranslate-v{version}-android-universal.apk"));
    }
    for name in names {
        if let Some(asset) = probe_release_asset(client, tag, version, name).await? {
            return Ok(Some(asset));
        }
    }
    Ok(None)
}

async fn probe_release_asset(
    client: &reqwest::Client,
    tag: &str,
    version: &str,
    name: String,
) -> Result<Option<ReleaseAsset>, String> {
    let mut download_url = reqwest::Url::parse(RELEASE_DOWNLOAD_BASE)
        .map_err(|error| format!("invalid release URL: {error}"))?;
    download_url
        .path_segments_mut()
        .map_err(|_| "invalid release URL".to_string())?
        .pop_if_empty()
        .push(tag)
        .push(&name);
    let response = client
        .head(download_url.clone())
        .header(ACCEPT_ENCODING, "identity")
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let response = response
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let size = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
        .filter(|size| *size > 0)
        .ok_or("release server did not report the package size")?;
    Ok(Some(ReleaseAsset {
        version: version.to_owned(),
        name,
        download_url: download_url.into(),
        size,
        sha256: None,
    }))
}

fn standard_release_asset_name(version: &str) -> String {
    if cfg!(target_os = "android") {
        format!(
            "XRTranslate-v{version}-android-{}.apk",
            android_architecture()
        )
    } else {
        let platform = if cfg!(target_os = "windows") {
            "win-x64"
        } else {
            "linux-x64"
        };
        format!("XRTranslate-v{version}-{platform}.zip")
    }
}

fn android_architecture() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

fn android_asset_matches(name: &str, architecture: &str) -> bool {
    let Some(version) = name.strip_prefix("xrtranslate-v") else {
        return false;
    };
    [architecture, "universal"].iter().any(|arch| {
        version
            .strip_suffix(&format!("-android-{arch}.apk"))
            .is_some_and(|version| parse_version(version).is_ok())
    })
}

async fn download_and_stage(
    project_root: PathBuf,
    asset: ReleaseAsset,
    sender: crossbeam_channel::Sender<Event>,
    proxy_url: Option<&str>,
) -> Result<PreparedUpdate, String> {
    if !asset.download_url.starts_with("https://") {
        return Err("The update download URL is not secure.".into());
    }
    if asset.size == 0 {
        return Err("The update package is empty.".into());
    }
    let updates_root = project_root.join("runtime").join("updates");
    let download_dir = updates_root.join("downloads");
    #[cfg(not(target_os = "android"))]
    let staging_root = updates_root.join(format!("v{}-staging", safe_path_segment(&asset.version)));
    #[cfg(not(target_os = "android"))]
    let payload = staging_root.join("payload");
    fs::create_dir_all(&download_dir)
        .map_err(|error| format!("Cannot create update download folder: {error}"))?;
    #[cfg(not(target_os = "android"))]
    reset_directory(&payload)?;

    if Path::new(&asset.name)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(&asset.name)
        || asset.name.contains(['/', '\\'])
    {
        return Err("The update package name is invalid.".into());
    }
    let archive = download_dir.join(&asset.name);
    let client = DownloadClient::with_proxy(USER_AGENT, proxy_url)
        .map_err(|error| format!("Cannot initialize update download: {error}"))?;
    let spec = asset.sha256.as_deref().map_or_else(
        || DownloadSpec::size_only(&asset.name, &asset.download_url, asset.size),
        |sha256| DownloadSpec::verified(&asset.name, &asset.download_url, asset.size, sha256),
    );
    client
        .download_to(spec, &archive, |progress| {
            let _ = sender.send(Event::Downloading {
                downloaded: progress.downloaded_bytes,
                total: progress.total_bytes,
            });
        })
        .await
        .map_err(|error| format!("Cannot download update: {error}"))?;

    #[cfg(target_os = "android")]
    {
        crate::android::validate_update(&archive, &asset.version)?;
        Ok(PreparedUpdate {
            source: archive,
            info: asset.info(),
        })
    }
    #[cfg(not(target_os = "android"))]
    {
        extract_zip(&archive, &payload)?;
        let source = release_source_directory(&payload)?;
        let updater_entrypoint = validate_staged_release(&source)?;
        Ok(PreparedUpdate {
            source,
            project_root,
            updater_entrypoint,
            info: asset.info(),
        })
    }
}

#[cfg(not(target_os = "android"))]
fn extract_zip(archive: &Path, destination: &Path) -> Result<(), String> {
    let file = fs::File::open(archive)
        .map_err(|error| format!("Cannot open update package {}: {error}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| format!("The update package is not a valid archive: {error}"))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| format!("Cannot read update package: {error}"))?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let output = destination.join(name);
        if entry.is_dir() {
            fs::create_dir_all(&output)
                .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
            continue;
        }
        if let Some(parent) = output.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Cannot create {}: {error}", parent.display()))?;
        }
        let mut file = fs::File::create(&output)
            .map_err(|error| format!("Cannot create {}: {error}", output.display()))?;
        io::copy(&mut entry, &mut file)
            .map_err(|error| format!("Cannot extract {}: {error}", output.display()))?;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&output, fs::Permissions::from_mode(mode)).map_err(|error| {
                format!("Cannot set permissions on {}: {error}", output.display())
            })?;
        }
    }
    Ok(())
}

#[cfg(not(target_os = "android"))]
fn release_source_directory(payload: &Path) -> Result<PathBuf, String> {
    if payload.join("release-manifest.json").is_file() {
        return Ok(payload.to_path_buf());
    }
    let release_roots = fs::read_dir(payload)
        .map_err(|error| format!("Cannot inspect update package: {error}"))?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("release-manifest.json").is_file())
        .collect::<Vec<_>>();
    match release_roots.as_slice() {
        [root] => Ok(root.clone()),
        _ => Err("The update package does not contain a valid XRTranslate release.".into()),
    }
}

#[cfg(not(target_os = "android"))]
fn validate_staged_release(source: &Path) -> Result<String, String> {
    let manifest_path = source.join("release-manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).map_err(|error| {
            format!(
                "Cannot read release manifest {}: {error}",
                manifest_path.display()
            )
        })?)
        .map_err(|error| format!("Invalid release manifest: {error}"))?;
    if manifest["python"].as_bool() != Some(false) {
        return Err("The selected release package is not supported by this client.".into());
    }
    let Some(client) = manifest
        .pointer("/entrypoints/client")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return Err("The update package is missing the application entrypoint.".into());
    };
    let Some(updater) = manifest
        .pointer("/entrypoints/updater")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
    else {
        return Err("The update package is missing the update helper.".into());
    };
    if !source.join(client).is_file() || !source.join(updater).is_file() {
        return Err("The update package is incomplete.".into());
    }
    Ok(updater.to_owned())
}

#[cfg(not(target_os = "android"))]
pub fn spawn_updater(install: AppUpdateInstall) -> Result<(), String> {
    let mut command = Command::new(&install.updater);
    command
        .arg("--source")
        .arg(&install.source)
        .arg("--target")
        .arg(&install.target)
        .arg("--current-pid")
        .arg(std::process::id().to_string())
        .arg("--restart")
        .current_dir(&install.target);
    crate::child_process::hide_console(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Cannot start update installer: {error}"))
}

fn select_release_asset(assets: &[GitHubAsset]) -> Option<&GitHubAsset> {
    assets
        .iter()
        .filter(|asset| {
            let name = asset.name.to_ascii_lowercase();
            if cfg!(target_os = "android") {
                android_asset_matches(&name, android_architecture())
            } else {
                name.ends_with(".zip") && name_matches_platform(&name)
            }
        })
        .max_by_key(|asset| platform_asset_score(&asset.name.to_ascii_lowercase()))
}

fn name_matches_platform(name: &str) -> bool {
    let tokens = name
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .collect::<Vec<_>>();
    let arch_ok = tokens.iter().any(|token| matches!(*token, "x64" | "amd64"))
        || tokens.windows(2).any(|pair| pair == ["x86", "64"]);
    let windows = tokens
        .iter()
        .any(|token| matches!(*token, "win" | "windows"));
    let linux = tokens.contains(&"linux");
    if cfg!(target_os = "windows") {
        arch_ok && windows && !linux
    } else if cfg!(target_os = "linux") {
        arch_ok && linux && !windows
    } else {
        false
    }
}

fn platform_asset_score(name: &str) -> u8 {
    if cfg!(target_os = "android") {
        return u8::from(name.ends_with(&format!("-android-{}.apk", android_architecture())));
    }
    let mut score = 0;
    if name.contains("x64") {
        score += 2;
    }
    if cfg!(target_os = "windows") && name.contains("win-x64") {
        score += 3;
    }
    if cfg!(target_os = "linux") && name.contains("linux-x64") {
        score += 3;
    }
    score
}

#[cfg(test)]
fn version_is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Ok(latest), Ok(current)) => latest > current,
        _ => false,
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ParsedVersion {
    core: [u64; 3],
    beta_rank: u64,
}

impl ParsedVersion {
    fn is_stable(&self) -> bool {
        self.beta_rank == u64::MAX
    }
}

impl std::fmt::Display for ParsedVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}.{}.{}",
            self.core[0], self.core[1], self.core[2]
        )?;
        if !self.is_stable() {
            write!(formatter, "-beta.{}", self.beta_rank)?;
        }
        Ok(())
    }
}

fn parse_version(value: &str) -> Result<ParsedVersion, String> {
    let normalized = value.trim().trim_start_matches(['v', 'V']);
    let normalized = normalized
        .split_once('+')
        .map_or(normalized, |parts| parts.0);
    let (core, suffix) = normalized.split_once('-').unwrap_or((normalized, ""));
    let core = core
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("Invalid release version {value:?}: {error}"))?;
    let [major, minor, patch] = core.as_slice() else {
        return Err(format!(
            "Invalid release version {value:?}: expected major.minor.patch"
        ));
    };
    let beta_rank = if suffix.is_empty() {
        u64::MAX
    } else {
        suffix
            .strip_prefix("beta.")
            .ok_or_else(|| format!("Unsupported release version {value:?}"))?
            .parse::<u64>()
            .map_err(|error| format!("Invalid release version {value:?}: {error}"))?
    };
    Ok(ParsedVersion {
        core: [*major, *minor, *patch],
        beta_rank,
    })
}

fn parse_sha256_digest(value: &str) -> Option<String> {
    let digest = value.strip_prefix("sha256:").unwrap_or(value).trim();
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

fn http_client(proxy_url: Option<&str>) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(45));
    if let Some(proxy_url) = proxy_url.filter(|url| !url.trim().is_empty()) {
        builder = builder.proxy(
            reqwest::Proxy::all(proxy_url)
                .map_err(|error| format!("Cannot configure download proxy: {error}"))?,
        );
    }
    builder
        .build()
        .map_err(|error| format!("Cannot create update client: {error}"))
}

fn run_async<F, Fut, T>(task: F) -> Result<T, String>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("Cannot initialize update worker: {error}"))?
        .block_on(task())
}

#[cfg(not(target_os = "android"))]
fn reset_directory(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path)
            .map_err(|error| format!("Cannot reset {}: {error}", path.display()))?;
    }
    fs::create_dir_all(path).map_err(|error| format!("Cannot create {}: {error}", path.display()))
}

#[cfg(not(target_os = "android"))]
fn safe_path_segment(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_os = "android"))]
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn update_check_failure_notifies_once_without_opening_an_error_modal() {
        use eframe::egui;
        let mut app = crate::XRTranslateApp::default();
        app.ui_language = crate::i18n::UiLanguage::English;
        app.last_error = None;
        app.modal_dialog = Default::default();
        app.notifications = Default::default();
        let ctx = egui::Context::default();
        app.notifications.install(&ctx);
        let expected = "Could not check for updates. Try again later.";

        for attempt in 0..2 {
            let (sender, receiver) = unbounded();
            app.app_update_manager = AppUpdateManager {
                state: AppUpdateState::Checking,
                events: Some(receiver),
                ..Default::default()
            };
            sender
                .send(Event::Checked(Err("Network unavailable".into())))
                .unwrap();
            app.poll_app_update();
            assert!(matches!(app.app_update_state(), AppUpdateState::Failed(_)));
            assert!(app.last_error.is_none());
            assert!(!app.modal_dialog.open);
            for (offset, visible) in [(0.0, true), (0.3, true), (10.0, false)] {
                app.poll_app_update();
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        time: Some(attempt as f64 * 20.0 + offset),
                        ..Default::default()
                    },
                    |_| {
                        crate::ui::automation::begin_frame(&ctx, "update-test");
                        app.render_modal_layer(&ctx);
                        app.notifications.show(&ctx, app.ui_language);
                        crate::ui::automation::finish_frame("update-test");
                    },
                );
                let driver = crate::ui::automation::driver();
                let state = driver.frame_state.lock().unwrap();
                let notice = state.last_snapshot.find_element("Notification");
                assert_eq!(notice.is_some(), visible);
                if let Some(notice) = notice {
                    assert_eq!(notice.value.as_text().as_deref(), Some(expected));
                }
                assert!(ctx.memory(|memory| memory.top_modal_layer()).is_none());
                output.textures_delta.clear();
            }
        }
    }

    #[cfg(not(target_os = "android"))]
    #[test]
    fn begin_install_targets_the_project_root() {
        let root = std::env::temp_dir().join(format!(
            "xrtranslate-update-test-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock before epoch")
                .as_nanos()
        ));
        let source = root.join("staging");
        std::fs::create_dir_all(&source).expect("create staging directory");
        let updater = source.join("updater");
        std::fs::write(&updater, b"test").expect("create updater entrypoint");

        let mut manager = AppUpdateManager {
            prepared: Some(PreparedUpdate {
                source: source.clone(),
                project_root: root.clone(),
                updater_entrypoint: "updater".into(),
                info: AppUpdateInfo {
                    version: "test".into(),
                    asset_name: "test.zip".into(),
                    size: 1,
                },
            }),
            ..Default::default()
        };
        let install = manager.begin_install().expect("begin install");
        assert_eq!(install.target, root);
        assert_eq!(install.source, source);
        let _ = std::fs::remove_dir_all(install.target);
    }

    #[test]
    fn selects_current_platform_zip_assets() {
        for arch in ["arm64", "x64"] {
            assert!(android_asset_matches(
                &format!("xrtranslate-v1.2.0-android-{arch}.apk"),
                arch
            ));
            assert!(android_asset_matches(
                "xrtranslate-v1.2.0-beta.2-android-universal.apk",
                arch
            ));
            for wrong in [
                "linux-x64.zip",
                "win-x64.zip",
                "android-arm64-debug.apk",
                "android-arm64-unsigned.apk",
            ] {
                assert!(!android_asset_matches(
                    &format!("xrtranslate-v1.2.0-{wrong}"),
                    arch
                ));
            }
        }
        assert!(!android_asset_matches(
            "xrtranslate-v1.2.0-android-x64.apk",
            "arm64"
        ));
        assert!(!android_asset_matches(
            "xrtranslate-v1.2.0-android-arm64.apk",
            "x64"
        ));
        let assets = vec![
            GitHubAsset {
                name: "XRTranslate-v1.2.0-android-arm64.apk".into(),
                browser_download_url: "https://example.invalid/arm64.apk".into(),
                size: 1,
                digest: None,
            },
            GitHubAsset {
                name: "XRTranslate-v1.2.0-android-x64.apk".into(),
                browser_download_url: "https://example.invalid/x64.apk".into(),
                size: 1,
                digest: None,
            },
            GitHubAsset {
                name: "XRTranslate-v1.2.0-linux-x64.zip".into(),
                browser_download_url: "https://example.invalid/linux.zip".into(),
                size: 1,
                digest: None,
            },
            GitHubAsset {
                name: "XRTranslate-v1.2.0-win-x64.zip".into(),
                browser_download_url: "https://example.invalid/win.zip".into(),
                size: 1,
                digest: None,
            },
        ];
        assert!(!name_matches_platform("xrtranslate-v1.2.0-darwin-x64.zip"));
        assert!(!name_matches_platform(
            "xrtranslate-v1.2.0-linux-win-x64.zip"
        ));
        let selected = select_release_asset(&assets).unwrap();
        if cfg!(target_os = "windows") {
            assert!(selected.name.contains("win-x64"));
        } else if cfg!(target_os = "linux") {
            assert!(selected.name.contains("linux-x64"));
        }
    }

    #[test]
    fn compares_release_versions_numerically() {
        assert!(version_is_newer("0.10.0", "0.2.9"));
        assert!(!version_is_newer("0.2.0", "0.2.0"));
        assert!(!version_is_newer("0.1.9", "0.2.0"));
        assert!(version_is_newer("0.2.7-beta.2", "0.2.7-beta.1"));
        assert!(version_is_newer("0.2.7", "0.2.7-beta.3"));
        assert!(!version_is_newer("0.2.7-beta.1", "0.2.7"));
    }

    fn release(tag: &str) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.into(),
            draft: false,
            prerelease: tag.contains("-beta."),
            assets: vec![GitHubAsset {
                name: standard_release_asset_name(tag.trim_start_matches('v')),
                browser_download_url: format!("https://example.invalid/{tag}.zip"),
                size: 1,
                digest: None,
            }],
        }
    }

    #[test]
    fn beta_catalogue_prefers_stable_over_prerelease_of_same_version() {
        let selected = release_asset_from_catalogue(
            vec![release("v99.0.0-beta.3"), release("v99.0.0")],
            UpdateChannel::Beta,
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.version, "99.0.0");
    }

    #[test]
    fn stable_catalogue_ignores_prereleases() {
        let mut marked_prerelease = release("v99.0.0");
        marked_prerelease.prerelease = true;
        let selected = release_asset_from_catalogue(
            vec![release("v99.0.0-beta.1"), marked_prerelease],
            UpdateChannel::Stable,
        )
        .unwrap();
        assert!(selected.is_none());
    }

    #[test]
    fn extracts_and_deduplicates_release_tags_from_html() {
        let html = r#"
            <a href="/NowLoadY/XRTranslate/releases/tag/v0.2.8-beta.2">beta</a>
            <a href="/NowLoadY/XRTranslate/releases/tag/v0.2.8">stable</a>
            <a href="/NowLoadY/XRTranslate/releases/tag/v0.2.8">duplicate</a>
        "#;
        assert_eq!(release_tags_from_html(html), ["v0.2.8-beta.2", "v0.2.8"]);
    }

    #[test]
    fn parses_github_sha256_digest() {
        let digest = "a".repeat(64);
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{digest}")).as_deref(),
            Some(digest.as_str())
        );
        assert_eq!(parse_sha256_digest("sha256:not-a-digest"), None);
    }

    #[test]
    fn catalogue_skips_newer_releases_without_current_platform_packages() {
        let mut other_platform = release("v99.0.1");
        other_platform.assets[0].name = if cfg!(target_os = "windows") {
            "XRTranslate-v99.0.1-linux-x64.zip".into()
        } else {
            "XRTranslate-v99.0.1-win-x64.zip".into()
        };
        for channel in [UpdateChannel::Stable, UpdateChannel::Beta] {
            let selected = release_asset_from_catalogue(
                vec![release("v99.0.0"), other_platform.clone()],
                channel,
            )
            .unwrap()
            .unwrap();
            assert_eq!(selected.version, "99.0.0");
        }
        assert!(
            release_asset_from_catalogue(vec![other_platform], UpdateChannel::Beta)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn fallback_asset_name_matches_release_packaging() {
        let name = standard_release_asset_name("0.2.5");
        if cfg!(target_os = "windows") {
            assert_eq!(name, "XRTranslate-v0.2.5-win-x64.zip");
        } else if cfg!(target_os = "android") {
            assert_eq!(
                name,
                format!("XRTranslate-v0.2.5-android-{}.apk", android_architecture())
            );
        } else {
            assert_eq!(name, "XRTranslate-v0.2.5-linux-x64.zip");
        }
    }
}
