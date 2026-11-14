use super::*;
use crate::history::{FeedTimeline, RecognitionHistoryEntry, TranslationHistoryEntry};

fn translation(turn: &str, source: &str, translated: &str) -> TranslationHistoryEntry {
    TranslationHistoryEntry::preview(
        1,
        crate::CaptureSource::Microphone,
        xrtranslate_protocol::TranslationPreview {
            source_text: source.into(),
            translated_text: translated.into(),
            turn_id: turn.into(),
            segment_index: 1,
            revision: 1,
            speaker_id: "speaker_42".into(),
        },
    )
}

fn recognition(text: &str) -> RecognitionHistoryEntry {
    RecognitionHistoryEntry {
        stream_id: Some(1),
        live: false,
        text: text.into(),
        turn_id: "one".into(),
        segment_index: 0,
        speaker_id: "speaker_42".into(),
        source_start_ms: 0.0,
        source_end_ms: 0.0,
        timing: Default::default(),
        boundary: Default::default(),
        activation_matches: Vec::new(),
        context_matches: Vec::new(),
        revisable: false,
        overlap_ratio: 0.0,
        authoritative_snapshot: false,
        revision_id: 1,
        revision: None,
    }
}

fn transcript(
    recognition: &[RecognitionHistoryEntry],
    translations: &[TranslationHistoryEntry],
    partial: &str,
) -> Transcript {
    Transcript::from_segments(
        &super::super::collect_history_segments(
            recognition,
            translations,
            &FeedTimeline::default(),
        ),
        partial,
    )
}

#[test]
fn documents_use_latest_revisions_without_speaker_or_language_markers() {
    let first = translation("one", "你好。", "Hello.");
    let mut revised = first.clone();
    revised.revision_id = 2;
    revised.translated = "Hello!".into();
    revised
        .additional_translations
        .push(xrtranslate_protocol::AdditionalTranslation {
            target_lang: "ja".into(),
            translated_text: "こんにちは。".into(),
            term_matches: Vec::new(),
        });
    let second = translation("two", "再见。", "Goodbye.");
    let document = transcript(&[], &[first, second, revised], "正在识别");
    assert_eq!(document.original, "你好。\n再见。\n正在识别");
    assert_eq!(document.translation, "Hello!\nこんにちは。\nGoodbye.");
    assert_eq!(
        document.bilingual,
        "你好。\nHello!\nこんにちは。\n\n再见。\nGoodbye.\n\n正在识别"
    );
}

#[test]
fn whole_turn_recognition_and_segmented_translations_stay_paired() {
    let first = translation("one", "Hello.", "你好。");
    let mut second = translation("one", "Goodbye.", "再见。");
    second.segment_index = 2;
    let document = transcript(&[recognition("Hello. Goodbye.")], &[second, first], "");
    assert_eq!(document.original, "Hello. Goodbye.");
    assert_eq!(document.translation, "你好。再见。");
    assert_eq!(document.bilingual, "Hello. Goodbye.\n你好。再见。");
}

#[test]
fn asr_only_preserves_pending_tail_then_uses_corrected_text_once() {
    let recognized = [recognition("hello world")];
    let mut entry = translation("one", "hello", "Hello");
    entry.asr_only = true;
    let pending = transcript(&recognized, &[entry.clone()], "");
    assert_eq!(pending.original, "hello world");
    assert_eq!(pending.bilingual, "hello world");
    assert!(pending.translation.is_empty());
    entry.source = "hello world".into();
    entry.translated = "Hello world.".into();
    let complete = transcript(&recognized, &[entry], "");
    assert_eq!(complete.original, "Hello world.");
    assert_eq!(complete.bilingual, "Hello world.");
    assert!(complete.translation.is_empty());
}

#[test]
fn empty_and_partial_results_do_not_add_blank_blocks_or_output() {
    let document = transcript(&[], &[translation("one", "  ", "\n")], "  Listening…  ");
    assert_eq!(document.original, "Listening…");
    assert_eq!(document.bilingual, "Listening…");
    assert!(document.translation.is_empty());
    assert!(transcript(&[], &[], "").bilingual.is_empty());
}

#[test]
fn existing_display_preferences_still_deserialize() {
    for (stored, expected) in [
        ("bubbles", DisplayMode::Bubbles),
        ("text", DisplayMode::Text),
        ("split", DisplayMode::Split),
    ] {
        let settings: super::super::HistoryDisplay =
            serde_json::from_value(serde_json::json!({"translation": stored})).unwrap();
        assert_eq!(settings.mode, expected);
    }
}

struct Harness {
    ctx: egui::Context,
    document: Transcript,
    split: bool,
    size: egui::Vec2,
    time: f64,
}

impl Harness {
    fn new(split: bool) -> Self {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        Self {
            ctx,
            document: transcript(
                &[],
                &[
                    translation("one", "你好。", "Hello."),
                    translation("two", "再见。", "Goodbye."),
                ],
                "",
            ),
            split,
            size: egui::vec2(320.0, 240.0),
            time: 0.0,
        }
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Vec<String> {
        self.time += 0.05;
        let output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
                time: Some(self.time),
                events,
                ..Default::default()
            },
            |ui| {
                render_document(ui, &self.document, self.split, UiLanguage::English, 14.0);
                assert!(
                    ui.min_rect().right() <= self.size.x + 1.0,
                    "horizontal overflow: {:?}",
                    ui.min_rect()
                );
                assert!(
                    ui.min_rect().bottom() <= self.size.y + 1.0,
                    "vertical overflow: {:?}",
                    ui.min_rect()
                );
            },
        );
        output
            .platform_output
            .commands
            .into_iter()
            .filter_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    fn click(&mut self, position: egui::Pos2) -> Vec<String> {
        self.frame(pointer(position, true));
        self.frame(pointer(position, false))
    }

