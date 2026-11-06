pub use xrtranslate_client_core::ui::components::*;
mod account_capsule;
pub(crate) use account_capsule::account_capsule;
mod showcase;
use eframe::egui::{self, Color32};
pub(crate) use showcase::{ShowcaseSlide, showcase};

pub fn render_runtime_fallback_notice(
    ui: &mut egui::Ui,
    language: crate::i18n::UiLanguage,
    installer: &crate::runtime_install::RuntimeInstaller,
) {
    let Some(reason) = installer.fallback_reason() else {
        return;
    };
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(reason)
            .size(11.5)
            .color(Color32::from_rgb(180, 83, 9)),
    );
    if reason.contains(crate::runtime_install::NVIDIA_APP_URL) {
        ui.hyperlink_to(
            crate::i18n::tr(language, "Open NVIDIA App driver download"),
            crate::runtime_install::NVIDIA_APP_URL,
        );
    }
}

pub mod selection_card;
