use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ElementKind {
    Button,
    Toggle,
    Checkbox,
    Slider,
    TextInput,
    ComboBox,
    Tab,
    Card,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ElementValue {
    Bool(bool),
    Number(f64),
    Text(String),
    None,
}

impl ElementValue {
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            Self::Text(s) => match s.to_lowercase().as_str() {
                "true" | "1" | "yes" | "on" => Some(true),
                "false" | "0" | "no" | "off" => Some(false),
                _ => None,
            },
            Self::Number(n) => Some(*n != 0.0),
            Self::None => None,
        }
    }

    #[must_use]
    pub fn as_number(&self) -> Option<f64> {
        match self {
            Self::Number(n) => Some(*n),
            Self::Text(s) => s.parse::<f64>().ok(),
            Self::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Self::None => None,
        }
    }

    #[must_use]
    pub fn as_text(&self) -> Option<String> {
        match self {
            Self::Text(s) => Some(s.clone()),
            Self::Number(n) => Some(n.to_string()),
            Self::Bool(b) => Some(b.to_string()),
            Self::None => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementDescriptor {
    pub index: usize,
    pub id_hex: String,
    pub label: String,
    pub kind: ElementKind,
    pub value: ElementValue,
    pub enabled: bool,
    pub rect: [f32; 4],
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FrameSnapshot {
    pub page: String,
    pub elements: Vec<ElementDescriptor>,
}

impl FrameSnapshot {
    #[must_use]
    pub fn find_element(&self, target: &str) -> Option<&ElementDescriptor> {
        let trimmed = target.trim();
        if trimmed.is_empty() {
            return None;
        }

        let target_lower = trimmed.to_lowercase();

        // 1. Match by explicit hex ID (16 hex chars)
        if trimmed.len() == 16 {
            if let Some(elem) = self
                .elements
                .iter()
                .find(|e| e.id_hex.to_lowercase() == target_lower)
            {
                return Some(elem);
            }
        }

        // 2. Match by explicit index (e.g. "#0", "#1")
        if let Some(index_str) = trimmed.strip_prefix('#') {
            if let Ok(idx) = index_str.parse::<usize>() {
                if let Some(elem) = self.elements.iter().find(|e| e.index == idx) {
                    return Some(elem);
                }
            }
        }

        // 3. Match by exact label (case-insensitive)
        if let Some(elem) = self
            .elements
            .iter()
            .find(|e| e.label.to_lowercase() == target_lower)
        {
            return Some(elem);
        }

        // 4. Match by plain numeric index (short integers like "0", "12")
        if trimmed.len() <= 4 {
            if let Ok(idx) = trimmed.parse::<usize>() {
                if let Some(elem) = self.elements.iter().find(|e| e.index == idx) {
                    return Some(elem);
                }
            }
        }

        // 5. Match by any hex ID
        if let Some(elem) = self
            .elements
            .iter()
            .find(|e| e.id_hex.to_lowercase() == target_lower)
        {
            return Some(elem);
        }

        // 6. Match by label contains substring (case-insensitive)
        if let Some(elem) = self
            .elements
            .iter()
            .find(|e| e.label.to_lowercase().contains(&target_lower))
        {
            return Some(elem);
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_snapshot_finds_by_index_and_label() {
        let snapshot = FrameSnapshot {
            page: "Settings".into(),
            elements: vec![
                ElementDescriptor {
                    index: 0,
                    id_hex: "0000000000000001".into(),
                    label: "Check for Updates".into(),
                    kind: ElementKind::Button,
                    value: ElementValue::None,
                    enabled: true,
                    rect: [0.0, 0.0, 100.0, 30.0],
                },
                ElementDescriptor {
                    index: 1,
                    id_hex: "0000000000000002".into(),
                    label: "Receive beta updates".into(),
                    kind: ElementKind::Checkbox,
                    value: ElementValue::Bool(false),
                    enabled: true,
                    rect: [0.0, 40.0, 200.0, 24.0],
                },
            ],
        };

        assert_eq!(snapshot.find_element("0").unwrap().label, "Check for Updates");
        assert_eq!(snapshot.find_element("#1").unwrap().label, "Receive beta updates");
        assert_eq!(snapshot.find_element("Check for Updates").unwrap().index, 0);
        assert_eq!(snapshot.find_element("beta").unwrap().index, 1);
        assert_eq!(snapshot.find_element("0000000000000001").unwrap().index, 0);
        assert!(snapshot.find_element("nonexistent").is_none());
    }
}
