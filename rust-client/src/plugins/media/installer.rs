//! In-app MPV runtime installer for Windows media playback.
//!
//! Downloads the compressed `mpv-2.zip` from the repository and extracts
//! `mpv-2.dll` and `libmpv-2.dll` into `resources/bin/`.

use crossbeam_channel::{Receiver, TryRecvError, unbounded};
use std::{fs, io, path::Path, thread};
use xrtranslate_download::{DownloadClient, DownloadProgress, DownloadSpec};

pub const MPV_DOWNLOAD_URL: &str =
    "https://github.com/NowLoadY/XRTranslate/raw/main/rust-client/resources/bin/mpv-2.zip";
pub const MPV_DOWNLOAD_BYTES: u64 = 46_816_131;
pub const MPV_DOWNLOAD_SHA256: &str =
    "378b772e0d3db87b35f26540f73a3c4fb8ba15ad7882b084a6233caa164c5944";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MpvInstallState {
    Idle,
    Downloading { downloaded: u64, total: u64 },
    Extracting,
    Ready,
    Failed(String),
}

impl MpvInstallState {
    pub fn is_busy(&self) -> bool {
        matches!(self, Self::Downloading { .. } | Self::Extracting)
    }
}

enum Event {
    Downloading { downloaded: u64, total: u64 },
    Extracting,
    Finished(Result<(), String>),
}

pub struct MpvInstaller {
    state: MpvInstallState,
    events: Option<Receiver<Event>>,
    proxy_url: Option<String>,
}

impl Default for MpvInstaller {
    fn default() -> Self {
        Self {
            state: MpvInstallState::Idle,
            events: None,
            proxy_url: None,
        }
    }
}

impl MpvInstaller {
    pub fn set_proxy_url(&mut self, proxy_url: Option<String>) {
        self.proxy_url = proxy_url.filter(|u| !u.trim().is_empty());
    }

    pub fn state(&self) -> &MpvInstallState {
        &self.state
    }

    pub fn is_busy(&self) -> bool {
        self.state.is_busy()
    }

    pub fn start_download(&mut self) -> Result<(), String> {
        if self.is_busy() {
            return Err("A download is already in progress.".into());
        }

        let (sender, receiver) = unbounded();
        let proxy_url = self.proxy_url.clone();

        thread::Builder::new()
            .name("mpv-downloader".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("Cannot create download runtime: {error}"))
                    .and_then(|runtime| {
                        runtime.block_on(download_and_install_mpv(
                            sender.clone(),
                            proxy_url.as_deref(),
                        ))
                    });
                let _ = sender.send(Event::Finished(result));
            })
            .map_err(|error| format!("Cannot start MPV downloader thread: {error}"))?;

        self.state = MpvInstallState::Downloading {
            downloaded: 0,
            total: MPV_DOWNLOAD_BYTES,
        };
        self.events = Some(receiver);
        Ok(())
    }

    pub fn poll(&mut self) -> Option<Result<(), String>> {
        let Some(events) = &self.events else {
            return None;
        };
        let mut final_result = None;
        loop {
            match events.try_recv() {
                Ok(Event::Downloading { downloaded, total }) => {
                    self.state = MpvInstallState::Downloading { downloaded, total };
                }
                Ok(Event::Extracting) => {
                    self.state = MpvInstallState::Extracting;
                }
                Ok(Event::Finished(result)) => {
                    match &result {
                        Ok(()) => self.state = MpvInstallState::Ready,
                        Err(error) => self.state = MpvInstallState::Failed(error.clone()),
                    }
                    final_result = Some(result);
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let err = "MPV downloader thread terminated unexpectedly.".to_string();
                    self.state = MpvInstallState::Failed(err.clone());
                    final_result = Some(Err(err));
                    break;
                }
            }
        }
        if final_result.is_some() {
            self.events = None;
        }
        final_result
    }
}

