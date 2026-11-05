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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
}

impl ElementDescriptor {
    pub fn feedback(&self, effect: crate::feedback::Effect) -> crate::feedback::Feedback {
        crate::feedback::Feedback::new(
            u64::from_str_radix(&self.id_hex, 16).expect("registry identity"),
            &self.label,
            effect,
        )
    }

    pub fn actions(&self) -> &'static [&'static str] {
        if !self.enabled {
            return &[];
        }
        match self.kind {
            ElementKind::Button | ElementKind::Tab | ElementKind::Card => &["click"],
            ElementKind::Toggle | ElementKind::Checkbox => &["click", "set"],
            ElementKind::Slider | ElementKind::TextInput => &["set"],
            ElementKind::ComboBox if !self.options.is_empty() => &["click", "set"],
            ElementKind::ComboBox => &["click"],
            ElementKind::Other => &[],
        }
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FrameSnapshot {
    pub location: crate::Location,
    pub elements: Vec<ElementDescriptor>,
}

impl FrameSnapshot {
    pub fn resolve_element<'a>(&'a self, target: &str) -> Result<&'a ElementDescriptor, String> {
        let target = target.trim();
        if target.is_empty() {
            return Err("Supply a control ID or label".into());
        }
        let unique = |elements: Vec<&'a ElementDescriptor>| match elements.as_slice() {
            [element] => Ok(*element),
            [] => Err(format!(
                "Control '{target}' not found on page '{}'",
                self.location.page
            )),
            _ => Err(format!(
                "Control '{target}' is ambiguous; use an ID from ui.read"
            )),
        };
        let ids: Vec<_> = self
            .elements
            .iter()
            .filter(|e| e.id_hex.eq_ignore_ascii_case(target))
            .collect();
        if !ids.is_empty() {
            return unique(ids);
        }
        if let Some(index) = target
            .strip_prefix('#')
            .and_then(|v| v.parse::<usize>().ok())
        {
            return unique(self.elements.iter().filter(|e| e.index == index).collect());
        }
        let exact: Vec<_> = self
            .elements
            .iter()
            .filter(|e| e.label.eq_ignore_ascii_case(target))
            .collect();
        if !exact.is_empty() {
            return unique(exact);
        }
        if let Ok(index) = target.parse::<usize>() {
            return unique(self.elements.iter().filter(|e| e.index == index).collect());
        }
        let lower = target.to_lowercase();
        unique(
            self.elements
                .iter()
                .filter(|e| e.label.to_lowercase().contains(&lower))
                .collect(),
        )
    }
}

/// Host-owned route; the framework never interprets application page types.
#[derive(Clone, Debug, Serialize)]
pub struct Page {
    pub route: String,
    pub title: String,
    pub label: String,
}
