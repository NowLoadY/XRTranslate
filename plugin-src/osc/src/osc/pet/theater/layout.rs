//! Adapter between chatbox allocation and the pet's independent world.
use super::{BannerContentType, Block, Frame, OscSettings};

pub(super) fn render(
    header: &str,
    footer: &str,
    settings: &OscSettings,
    music: bool,
    frame: &mut Frame<'_>,
) -> Option<String> {
    let configs = [&settings.header_config, &settings.footer_config];
    if !configs
        .iter()
        .any(|config| config.content_type == BannerContentType::TextPet)
    {
        return None;
    }
    let mut banners = [String::new(), String::new()];
    for (index, (config, text)) in configs.into_iter().zip([header, footer]).enumerate() {
        if config.content_type != BannerContentType::TextPet {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            banners[index] = frame.banner(
                if index == 0 {
                    Block::Header
                } else {
                    Block::Footer
                },
                &text,
                (settings.text_limit() / 3).min(24),
                config.content_type,
            );
        }
    }
    let occupied = banners.iter().filter(|banner| !banner.is_empty()).count();
    let budget = settings.text_limit().saturating_sub(
        banners
            .iter()
            .map(|banner| banner.chars().count())
            .sum::<usize>()
            + occupied,
    );
    let body = frame.renderer.theater.render(
        budget,
        settings.line_limit().saturating_sub(occupied),
        music,
        frame.now,
    )?;
    let text = frame.compose(&banners[0], &body, &banners[1], settings.line_limit());
    frame.renderer.regions.pet_stage = true;
    Some(text)
}
