//! Repeatable audio input for Director sessions, using the normal media decoder and ASR channel.
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

use crate::media_import::{AudioImportHandle, AudioImportOptions, import_audio_file};
use crate::translation_service::{ChannelScope, TaskChannel};
use crossbeam_channel::Sender;

#[derive(Default)]
struct FileInput {
    path: Option<PathBuf>,
    sink: Option<Sender<Vec<f32>>>,
    scope: Option<Arc<ChannelScope>>,
    playback: Option<AudioImportHandle>,
}

fn input() -> &'static Mutex<FileInput> {
    static INPUT: OnceLock<Mutex<FileInput>> = OnceLock::new();
    INPUT.get_or_init(Mutex::default)
}

impl FileInput {
    fn play(&mut self) -> Result<(), String> {
        self.playback = None;
        if self
            .scope
            .as_ref()
            .is_none_or(|scope| !scope.accepts_events())
        {
            return Ok(());
        }
        if let (Some(path), Some(sink)) = (&self.path, &self.sink) {
            self.playback = Some(
                import_audio_file(path, sink.clone(), AudioImportOptions::default())
                    .map_err(|error| error.to_string())?,
            );
        }
        Ok(())
    }
}

/// Arm a file before pressing Start; setting another file during the session replays it.
pub(super) fn configure(path: Option<PathBuf>) -> Result<(), String> {
    if path.as_ref().is_some_and(|path| !path.is_file()) {
        return Err("Audio input file does not exist".into());
    }
    let mut input = input().lock().unwrap();
    input.path = path;
    input.play()
}

pub(crate) fn attach(channel: &TaskChannel) -> Option<Result<(), String>> {
    let mut input = input().lock().unwrap();
    input.path.as_ref()?;
    if input
        .scope
        .as_ref()
        .is_some_and(|scope| scope.accepts_events() && !Arc::ptr_eq(scope, &channel.scope))
    {
        return Some(Err(
            "Director file input requires one active audio channel".into()
        ));
    }
    input.sink = channel.audio_tx.clone();
    input.scope = Some(channel.scope.clone());
    Some(input.play())
}

pub fn detach() {
    let mut input = input().lock().unwrap();
    input.playback = None;
    input.sink = None;
    input.scope = None;
}
