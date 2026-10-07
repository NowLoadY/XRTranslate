//! Optional local history: the event pump only queues finalized facts; SQLite
//! writes and searches run on one worker, independently of UI rendering.
use crate::session_coordinator::{
    SessionEventSubscriber, TranslationEvent, TranslationSegment, TranslationSessionOwner,
};
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use std::{
    collections::HashSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) const PAGE_SIZE: usize = 100;
mod store;
pub(crate) use store::{DeleteSelection, Record};
use store::{RecordId, open_database, prepare_deletion, remove, save, search};

enum Command {
    Save {
        epoch: u64,
        segments: Vec<TranslationSegment>,
        replace: bool,
    },
    Search {
        saved: bool,
        revision: u64,
        query: String,
        offset: usize,
        ctx: eframe::egui::Context,
    },
    PrepareDelete {
        request: u64,
        selection: DeleteSelection,
        ctx: eframe::egui::Context,
    },
    Delete {
        request: u64,
        ctx: eframe::egui::Context,
    },
    CancelDelete(u64),
    Stop,
}

enum Response {
    Changed,
    Rows(u64, Vec<Record>),
    DeletionReady(u64, usize),
    Deleted(u64, usize),
    DeleteFailed(u64, String),
    Error(String),
}

pub(crate) struct DeletionPreview {
    request: u64,
    pub description: String,
    pub count: Option<usize>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub(crate) struct Subscriber {
    tx: Sender<Command>,
    epoch: Arc<AtomicU64>,
    overflow: Arc<AtomicBool>,
    included_plugins: &'static [&'static str],
}
impl SessionEventSubscriber for Subscriber {
    fn accepts_owner(&self, owner: &TranslationSessionOwner) -> bool {
        owner.is_host()
            || owner
                .plugin()
                .is_some_and(|owner| self.included_plugins.contains(&owner.plugin_id()))
    }
    fn on_translation_event(&self, _: &TranslationSessionOwner, event: &TranslationEvent) {
        let epoch = self.epoch.load(Ordering::Acquire);
        let (segments, replace) = match event {
            TranslationEvent::Segment(segment) if final_translation(segment) => {
                (vec![segment.clone()], false)
            }
            TranslationEvent::ReplaceSegments(segments)
                if !segments.is_empty() && segments.iter().all(final_translation) =>
            {
                (segments.clone(), true)
            }
            _ => return,
        };
        if self
            .tx
            .try_send(Command::Save {
                epoch,
                segments,
                replace,
            })
            .is_err()
        {
            self.overflow.store(true, Ordering::Release);
        }
    }
}
fn final_translation(segment: &TranslationSegment) -> bool {
    !segment.revisable
        && segment
            .translated
            .as_deref()
            .is_some_and(|s| !s.trim().is_empty())
}

