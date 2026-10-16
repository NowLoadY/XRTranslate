//! On-demand media titles and current-song lyrics. The host supplies activation, character
//! budget and a text sink; outputs choose their own layout. This plugin never
//! imports OSC or starts translation.
mod cache;
mod netease;
mod output;
mod player;
mod timeline;

use crate::plugins::text_output::LyricsText;
use output::Lookup;
pub(crate) use output::Status;
use parking_lot::Mutex;
use player::Playback;
use std::{
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};
use tokio::sync::watch;

// Publish original timing and adjusted timing together; the host selects the
// presentation for each output without duplicating observation or lyric lookup.
type Publish = Arc<dyn Fn(LyricsText, LyricsText) + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Eq)]
struct DisplayConfig {
    budget: usize,
    offset_ms: i32,
}

pub(crate) struct LyricsPlugin {
    publish: Publish,
    worker: Option<(watch::Sender<Option<DisplayConfig>>, JoinHandle<()>)>,
    cache: Arc<cache::Cache>,
    cleanup: Option<JoinHandle<()>>,
    status: Arc<Mutex<Status>>,
}

impl LyricsPlugin {
    pub fn new(
        cache_directory: PathBuf,
        publish: impl Fn(LyricsText, LyricsText) + Send + Sync + 'static,
    ) -> Self {
        Self {
            publish: Arc::new(publish),
            worker: None,
            cache: Arc::new(cache::Cache::new(cache_directory)),
            cleanup: None,
            status: Arc::new(Mutex::new(Status::Idle)),
        }
    }

    /// No playback worker, media subscription or HTTP client while off.
    pub fn configure(&mut self, budget: Option<usize>, offset_ms: i32) {
        let config = budget
            .filter(|budget| *budget > 0)
            .map(|budget| DisplayConfig { budget, offset_ms });
        if config.is_none() {
            if let Some((tx, worker)) = self.worker.take() {
                let _ = tx.send(None);
                let _ = worker.join();
            }
            (self.publish)(LyricsText::default(), LyricsText::default());
            *self.status.lock() = Status::Idle;
            return;
        }
        if let Some((tx, _)) = &self.worker {
            tx.send_if_modified(|current| {
                if *current == config {
                    return false;
                }
                *current = config;
                true
            });
            return;
        }
        let (tx, rx) = watch::channel(config);
        let publish = self.publish.clone();
        let cache = self.cache.clone();
        let status = self.status.clone();
        *status.lock() = Status::Waiting;
        match thread::Builder::new()
            .name("current-song-lyrics".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(run(rx, &publish, cache, &status)),
                    Err(error) => log::warn!("Lyrics: {error}"),
                }
                publish(LyricsText::default(), LyricsText::default());
            }) {
            Ok(worker) => self.worker = Some((tx, worker)),
            Err(error) => log::warn!("Lyrics: {error}"),
        }
    }

    pub(crate) fn status(&self) -> Status {
        *self.status.lock()
    }

    pub fn cache_is_clearing(&self) -> bool {
        self.cleanup
            .as_ref()
            .is_some_and(|worker| !worker.is_finished())
    }

    /// Manual cleanup is available even when playback is disabled. Disk work
    /// stays off the UI thread; the displayed line can remain in memory.
    pub fn clear_cache(&mut self) {
        if self.cache_is_clearing() {
            return;
        }
        if let Some(worker) = self.cleanup.take() {
            let _ = worker.join();
        }
        self.cache.invalidate();
        let cache = self.cache.clone();
        match thread::Builder::new()
            .name("lyric-cache-clear".into())
            .spawn(move || {
                if let Err(error) = cache.clear() {
                    log::warn!("Lyrics cache: {error}");
                }
            }) {
            Ok(worker) => self.cleanup = Some(worker),
            Err(error) => log::warn!("Lyrics cache: {error}"),
        }
    }
}

impl Drop for LyricsPlugin {
    fn drop(&mut self) {
        self.configure(None, 0);
        if let Some(worker) = self.cleanup.take() {
            let _ = worker.join();
        }
    }
}

async fn run(
    config: watch::Receiver<Option<DisplayConfig>>,
    publish: &Publish,
    cache: Arc<cache::Cache>,
    status: &Mutex<Status>,
) {
    let (tx, rx) = watch::channel(None);
    // Dropping either future cancels its in-flight platform/HTTP operations.
    tokio::select! {
        _ = observe(tx) => {},
        _ = display(config, rx, publish, cache, status) => {},
    }
}

async fn observe(tx: watch::Sender<Option<Playback>>) {
    let mut player = None;
    let mut last_error = String::new();
    loop {
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            if player.is_none() {
                player = Some(player::Player::new().await?);
            }
            player.as_mut().unwrap().poll().await
        })
        .await
        .unwrap_or_else(|_| Err("Media session query timed out".into()));
        let delay = match result {
            Ok(value) => {
                last_error.clear();
                if tx.send(value).is_err() {
                    return;
                }
                Duration::from_secs(1)
            }
            Err(error) => {
                if error != last_error {
                    log::warn!("Lyrics: {error}");
                    last_error = error;
                }
                player = None;
                if tx.send(None).is_err() {
                    return;
                }
                Duration::from_secs(5)
            }
        };
        tokio::time::sleep(delay).await;
    }
}

type Fetch = Pin<Box<dyn Future<Output = Result<Vec<timeline::Cue>, String>>>>;

