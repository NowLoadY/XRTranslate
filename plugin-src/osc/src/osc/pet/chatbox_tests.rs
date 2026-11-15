use super::*;
use crate::osc::pet::{PetMood, REFRESH_INTERVAL};
use std::{collections::BTreeSet, time::Duration};

fn settings() -> OscSettings {
    let mut settings = OscSettings::default();
    settings.header_config.content_type = BannerContentType::TextPet;
    settings.footer_config.content_type = BannerContentType::TextPet;
    settings.footer_config.pet.mood = PetMood::Shy;
    settings
}

#[test]
fn two_pets_animate_independently_without_messages_or_persistent_banners() {
    let now = Instant::now();
    let mut settings = settings();
    let mut renderer = ChatboxRenderer::default();
    let mut scenes = BTreeSet::new();
    for tick in 0..12 {
        let text = renderer.render(
            &[],
            &[],
            None,
            &settings,
            &SystemMetrics::default(),
            &LyricsText::default(),
            now + REFRESH_INTERVAL * tick,
        );
        assert_eq!(text.lines().count(), 3);
        assert!(renderer.regions.pet_stage);
        assert!(renderer.regions.header.is_empty() && renderer.regions.footer.is_empty());
        scenes.insert(renderer.regions.body.clone());
        assert!(
            !renderer.needs_refresh(),
            "Pets use their own banner cadence, not marquee scrolling"
        );
    }
    assert!(scenes.len() > 2);
    settings.header_config.content_type = BannerContentType::None;
    settings.footer_config.content_type = BannerContentType::None;
    assert!(
        renderer
            .render(
                &[],
                &[],
                None,
                &settings,
                &SystemMetrics::default(),
                &LyricsText::default(),
                now + Duration::from_secs(15)
            )
            .is_empty()
    );
}

#[test]
fn pet_motion_keeps_caption_budgets_stable_and_never_extends_the_message() {
    let now = Instant::now();
    let settings = settings();
    let message = HistoryMessage {
        stream_id: 1,
        source_kind: OscInputSource::Microphone,
        source: String::new(),
        translated: "A".repeat(240) + "TAIL",
        additional_translations: Vec::new(),
        speaker_id: String::new(),
        updated_at: now,
        expires_at: now + Duration::from_secs(15),
    };
    let mut renderer = ChatboxRenderer::default();
    let mut first = None;
    for tick in 0..12 {
        let text = renderer.render(
            std::slice::from_ref(&message),
            &[],
            None,
            &settings,
            &SystemMetrics::default(),
            &LyricsText::default(),
            now + REFRESH_INTERVAL * tick,
        );
        assert!(text.chars().count() <= 144);
        let state = renderer.blocks.values().next().unwrap();
        let window = (state.budget, state.started);
        assert_eq!(*first.get_or_insert(window), window);
    }
    assert!(renderer.regions.body.ends_with("TAIL"));
    renderer.render(
        &[],
        &[],
        None,
        &settings,
        &SystemMetrics::default(),
        &LyricsText::default(),
        now + Duration::from_secs(15),
    );
    assert!(renderer.regions.pet_stage);
    assert!(!renderer.regions.body.is_empty());
    assert!(!renderer.needs_refresh());
}

#[test]
fn pets_and_lyrics_obey_even_tiny_budgets() {
    let now = Instant::now();
    let messages = [HistoryMessage {
        stream_id: 2,
        source_kind: OscInputSource::Microphone,
        source: "原文".repeat(100),
        translated: "A long translation ".repeat(20),
        additional_translations: Vec::new(),
        speaker_id: String::new(),
        updated_at: now,
        expires_at: now + Duration::from_secs(15),
    }];
    let manual = ManualMessage {
        text: "A typed message ".repeat(20),
        updated_at: now,
        expires_at: now + Duration::from_secs(15),
    };
    let lyrics = LyricsText {
        title: "A song".into(),
        lyric: "A lyric".into(),
        ..Default::default()
    };
    for limit in 0..=144 {
        for footer in [BannerContentType::TextPet, BannerContentType::Lyrics] {
            let mut settings = settings();
            settings.max_text_length = limit;
            settings.footer_config.content_type = footer;
            for (history, manual) in [
                (&[][..], None),
                (&messages[..], None),
                (&messages[..], Some(&manual)),
            ] {
                let mut renderer = ChatboxRenderer::default();
                for tick in [0, 4, 8] {
                    let text = renderer.render(
                        history,
                        &[],
                        manual,
                        &settings,
                        &SystemMetrics::default(),
                        &lyrics,
                        now + REFRESH_INTERVAL * tick,
                    );
                    assert!(text.chars().count() <= limit, "limit {limit}: {text}");
                    assert!(text.lines().count() <= settings.line_limit());
                    let pet = &renderer.regions.header;
                    assert!(
                        pet.is_empty()
                            || (pet.contains('(') && pet.contains(')'))
                            || pet.chars().count() == 3
                    );
                }
            }
        }
    }
}

#[test]
fn theater_yields_to_captions_manual_messages_and_selected_lyrics() {
    use crate::osc::runtime::{IdleContent, OscFormatMode};
    let now = Instant::now();
    let mut settings = settings();
    settings.format_mode = OscFormatMode::TargetOnly;
    let mut renderer = ChatboxRenderer::default();
    let metrics = SystemMetrics::default();
    let empty = LyricsText::default();
    let first = renderer.render(&[], &[], None, &settings, &metrics, &empty, now);
    assert!(renderer.regions.pet_stage);
    let manual = ManualMessage {
        text: "Hello".into(),
        updated_at: now,
        expires_at: now + Duration::from_secs(15),
    };
    renderer.render(
        &[],
        &[],
        Some(&manual),
        &settings,
        &metrics,
        &empty,
        now + Duration::from_secs(1),
    );
    assert_eq!(renderer.regions.body, "💬 Hello");
    assert!(!renderer.regions.pet_stage);
    assert_eq!(
        renderer.render(
            &[],
            &[],
            None,
            &settings,
            &metrics,
            &empty,
            now + Duration::from_secs(16)
        ),
        first
    );
    let preview = HistoryMessage {
        stream_id: 1,
        source_kind: OscInputSource::Microphone,
        source: "Recognizing".into(),
        translated: String::new(),
        additional_translations: Vec::new(),
        speaker_id: String::new(),
        updated_at: now,
        expires_at: now + Duration::from_secs(20),
    };
    renderer.render(
        &[],
        &[preview],
        None,
        &settings,
        &metrics,
        &empty,
        now + Duration::from_secs(17),
    );
    assert!(!renderer.regions.pet_stage);
    assert!(renderer.regions.body.is_empty());
    settings.idle_content = IdleContent::Lyrics;
    let lyrics = LyricsText {
        title: "Song".into(),
        lyric: "Line".into(),
        ..Default::default()
    };
    renderer.render(
        &[],
        &[],
        None,
        &settings,
        &metrics,
        &lyrics,
        now + Duration::from_secs(18),
    );
    assert_eq!(renderer.regions.body, "Line");
    assert!(!renderer.regions.pet_stage);
    renderer.render(
        &[],
        &[],
        None,
        &settings,
        &metrics,
        &empty,
        now + Duration::from_secs(19),
    );
    assert!(renderer.regions.pet_stage);
}
