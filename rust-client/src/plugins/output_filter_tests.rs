use super::osc::{OscPlugin, runtime::OscSettings};
use super::vr_overlay::{VrOverlayPlugin, VrOverlaySettings};
use crate::client_settings::{CaptureSource, ClientSettings};
use crate::session_coordinator::{
    AudioSourceFilter, CaptionUpdate, HostOutputEvent, HostOutputSubscriber,
};
use std::net::UdpSocket;
use std::time::{Duration, Instant};

fn caption(
    outputs: &[&dyn HostOutputSubscriber],
    stream_id: u64,
    audio_source: CaptureSource,
    translated: &str,
    is_typing: bool,
    update: CaptionUpdate,
) {
    for output in outputs {
        output.on_host_output(HostOutputEvent::Caption {
            stream_id,
            audio_source,
            is_typing,
            source: "",
            translated,
            speaker: "",
            additional_translations: &[],
            asr_only: false,
            update,
        });
    }
}

fn wait_for_checkpoint(
    osc: &OscPlugin,
    vr: &VrOverlayPlugin,
    checkpoint: &str,
) -> (String, String) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let osc_text = osc.manager().chatbox_preview().text;
        let vr_text = vr
            .manager()
            .status()
            .cards
            .iter()
            .map(|card| card.translated.clone())
            .collect::<Vec<_>>()
            .join("\n");
        if osc_text.contains(checkpoint) && vr_text.contains(checkpoint) {
            return (osc_text, vr_text);
        }
        assert!(
            Instant::now() < deadline,
            "outputs did not reach {checkpoint}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn audio_source_filters_are_independent_and_prune_live_completed_and_pending_output() {
    use CaptionUpdate::{Append, Replace, RollOver};
    use CaptureSource::{Microphone, SystemAudio};

    let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
    let mut osc = OscPlugin::new(
        OscSettings {
            listen_port: 0,
            send_port: receiver.local_addr().unwrap().port(),
            max_text_length: 512,
            microphone_prefix: String::new(),
            system_audio_prefix: String::new(),
            typing_prefix: String::new(),
            ..Default::default()
        },
        true,
    );
    let mut vr = VrOverlayPlugin::new(
        VrOverlaySettings {
            max_items: 5,
            ..Default::default()
        },
        true,
    );
    let osc_output = osc.publisher();
    let vr_output = vr.handle();
    let outputs: [&dyn HostOutputSubscriber; 2] = [&osc_output, &vr_output];
    caption(&outputs, 1, Microphone, "mic-live", false, Replace);
    caption(&outputs, 2, SystemAudio, "system-live", false, Replace);
    caption(&outputs, 3, SystemAudio, "system-final", false, RollOver);
    caption(&outputs, 4, Microphone, "checkpoint-initial", true, Replace);
    let (osc_text, vr_text) = wait_for_checkpoint(&osc, &vr, "checkpoint-initial");
    for text in [&osc_text, &vr_text] {
        assert!(text.contains("mic-live"));
        assert!(text.contains("system-live"));
        assert!(text.contains("system-final"));
    }

    // Opposite selections on the same event stream, applied while results exist.
    osc.draft_mut().audio_source_filter.system_audio = false;
    osc.apply_draft().unwrap();
    vr.draft_mut().audio_source_filter.microphone = false;
    vr.sync_settings();
    caption(&outputs, 1, Microphone, "mic-new", false, Replace);
    caption(&outputs, 2, SystemAudio, "system-new", false, Append);
    caption(
        &outputs,
        4,
        Microphone,
        "checkpoint-filtered",
        true,
        Replace,
    );
    let (osc_text, vr_text) = wait_for_checkpoint(&osc, &vr, "checkpoint-filtered");
    assert!(osc_text.contains("mic-new"));
    assert!(!osc_text.contains("system-"));
    assert!(!vr_text.contains("mic-"));
    assert!(vr_text.contains("system-new"));
    assert!(vr_text.contains("system-final"));

    for filter in [
        &mut osc.draft_mut().audio_source_filter,
        &mut vr.draft_mut().audio_source_filter,
    ] {
        filter.microphone = false;
        filter.system_audio = false;
    }
    osc.apply_draft().unwrap();
    vr.sync_settings();
    caption(&outputs, 1, Microphone, "mic-late", false, RollOver);
    caption(&outputs, 2, SystemAudio, "system-late", false, Replace);
    for output in outputs {
        output.on_host_output(HostOutputEvent::StreamEnded(1));
        output.on_host_output(HostOutputEvent::StreamEnded(2));
    }
    caption(&outputs, 4, Microphone, "checkpoint-hidden", true, Replace);
    let (osc_text, vr_text) = wait_for_checkpoint(&osc, &vr, "checkpoint-hidden");
    assert_eq!(osc_text, "checkpoint-hidden");
    assert_eq!(vr_text, "checkpoint-hidden");

    osc.draft_mut().audio_source_filter = AudioSourceFilter::default();
    osc.apply_draft().unwrap();
    vr.draft_mut().audio_source_filter = AudioSourceFilter::default();
    vr.sync_settings();
    caption(&outputs, 4, Microphone, "checkpoint-resumed", true, Replace);
    let (osc_text, vr_text) = wait_for_checkpoint(&osc, &vr, "checkpoint-resumed");
    assert_eq!(osc_text, "checkpoint-resumed");
    assert_eq!(vr_text, "checkpoint-resumed");

    caption(&outputs, 1, Microphone, "mic-resumed", false, Replace);
    caption(&outputs, 2, SystemAudio, "system-resumed", false, RollOver);
    caption(&outputs, 4, Microphone, "checkpoint-new", true, Replace);
    let (osc_text, vr_text) = wait_for_checkpoint(&osc, &vr, "checkpoint-new");
    for text in [&osc_text, &vr_text] {
        assert!(text.contains("mic-resumed"));
        assert!(text.contains("system-resumed"));
        assert!(!text.contains("late"));
    }
}

