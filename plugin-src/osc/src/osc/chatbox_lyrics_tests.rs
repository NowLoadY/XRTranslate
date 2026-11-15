use super::super::{
    ChatboxRenderer, HistoryMessage, ManualMessage, SystemMetrics, compose_chatbox,
};
use super::*;
use crate::osc::runtime::{IdleContent, OscFormatMode, OscInputSource};
use std::time::{Duration, Instant};

fn song() -> LyricsText {
    LyricsText {
        title: "🎵 Song · Artist".into(),
        lyric: "🎵 line 5".into(),
        window: Some(LyricWindow {
            lines: (0..12).map(|index| format!("line {index}")).collect(),
            current: 5,
            progress: 500,
        }),
    }
}

fn settings(unified: bool) -> OscSettings {
    let mut settings = OscSettings {
        idle_content: IdleContent::Lyrics,
        ..Default::default()
    };
    if unified {
        settings.header_config.content_type = BannerContentType::Lyrics;
        settings.footer_config.content_type = BannerContentType::Lyrics;
    }
    settings
}

#[test]
fn line_count_follows_space_and_keeps_current_cue_with_its_neighbours() {
    let window = song().window.unwrap();
    assert_eq!(fit_window(&window, 8, 9), "› line 5");
    assert_eq!(fit_window(&window, 15, 9), "› line 5\nline 6");
    assert_eq!(fit_window(&window, 22, 9), "line 4\n› line 5\nline 6");
    assert_eq!(fit_window(&window, 144, 3), "line 4\n› line 5\nline 6");
    assert_eq!(fit_window(&window, 144, 9).lines().count(), 9);
    let mut gap = window;
    gap.lines[6].clear();
    assert!(!fit_window(&gap, 144, 9).contains("line 7"));
}

#[test]
fn three_selected_regions_share_one_title_and_all_nine_rows() {
    let now = Instant::now();
    let mut renderer = ChatboxRenderer::default();
    let text = renderer.render(
        &[],
        &[],
        None,
        &settings(true),
        &SystemMetrics::default(),
        &song(),
        now,
    );
    assert!(renderer.regions.unified_lyrics);
    assert_eq!(text.matches("Song · Artist").count(), 1);
    assert_eq!(text.lines().count(), 9);
    assert_eq!(renderer.regions.body.lines().count(), 8);
    assert!(renderer.regions.footer.is_empty());
    assert_eq!(text.matches("line 5").count(), 1);
    assert!(text.contains("line 4\n› line 5\nline 6"));
    assert!(text.chars().count() > settings(true).lyric_budget().unwrap());
}

#[test]
fn messages_temporarily_own_the_body_then_idle_lyrics_return() {
    let now = Instant::now();
    let mut settings = settings(true);
    settings.format_mode = OscFormatMode::TargetOnly;
    let caption = HistoryMessage {
        stream_id: 1,
        source_kind: OscInputSource::Typing,
        source: String::new(),
        translated: "Hello".into(),
        additional_translations: Vec::new(),
        speaker_id: String::new(),
        updated_at: now,
        expires_at: now + Duration::from_secs(15),
    };
    let mut renderer = ChatboxRenderer::default();
    let metrics = SystemMetrics::default();
    let song = song();
    let idle = renderer.render(&[], &[], None, &settings, &metrics, &song, now);
    renderer.render(&[caption], &[], None, &settings, &metrics, &song, now);
    assert_eq!(renderer.regions.body, "💬 Hello");
    assert!(!renderer.regions.unified_lyrics);
    let manual = ManualMessage {
        text: "Typed message".into(),
        updated_at: now,
        expires_at: now + Duration::from_secs(15),
    };
    renderer.render(&[], &[], Some(&manual), &settings, &metrics, &song, now);
    assert_eq!(renderer.regions.body, "💬 Typed message");
    assert!(!renderer.regions.unified_lyrics);
    assert_eq!(
        renderer.render(
            &[],
            &[],
            None,
            &settings,
            &metrics,
            &song,
            now + Duration::from_secs(16)
        ),
        idle
    );
}

