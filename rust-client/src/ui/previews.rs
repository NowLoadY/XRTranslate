use super::components::ShowcaseSlide;
use eframe::egui;

pub(crate) fn welcome() -> [ShowcaseSlide; 4] {
    let [avatar, ..] = avatar();
    [
        ShowcaseSlide {
            title: "Live translation",
            description: "Translate microphone and desktop audio in real time.",
            image: egui::include_image!("../../../assets/showcase-translation.webp"),
            detail: None,
            url: None,
        },
        ShowcaseSlide {
            title: "Media subtitles",
            description: "Create and translate subtitles for audio and video.",
            image: egui::include_image!("../../../assets/showcase-media.webp"),
            detail: None,
            url: None,
        },
        ShowcaseSlide {
            title: "Translation styles",
            description: "Choose how your translations sound.",
            image: egui::include_image!("../../../assets/showcase-styles.webp"),
            detail: None,
            url: None,
        },
        avatar,
    ]
}

pub(crate) fn avatar() -> [ShowcaseSlide; 5] {
    [
        (
            "VRChat",
            egui::include_image!("../../../assets/avatar-plugin-preview.jpg"),
        ),
        (
            "ChilloutVR",
            egui::include_image!("../../../assets/avatar-plugin-chilloutvr.webp"),
        ),
        (
            "Overte",
            egui::include_image!("../../../assets/avatar-plugin-overte.webp"),
        ),
        (
            "Resonite",
            egui::include_image!("../../../assets/avatar-plugin-resonite.webp"),
        ),
        (
            "Second Life",
            egui::include_image!("../../../assets/avatar-plugin-secondlife.webp"),
        ),
    ]
    .map(|(game, image)| ShowcaseSlide {
        title: "XRT-Avatar-Plugin",
        description: "Add live translations to your avatar!",
        image,
        detail: Some(game),
        url: Some("https://github.com/NowLoadY/XRT-Avatar-Plugin"),
    })
}
