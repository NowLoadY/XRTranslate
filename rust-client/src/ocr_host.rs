//! Composes the screen reader, plugin and shared translator without coupling them.
use crate::{
    PendingResourceDeletion, XRTranslateApp,
    ocr_capture::{CaptureEvent, OcrCapture},
    overlay_ipc::{OverlayEvent, OverlayRegion},
    plugins::{PluginId, ocr::OcrPlugin},
};

#[derive(Default)]
pub(crate) struct OcrHost {
    pub plugin: OcrPlugin,
    pub enabled: bool,
    pub resource_deletion: Option<PendingResourceDeletion>,
    capture: Option<OcrCapture>,
    stopping: bool,
    requested: bool,
    region: Option<OverlayRegion>,
    result_region: Option<OverlayRegion>,
    companion_region: Option<OverlayRegion>,
    result_moving: bool,
    revision: u64,
    languages: Option<(String, String)>,
    model: Option<xrtranslate_assets::ModelAssetId>,
}

impl OcrHost {
    pub fn reset_window(&mut self) {
        self.enabled = false;
        self.region = None;
        self.result_region = None;
        self.companion_region = None;
        self.result_moving = false;
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping && self.capture.is_some()
    }

    fn exclusions(&self) -> [Option<OverlayRegion>; 2] {
        [self.result_region, self.companion_region]
            .map(|area| crate::screen_capture::intersection(self.region?, area?).ok())
    }
}

impl XRTranslateApp {
    pub(crate) fn set_ocr_enabled(&mut self, enabled: bool) {
        self.ocr.enabled = enabled && self.plugin_enabled(PluginId::OCR);
        if self.ocr.enabled {
            // Opening the region only selects the OCR input. Capture starts
            // when the independently controlled translation service is active.
            self.refresh_ocr();
        } else {
            self.stop_ocr();
        }
    }

    fn cancel_ocr_translation(&mut self) {
        self.stop_plugin_task(PluginId::OCR);
        self.ocr.plugin.clear();
    }

    pub(crate) fn stop_ocr(&mut self) {
        self.ocr.requested = false;
        if let Some(capture) = &self.ocr.capture {
            capture.cancel();
            self.ocr.stopping = true;
        }
        self.cancel_ocr_translation();
    }

    pub(crate) fn poll_ocr_cleanup(&mut self) {
        if self.first_run {
            self.stop_ocr();
        }
        if self.ocr.stopping
            && self
                .ocr
                .capture
                .as_ref()
                .is_none_or(OcrCapture::is_finished)
        {
            self.ocr.capture = None;
            self.ocr.stopping = false;
        }
        if !self.ocr.is_stopping()
            && let Some(resource) = self.ocr.resource_deletion.take()
        {
            self.delete_resource(resource);
        }
    }

    pub(crate) fn refresh_ocr(&mut self) {
        self.request_ocr_capture(true);
    }

    fn request_ocr_capture(&mut self, clear_result: bool) {
        if self.first_run
            || !self.ocr.enabled
            || !self.floating_subtitles_enabled
            || !self.translation_enabled
            || !self.plugin_enabled(PluginId::OCR)
            || self.ocr.resource_deletion.is_some()
        {
            return;
        }
        if clear_result {
            self.cancel_ocr_translation();
        }
        if self
            .ocr
            .capture
            .as_ref()
            .is_some_and(OcrCapture::is_finished)
        {
            self.ocr.capture = None;
            self.ocr.stopping = false;
        }
        self.ocr.revision = self.ocr.revision.wrapping_add(1);
        self.ocr.requested = !self.ocr.result_moving;
        if let Some(region) = self.ocr.region
            && !self.ocr.stopping
            && let Some(capture) = &self.ocr.capture
        {
            capture.update(
                self.ocr.requested.then_some(region),
                self.ocr.revision,
                self.ocr.exclusions(),
            );
        }
    }

