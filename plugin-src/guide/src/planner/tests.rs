use super::*;
use unicode_segmentation::UnicodeSegmentation;

fn context(editor: bool) -> Context {
    Context {
        page: "PromptStudio".into(),
        language: "en".into(),
        pages: ["Translation", "PromptStudio", "Settings"]
            .into_iter()
            .map(|route| xrtranslate_plugin::guide::Page {
                route: route.into(),
                label: route.into(),
            })
            .collect(),
        capabilities: if editor {
            vec!["prompt_editor".into()]
        } else {
            vec![]
        },
    }
}

#[test]
fn admission_controls_catalog_selection_and_both_navigation_directions() {
    for editor in [false, true] {
        let mut planner = Planner::default();
        planner.update(context(editor));
        assert_eq!(
            planner.catalog().unwrap()[1].steps.len(),
            if editor { 2 } else { 1 }
        );
        assert_eq!(
            planner.select("PromptStudio", Some("edit-prompts")).is_ok(),
            editor
        );
        planner.start(None).unwrap();
        if editor {
            planner.advance(true).unwrap();
            assert_eq!(
                planner
                    .state
                    .session
                    .as_ref()
                    .unwrap()
                    .current()
                    .unwrap()
                    .1
                    .id,
                "edit-prompts"
            );
        }
        planner.advance(true).unwrap();
        assert_eq!(
            planner
                .state
                .session
                .as_ref()
                .unwrap()
                .current()
                .unwrap()
                .0
                .page,
            "Settings"
        );
        planner.advance(false).unwrap();
        assert_eq!(
            planner.state.session.as_ref().unwrap().step,
            usize::from(editor)
        );
    }
}

#[test]
fn revoked_admission_replaces_active_edit_and_invalidates_old_readiness() {
    let mut planner = Planner::default();
    planner.update(context(true));
    planner
        .select("PromptStudio", Some("edit-prompts"))
        .unwrap();
    let old = planner.state.revision;
    planner.presented(old);
    assert!(planner.state.ready);
    planner.update(context(false));
    planner.presented(old);
    assert!(!planner.state.ready);
    assert_eq!(
        planner
            .state
            .session
            .as_ref()
            .unwrap()
            .current()
            .unwrap()
            .1
            .id,
        "translation-style"
    );
    assert!(
        planner
            .select("PromptStudio", Some("edit-prompts"))
            .is_err()
    );
    planner.presented(planner.state.revision);
    assert!(planner.state.ready);
}

#[test]
fn host_navigation_acknowledgement_preserves_requested_topic() {
    let mut planner = Planner::default();
    planner.update(context(false));
    planner.select("Settings", Some("models")).unwrap();
    let revision = planner.state.revision;
    let mut next = context(false);
    next.page = "Settings".into();
    planner.update(next);
    assert_eq!(planner.state.revision, revision);
    planner.stop();
    planner.presented(revision);
    assert!(planner.state.session.is_none());
    assert!(!planner.state.ready);
}

#[test]
fn unavailable_pages_and_finished_tours_do_not_fall_back_to_another_topic() {
    let mut planner = Planner::default();
    planner.update(context(false));
    assert!(planner.start(Some("plugin:not-installed")).is_err());
    let chapters = planner.catalog().unwrap();
    let last = chapters.last().unwrap();
    planner
        .select(&last.page, Some(&last.steps.last().unwrap().id))
        .unwrap();
    planner.advance(true).unwrap();
    assert!(planner.state.session.is_none());
}

#[test]
fn every_packaged_lesson_is_brief_localized_and_uniquely_addressable() {
    let planner = Planner::default();
    for page in &planner.lessons {
        let mut ids = std::collections::HashSet::new();
        for step in &page.steps {
            assert!(ids.insert(&step.id), "{}: {}", page.page, step.id);
            for language in ["en", "zh", "ja", "ko", "ru"] {
                let text = step
                    .text
                    .0
                    .get(language)
                    .expect("missing guide translation");
                assert!(
                    !text.is_empty() && text.graphemes(true).count() <= 96,
                    "{text}"
                );
            }
        }
    }
    let manifest: xrtranslate_plugin::Manifest =
        serde_json::from_str(include_str!("../../plugin.json")).unwrap();
    manifest.validate().unwrap();
    assert!(manifest.bundle_with_host);
    assert!(
        manifest
            .requires
            .contains(&xrtranslate_plugin::guide::CAPABILITY.into())
    );
}