pub(crate) struct Archive {
    pub open: bool,
    pub saved: bool,
    pub query: String,
    pub offset: usize,
    pub rows: Vec<Record>,
    pub loading: bool,
    pub has_more: bool,
    pub error: Option<String>,
    pub pending_delete: Option<DeletionPreview>,
    pub date_from: String,
    pub date_through: String,
    pub date_delete_open: bool,
    pub deleted_count: Option<usize>,
    deleting: Option<u64>,
    delete_request: u64,
    subscriber: Subscriber,
    rx: Receiver<Response>,
    revision: u64,
    refresh_at: Option<Instant>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Archive {
    pub fn new(root: &Path, enabled: bool, included_plugins: &'static [&'static str]) -> Self {
        let (tx, commands) = bounded(256);
        let (response, rx) = unbounded();
        let epoch = Arc::new(AtomicU64::new(u64::from(enabled)));
        let subscriber = Subscriber {
            tx,
            epoch: epoch.clone(),
            overflow: Arc::new(AtomicBool::new(false)),
            included_plugins,
        };
        let path = root.join("runtime/translation-history.sqlite3");
        let worker = std::thread::Builder::new()
            .name("translation-history".into())
            .spawn(move || run_worker(commands, response, epoch, &path));
        let (worker, error) = match worker {
            Ok(worker) => (Some(worker), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Self {
            open: false,
            saved: false,
            query: String::new(),
            offset: 0,
            rows: Vec::new(),
            loading: false,
            has_more: false,
            error,
            pending_delete: None,
            date_from: String::new(),
            date_through: String::new(),
            date_delete_open: false,
            deleted_count: None,
            deleting: None,
            delete_request: 0,
            subscriber,
            rx,
            revision: 0,
            refresh_at: None,
            worker,
        }
    }
    pub fn subscriber(&self) -> Subscriber {
        self.subscriber.clone()
    }
    pub fn enabled(&self) -> bool {
        self.subscriber.epoch.load(Ordering::Acquire) & 1 == 1
    }
    pub fn set_enabled(&mut self, enabled: bool) {
        if enabled != self.enabled() {
            self.subscriber.epoch.fetch_add(1, Ordering::AcqRel);
        }
    }
    pub fn deleting(&self) -> bool {
        self.deleting.is_some()
    }
    pub fn prepare_delete(
        &mut self,
        selection: DeleteSelection,
        description: String,
        ctx: &eframe::egui::Context,
    ) {
        if self.deleting() {
            return;
        }
        self.delete_request += 1;
        self.deleted_count = None;
        let request = self.delete_request;
        let error = self
            .subscriber
            .tx
            .try_send(Command::PrepareDelete {
                request,
                selection,
                ctx: ctx.clone(),
            })
            .err()
            .map(|_| "History worker is busy. Please retry.".into());
        self.pending_delete = Some(DeletionPreview {
            request,
            description,
            count: None,
            error,
        });
    }
    pub fn cancel_delete(&mut self) {
        if let Some(pending) = self.pending_delete.take() {
            let _ = self
                .subscriber
                .tx
                .try_send(Command::CancelDelete(pending.request));
        }
    }
    pub fn confirm_delete(&mut self, ctx: &eframe::egui::Context) {
        let Some(pending) = &mut self.pending_delete else {
            return;
        };
        if pending.error.is_some() || !pending.count.is_some_and(|count| count > 0) {
            return;
        }
        if self
            .subscriber
            .tx
            .try_send(Command::Delete {
                request: pending.request,
                ctx: ctx.clone(),
            })
            .is_err()
        {
            pending.error = Some("History worker is busy. Please retry.".into());
            return;
        }
        self.deleting = Some(pending.request);
        self.pending_delete = None;
    }
    pub fn refresh(&mut self, debounce: bool) {
        self.revision += 1;
        self.loading = true;
        self.refresh_at = Some(
            Instant::now()
                + if debounce {
                    Duration::from_millis(180)
                } else {
                    Duration::ZERO
                },
        );
    }
    pub fn poll(&mut self, ctx: &eframe::egui::Context) {
        if let Some(at) = self.refresh_at {
            if at <= Instant::now() {
                self.refresh_at = None;
                if self
                    .subscriber
                    .tx
                    .try_send(Command::Search {
                        saved: self.saved,
                        revision: self.revision,
                        query: self.query.clone(),
                        offset: self.offset,
                        ctx: ctx.clone(),
                    })
                    .is_err()
                {
                    self.error = Some("History worker is busy. Please retry.".into());
                    self.loading = false;
                }
            } else {
                ctx.request_repaint_after(at.saturating_duration_since(Instant::now()));
            }
        }
        let mut changed = false;
        for response in self.rx.try_iter() {
            match response {
                Response::Changed => changed = true,
                Response::Rows(revision, mut rows) if revision == self.revision => {
                    self.has_more = rows.len() > PAGE_SIZE;
                    rows.truncate(PAGE_SIZE);
                    self.rows = rows;
                    self.loading = false;
                }
                Response::Error(error) => {
                    self.error = Some(error);
                    self.loading = false;
                }
                Response::DeletionReady(request, count) => {
                    if let Some(pending) = &mut self.pending_delete
                        && pending.request == request
                    {
                        pending.count = Some(count);
                    }
                }
                Response::Deleted(request, count) if self.deleting == Some(request) => {
                    self.deleting = None;
                    self.deleted_count = Some(count);
                    self.offset = 0;
                    changed = true;
                }
                Response::DeleteFailed(request, error) => {
                    if let Some(pending) = &mut self.pending_delete
                        && pending.request == request
                    {
                        pending.error = Some(error);
                    } else if self.deleting == Some(request) {
                        self.deleting = None;
                        self.error = Some(error);
                        changed = true;
                    }
                }
                _ => {}
            }
        }
        if changed && self.open && self.refresh_at.is_none() {
            self.refresh(true);
        }
        if self.subscriber.overflow.swap(false, Ordering::AcqRel) {
            self.error = Some(
                "Some translations could not be saved because history storage is busy.".into(),
            );
        }
    }
}
impl Drop for Archive {
    fn drop(&mut self) {
        let _ = self.subscriber.tx.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_worker(
    commands: Receiver<Command>,
    response: Sender<Response>,
    epoch: Arc<AtomicU64>,
    path: &Path,
) {
    let session = uuid::Uuid::new_v4().to_string();
    let mut connection = None;
    let mut recent = None;
    let mut pending_delete: Option<(u64, Vec<RecordId>)> = None;
    let mut deleted = HashSet::new();
    while let Ok(mut command) = commands.recv() {
        if matches!(command, Command::Stop) {
            break;
        }
        // Late revisions must not restore records explicitly deleted this session.
        if !deleted.is_empty()
            && let Command::Save { segments, .. } = &mut command
        {
            segments.retain(|segment| !deleted.contains(&RecordId::for_segment(&session, segment)));
        }
        let result = (|| -> Result<(), String> {
            match &command {
                Command::Save {
                    epoch: queued,
                    segments,
                    replace,
                } => {
                    if segments.is_empty() {
                        return Ok(());
                    }
                    if recent.is_none() {
                        recent =
                            Some(open_database(Path::new(":memory:")).map_err(|e| e.to_string())?);
                    }
                    let memory = recent.as_mut().unwrap();
                    save(memory, &session, segments, *replace).map_err(|e| e.to_string())?;
                    memory
                        .execute(
                            "DELETE FROM translations WHERE rowid IN (
                            SELECT rowid FROM translations ORDER BY saved_at DESC,rowid DESC
                            LIMIT -1 OFFSET 1000)",
                            [],
                        )
                        .map_err(|e| e.to_string())?;
                    let _ = response.send(Response::Changed);
                    if queued & 1 == 1 && *queued == epoch.load(Ordering::Acquire) {
                        if connection.is_none() {
                            std::fs::create_dir_all(path.parent().unwrap())
                                .map_err(|e| e.to_string())?;
                            connection = Some(open_database(path).map_err(|e| e.to_string())?);
                        }
                        save(connection.as_mut().unwrap(), &session, segments, *replace)
                            .map_err(|e| e.to_string())?;
                    }
                }
                Command::Search {
                    saved,
                    revision,
                    query,
                    offset,
                    ..
                } => {
                    // Browsing an empty archive must not create a file.
                    if *saved && connection.is_none() && path.exists() {
                        connection = Some(open_database(path).map_err(|e| e.to_string())?);
                    }
                    let database = if *saved {
                        connection.as_ref()
                    } else {
                        recent.as_ref()
                    };
                    let rows = database
                        .map(|db| search(db, query, *offset))
                        .transpose()
                        .map_err(|e| e.to_string())?
                        .unwrap_or_default();
                    let _ = response.send(Response::Rows(*revision, rows));
                }
                Command::PrepareDelete {
                    request, selection, ..
                } => {
                    pending_delete = None;
                    if recent.is_none() {
                        recent =
                            Some(open_database(Path::new(":memory:")).map_err(|e| e.to_string())?);
                    }
                    if connection.is_none() && path.exists() {
                        connection = Some(open_database(path).map_err(|e| e.to_string())?);
                    }
                    let ids =
                        prepare_deletion(recent.as_ref().unwrap(), connection.as_ref(), selection)?;
                    let _ = response.send(Response::DeletionReady(*request, ids.len()));
                    pending_delete = Some((*request, ids));
                }
                Command::Delete { request, .. } => {
                    let Some((id, ids)) = pending_delete.as_ref().filter(|(id, _)| id == request)
                    else {
                        return Err("Deletion preview expired. Please try again.".into());
                    };
                    // Disk first: a storage failure must not silently clear the visible session copy.
                    if let Some(db) = &mut connection {
                        remove(db, ids).map_err(|e| e.to_string())?;
                    }
                    if let Some(db) = &mut recent {
                        remove(db, ids).map_err(|e| e.to_string())?;
                    }
                    let _ = response.send(Response::Deleted(*id, ids.len()));
                    deleted.extend(
                        pending_delete
                            .take()
                            .unwrap()
                            .1
                            .into_iter()
                            .filter(|id| id.belongs_to(&session)),
                    );
                }
                Command::CancelDelete(request) => {
                    if pending_delete.as_ref().is_some_and(|(id, _)| id == request) {
                        pending_delete = None;
                    }
                }
                Command::Stop => {}
            }
            Ok(())
        })();
        if let Err(error) = result {
            let result = match &command {
                Command::PrepareDelete { request, .. } | Command::Delete { request, .. } => {
                    Response::DeleteFailed(*request, error)
                }
                _ => Response::Error(error),
            };
            let _ = response.send(result);
        }
        if let Command::Search { ctx, .. }
        | Command::PrepareDelete { ctx, .. }
        | Command::Delete { ctx, .. } = command
        {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn records(archive: &mut Archive, query: &str) -> Vec<Record> {
        archive
            .subscriber
            .tx
            .send(Command::Search {
                saved: true,
                revision: 7,
                query: query.into(),
                offset: 0,
                ctx: Default::default(),
            })
            .unwrap();
        loop {
            match archive.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
                Response::Rows(7, rows) => return rows,
                Response::Changed => {}
                _ => panic!("history operation failed"),
            }
        }
    }
    #[test]
    fn history_is_opt_in_searchable_deduplicated_and_survives_restart() {
        let root = tempfile::tempdir().unwrap();
        let mut archive = Archive::new(root.path(), false, &["quick_translate"]);
        let sink = archive.subscriber();
        let owner = TranslationSessionOwner::Host {
            capture_source: crate::CaptureSource::Microphone,
        };
        let segment = crate::session_coordinator::test_segment("Äpfel 100%", "苹果");
        let event = TranslationEvent::Segment(segment.clone());
        sink.on_translation_event(&owner, &event);
        assert!(records(&mut archive, "").is_empty());
        assert!(!root.path().join("runtime").exists());
        archive
            .subscriber
            .tx
            .send(Command::Search {
                saved: false,
                revision: 8,
                query: "苹果".into(),
                offset: 0,
                ctx: Default::default(),
            })
            .unwrap();
        assert!(
            matches!(archive.rx.recv_timeout(Duration::from_secs(5)).unwrap(), Response::Rows(8, rows) if rows.len() == 1)
        );
        assert!(!root.path().join("runtime").exists());
        archive.set_enabled(true);
        sink.on_translation_event(&owner, &event);
        sink.on_translation_event(&owner, &event);
        let rows = records(&mut archive, "äPFEL");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].translated, "苹果");
        assert_eq!(records(&mut archive, "%").len(), 1);
        let mut corrected = segment;
        corrected.translated = Some("青苹果".into());
        sink.on_translation_event(&owner, &TranslationEvent::ReplaceSegments(vec![corrected]));
        assert_eq!(records(&mut archive, "青苹果").len(), 1);
        archive.set_enabled(false);
        sink.on_translation_event(
            &owner,
            &TranslationEvent::Segment(crate::session_coordinator::test_segment(
                "ignored",
                "未保存",
            )),
        );
        assert_eq!(records(&mut archive, "").len(), 1);
        drop(archive);
        let mut reopened = Archive::new(root.path(), false, &[]);
        assert_eq!(records(&mut reopened, "青苹果").len(), 1);
        assert!(records(&mut reopened, "未保存").is_empty());
    }
    #[test]
    fn confirmed_deletion_clears_both_lists_and_late_results_cannot_restore_it() {
        let root = tempfile::tempdir().unwrap();
        let mut archive = Archive::new(root.path(), true, &[]);
        let ctx = eframe::egui::Context::default();
        let sink = archive.subscriber();
        let owner = TranslationSessionOwner::Host {
            capture_source: crate::CaptureSource::Microphone,
        };
        let segment = crate::session_coordinator::test_segment("delete me", "删除此条");
        let event = TranslationEvent::Segment(segment.clone());
        sink.on_translation_event(&owner, &event);
        let id = records(&mut archive, "")[0].id.clone();
        let wait = |archive: &mut Archive, done: &dyn Fn(&Archive) -> bool| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                archive.poll(&ctx);
                if done(archive) {
                    break;
                }
                assert!(Instant::now() < deadline, "history worker did not respond");
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        archive.prepare_delete(
            DeleteSelection::Record(id.clone()),
            "delete me".into(),
            &ctx,
        );
        wait(&mut archive, &|archive| {
            archive.pending_delete.as_ref().unwrap().count.is_some()
        });
        assert_eq!(archive.pending_delete.as_ref().unwrap().count, Some(1));
        archive.cancel_delete();
        archive.confirm_delete(&ctx);
        assert_eq!(
            records(&mut archive, "").len(),
            1,
            "cancel must preserve the record"
        );
        archive.prepare_delete(DeleteSelection::Record(id), "delete me".into(), &ctx);
        wait(&mut archive, &|archive| {
            archive.pending_delete.as_ref().unwrap().count.is_some()
        });
        archive.confirm_delete(&ctx);
        wait(&mut archive, &|archive| !archive.deleting());
        assert_eq!(archive.deleted_count, Some(1));
        sink.on_translation_event(&owner, &TranslationEvent::ReplaceSegments(vec![segment]));
        assert!(records(&mut archive, "").is_empty());
        archive.saved = false;
        archive.refresh(false);
        wait(&mut archive, &|archive| !archive.loading);
        assert!(archive.rows.is_empty(), "session copy is also removed");
        drop(archive);
        let mut reopened = Archive::new(root.path(), false, &[]);
        assert!(
            records(&mut reopened, "").is_empty(),
            "deletion survives restart"
        );
    }
}
