use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioChannelItem {
    pub index: usize,
    pub id: String,
    pub name: String,
    pub is_left: bool,
    pub is_right: bool,
    pub is_center: bool,
    pub playback: bool,
    pub recognition: bool,
}

impl AudioChannelItem {
    pub fn label(&self) -> &'static str {
        match self.id.as_str() {
            "mono" => "Mono",
            "fl" => "Front left",
            "fr" => "Front right",
            "fc" => "Center",
            "lfe" => "Bass",
            "bl" => "Back left",
            "br" => "Back right",
            "sl" => "Side left",
            "sr" => "Side right",
            _ => "Channel",
        }
    }

    pub fn default_for_count(count: usize) -> Vec<AudioChannelItem> {
        let layout: &[&str] = match count {
            1 => &["mono"],
            2 => &["fl", "fr"],
            3 => &["fl", "fr", "lfe"],
            4 => &["fl", "fr", "bl", "br"],
            5 => &["fl", "fr", "fc", "sl", "sr"],
            6 => &["fl", "fr", "fc", "lfe", "sl", "sr"],
            8 => &["fl", "fr", "fc", "lfe", "bl", "br", "sl", "sr"],
            _ => &[],
        };
        (0..count)
            .map(|index| {
                let id = layout.get(index).copied();
                let name = match id {
                    Some("mono") => "Mono (单声道)",
                    Some("fl") if count < 4 => "FL (左声道 / Front Left)",
                    Some("fr") if count < 4 => "FR (右声道 / Front Right)",
                    Some("fl") => "FL (前左 / Front Left)",
                    Some("fr") => "FR (前右 / Front Right)",
                    Some("fc") => "FC (中置对白 / Center Dialogue)",
                    Some("lfe") => "LFE (低音炮 / Subwoofer)",
                    Some("bl") => "BL (后左 / Back Left)",
                    Some("br") => "BR (后右 / Back Right)",
                    Some("sl") if count == 8 => "SL (左侧环绕 / Side Left)",
                    Some("sr") if count == 8 => "SR (右侧环绕 / Side Right)",
                    Some("sl") => "SL (左环绕 / Surround Left)",
                    Some("sr") => "SR (右环绕 / Surround Right)",
                    _ => "",
                };
                Self {
                    index,
                    id: id.map(str::to_owned).unwrap_or_else(|| format!("c{index}")),
                    name: if id.is_some() {
                        name.into()
                    } else {
                        format!("CH {} (声道 {})", index + 1, index + 1)
                    },
                    is_left: id.map_or(index % 2 == 0, |id| {
                        matches!(id, "mono" | "fl" | "bl" | "sl")
                    }),
                    is_right: id.map_or(index % 2 != 0, |id| {
                        matches!(id, "mono" | "fr" | "br" | "sr")
                    }),
                    is_center: id.is_some_and(|id| matches!(id, "mono" | "fc" | "lfe")),
                    playback: true,
                    recognition: id.is_none_or(|id| matches!(id, "mono" | "fl" | "fr" | "fc")),
                }
            })
            .collect()
    }
}
