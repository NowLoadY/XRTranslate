use super::*;

#[test]
fn pet_timer_runs_silently_without_captions_and_stops_when_disabled() {
    let now = Instant::now();
    let mut settings = OscSettings::default();
    settings.footer_config.content_type = BannerContentType::TextPet;
    let deadline = banner_refresh_deadline(&settings, now, false).unwrap();
    assert_eq!(
        deadline.duration_since(now),
        crate::osc::pet::REFRESH_INTERVAL
    );
    assert!(dispatch_wait(false, now, None, Some(deadline)) <= crate::osc::pet::REFRESH_INTERVAL);
    let mut renderer = ChatboxRenderer::default();
    for tick in 0..6 {
        let message = build_queued_message(
            &[],
            &[],
            None,
            &settings,
            &Default::default(),
            &Default::default(),
            false,
            false,
            &mut renderer,
            now + crate::osc::pet::REFRESH_INTERVAL * tick,
        );
        assert!(!message.text.is_empty());
        assert!(!message.typing);
        assert!(!message.notify);
        assert_eq!(message.expires_at, None);
    }
    settings.enabled = false;
    assert_eq!(banner_refresh_deadline(&settings, now, false), None);
    settings.enabled = true;
    settings.footer_config.content_type = BannerContentType::None;
    assert_eq!(banner_refresh_deadline(&settings, now, false), None);
    settings.footer_config.content_type = BannerContentType::TextPet;
    settings.max_text_length = 2;
    settings.persistent_banners = true;
    assert_eq!(banner_refresh_deadline(&settings, now, false), None);
}