    pub(crate) fn handle_ocr_event(&mut self, event: OverlayEvent) {
        if !self.floating_subtitles_enabled || self.first_run {
            return;
        }
        match event {
            OverlayEvent::OcrEnabled(enabled) => self.set_ocr_enabled(enabled),
            OverlayEvent::RegionChanged(region) => {
                self.ocr.region = Some(region);
                self.refresh_ocr();
            }
            OverlayEvent::RegionChanging => {
                self.ocr.region = None;
                self.ocr.requested = false;
                self.ocr.revision = self.ocr.revision.wrapping_add(1);
                self.cancel_ocr_translation();
                if let Some(capture) = &self.ocr.capture {
                    capture.update(None, self.ocr.revision, [None; 2]);
                }
            }
            OverlayEvent::ResultChanging => {
                self.ocr.result_moving = true;
                self.ocr.requested = false;
                self.ocr.revision = self.ocr.revision.wrapping_add(1);
                // Presentation moved, but accepted text is still valid. Keep
                // its translation running while new pixels are unavailable.
                if let Some(capture) = &self.ocr.capture {
                    capture.update(None, self.ocr.revision, [None; 2]);
                }
            }
            OverlayEvent::ResultRegionChanged(region) => {
                let before = self.ocr.exclusions();
                let was_moving = std::mem::replace(&mut self.ocr.result_moving, false);
                self.ocr.result_region = region;
                if was_moving || before != self.ocr.exclusions() {
                    self.request_ocr_capture(false);
                }
            }
            OverlayEvent::CompanionRegionChanged(region) => {
                let before = self.ocr.exclusions();
                self.ocr.companion_region = region;
                if before != self.ocr.exclusions() {
                    self.request_ocr_capture(false);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn poll_ocr(&mut self) {
        self.poll_ocr_cleanup();
        let available = self.plugin_enabled(PluginId::OCR);
        if !available
            || !self.floating_subtitles_enabled
            || !self.translation_enabled
            || !self.ocr.enabled
        {
            self.stop_ocr();
            if !available || !self.floating_subtitles_enabled {
                self.ocr.enabled = false;
            }
        }
        let languages = (self.source_lang.clone(), self.target_lang.clone());
        let model = self.service_config.ocr_model_asset_id();
        let changed_model = self.ocr.model != model;
        let changed_languages = self.ocr.languages.as_ref() != Some(&languages);
        self.ocr.model = model;
        self.ocr.languages = Some(languages);
        if changed_model {
            self.stop_ocr();
        }
        if changed_model || changed_languages {
            self.refresh_ocr();
        }
        if self.ocr.requested
            && self.ocr.capture.is_none()
            && let Some(region) = self.ocr.region
        {
            match OcrCapture::start(
                self.project_root(),
                region,
                self.ocr.revision,
                self.ocr.exclusions(),
            ) {
                Ok(capture) => self.ocr.capture = Some(capture),
                Err(error) => {
                    self.ocr.requested = false;
                    self.ocr.plugin.failed(error);
                }
            }
        }
        let events: Vec<_> = self
            .ocr
            .capture
            .as_ref()
            .filter(|_| !self.ocr.stopping)
            .map(|capture| capture.poll().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                CaptureEvent::Busy(revision) if revision == self.ocr.revision => {
                    self.ocr.plugin.set_recognizing(true);
                }
                CaptureEvent::Idle(revision) if revision == self.ocr.revision => {
                    self.ocr.plugin.set_recognizing(false);
                }
                CaptureEvent::Text(revision, text) if revision == self.ocr.revision => {
                    if let Some(binding) = self.ocr.plugin.recognized(text.clone()) {
                        if !self.submit_text_translation(&text, None, None, Some(binding)) {
                            self.ocr
                                .plugin
                                .failed(self.last_error.clone().unwrap_or_default());
                        }
                    }
                }
                CaptureEvent::Failed {
                    revision,
                    error,
                    fatal,
                } if revision == self.ocr.revision => {
                    if fatal {
                        self.stop_ocr();
                        self.ocr.plugin.failed(error);
                    } else {
                        self.ocr.plugin.capture_failed(error);
                    }
                }
                _ => {}
            }
        }
        self.ocr.plugin.poll();
        if let Ok(mut manager) = self.overlay_manager.lock() {
            manager.send_ocr(available.then(|| self.ocr.plugin.state.clone()));
        }
    }
}
