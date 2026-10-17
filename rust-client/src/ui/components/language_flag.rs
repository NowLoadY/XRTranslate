use eframe::egui;

/// One badge per country, with every represented language available on hover.
pub fn language_flags(ui: &mut egui::Ui, language: crate::i18n::UiLanguage, codes: &[&str]) {
    let mut groups: Vec<(&str, egui::ImageSource<'static>, Vec<&str>)> = Vec::new();
    for code in codes {
        let (country, source) = flag_for_language(code);
        let label_code = if code.eq_ignore_ascii_case("tl") {
            "fil"
        } else {
            code
        };
        let label = crate::language_label(language, label_code);
        if let Some((_, _, labels)) = groups.iter_mut().find(|(key, _, _)| *key == country) {
            if !labels.contains(&label) {
                labels.push(label);
            }
        } else {
            groups.push((country, source, vec![label]));
        }
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
        for (_, source, labels) in groups {
            let label = labels.join(" · ");
            ui.add(
                egui::Image::new(source)
                    .fit_to_exact_size(egui::vec2(30.0, 30.0))
                    .alt_text(&label),
            )
            .on_hover_text(label);
        }
    });
}

fn flag_for_language(code: &str) -> (&'static str, egui::ImageSource<'static>) {
    let code = code.replace('_', "-").to_ascii_lowercase();
    let mut parts = code.split('-');
    let base = parts.next().unwrap_or_default();
    // An explicit region wins over the language's usual representative flag.
    for region in parts.filter(|part| part.len() == 2) {
        if let Some(flag) = country_flag(region) {
            return flag;
        }
    }
    let country = match base {
        "af" => "za",
        "ar" => "eg",
        "bg" => "bg",
        "bn" => "bd",
        "bo" | "ug" | "yue" | "zh" => "cn",
        "cs" => "cz",
        "da" => "dk",
        "de" => "de",
        "el" => "gr",
        "en" => "gb",
        "es" => "es",
        "fa" => "ir",
        "fi" => "fi",
        "fil" | "tl" => "ph",
        "fr" => "fr",
        "gu" | "hi" | "mr" | "ta" | "te" => "in",
        "he" => "il",
        "hr" => "hr",
        "hu" => "hu",
        "id" => "id",
        "it" => "it",
        "ja" => "jp",
        "kk" => "kz",
        "km" => "kh",
        "ko" => "kr",
        "mk" => "mk",
        "mn" => "mn",
        "ms" => "my",
        "my" => "mm",
        "nl" => "nl",
        "no" => "no",
        "pl" => "pl",
        "pt" => "pt",
        "ro" => "ro",
        "ru" => "ru",
        "sk" => "sk",
        "sv" => "se",
        "th" => "th",
        "tr" => "tr",
        "uk" => "ua",
        "ur" => "pk",
        "vi" => "vn",
        _ => "globe",
    };
    country_flag(country).unwrap_or_else(|| {
        (
            "globe",
            egui::include_image!("../../../../assets/icons/flags/globe.svg"),
        )
    })
}

fn country_flag(country: &str) -> Option<(&'static str, egui::ImageSource<'static>)> {
    macro_rules! flags {
        ($($code:literal),+ $(,)?) => {
            match country {
                $($code => Some(($code, egui::include_image!(concat!(
                    "../../../../assets/icons/flags/", $code, ".svg"
                )))),)+
                _ => None,
            }
        };
    }
    flags!(
        "bd", "bg", "br", "cn", "cz", "de", "dk", "eg", "es", "fi", "fr", "gb", "gr", "hk", "hr",
        "hu", "id", "il", "in", "ir", "it", "jp", "kh", "kr", "kz", "mk", "mm", "mn", "my", "nl",
        "no", "ph", "pk", "pl", "pt", "ro", "ru", "se", "sk", "th", "tr", "tw", "ua", "us", "vn",
        "za",
    )
}
