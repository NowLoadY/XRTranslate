use super::*;

fn render_cards(
    size: egui::Vec2,
    detail: &str,
    installed: bool,
    active_state: Option<&NativeModelTaskState>,
    count: usize,
) -> (Vec<egui::Rect>, egui::FullOutput) {
    let ctx = egui::Context::default();
    theme::apply_theme(&ctx);
    let item = DownloadItem {
        id: ModelAssetId::Qwen3AsrGguf,
        category_title: "Speech Recognition Model",
        detail: detail.to_owned(),
        download_bytes: 1024 * 1024,
        installed_bytes: 2 * 1024 * 1024,
        installed,
        hardware_available: true,
        stroke_color: Color32::BLUE,
    };
    let mut rects = Vec::new();
    let mut output = egui::FullOutput::default();
    // Allow the scroll bar and fonts to settle just as they do in the wizard.
    for _ in 0..3 {
        rects.clear();
        output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                ..Default::default()
            },
            |ui| {
                ui.add_space(48.0);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for index in 0..count {
                        let response = ui.push_id(index, |ui| {
                            render_download_card(
                                ui,
                                i18n::UiLanguage::English,
                                &item,
                                if active_state.is_some() {
                                    "Downloading"
                                } else {
                                    "Download"
                                },
                                !installed && active_state.is_none(),
                                active_state.is_none(),
                                None,
                                active_state,
                            );
                        });
                        rects.push(response.response.rect);
                        ui.add_space(8.0);
                    }
                });
            },
        );
        output.textures_delta.clear();
    }
    (rects, output)
}

fn text_rect(output: &egui::FullOutput, text: &str) -> egui::Rect {
    fn find(shape: &egui::Shape, text: &str) -> Option<egui::Rect> {
        match shape {
            egui::Shape::Text(shape) if shape.galley.text() == text => {
                Some(shape.galley.rect.translate(shape.pos.to_vec2()))
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, text)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, text))
        .unwrap_or_else(|| panic!("missing UI text: {text}"))
}

#[test]
fn download_card_height_does_not_depend_on_remaining_scroll_space() {
    for width in [1050.0, 360.0] {
        let mut previous_height: Option<f32> = None;
        for height in [420.0, 900.0] {
            let (cards, output) = render_cards(
                egui::vec2(width, height),
                "Qwen3-ASR 1.7B · Q4_K_M",
                true,
                None,
                3,
            );
            let first = cards[0];
            assert!(first.height() < 140.0, "oversized card: {first:?}");
            for card in &cards {
                assert!(
                    (card.height() - first.height()).abs() < 1.0,
                    "identical cards must have equal heights: {cards:?}"
                );
            }
            if let Some(previous_height) = previous_height {
                assert!(
                    (first.height() - previous_height).abs() < 1.0,
                    "card height changed with viewport height: {cards:?}"
                );
            }
            previous_height = Some(first.height());
            let title = text_rect(&output, "Speech Recognition Model");
            assert!(
                title.top() - first.top() < 20.0,
                "card contains blank space above title: {first:?}, {title:?}"
            );
        }
    }
}

#[test]
fn download_cards_keep_desktop_columns_and_stack_wrapped_content_on_narrow_screens() {
    let detail = "A model with a longer localized description and hardware details. ".repeat(6);
    let (wide_cards, wide_output) = render_cards(egui::vec2(1050.0, 900.0), &detail, true, None, 1);
    let wide_detail = text_rect(&wide_output, &detail);
    let wide_action = text_rect(&wide_output, "Installed");
    assert!(wide_detail.right() < wide_action.left());
    assert!(wide_detail.height() > 30.0, "details must wrap");
    assert!(wide_cards[0].contains_rect(wide_detail));
    assert!(wide_cards[0].contains_rect(wide_action));

    let (short_cards, short_output) =
        render_cards(egui::vec2(1050.0, 900.0), "Short model name", true, None, 1);
    let short_action = text_rect(&short_output, "Installed");
    assert!((short_action.left() - wide_action.left()).abs() < 1.0);
    assert!(wide_cards[0].height() > short_cards[0].height());

    let (narrow_cards, narrow_output) =
        render_cards(egui::vec2(360.0, 900.0), &detail, true, None, 1);
    let narrow_detail = text_rect(&narrow_output, &detail);
    let narrow_action = text_rect(&narrow_output, "Installed");
    assert!(narrow_action.top() > narrow_detail.bottom());
    assert!(narrow_cards[0].contains_rect(narrow_detail));
    assert!(narrow_cards[0].contains_rect(narrow_action));
    assert!(narrow_cards[0].right() <= 360.0);
    assert!(narrow_cards[0].height() > wide_cards[0].height());
}

#[test]
fn download_card_progress_follows_content_without_stretching_the_card() {
    let detail = "Qwen3-ASR 1.7B · Q4_K_M";
    let state = NativeModelTaskState::Installing {
        asset_id: ModelAssetId::Qwen3AsrGguf,
        relative_path: None,
        downloaded_bytes: 1024,
        total_bytes: 2048,
    };
    for width in [1050.0, 360.0] {
        let (idle_cards, _) = render_cards(egui::vec2(width, 900.0), detail, false, None, 1);
        let (active_cards, output) =
            render_cards(egui::vec2(width, 900.0), detail, false, Some(&state), 1);
        let progress = text_rect(
            &output,
            &format!(
                "{} / {}",
                components::format_file_size(1024),
                components::format_file_size(2048)
            ),
        );
        let metadata = text_rect(&output, "· Download 1.0 MiB · Installed size 2.0 MiB");
        assert!(progress.top() > metadata.bottom());
        assert!(active_cards[0].contains_rect(progress));
        let added_height = active_cards[0].height() - idle_cards[0].height();
        assert!(
            added_height > 0.0 && added_height < 50.0,
            "progress must only add its own row: {added_height}"
        );
    }
}