async fn display(
    mut config: watch::Receiver<Option<DisplayConfig>>,
    mut player: watch::Receiver<Option<Playback>>,
    publish: &Publish,
    cache: Arc<cache::Cache>,
    status: &Mutex<Status>,
) {
    let mut current: Option<Playback> = None;
    let mut cues = Vec::new();
    let mut fetching: Option<Fetch> = None;
    let mut lookup = Lookup::NotStarted;
    let mut last_text = (LyricsText::default(), LyricsText::default());
    let mut renderer = output::Renderer::default();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            changed = config.changed() => {
                if changed.is_err() || config.borrow().is_none() { return; }
            },
            changed = player.changed() => {
                if changed.is_err() { return; }
                let next = player.borrow_and_update().clone();
                let same = current.as_ref().zip(next.as_ref()).is_some_and(|(a, b)| a.track == b.track && a.is_music == b.is_music && a.embedded_lyrics == b.embedded_lyrics);
                if !same {
                    fetching = None;
                    cues.clear();
                    lookup = Lookup::NotStarted;
                }
                current = next;
                if !current.as_ref().is_some_and(Playback::can_sync_lyrics) && fetching.take().is_some() {
                    lookup = Lookup::NotStarted;
                }
                if let Some(playback) = &current
                    && playback.can_sync_lyrics() && lookup == Lookup::NotStarted
                {
                    lookup = Lookup::Loading;
                    fetching = Some(Box::pin(load_lyrics(cache.clone(), playback.clone(), cache.generation())));
                }
            },
            result = async { fetching.as_mut().unwrap().await }, if fetching.is_some() => {
                fetching = None;
                match result {
                    Ok(result) => { cues = result; lookup = Lookup::Ready; },
                    Err(error) => { log::warn!("Lyrics: {error}"); lookup = Lookup::Failed; },
                }
            },
            _ = tick.tick() => {},
        }
        let config = *config.borrow();
        let Some(config) = config else {
            return;
        };
        let now = std::time::Instant::now();
        let (text, next_status) =
            renderer.render(current.as_ref(), &cues, lookup, config.budget, 0, now);
        let adjusted = if config.offset_ms == 0 {
            text.clone()
        } else {
            renderer
                .render(
                    current.as_ref(),
                    &cues,
                    lookup,
                    config.budget,
                    config.offset_ms,
                    now,
                )
                .0
        };
        let text = (text, adjusted);
        *status.lock() = next_status;
        if text != last_text {
            publish(text.0.clone(), text.1.clone());
            last_text = text;
        }
    }
}

async fn load_lyrics(
    cache: Arc<cache::Cache>,
    playback: Playback,
    generation: u64,
) -> Result<Vec<timeline::Cue>, String> {
    match cache.load(&playback.track, playback.duration_ms) {
        Ok(Some(cues)) => return Ok(cues),
        Ok(None) => {}
        Err(error) => log::warn!("Lyrics cache: {error}"),
    }
    let mut lrc = playback.embedded_lyrics.unwrap_or_default();
    let mut cues = timeline::parse(&lrc);
    if cues.iter().all(|cue| cue.text.is_empty()) {
        let client = netease::client()?;
        lrc = netease::fetch(&client, &playback.track, playback.duration_ms).await?;
        cues = timeline::parse(&lrc);
    }
    // Failed lookups and plain/empty lyrics must never poison a later cache hit.
    if cues.iter().any(|cue| !cue.text.is_empty())
        && let Err(error) = cache.store(&playback.track, playback.duration_ms, &lrc, generation)
    {
        log::warn!("Lyrics cache: {error}");
    }
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn live_offset_changes_preserve_the_original_output_and_reset() {
        let cache_directory = tempfile::tempdir().unwrap();
        let cache = Arc::new(cache::Cache::new(cache_directory.path().to_owned()));
        let (config, config_rx) = watch::channel(Some(DisplayConfig {
            budget: 24,
            offset_ms: 0,
        }));
        let (player, player_rx) = watch::channel(None);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let publish: Publish = Arc::new(move |original, adjusted| {
            let _ = tx.send((original, adjusted));
        });
        let status = Mutex::new(Status::Waiting);
        tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                display(config_rx, player_rx, &publish, cache, &status),
                async {
                    player
                        .send(Some(Playback {
                            track: player::Track {
                                title: "Song".into(),
                                artists: vec!["Artist".into()],
                                album: String::new(),
                            },
                            is_music: true,
                            embedded_lyrics: Some("[00:00]First\n[00:05.050]Second".into()),
                            duration_ms: 30_000,
                            position_ms: Some(5_000),
                            observed: std::time::Instant::now(),
                            rate: 0.0,
                            playing: true,
                        }))
                        .unwrap();
                    let original = loop {
                        let (original, adjusted) = rx.recv().await.unwrap();
                        if original.lyric == "🎵 First" {
                            assert_eq!(original, adjusted);
                            break original;
                        }
                    };
                    config
                        .send(Some(DisplayConfig {
                            budget: 24,
                            offset_ms: -100,
                        }))
                        .unwrap();
                    let (unchanged, adjusted) = rx.recv().await.unwrap();
                    assert_eq!(unchanged, original);
                    assert_eq!(adjusted.title, original.title);
                    assert_eq!(adjusted.lyric, "🎵 Second");
                    config
                        .send(Some(DisplayConfig {
                            budget: 24,
                            offset_ms: 0,
                        }))
                        .unwrap();
                    let (unchanged, adjusted) = rx.recv().await.unwrap();
                    assert_eq!(unchanged, original);
                    assert_eq!(adjusted, original);
                    config.send(None).unwrap();
                }
            );
        })
        .await
        .expect("the lyric worker did not apply the live timing change");
    }
}