    fn select_all(&mut self) {
        self.frame(vec![egui::Event::Key {
            key: egui::Key::A,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        }]);
    }
}

fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        egui::Event::PointerButton {
            pos,
            pressed,
            button: egui::PointerButton::Primary,
            modifiers: egui::Modifiers::NONE,
        },
    ]
}

#[test]
fn select_all_copies_across_sentences_and_freezes_live_revisions() {
    let mut harness = Harness::new(false);
    harness.frame(vec![]);
    harness.click(egui::pos2(25.0, 48.0));
    assert!(harness.ctx.memory(|memory| memory.focused().is_some()));
    harness.select_all();
    let selected = harness.document.bilingual.clone();
    harness.document.bilingual = "A revised live transcript".into();
    assert_eq!(harness.frame(vec![egui::Event::Copy]), [selected]);
    // Returning to live text releases the selection and refreshes the document.
    assert!(harness.click(egui::pos2(304.0, 16.0)).is_empty());
    harness.click(egui::pos2(25.0, 36.0));
    harness.frame(vec![egui::Event::Text(
        "must not edit the transcript".into(),
    )]);
    harness.select_all();
    assert_eq!(
        harness.frame(vec![egui::Event::Copy]),
        [harness.document.bilingual.clone()]
    );
}

#[test]
fn split_selection_copies_only_the_focused_document() {
    let mut harness = Harness::new(true);
    harness.frame(vec![]);
    assert!(harness.click(egui::pos2(304.0, 16.0)).is_empty());
    harness.click(egui::pos2(25.0, 48.0));
    harness.select_all();
    assert_eq!(
        harness.frame(vec![egui::Event::Copy]),
        [harness.document.original.clone()]
    );
    assert!(harness.click(egui::pos2(304.0, 141.0)).is_empty());
    harness.click(egui::pos2(25.0, 173.0));
    harness.select_all();
    assert_eq!(
        harness.frame(vec![egui::Event::Copy]),
        [harness.document.translation.clone()]
    );
}

#[test]
fn selecting_original_keeps_the_translation_live() {
    let mut harness = Harness::new(true);
    harness.frame(vec![]);
    harness.click(egui::pos2(25.0, 48.0));
    harness.select_all();
    let selected = harness.document.original.clone();
    harness.document.original = "修订后的原文".into();
    harness.document.translation = "Revised translation".into();
    assert_eq!(harness.frame(vec![egui::Event::Copy]), [selected]);
    harness.click(egui::pos2(25.0, 173.0));
    harness.select_all();
    assert_eq!(
        harness.frame(vec![egui::Event::Copy]),
        ["Revised translation"]
    );
}

#[test]
fn clicking_bubble_text_or_padding_copies_only_that_bubbles_body() {
    use super::super::{HistoryFeedScale, collect_history_segments, history_bubble, prepare_turn};

    let ctx = egui::Context::default();
    let entries = [
        translation("one", "你好。", "Hello."),
        translation("two", "再见。", "Goodbye."),
    ];
    let segments = collect_history_segments(&[], &entries, &FeedTimeline::default());
    let mut time = 0.0;
    let mut frame = |events| {
        time += 0.05;
        let mut rects = Vec::new();
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 480.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                ui.style_mut().interaction.selectable_labels = true;
                let scale = HistoryFeedScale::compute(ui.available_width(), ui.available_height());
                for (index, segment) in segments.iter().enumerate() {
                    let (height, row) = prepare_turn(ui, segment, &scale, egui::Id::unique(index));
                    rects.push(
                        history_bubble(ui, &row, 1.0, height, &scale, UiLanguage::English, || {
                            segment_text(segment)
                        })
                        .rect,
                    );
                }
            },
        );
        let copied = output
            .platform_output
            .commands
            .into_iter()
            .filter_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>();
        (copied, rects)
    };
    let (_, rects) = frame(vec![]);
    let text = rects[0].min + egui::vec2(20.0, 14.0);
    frame(pointer(text, true));
    assert_eq!(frame(pointer(text, false)).0, ["你好。\nHello."]);
    let padding = egui::pos2(rects[1].right() - 2.0, rects[1].center().y);
    frame(pointer(padding, true));
    assert_eq!(frame(pointer(padding, false)).0, ["再见。\nGoodbye."]);
}

#[test]
fn long_documents_fit_small_panes_and_resizing_keeps_both_visible() {
    let mut harness = Harness::new(true);
    harness.size = egui::vec2(220.0, 180.0);
    harness.document.original = "Long original sentence. ".repeat(200);
    harness.document.translation = "Long translated sentence. ".repeat(200);
    harness.frame(vec![]);
    harness.frame(pointer(egui::pos2(110.0, 90.0), true));
    harness.frame(vec![egui::Event::PointerMoved(egui::pos2(110.0, 170.0))]);
    harness.frame(pointer(egui::pos2(110.0, 170.0), false));
}
