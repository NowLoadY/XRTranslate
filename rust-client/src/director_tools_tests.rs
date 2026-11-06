use serde_json::json;
use ui_director::egui as automation;
use ui_director::egui::tools::UiTools;
use ui_director::{ToolCall, ToolProvider};

use crate::ui::components;
use eframe::egui;

#[test]
fn shared_components_are_discovered_and_actions_follow_real_frame_handlers() {
    let ctx = egui::Context::default();
    let driver = automation::driver();
    let mut tools = UiTools::default();
    let mut checked = false;
    let mut disabled = false;
    let mut text = String::new();
    let mut selection = 0;
    let mut generic = 0;
    let mut clicks = 0;
    let mut time = 0.0;
    let mut note = String::new();
    let mut number = 1u16;
    let mut detail_seen = false;
    let plugin_pages = crate::plugins::PluginPages {
        installed_pages: vec![crate::plugins::PluginPage {
            id: crate::plugins::PluginId::MEDIA,
            title: "Media".into(),
            order: 150,
            group: String::new(),
            icon: None,
        }],
    };
    let mut frame = |checked: &mut bool,
                     text: &mut String,
                     selection: &mut usize,
                     clicks: &mut usize,
                     generic: &mut usize| {
        time += 0.1;
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(900.0, 700.0),
                )),
                time: Some(time),
                ..Default::default()
            },
            |ui| {
                automation::begin_frame(
                    &ctx,
                    "Fixture",
                    crate::ui::navigation_pages(
                        &plugin_pages,
                        true,
                        crate::i18n::UiLanguage::English,
                    ),
                );
                automation::record_location(ui, "projects", "Projects");
                if components::primary_button(ui, "Run").clicked() {
                    *clicks += 1;
                }
                components::checkbox(ui, checked, "Enabled");
                ui.add_enabled_ui(false, |ui| {
                    components::checkbox(ui, &mut disabled, "Disabled");
                });
                components::input_field(ui, text, "Text");
                components::CollapsingHeader::new("Details").show(ui, |ui| {
                    detail_seen = true;
                    let response = ui.text_edit_multiline(&mut note);
                    automation::text_response(ui, "Draft", &mut note, response);
                    components::number_input(ui, &mut number, "Count", 1..=10);
                });
                let options: [(usize, String); 2] = [(0, "First".into()), (1, "Second".into())];
                components::searchable_combobox(
                    ui,
                    "choice",
                    options[*selection].1.clone(),
                    selection,
                    &options,
                );
                components::combobox_ui(ui, "generic-choice", format!("Generic {generic}"), |ui| {
                    if components::selectable_value(ui, generic, 0, "Option A").clicked() {
                        ui.close();
                    }
                    if components::selectable_value(ui, generic, 1, "Option B").clicked() {
                        ui.close();
                    }
                });
                for index in 0..2 {
                    ui.push_id(index, |ui| {
                        let response = ui.button("Duplicate");
                        automation::Control::button("Duplicate").response(ui, response);
                    });
                }
                // As in the app, navigation resolves before drawing the active detail view.
                let (id, label) = if *selection == 0 {
                    ("one", "First project")
                } else {
                    ("two", "Second project")
                };
                automation::record_location(ui, id, label);
                ui.label(label);
                driver.finish_frame("Fixture");
            },
        );
        output.textures_delta.clear();
    };
    let call = |tools: &mut UiTools, name: &str, arguments| {
        tools.call(&ToolCall {
            name: name.into(),
            arguments,
        })
    };
    frame(
        &mut checked,
        &mut text,
        &mut selection,
        &mut clicks,
        &mut generic,
    );
    let snapshot = call(&mut tools, "ui.read", json!({})).unwrap();
    assert_eq!(snapshot["location"], json!(driver.location()));
    assert_eq!(snapshot["location"]["path"][1]["id"], "one");
    assert!(
        snapshot["controls"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["label"] == "Run" && c["actions"][0] == "click")
    );
    let choices = call(
        &mut tools,
        "ui.options",
        json!({"target":"First","offset":1,"limit":1}),
    )
    .unwrap();
    assert_eq!(choices["options"], json!(["Second"]));
    for (name, args) in [
        ("ui.click", json!({"target":"Run"})),
        ("ui.set", json!({"target":"Enabled","value":true})),
        ("ui.set", json!({"target":"Text","value":"changed"})),
        ("ui.set", json!({"target":"First","value":"Second"})),
    ] {
        let mut receipt = None;
        let output = ctx.run_logic(&egui::RawInput::default(), |_| {
            receipt = Some(call(&mut tools, name, args).unwrap());
        });
        assert!(
            output.viewport_commands[&egui::ViewportId::ROOT]
                .contains(&egui::ViewportCommand::RequestPaintWhileHidden)
        );
        let receipt = receipt.unwrap();
        let wait: ToolCall = serde_json::from_value(receipt["pending"].clone()).unwrap();
        assert!(tools.call(&wait).unwrap().get("pending").is_some());
        frame(
            &mut checked,
            &mut text,
            &mut selection,
            &mut clicks,
            &mut generic,
        );
        let result = tools.call(&wait).unwrap();
        assert_eq!(
            tools.call(&wait).unwrap(),
            result,
            "reading a result must not consume it"
        );
    }
    assert_eq!(clicks, 1);
    assert!(checked);
    assert_eq!(text, "changed");
    assert_eq!(selection, 1);
    let updated = call(&mut tools, "ui.read", json!({"target":"Enabled"})).unwrap();
    assert_eq!(updated["controls"][0]["value"], true);
    assert_eq!(updated["location"]["path"][1]["id"], "two");
    for (name, args) in [
        ("ui.click", json!({"target":"Duplicate"})),
        ("ui.set", json!({"target":"Disabled","value":true})),
        ("ui.set", json!({"target":"Second","value":"Not an option"})),
    ] {
        let receipt = call(&mut tools, name, args).unwrap();
        frame(
            &mut checked,
            &mut text,
            &mut selection,
            &mut clicks,
            &mut generic,
        );
        let wait = serde_json::from_value(receipt["pending"].clone()).unwrap();
        assert!(tools.call(&wait).is_err());
    }
    let receipt = call(&mut tools, "ui.click", json!({"target":"Run"})).unwrap();
    tools
        .call(&serde_json::from_value(receipt["cancel"].clone()).unwrap())
        .unwrap();
    frame(
        &mut checked,
        &mut text,
        &mut selection,
        &mut clicks,
        &mut generic,
    );
    assert_eq!(clicks, 1, "cancelled clicks must not execute later");
    let receipt = call(&mut tools, "ui.click", json!({"target":"Generic 0"})).unwrap();
    frame(
        &mut checked,
        &mut text,
        &mut selection,
        &mut clicks,
        &mut generic,
    );
    tools
        .call(&serde_json::from_value(receipt["pending"].clone()).unwrap())
        .unwrap();
    frame(
        &mut checked,
        &mut text,
        &mut selection,
        &mut clicks,
        &mut generic,
    );
    let option = call(&mut tools, "ui.read", json!({"filter":"Option B"})).unwrap();
    let receipt = call(
        &mut tools,
        "ui.click",
        json!({"target":option["controls"][0]["id_hex"]}),
    )
    .unwrap();
    frame(
        &mut checked,
        &mut text,
        &mut selection,
        &mut clicks,
        &mut generic,
    );
    tools
        .call(&serde_json::from_value(receipt["pending"].clone()).unwrap())
        .unwrap();
    assert_eq!(
        generic, 1,
        "a dynamically rendered option must use its normal selection handler"
    );
    assert_eq!(selection, 1);
    let pages = call(&mut tools, "ui.pages", json!({})).unwrap();
    assert_eq!(pages["location"], json!(driver.location()));
    assert!(
        pages["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|page| page["route"] == "Translation")
    );
    assert!(
        pages["pages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|page| page["route"] == "plugin:media")
    );
    for (target, value) in [
        ("Details", json!(true)),
        ("Draft", json!("原文与译文")),
        ("Count", json!(100)),
    ] {
        let receipt = call(&mut tools, "ui.set", json!({"target":target,"value":value})).unwrap();
        // A disclosure animates its contents into the following frame.
        for _ in 0..2 {
            frame(
                &mut checked,
                &mut text,
                &mut selection,
                &mut clicks,
                &mut generic,
            );
        }
        tools
            .call(&serde_json::from_value(receipt["pending"].clone()).unwrap())
            .unwrap();
    }
    assert!(
        automation::feedback::current(&ctx).iter().any(|item| {
            item.feedback.label == "Count"
                && matches!(&item.feedback.effect,
                ui_director::feedback::Effect::Change { before, after }
                if before == "1" && after == "10")
        }),
        "feedback must show the actual bounded value, not the requested 100"
    );
    let text_page = call(
        &mut tools,
        "ui.text",
        json!({"target":"Draft","offset":2,"limit":2}),
    )
    .unwrap();
    assert_eq!(text_page["text"], "与译");
    assert_eq!(text_page["next_offset"], 4);
    assert!(detail_seen);
    assert!(!disabled);
    assert_eq!(note, "原文与译文");
    assert_eq!(number, 10, "native numeric controls retain their bounds");
    let _ = ctx.run_ui(Default::default(), |ui| {
        automation::begin_frame(ui.ctx(), "Library", vec![]);
        automation::finish_frame("Library");
    });
    assert_eq!(json!(driver.location()), json!({"page":"Library"}));
    assert_eq!(
        call(&mut tools, "ui.read", json!({})).unwrap()["location"],
        json!({"page":"Library"})
    );
}