async fn download_and_install_mpv(
    sender: crossbeam_channel::Sender<Event>,
    proxy_url: Option<&str>,
) -> Result<(), String> {
    let temp_dir = std::env::temp_dir().join(format!("xrtranslate_mpv_{}", std::process::id()));
    fs::create_dir_all(&temp_dir).map_err(|e| {
        format!(
            "Cannot create temporary directory {}: {e}",
            temp_dir.display()
        )
    })?;

    let complete_path = temp_dir.join("mpv-2.zip");

    let client = DownloadClient::with_proxy("XRTranslate", proxy_url)
        .map_err(|e| format!("Cannot initialize download client: {e}"))?;

    let spec = DownloadSpec::verified(
        "mpv-2.zip",
        MPV_DOWNLOAD_URL,
        MPV_DOWNLOAD_BYTES,
        MPV_DOWNLOAD_SHA256,
    );

    client
        .download_to(spec, &complete_path, |progress: DownloadProgress| {
            let _ = sender.send(Event::Downloading {
                downloaded: progress.downloaded_bytes,
                total: progress.total_bytes,
            });
        })
        .await
        .map_err(|e| format!("Download failed: {e}"))?;

    let _ = sender.send(Event::Extracting);

    // Extract to target bin directories
    for bin_dir in super::runtime_bin_directories() {
        let _ = fs::create_dir_all(&bin_dir);
        if bin_dir.is_dir() {
            extract_mpv_zip(&complete_path, &bin_dir)?;
        }
    }

    let _ = fs::remove_dir_all(&temp_dir);

    // Configure DLL search path on Windows
    #[cfg(windows)]
    {
        use windows::Win32::System::LibraryLoader::SetDllDirectoryW;
        use windows::core::HSTRING;
        for bin_dir in super::runtime_bin_directories() {
            if bin_dir.is_dir() {
                let _ = unsafe { SetDllDirectoryW(&HSTRING::from(bin_dir.as_os_str())) };
                break;
            }
        }
    }

    Ok(())
}

fn extract_mpv_zip(archive_path: &Path, target_dir: &Path) -> Result<(), String> {
    let file = fs::File::open(archive_path).map_err(|e| format!("Cannot open archive: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("Invalid zip archive: {e}"))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| format!("Failed reading zip entry: {e}"))?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let out_path = target_dir.join(name);
        if entry.is_dir() {
            fs::create_dir_all(&out_path).map_err(|e| format!("Cannot create dir: {e}"))?;
            continue;
        }
        if let Some(p) = out_path.parent() {
            fs::create_dir_all(p).map_err(|e| format!("Cannot create dir: {e}"))?;
        }
        let mut out_file =
            fs::File::create(&out_path).map_err(|e| format!("Cannot create file: {e}"))?;
        io::copy(&mut entry, &mut out_file).map_err(|e| format!("Extraction copy failed: {e}"))?;
    }
    // If mpv-2.dll exists but libmpv-2.dll does not, copy it
    let mpv_path = target_dir.join("mpv-2.dll");
    let libmpv_path = target_dir.join("libmpv-2.dll");
    if mpv_path.is_file() && !libmpv_path.exists() {
        let _ = fs::copy(&mpv_path, &libmpv_path);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_mpv_zip_extracts_and_creates_libmpv_copy() {
        let temp =
            std::env::temp_dir().join(format!("xrt_mpv_extract_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        let zip_path = temp.join("test_mpv.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mpv-2.dll", options).unwrap();
            use std::io::Write;
            zip.write_all(b"fake_mpv_dll_binary").unwrap();
            zip.finish().unwrap();
        }

        let target_dir = temp.join("resources_bin");
        fs::create_dir_all(&target_dir).unwrap();

        extract_mpv_zip(&zip_path, &target_dir).unwrap();

        assert_eq!(
            fs::read(target_dir.join("mpv-2.dll")).unwrap(),
            b"fake_mpv_dll_binary"
        );
        assert_eq!(
            fs::read(target_dir.join("libmpv-2.dll")).unwrap(),
            b"fake_mpv_dll_binary"
        );

        let _ = fs::remove_dir_all(&temp);
    }
}
