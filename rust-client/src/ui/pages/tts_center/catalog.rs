//! Background reference decoding, optional transcription and catalog persistence.
use super::editor::VoiceDraft;
use crate::i18n::{UiLanguage, tr};
use crate::media_import::{self, AudioImportEvent, AudioImportOptions, AudioImportPacing};
use eframe::egui;
use std::{path::PathBuf, sync::mpsc, time::Duration};
use xrtranslate_assets::voices::{MAX_REFERENCE_SECONDS, SAMPLE_RATE, VoiceCard, VoiceCatalog};
use xrtranslate_engine::audio::f32_to_pcm16le;

enum Completed {
    Loaded(Vec<VoiceCard>),
    Added(VoiceCard),
    Preview(String, Vec<u8>),
}

pub struct CatalogController {
    pub cards: Vec<VoiceCard>,
    pub visible: Vec<usize>,
    pub query: String,
    pub preview: Option<(String, Vec<u8>)>,
    root: Option<PathBuf>,
    filtered_query: String,
    language: Option<UiLanguage>,
    worker: Option<mpsc::Receiver<Result<Completed, String>>>,
}

impl Default for CatalogController {
    fn default() -> Self {
        let cards = VoiceCatalog::builtin_cards();
        Self {
            visible: (0..cards.len()).collect(),
            cards,
            query: String::new(),
            preview: None,
            root: None,
            filtered_query: String::new(),
            language: None,
            worker: None,
        }
    }
}

impl CatalogController {
    pub fn busy(&self) -> bool {
        self.worker.is_some()
    }

    /// Returns whether an import committed, so the editor can close only on success.
    pub fn poll(&mut self, root: PathBuf, ctx: &egui::Context) -> Result<bool, String> {
        if self.root.as_ref() != Some(&root) && !self.busy() {
            self.root = Some(root);
            self.reload(ctx);
        }
        let Some(worker) = &self.worker else {
            return Ok(false);
        };
        let result = match worker.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return Ok(false),
            Err(mpsc::TryRecvError::Disconnected) => Err("Voice catalog worker stopped.".into()),
        };
        self.worker = None;
        let added = match result? {
            Completed::Loaded(cards) => {
                self.cards = cards;
                false
            }
            Completed::Added(card) => {
                let index = self
                    .cards
                    .iter()
                    .position(|voice| !voice.is_builtin())
                    .unwrap_or(self.cards.len());
                self.cards.insert(index, card);
                self.query.clear();
                true
            }
            Completed::Preview(id, pcm) => {
                self.preview = Some((id, pcm));
                false
            }
        };
        self.filter();
        Ok(added)
    }

    pub fn filter_if_changed(&mut self, language: UiLanguage) {
        if self.filtered_query != self.query || self.language != Some(language) {
            self.language = Some(language);
            self.filter();
        }
    }

    fn filter(&mut self) {
        let query = self.query.trim().to_lowercase();
        self.visible = self
            .cards
            .iter()
            .enumerate()
            .filter_map(|(i, card)| {
                let translated = self
                    .language
                    .zip(xrtranslate_assets::voices::builtin(&card.id))
                    .is_some_and(|(language, voice)| {
                        tr(language, voice.name).to_lowercase().contains(&query)
                            || tr(language, voice.description)
                                .to_lowercase()
                                .contains(&query)
                    });
                (query.is_empty()
                    || translated
                    || card.name.to_lowercase().contains(&query)
                    || card.description.to_lowercase().contains(&query))
                .then_some(i)
            })
            .collect();
        self.filtered_query.clone_from(&self.query);
    }

    pub fn reload(&mut self, ctx: &egui::Context) {
        self.spawn(ctx, |catalog| catalog.list().map(Completed::Loaded));
    }

    pub fn import(&mut self, draft: VoiceDraft, server_url: Option<String>, ctx: &egui::Context) {
        self.spawn(ctx, move |catalog| {
            let path = draft.path.ok_or("Choose a reference audio file.")?;
            let (send, receive) = crossbeam_channel::bounded(4);
            let handle = media_import::import_audio_file(
                path,
                send,
                AudioImportOptions {
                    pacing: AudioImportPacing::AsFastAsPossible,
                    gate_threshold_db: None,
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?;
            let mut pcm = Vec::new();
            // A bounded decoder channel plus the duration limit keeps imports independent
            // of the source file size. Dropping the handle cancels early on any error.
            loop {
                match receive.recv_timeout(Duration::from_secs(30)) {
                    Ok(chunk) => {
                        if pcm.len() / 2 + chunk.len()
                            > MAX_REFERENCE_SECONDS * SAMPLE_RATE as usize
                        {
                            return Err(
                                "Reference audio must be between 0.5 and 60 seconds.".into()
                            );
                        }
                        pcm.extend(f32_to_pcm16le(chunk));
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        return Err("Audio decoding timed out.".into());
                    }
                }
            }
            let mut completed = false;
            for event in handle.events().iter() {
                match event {
                    AudioImportEvent::Error(error) => return Err(error),
                    AudioImportEvent::Completed { .. } => {
                        completed = true;
                        break;
                    }
                    _ => {}
                }
            }
            if !completed {
                return Err("Audio import did not finish.".into());
            }
            let transcript = if draft.manual_transcript {
                draft.transcript
            } else {
                let response: xrtranslate_protocol::tts::ReferenceTranscript =
                    super::transport::request(
                        server_url.as_deref().ok_or("ASR service is not ready.")?,
                        "/tts/reference/transcribe",
                        |client, mut url| {
                            url.query_pairs_mut()
                                .append_pair("language", &draft.source_language);
                            client
                                .post(url)
                                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                                .body(pcm.clone())
                        },
                    )?;
                response.text
            };
            catalog
                .add(&draft.name, &draft.description, &transcript, &pcm)
                .map(Completed::Added)
        });
    }

    pub fn preview(&mut self, id: String, ctx: &egui::Context) {
        self.spawn(ctx, move |catalog| {
            let (wav, _) = catalog.reference(&id)?;
            Ok(Completed::Preview(id, wav[44..].to_vec()))
        });
    }

    fn spawn(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce(VoiceCatalog) -> Result<Completed, String> + Send + 'static,
    ) {
        if self.busy() {
            return;
        }
        let Some(root) = &self.root else {
            return;
        };
        let catalog = VoiceCatalog::new(root);
        let (send, receive) = mpsc::channel();
        let ctx = ctx.clone();
        self.worker = Some(receive);
        // A spawn failure disconnects the receiver and follows the normal error path.
        let _ = std::thread::Builder::new()
            .name("tts-voice-catalog".into())
            .spawn(move || {
                let _ = send.send(work(catalog));
                ctx.request_repaint();
            });
    }
}
