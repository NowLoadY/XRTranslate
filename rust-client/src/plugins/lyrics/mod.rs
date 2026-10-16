//! On-demand media titles and current-song lyrics. The host supplies activation, character
//! budget and a text sink; outputs choose their own layout. This plugin never
//! imports OSC or starts translation.
mod cache;
mod netease;
mod output;
mod player;
mod timeline;

use crate::plugins::{PluginId, text_output::LyricsText};
use output::Lookup;
pub(crate) use output::Status;
use parking_lot::Mutex;
use player::Playback;
use std::{
    collections::BTreeMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};
use tokio::sync::watch;

// Each output owns its timing and budget; playback and lyric lookup are shared.
type Publish = Arc<dyn Fn(PluginId, LyricsText) + Send + Sync>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct DisplayRequest {
    pub output: PluginId,
    pub budget: usize,
    pub offset_ms: i32,
}

pub(crate) struct LyricsPlugin {
    publish: Publish,
    worker: Option<(watch::Sender<Vec<DisplayRequest>>, JoinHandle<()>)>,
    cache: Arc<cache::Cache>,
    cleanup: Option<JoinHandle<()>>,
    status: Arc<Mutex<Status>>,
}

impl LyricsPlugin {
    pub fn new(
        cache_directory: PathBuf,
        publish: impl Fn(PluginId, LyricsText) + Send + Sync + 'static,
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
    pub fn configure(&mut self, mut config: Vec<DisplayRequest>) {
        config.retain(|request| request.budget > 0);
        if config.is_empty() {
            if let Some((tx, worker)) = self.worker.take() {
                let _ = tx.send(config);
                let _ = worker.join();
            }
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
        self.configure(Vec::new());
        if let Some(worker) = self.cleanup.take() {
            let _ = worker.join();
        }
    }
}

async fn run(
    config: watch::Receiver<Vec<DisplayRequest>>,
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
    mut config: watch::Receiver<Vec<DisplayRequest>>,
    mut player: watch::Receiver<Option<Playback>>,
    publish: &Publish,
    cache: Arc<cache::Cache>,
    status: &Mutex<Status>,
) {
    let mut current: Option<Playback> = None;
    let mut cues = Vec::new();
    let mut fetching: Option<Fetch> = None;
    let mut lookup = Lookup::NotStarted;
    let mut last_text = BTreeMap::<PluginId, LyricsText>::new();
    let mut renderer = output::Renderer::default();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            changed = config.changed() => {
                if changed.is_err() { break; }
            },
            changed = player.changed() => {
                if changed.is_err() { break; }
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
        let config = config.borrow().clone();
        last_text.retain(|output, _| {
            let keep = config.iter().any(|request| request.output == *output);
            if !keep {
                publish(*output, LyricsText::default());
            }
            keep
        });
        if config.is_empty() {
            break;
        }
        let now = std::time::Instant::now();
        for request in config {
            let (text, next_status) = renderer.render(
                current.as_ref(),
                &cues,
                lookup,
                request.budget,
                request.offset_ms,
                now,
            );
            *status.lock() = next_status;
            if last_text.get(&request.output) != Some(&text) {
                publish(request.output, text.clone());
                last_text.insert(request.output, text);
            }
        }
    }
    for output in last_text.keys() {
        publish(*output, LyricsText::default());
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
    async fn live_offsets_keep_outputs_independent_and_clear_removed_outputs() {
        let cache_directory = tempfile::tempdir().unwrap();
        let cache = Arc::new(cache::Cache::new(cache_directory.path().to_owned()));
        let requests = |offset_ms| {
            vec![
                DisplayRequest {
                    output: PluginId::AVATAR,
                    budget: 24,
                    offset_ms: 0,
                },
                DisplayRequest {
                    output: PluginId::OSC,
                    budget: 24,
                    offset_ms,
                },
                DisplayRequest {
                    output: PluginId::CHILLOUTVR_OSC,
                    budget: 24,
                    offset_ms: 100,
                },
            ]
        };
        let (config, config_rx) = watch::channel(requests(0));
        let (player, player_rx) = watch::channel(None);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let publish: Publish = Arc::new(move |output, text| {
            let _ = tx.send((output, text));
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
                    let mut outputs = BTreeMap::new();
                    while outputs.len() < 3 {
                        let (output, text) = rx.recv().await.unwrap();
                        if text.lyric == "🎵 First" {
                            outputs.insert(output, text);
                        }
                    }
                    config.send(requests(-100)).unwrap();
                    let (output, adjusted) = rx.recv().await.unwrap();
                    assert_eq!(output, PluginId::OSC);
                    assert_eq!(adjusted.lyric, "🎵 Second");
                    assert_eq!(adjusted.title, outputs[&PluginId::OSC].title);
                    config.send(requests(0)).unwrap();
                    let (output, restored) = rx.recv().await.unwrap();
                    assert_eq!(output, PluginId::OSC);
                    assert_eq!(restored, outputs[&PluginId::OSC]);
                    config.send(vec![requests(0)[0]]).unwrap();
                    let mut cleared = Vec::new();
                    for _ in 0..2 {
                        let (output, text) = rx.recv().await.unwrap();
                        assert_eq!(text, LyricsText::default());
                        cleared.push(output);
                    }
                    assert!(cleared.contains(&PluginId::OSC));
                    assert!(cleared.contains(&PluginId::CHILLOUTVR_OSC));
                    config.send(Vec::new()).unwrap();
                    let (output, text) = rx.recv().await.unwrap();
                    assert_eq!(output, PluginId::AVATAR);
                    assert_eq!(text, LyricsText::default());
                }
            );
        })
        .await
        .expect("the lyric worker did not apply the timing change");
    }
}
