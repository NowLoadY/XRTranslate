use eframe::egui;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::media_import::podcast::{self, PodcastEpisode};

#[derive(Default)]
pub(crate) struct PodcastBrowser {
    pub url: String,
    pub episodes: Vec<PodcastEpisode>,
    pub selected: usize,
    resolved_url: String,
    pending: Option<Lookup>,
}

struct Lookup {
    stop: Arc<AtomicBool>,
    result: crossbeam_channel::Receiver<Result<Vec<PodcastEpisode>, String>>,
}

impl Drop for Lookup {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

impl PodcastBrowser {
    pub fn render_selection(
        &mut self,
        ui: &mut egui::Ui,
        language: crate::i18n::UiLanguage,
        proxy: &str,
    ) -> Result<bool, String> {
        let mut changed = self.poll()?;
        let chinese = language == crate::i18n::UiLanguage::Chinese;
        if ui
            .add_enabled(
                !self.loading() && !self.url.trim().is_empty(),
                egui::Button::new(if chinese {
                    "加载链接 / RSS"
                } else {
                    "Load link / RSS"
                }),
            )
            .clicked()
        {
            self.load(proxy.to_owned(), ui.ctx().clone())?;
        }
        if self.loading() {
            ui.spinner();
        }
        if !self.episodes.is_empty() {
            let options = self
                .episodes
                .iter()
                .enumerate()
                .map(|(index, episode)| {
                    use crate::media_import::podcast::AudioKind;
                    let label = match (episode.kind, chinese) {
                        (AudioKind::Live, true) => "直播",
                        (AudioKind::Live, false) => "Live",
                        (AudioKind::Recorded, true) => "录播",
                        (AudioKind::Recorded, false) => "Recorded",
                        (AudioKind::Pending, true) => "未开播",
                        (AudioKind::Pending, false) => "Upcoming",
                        (AudioKind::Ended, true) => "已结束",
                        (AudioKind::Ended, false) => "Ended",
                        (AudioKind::Unknown, true) => "网络音源",
                        (AudioKind::Unknown, false) => "Online audio",
                    };
                    (index, format!("[{label}] {}", episode.title))
                })
                .collect::<Vec<_>>();
            let title = options
                .get(self.selected)
                .map(|(_, title)| title.clone())
                .unwrap_or_default();
            let previous = self.selected;
            super::components::searchable_combobox(
                ui,
                "online_audio_item",
                &title,
                &mut self.selected,
                &options,
            );
            changed |= self.selected != previous;
        }
        Ok(changed)
    }

    pub fn selected_episode(&self) -> Option<&PodcastEpisode> {
        (self.resolved_url == self.url.trim())
            .then(|| self.episodes.get(self.selected))
            .flatten()
    }

    pub fn loading(&self) -> bool {
        self.pending.is_some()
    }

    pub fn invalidate(&mut self) {
        self.pending = None;
        self.episodes.clear();
        self.resolved_url.clear();
        self.selected = 0;
    }

    pub fn load(&mut self, proxy: String, ctx: egui::Context) -> Result<(), String> {
        self.invalidate();
        let url = self.url.trim().to_owned();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let (tx, result) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("podcast-discovery".into())
            .spawn(move || {
                let _ = tx.send(podcast::resolve(&url, &proxy, worker_stop));
                ctx.request_repaint();
            })
            .map_err(|e| e.to_string())?;
        self.pending = Some(Lookup { stop, result });
        Ok(())
    }

    pub fn poll(&mut self) -> Result<bool, String> {
        let Some(pending) = &self.pending else {
            return Ok(false);
        };
        let result = match pending.result.try_recv() {
            Ok(result) => result,
            Err(crossbeam_channel::TryRecvError::Empty) => return Ok(false),
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                Err("Podcast lookup stopped unexpectedly".into())
            }
        };
        self.pending = None;
        self.episodes = result?;
        self.resolved_url = self.url.trim().to_owned();
        Ok(true)
    }
}
