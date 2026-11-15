use super::super::super::runtime::ChatboxRegions;
use super::*;

fn text_shapes(shape: &egui::Shape, output: &mut Vec<(String, egui::Rect)>) {
    match shape {
        egui::Shape::Text(text) => output.push((
            text.galley.text().to_owned(),
            text.galley.rect.translate(text.pos.to_vec2()),
        )),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                text_shapes(shape, output);
            }
        }
        _ => {}
    }
}

fn frame(
    ctx: &egui::Context,
    width: f32,
    settings: &mut OscSettings,
    preview: &ChatboxPreview,
    events: Vec<egui::Event>,
) -> Vec<(String, egui::Rect)> {
    let output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 4096.0),
            )),
            events,
            ..Default::default()
        },
        |ui| {
            render(ui, settings, preview, UiLanguage::English, true);
        },
    );
    let mut text = Vec::new();
    for shape in &output.shapes {
        text_shapes(&shape.shape, &mut text);
    }
    text
}

fn bounds(shapes: &[(String, egui::Rect)], text: &str) -> egui::Rect {
    shapes
        .iter()
        .find(|(value, _)| value == text)
        .unwrap_or_else(|| panic!("Missing {text:?}: {shapes:?}"))
        .1
}

fn click(
    ctx: &egui::Context,
    settings: &mut OscSettings,
    preview: &ChatboxPreview,
    at: egui::Pos2,
) -> Vec<(String, egui::Rect)> {
    for pressed in [true, false] {
        frame(
            ctx,
            620.0,
            settings,
            preview,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    frame(ctx, 620.0, settings, preview, Vec::new())
}

#[test]
fn dropdowns_follow_rendered_rows_at_narrow_and_wide_widths() {
    for width in [360.0, 620.0, 900.0] {
        let ctx = egui::Context::default();
        let mut settings = OscSettings::default();
        let mut preview = ChatboxPreview {
            regions: ChatboxRegions {
                header: "HEADER".into(),
                body: "Short message".into(),
                footer: "FOOTER".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut short_footer = 0.0;
        for long in [false, true] {
            if long {
                preview.regions.body =
                    "A translated sentence that wraps within its available preview space. "
                        .repeat(8);
            }
            let mut shapes = Vec::new();
            for _ in 0..3 {
                shapes = frame(&ctx, width, &mut settings, &preview, Vec::new());
            }
            for (region, definition) in [
                (&preview.regions.header, "Header: · None"),
                (&preview.regions.footer, "Footer: · None"),
            ] {
                let text = bounds(&shapes, region);
                let control = bounds(&shapes, definition);
                assert!(control.left() > text.right());
                assert!(
                    (control.center().y - text.center().y).abs() <= 2.0,
                    "width {width}, {definition}: {text:?} versus {control:?}"
                );
                assert!(control.right() <= width + 1.0);
            }
            let body = bounds(&shapes, &preview.regions.body);
            let selectors = bounds(&shapes, "Message area: · None")
                .union(bounds(&shapes, "Microphone tail: · None"));
            assert!(selectors.left() > body.right());
            assert!(
                (selectors.center().y - body.center().y).abs() <= 2.0,
                "{selectors:?} vs {body:?}"
            );
            let footer = bounds(&shapes, "Footer: · None").center().y;
            if long {
                assert!(footer > short_footer + 30.0);
            } else {
                short_footer = footer;
            }
        }
    }
}

#[test]
fn suffix_dropdown_enables_and_edits_literal_text() {
    let ctx = egui::Context::default();
    let mut settings = OscSettings::default();
    let preview = ChatboxPreview::default();
    let mut shapes = Vec::new();
    for _ in 0..3 {
        shapes = frame(&ctx, 620.0, &mut settings, &preview, Vec::new());
    }
    let open = bounds(&shapes, "Microphone tail: · None").center();
    let shapes = click(&ctx, &mut settings, &preview, open);
    let custom = bounds(&shapes, "Custom Text").center();
    let shapes = click(&ctx, &mut settings, &preview, custom);
    assert!(settings.microphone_suffix_enabled);
    assert!(egui::Popup::is_any_open(&ctx));
    let input = bounds(&shapes, "喵").center();
    click(&ctx, &mut settings, &preview, input);
    assert!(egui::Popup::is_any_open(&ctx));
    frame(
        &ctx,
        620.0,
        &mut settings,
        &preview,
        vec![egui::Event::Text(" 喵。".into())],
    );
    assert_eq!(settings.microphone_suffix, " 喵。");
}

#[test]
fn body_lyrics_are_selectable_and_merged_preview_is_one_contiguous_block() {
    let ctx = egui::Context::default();
    let mut settings = OscSettings::default();
    let mut preview = ChatboxPreview::default();
    let mut shapes = Vec::new();
    for _ in 0..3 {
        shapes = frame(&ctx, 620.0, &mut settings, &preview, Vec::new());
    }
    let at = bounds(&shapes, "Message area: · None").center();
    shapes = click(&ctx, &mut settings, &preview, at);
    let at = bounds(&shapes, "Lyrics").center();
    click(&ctx, &mut settings, &preview, at);
    assert_eq!(settings.idle_content, IdleContent::Lyrics);
    egui::Popup::close_all(&ctx);
    settings.header_config.content_type = BannerContentType::Lyrics;
    settings.footer_config.content_type = BannerContentType::Lyrics;
    preview.regions = ChatboxRegions {
        header: "Song · Artist".into(),
        body: "Previous\n› Current\nNext".into(),
        unified_lyrics: true,
        ..Default::default()
    };
    for _ in 0..3 {
        shapes = frame(&ctx, 620.0, &mut settings, &preview, Vec::new());
    }
    let content = "Song · Artist\nPrevious\n› Current\nNext";
    let block = bounds(&shapes, content);
    assert_eq!(shapes.iter().filter(|(text, _)| text == content).count(), 1);
    for label in [
        "Header: · Lyrics",
        "Message area: · Lyrics",
        "Footer: · Lyrics",
    ] {
        assert!(bounds(&shapes, label).left() > block.right());
    }
    assert!(
        !shapes
            .iter()
            .any(|(text, _)| text == "Footer:" || text == "Empty")
    );
}

#[test]
fn theater_canvas_keeps_its_three_rows_at_narrow_preview_widths() {
    let text = "│        ♡             │\n│ (=^ω^=)              │\n│                  ○   │"
        .replace(' ', "\u{2002}");
    for width in [360.0, 620.0, 900.0] {
        let ctx = egui::Context::default();
        let mut settings = OscSettings::default();
        settings.footer_config.content_type = BannerContentType::TextPet;
        let preview = ChatboxPreview {
            regions: ChatboxRegions {
                body: text.clone(),
                pet_stage: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = frame(&ctx, width, &mut settings, &preview, Vec::new());
        }
        let body = bounds(&shapes, &text);
        let selector = bounds(&shapes, "Message area: · None");
        assert!(body.left() >= 0.0 && body.right() < selector.left());
        // Space is spent on the scene itself, not a wrapping decorative frame.
        assert!(body.height() <= 54.0, "{width}: {body:?}");
    }
}

#[test]
fn banner_dropdowns_keep_their_custom_editors_clickable() {
    for (name, header) in [("Header: · None", true), ("Footer: · None", false)] {
        let ctx = egui::Context::default();
        let mut settings = OscSettings::default();
        let preview = ChatboxPreview::default();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = frame(&ctx, 620.0, &mut settings, &preview, Vec::new());
        }
        let open = bounds(&shapes, name).center();
        let shapes = click(&ctx, &mut settings, &preview, open);
        let custom = bounds(&shapes, "Custom Text").center();
        let shapes = click(&ctx, &mut settings, &preview, custom);
        // The input's placeholder is the second occurrence after the option.
        let input = shapes
            .iter()
            .rev()
            .find(|(text, _)| text == "Custom Text")
            .unwrap()
            .1
            .center();
        click(&ctx, &mut settings, &preview, input);
        frame(
            &ctx,
            620.0,
            &mut settings,
            &preview,
            vec![egui::Event::Text("Banner".into())],
        );
        let banner = if header {
            &settings.header_config
        } else {
            &settings.footer_config
        };
        assert_eq!(banner.content_type, BannerContentType::CustomText);
        assert_eq!(banner.custom_text, "Banner");
    }
}

#[test]
fn pet_can_be_chosen_and_customized_independently_in_either_banner() {
    use crate::osc::pet::PetMood;
    for (name, header) in [("Header: · None", true), ("Footer: · None", false)] {
        let ctx = egui::Context::default();
        let mut settings = OscSettings::default();
        let preview = ChatboxPreview::default();
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = frame(&ctx, 620.0, &mut settings, &preview, Vec::new());
        }
        for label in [name, "Text pet", "Sleepy"] {
            let at = bounds(&shapes, label).center();
            shapes = click(&ctx, &mut settings, &preview, at);
        }
        let (selected, untouched) = if header {
            (&settings.header_config, &settings.footer_config)
        } else {
            (&settings.footer_config, &settings.header_config)
        };
        assert_eq!(selected.content_type, BannerContentType::TextPet);
        assert_eq!(selected.pet.mood, PetMood::Sleepy);
        assert_eq!(untouched, &BannerConfig::default());
        assert!(egui::Popup::is_any_open(&ctx));
    }
}
