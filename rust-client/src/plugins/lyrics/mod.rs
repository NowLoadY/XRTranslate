//! On-demand media titles and current-song lyrics. The host supplies activation, character
//! budget and a text sink; this plugin never imports OSC or starts translation.
mod cache;
mod netease;
mod output;
mod player;
mod timeline;

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

type Publish = Arc<dyn Fn(String) + Send + Sync>;

pub(crate) struct LyricsPlugin {
    publish: Publish,
    worker: Option<(watch::Sender<Option<usize>>, JoinHandle<()>)>,
    cache: Arc<cache::Cache>,
    cleanup: Option<JoinHandle<()>>,
    status: Arc<Mutex<Status>>,
}

impl LyricsPlugin {
    pub fn new(cache_directory: PathBuf, publish: impl Fn(String) + Send + Sync + 'static) -> Self {
        Self {
            publish: Arc::new(publish),
            worker: None,
            cache: Arc::new(cache::Cache::new(cache_directory)),
            cleanup: None,
            status: Arc::new(Mutex::new(Status::Idle)),
        }
    }

    /// No playback worker, media subscription or HTTP client while off.
    pub fn set_budget(&mut self, budget: Option<usize>) {
        let budget = budget.filter(|budget| *budget > 0);
        if budget.is_none() {
            if let Some((tx, worker)) = self.worker.take() {
                let _ = tx.send(None);
                let _ = worker.join();
            }
            (self.publish)(String::new());
            *self.status.lock() = Status::Idle;
            return;
        }
        if let Some((tx, _)) = &self.worker {
            tx.send_if_modified(|current| {
                if *current == budget {
                    return false;
                }
                *current = budget;
                true
            });
            return;
        }
        let (tx, rx) = watch::channel(budget);
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
                publish(String::new());
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
        self.set_budget(None);
        if let Some(worker) = self.cleanup.take() {
            let _ = worker.join();
        }
    }
}

async fn run(
    config: watch::Receiver<Option<usize>>,
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
    mut config: watch::Receiver<Option<usize>>,
    mut player: watch::Receiver<Option<Playback>>,
    publish: &Publish,
    cache: Arc<cache::Cache>,
    status: &Mutex<Status>,
) {
    let mut current: Option<Playback> = None;
    let mut cues = Vec::new();
    let mut fetching: Option<Fetch> = None;
    let mut lookup = Lookup::NotStarted;
    let mut last_text = String::new();
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
        let budget = *config.borrow();
        let Some(budget) = budget else {
            return;
        };
        let (text, next_status) = renderer.render(
            current.as_ref(),
            &cues,
            lookup,
            budget,
            std::time::Instant::now(),
        );
        *status.lock() = next_status;
        if text != last_text {
            publish(text.clone());
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