#[test]
fn audio_source_filters_preserve_legacy_settings_and_persist_independently() {
    let mut settings = ClientSettings::default();
    let mut legacy = serde_json::to_value(&settings).unwrap();
    for plugin in ["osc_settings", "vr_overlay_settings"] {
        legacy[plugin]
            .as_object_mut()
            .unwrap()
            .remove("audio_source_filter");
    }
    let restored: ClientSettings = serde_json::from_value(legacy).unwrap();
    assert_eq!(
        restored.osc_settings.audio_source_filter,
        AudioSourceFilter::default()
    );
    assert_eq!(
        restored.vr_overlay_settings.audio_source_filter,
        AudioSourceFilter::default()
    );

    settings.osc_settings.audio_source_filter.system_audio = false;
    settings.vr_overlay_settings.audio_source_filter.microphone = false;
    let saved = serde_json::to_string(&settings).unwrap();
    let restored: ClientSettings = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        restored.osc_settings.audio_source_filter,
        settings.osc_settings.audio_source_filter
    );
    assert_eq!(
        restored.vr_overlay_settings.audio_source_filter,
        settings.vr_overlay_settings.audio_source_filter
    );
}

#[test]
fn audio_source_filter_controls_apply_and_save_through_each_plugin_page() {
    use crate::i18n::{UiLanguage, tr};
    use crate::ui::automation::{
        self,
        driver::{CommandEnvelope, DirectorCommand},
    };
    use eframe::egui;

    for language in UiLanguage::ALL {
        for is_osc in [true, false] {
            let ctx = egui::Context::default();
            crate::ui::fonts::configure_multilingual_fonts(&ctx);
            let mut osc = OscPlugin::new(
                OscSettings {
                    enabled: false,
                    listen_port: 0,
                    ..Default::default()
                },
                true,
            );
            let mut vr = VrOverlaySettings::default();
            let status = super::vr_overlay::runtime::VrRuntimeStatus::default();
            let mut osc_actions = Vec::new();
            let mut vr_actions = Vec::new();
            // Register the current page before resolving a label to its widget ID.
            for click in [false, true] {
                let (tx, _rx) = crossbeam_channel::bounded(1);
                if click {
                    automation::driver()
                        .channel()
                        .send(CommandEnvelope {
                            command: DirectorCommand::Click(tr(language, "System Audio").into()),
                            responder: tx,
                        })
                        .unwrap();
                }
                automation::begin_frame(&ctx, "filter-test");
                let mut output = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(320.0, 720.0),
                        )),
                        ..Default::default()
                    },
                    |ui| {
                        if is_osc {
                            super::osc::ui::toolbar::render_toolbar(
                                &mut osc,
                                ui,
                                language,
                                false,
                                &mut osc_actions,
                            );
                        } else {
                            vr_actions.extend(super::vr_overlay::ui::render(
                                &mut vr,
                                ui,
                                super::vr_overlay::VrOverlayPageContext {
                                    language,
                                    status: &status,
                                },
                            ));
                        }
                    },
                );
                automation::finish_frame("filter-test");
                output.textures_delta.clear();
            }
            if is_osc {
                assert!(osc_actions.contains(&super::osc::OscUiAction::SettingsApplied(Ok(()))));
                assert!(osc_actions.contains(&super::osc::OscUiAction::SaveSettings));
                assert!(osc.draft().audio_source_filter.microphone);
                assert!(!osc.draft().audio_source_filter.system_audio);
            } else {
                assert!(
                    vr_actions.contains(&super::vr_overlay::VrOverlayUiAction::SettingsChanged)
                );
                assert!(vr.audio_source_filter.microphone);
                assert!(!vr.audio_source_filter.system_audio);
            }
        }
    }
}
