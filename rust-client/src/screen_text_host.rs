//! Composes the screen reader, plugin and shared translator without coupling them.
use crate::{
    PendingResourceDeletion, XRTranslateApp,
    ocr_capture::{CaptureEvent, OcrCapture},
    overlay_ipc::{OverlayEvent, OverlayRegion},
};

#[derive(Default)]
pub(crate) struct ScreenTextHost {
    pub state: xrtranslate_client_core::screen_text::ScreenTextState,
    owner: Option<String>,
    pub enabled: bool,
    pub resource_deletion: Option<PendingResourceDeletion>,
    capture: Option<OcrCapture>,
    stopping: bool,
    requested: bool,
    region: Option<OverlayRegion>,
    result_region: Option<OverlayRegion>,
    controller_region: Option<OverlayRegion>,
    result_moving: bool,
    revision: u64,
    languages: Option<(String, String)>,
    model: Option<xrtranslate_assets::ModelAssetId>,
}

impl ScreenTextHost {
    pub fn reset_window(&mut self) {
        self.enabled = false;
        self.region = None;
        self.result_region = None;
        self.controller_region = None;
        self.result_moving = false;
    }

    pub fn is_stopping(&self) -> bool {
        self.stopping && self.capture.is_some()
    }

    fn exclusions(&self) -> [Option<OverlayRegion>; 2] {
        [self.result_region, self.controller_region]
            .map(|area| crate::screen_capture::intersection(self.region?, area?).ok())
    }
}

impl XRTranslateApp {
    pub(crate) fn screen_text_available(&self) -> bool {
        self.service_config.ocr_is_configured()
            && self.plugin_host.state_owner("screen_text").is_some()
    }
    pub(crate) fn set_ocr_enabled(&mut self, enabled: bool) {
        self.screen_text.enabled = enabled && self.screen_text_available();
        if self.screen_text.enabled {
            // Opening the region only selects the OCR input. Capture starts
            // when the independently controlled translation service is active.
            self.refresh_ocr();
        } else {
            self.stop_ocr();
        }
    }

    fn cancel_ocr_translation(&mut self) {
        self.screen_text.revision = self.screen_text.revision.wrapping_add(1);
        self.screen_text.state = Default::default();
        if let Some(owner) = &self.screen_text.owner {
            self.plugin_host.send_event(
                owner,
                "screen.clear",
                serde_json::json!(self.screen_text.revision),
            );
        }
    }

    pub(crate) fn stop_ocr(&mut self) {
        if !self.screen_text.requested
            && (self.screen_text.capture.is_none() || self.screen_text.stopping)
        {
            return;
        }
        self.screen_text.requested = false;
        if let Some(capture) = &self.screen_text.capture {
            capture.cancel();
            self.screen_text.stopping = true;
        }
        self.cancel_ocr_translation();
    }

    pub(crate) fn poll_ocr_cleanup(&mut self) {
        if self.first_run {
            self.stop_ocr();
        }
        if self.screen_text.stopping
            && self
                .screen_text
                .capture
                .as_ref()
                .is_none_or(OcrCapture::is_finished)
        {
            self.screen_text.capture = None;
            self.screen_text.stopping = false;
        }
        if !self.screen_text.is_stopping()
            && let Some(resource) = self.screen_text.resource_deletion.take()
        {
            self.delete_resource(resource);
        }
    }

    pub(crate) fn refresh_ocr(&mut self) {
        self.request_ocr_capture(true);
    }