#[test]
fn idle_lyrics_preserve_other_banners_without_copying_song_information() {
    let mut settings = settings(false);
    settings.header_config.content_type = BannerContentType::Lyrics;
    settings.footer_config.content_type = BannerContentType::CustomText;
    settings.footer_config.custom_text = "Footer".into();
    settings.persistent_banners = true;
    let mut renderer = ChatboxRenderer::default();
    let mut lyrics = song();
    let text = renderer.render(
        &[],
        &[],
        None,
        &settings,
        &SystemMetrics::default(),
        &lyrics,
        Instant::now(),
    );
    assert!(text.starts_with("🎵 Song · Artist\n"));
    assert!(text.ends_with("\nFooter"));
    assert!(renderer.regions.body.lines().count() > 1);
    assert!(!renderer.regions.unified_lyrics);
    lyrics.window = None;
    lyrics.lyric.clear();
    let text = renderer.render(
        &[],
        &[],
        None,
        &settings,
        &SystemMetrics::default(),
        &lyrics,
        Instant::now(),
    );
    assert_eq!(text, "🎵 Song · Artist\nFooter");
    assert!(renderer.regions.body.is_empty());
}

#[test]
fn pet_animation_does_not_resize_the_idle_lyric_window() {
    use crate::osc::pet::REFRESH_INTERVAL;
    use std::collections::BTreeSet;

    let now = Instant::now();
    for limit in 24..=144 {
        for pet_in_header in [false, true] {
            let mut settings = settings(false);
            settings.max_text_length = limit;
            let banner = if pet_in_header {
                &mut settings.header_config
            } else {
                &mut settings.footer_config
            };
            banner.content_type = BannerContentType::TextPet;
            let mut renderer = ChatboxRenderer::default();
            let mut bodies = BTreeSet::new();
            for tick in 0..8 {
                let text = renderer.render(
                    &[],
                    &[],
                    None,
                    &settings,
                    &SystemMetrics::default(),
                    &song(),
                    now + REFRESH_INTERVAL * tick,
                );
                assert!(text.chars().count() <= limit);
                bodies.insert(renderer.regions.body.clone());
            }
            assert_eq!(bodies.len(), 1, "limit {limit}: {bodies:?}");
        }
    }
}

#[test]
fn every_layout_respects_characters_lines_and_graphemes() {
    let mut song = song();
    song.window.as_mut().unwrap().lines = (0..12).map(|_| "甲👩‍💻e\u{301}🇨🇳乙".repeat(20)).collect();
    for limit in 0..=144 {
        for unified in [false, true] {
            let mut settings = settings(unified);
            settings.max_text_length = limit;
            let mut renderer = ChatboxRenderer::default();
            let text = renderer.render(
                &[],
                &[],
                None,
                &settings,
                &SystemMetrics::default(),
                &song,
                Instant::now(),
            );
            assert!(text.chars().count() <= limit, "{limit}: {text:?}");
            assert!(text.lines().count() <= settings.line_limit());
            assert_eq!(
                text,
                compose_chatbox(
                    &renderer.regions.header,
                    &renderer.regions.body,
                    &renderer.regions.footer
                )
            );
            for grapheme in unicode_segmentation::UnicodeSegmentation::graphemes(
                renderer.regions.body.as_str(),
                true,
            ) {
                assert!(
                    !matches!(grapheme, "👩" | "💻" | "\u{200d}" | "\u{301}" | "🇨" | "🇳"),
                    "split grapheme: {text:?}"
                );
            }
        }
    }
}

#[test]
fn old_preferences_default_off_and_body_only_requests_lyrics() {
    let mut old = serde_json::to_value(OscSettings::default()).unwrap();
    old.as_object_mut().unwrap().remove("idle_content");
    let mut settings: OscSettings = serde_json::from_value(old).unwrap();
    assert_eq!(settings.idle_content, IdleContent::None);
    assert!(settings.lyric_budget().is_none());
    assert_eq!(settings.lyric_window_lines(), 0);
    settings.idle_content = IdleContent::Lyrics;
    assert_eq!(settings.lyric_budget(), Some(24));
    assert_eq!(settings.lyric_window_lines(), 9);
    assert_eq!(
        serde_json::from_value::<OscSettings>(serde_json::to_value(&settings).unwrap()).unwrap(),
        settings
    );
    settings.enabled = false;
    assert!(settings.lyric_budget().is_none());
    assert_eq!(settings.lyric_window_lines(), 0);
}
