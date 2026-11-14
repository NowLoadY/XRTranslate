use super::*;

fn entry() -> crate::history::TranslationHistoryEntry {
    crate::history::TranslationHistoryEntry::preview(
        1,
        crate::CaptureSource::Microphone,
        xrtranslate_protocol::TranslationPreview {
            source_text: "你好，世界。".into(),
            translated_text: "Hello, world.".into(),
            turn_id: "one".into(),
            segment_index: 1,
            revision: 1,
            speaker_id: "speaker_42".into(),
        },
    )
}

#[test]
fn bilingual_pairs_original_and_output_without_repeating_asr() {
    let mut entry = entry();
    let translated = PresentationLine::new(egui::Id::unique("line"), &entry, false);
    assert_eq!(translated.text, "Hello, world.");
    assert_eq!(translated.source_end, 0);
    let bilingual = PresentationLine::new(translated.id, &entry, true);
    assert_eq!(bilingual.text, "你好，世界。\nHello, world.");
    assert_eq!(&bilingual.text[..bilingual.source_end], "你好，世界。\n");
    assert_eq!(
        bilingual
            .ranges
            .iter()
            .map(|range| &bilingual.text[range.clone()])
            .collect::<Vec<_>>(),
        ["你好，世界。", "Hello, world."]
    );
    entry.asr_only = true;
    entry.translated = entry.source.clone();
    assert_eq!(
        PresentationLine::new(translated.id, &entry, true).text,
        "你好，世界。"
    );
}

#[test]
fn source_only_preview_keeps_a_valid_unicode_boundary() {
    let mut entry = entry();
    entry.translated.clear();
    let line = PresentationLine::new(egui::Id::unique("line"), &entry, true);
    assert_eq!(&line.text[..line.source_end], "你好，世界。");
    assert!(line.text[line.source_end..].is_empty());
}

#[test]
fn manual_scrolling_to_bottom_resumes_following_new_results() {
    let ctx = egui::Context::default();
    let mut fonts = egui::FontDefinitions::default();
    fonts.families.insert(
        crate::ui::fonts::bold_family(),
        fonts.families[&egui::FontFamily::Proportional].clone(),
    );
    ctx.set_fonts(fonts);
    let mut state = Presentation::default();
    state.enter();
    let mut selection = components::selectable_text::SelectionState::default();
    let mut lines = (0..8)
        .map(|index| PresentationLine::new(egui::Id::unique(index), &entry(), false))
        .collect::<Vec<_>>();
    let mut time = 0.0;
    let mut frame =
        |state: &mut Presentation, lines: &[PresentationLine], events: Vec<egui::Event>| {
            time += 0.05;
            let mut offset = 0.0;
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(600.0, 320.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| {
                    render_lines(
                        lines,
                        state,
                        &mut selection,
                        crate::i18n::UiLanguage::English,
                        Color32::BLACK,
                        Color32::WHITE,
                        ui,
                    );
                    offset = egui::scroll_area::State::load(
                        ui.ctx(),
                        ui.make_persistent_id(egui::IdSalt::new("translation_presentation")),
                    )
                    .unwrap()
                    .clamped_offset()
                    .y;
                },
            );
            offset
        };
    for _ in 0..12 {
        frame(&mut state, &lines, vec![]);
    }
    let initial = frame(&mut state, &lines, vec![]);
    assert!(initial > 0.0);
    let wheel = |delta| {
        vec![
            egui::Event::PointerMoved(egui::pos2(300.0, 160.0)),
            egui::Event::MouseWheel {
                source: egui::MouseWheelSource::Wheel,
                phase: egui::TouchPhase::Move,
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, delta),
                modifiers: egui::Modifiers::NONE,
            },
        ]
    };
    frame(&mut state, &lines, wheel(240.0));
    for _ in 0..20 {
        frame(&mut state, &lines, vec![]);
    }
    assert!(!state.following);
    assert!(frame(&mut state, &lines, vec![]) < initial - 100.0);
    frame(&mut state, &lines, wheel(-2000.0));
    for _ in 0..20 {
        frame(&mut state, &lines, vec![]);
    }
    assert!(state.following);
    lines.push(PresentationLine::new(egui::Id::unique(9), &entry(), true));
    for _ in 0..20 {
        frame(&mut state, &lines, vec![]);
    }
    assert!(frame(&mut state, &lines, vec![]) > initial);
    assert!(state.following);
}