    fn request_ocr_capture(&mut self, clear_result: bool) {
        if self.first_run
            || !self.screen_text.enabled
            || !self.floating_subtitles_enabled
            || !self.translation_enabled
            || !self.screen_text_available()
            || self.screen_text.resource_deletion.is_some()
        {
            return;
        }
        if clear_result {
            self.cancel_ocr_translation();
        }
        if self
            .screen_text
            .capture
            .as_ref()
            .is_some_and(OcrCapture::is_finished)
        {
            self.screen_text.capture = None;
            self.screen_text.stopping = false;
        }
        self.screen_text.revision = self.screen_text.revision.wrapping_add(1);
        self.screen_text.requested = !self.screen_text.result_moving;
        if let Some(region) = self.screen_text.region
            && !self.screen_text.stopping
            && let Some(capture) = &self.screen_text.capture
        {
            capture.update(
                self.screen_text.requested.then_some(region),
                self.screen_text.revision,
                self.screen_text.exclusions(),
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
                self.screen_text.region = Some(region);
                self.refresh_ocr();
            }
            OverlayEvent::RegionChanging => {
                self.screen_text.region = None;
                self.screen_text.requested = false;
                self.screen_text.revision = self.screen_text.revision.wrapping_add(1);
                self.cancel_ocr_translation();
                if let Some(capture) = &self.screen_text.capture {
                    capture.update(None, self.screen_text.revision, [None; 2]);
                }
            }
            OverlayEvent::ResultChanging => {
                self.screen_text.result_moving = true;
                self.screen_text.requested = false;
                self.screen_text.revision = self.screen_text.revision.wrapping_add(1);
                // Presentation moved, but accepted text is still valid. Keep
                // its translation running while new pixels are unavailable.
                if let Some(capture) = &self.screen_text.capture {
                    capture.update(None, self.screen_text.revision, [None; 2]);
                }
            }
            OverlayEvent::ResultRegionChanged(region) => {
                let before = self.screen_text.exclusions();
                let was_moving = std::mem::replace(&mut self.screen_text.result_moving, false);
                self.screen_text.result_region = region;
                if was_moving || before != self.screen_text.exclusions() {
                    self.request_ocr_capture(false);
                }
            }
            OverlayEvent::ControllerRegionChanged(region) => {
                let before = self.screen_text.exclusions();
                self.screen_text.controller_region = region;
                if before != self.screen_text.exclusions() {
                    self.request_ocr_capture(false);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn poll_ocr(&mut self) {
        let owner = self
            .plugin_host
            .state_owner("screen_text")
            .map(str::to_owned);
        if owner != self.screen_text.owner {
            self.stop_ocr();
            self.screen_text.owner = owner;
            self.screen_text.state = Default::default();
            self.screen_text.enabled = false;
        }
        self.poll_ocr_cleanup();
        let available = self.screen_text_available();
        if !available
            || !self.floating_subtitles_enabled
            || !self.translation_enabled
            || !self.screen_text.enabled
        {
            self.stop_ocr();
            if !available || !self.floating_subtitles_enabled {
                self.screen_text.enabled = false;
            }
        }
        let languages = (self.source_lang.clone(), self.target_lang.clone());
        let model = self.service_config.ocr_model_asset_id();
        let changed_model = self.screen_text.model != model;
        let changed_languages = self.screen_text.languages.as_ref() != Some(&languages);
        self.screen_text.model = model;
        self.screen_text.languages = Some(languages);
        if changed_model {
            self.stop_ocr();
        }
        if changed_model || changed_languages {
            self.refresh_ocr();
        }
        if self.screen_text.requested
            && self.screen_text.capture.is_none()
            && let Some(region) = self.screen_text.region
        {
            match OcrCapture::start(
                self.project_root(),
                region,
                self.screen_text.revision,
                self.screen_text.exclusions(),
            ) {
                Ok(capture) => self.screen_text.capture = Some(capture),
                Err(error) => {
                    self.screen_text.requested = false;
                    if let Some(owner) = &self.screen_text.owner {
                        self.plugin_host.send_event(
                            owner,
                            "screen.text",
                            serde_json::json!(CaptureEvent::Failed {
                                revision: self.screen_text.revision,
                                error,
                                fatal: true
                            }),
                        );
                    }
                }
            }
        }
        let events: Vec<_> = self
            .screen_text
            .capture
            .as_ref()
            .filter(|_| !self.screen_text.stopping)
            .map(|capture| capture.poll().collect())
            .unwrap_or_default();
        for mut event in events {
            if let CaptureEvent::Failed {
                fatal: true,
                revision,
                ..
            } = &mut event
            {
                self.stop_ocr();
                *revision = self.screen_text.revision;
            }
            if let Some(owner) = &self.screen_text.owner {
                self.plugin_host
                    .send_event(owner, "screen.text", serde_json::json!(event));
            }
        }
        if let Some(state) = self.screen_text.owner.as_ref().and_then(|owner| {
            let state = &self.plugin_host.state(owner)["screen_text"];
            (state["revision"].as_u64() == Some(self.screen_text.revision))
                .then(|| serde_json::from_value(state["view"].clone()).ok())
                .flatten()
        }) {
            self.screen_text.state = state;
        }
        if let Ok(mut manager) = self.overlay_manager.lock() {
            manager.send_ocr(available.then(|| self.screen_text.state.clone()));
        }
    }
}
