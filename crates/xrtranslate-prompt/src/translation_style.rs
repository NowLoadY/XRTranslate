//! One shared Compose node owns style. This metadata only supplies the editor's saved texts.
use serde::{Deserialize, Serialize};

use crate::{PromptNodeGraph, PromptNodeKind, PromptNodePage, PromptProviderTarget};

pub(crate) const DEFAULT_TRANSLATION_STYLE: &str = concat!(
    "Translate into 100% natural, idiomatic target-language expression. Do not translate word-for-word, preserve original sentence structure, or produce translationese.\n\n",
    "The translation should sound like something a native speaker of the target language would naturally say or type in Discord, QQ, WeChat, gaming chats, and everyday conversations. Naturally adjust target-language word order, sentence structure, and wording according to the context.\n\n",
    "Preserve the original meaning, tone, emotion, attitude, personality, and level of formality. Do not unnecessarily add, remove, or change the original meaning.\n\n",
    "Use vocabulary and expressions commonly and naturally used in the target language. Do not use non-standard phrasing when a natural expression exists.\n\n",
    "When encountering slang, idioms, internet expressions, or conversational speech, convey the intended meaning using an expression that native speakers of the target language would naturally understand and use rather than translating it literally."
);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationStyle {
    pub node_id: String,
    #[serde(default)]
    pub presets: Vec<TranslationStylePreset>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranslationStylePreset {
    pub name: String,
    pub text: String,
}

impl PromptNodeGraph {
    /// Decode a literal Compose value. Connected templates cannot be edited as plain text.
    pub fn static_text(&self, node_id: &str) -> Option<String> {
        let node = self.nodes.iter().find(|node| node.id == node_id)?;
        match &node.kind {
            PromptNodeKind::Compose { text } => {
                crate::template::render_compose_text(text, |_| None).ok()
            }
            PromptNodeKind::Input {
                block: crate::TranslationPromptBlock::CustomText { text },
            } => Some(text.clone()),
            _ => None,
        }
    }

    /// Explicit binding by node ID, independent of the node's display name.
    pub fn bind_translation_style(&mut self, node_id: &str) -> bool {
        if self.static_text(node_id).is_none() || self.links.iter().any(|link| link.to == node_id) {
            return false;
        }
        // A literal leaf has no provider dependencies, so it can safely be shared in place.
        self.nodes
            .iter_mut()
            .find(|node| node.id == node_id)
            .unwrap()
            .page = PromptNodePage::Shared;
        self.translation_style
            .get_or_insert_with(|| TranslationStyle {
                node_id: node_id.into(),
                presets: Vec::new(),
            })
            .node_id = node_id.into();
        true
    }

    pub fn style_text(&self) -> Option<String> {
        self.static_text(&self.translation_style.as_ref()?.node_id)
    }

    pub fn set_style_text(&mut self, value: &str) -> bool {
        let Some(style) = &self.translation_style else {
            return false;
        };
        if self.static_text(&style.node_id).is_none() {
            return false;
        }
        let id = style.node_id.clone();
        let before = self.clone();
        match &mut self
            .nodes
            .iter_mut()
            .find(|node| node.id == id)
            .unwrap()
            .kind
        {
            PromptNodeKind::Compose { text } => *text = value.replace('{', "{{").replace('}', "}}"),
            PromptNodeKind::Input {
                block: crate::TranslationPromptBlock::CustomText { text },
            } => *text = value.into(),
            _ => return false,
        }
        self.sync_text_switch_cases(&before);
        true
    }

    pub fn save_style_preset(&mut self, name: &str) -> bool {
        let name = name.trim();
        let Some(text) = self.style_text().filter(|_| !name.is_empty()) else {
            return false;
        };
        let presets = &mut self.translation_style.as_mut().unwrap().presets;
        if let Some(preset) = presets.iter_mut().find(|preset| preset.name == name) {
            preset.text = text;
        } else {
            presets.push(TranslationStylePreset {
                name: name.into(),
                text,
            });
        }
        true
    }

    pub fn style_reaches_target(&self, target: PromptProviderTarget) -> bool {
        let Some(style) = &self.translation_style else {
            return false;
        };
        let mut pending = vec![style.node_id.as_str()];
        let mut visited = std::collections::HashSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            if self.nodes.iter().any(|node| {
                node.id == id
                    && matches!(node.kind, PromptNodeKind::Request { target: t, .. } if t == target)
            }) {
                return true;
            }
            pending.extend(
                self.links
                    .iter()
                    .filter(|link| link.from == id)
                    .map(|link| link.to.as_str()),
            );
        }
        false
    }

    pub(crate) fn add_builtin_translation_style(&mut self) {
        let style = self.add_compose(
            PromptNodePage::Shared,
            DEFAULT_TRANSLATION_STYLE.into(),
            [0.0, 0.0],
        );
        self.nodes
            .iter_mut()
            .find(|node| node.id == style)
            .unwrap()
            .label = "Translation style".into();
        self.bind_translation_style(&style);
        self.save_style_preset("Default");
        for (prefix, page) in [
            ("openai", PromptNodePage::OpenAiCompatible),
            ("hunyuan", PromptNodePage::Hunyuan),
        ] {
            let instruction = format!("{prefix}-instruction");
            let join = self.add_compose(page, "{0}\n\n{1}".into(), [0.0, 0.0]);
            self.nodes
                .iter_mut()
                .find(|node| node.id == join)
                .unwrap()
                .label = "Instruction + style".into();
            // Both context branches reuse this instruction, before any user input is appended.
            for link in self
                .links
                .iter_mut()
                .filter(|link| link.from == instruction)
            {
                link.from = join.clone();
            }
            self.connect(&instruction, &join, 0);
            self.connect(&style, &join, 1);
        }
        self.auto_layout();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AsrPromptContext, PromptMode, PromptTemplateLibrary, PromptTemplateProfile,
        TranslationPromptContext,
    };

    #[test]
    fn one_style_is_replaced_in_every_translation_branch_and_never_asr() {
        let original = PromptNodeGraph::builtin_default();
        let mut graph = original.clone();
        let id = graph.translation_style.as_ref().unwrap().node_id.clone();
        assert_eq!(graph.nodes.iter().filter(|node| matches!(&node.kind, PromptNodeKind::Compose { text } if text.contains("Discord"))).count(), 1);
        assert_eq!(
            graph.nodes.iter().find(|node| node.id == id).unwrap().page,
            PromptNodePage::Shared
        );
        let custom = "Use ornate prose; preserve {0}, {names}, and meaning.";
        assert!(graph.set_style_text(custom));
        assert_eq!(graph.style_text().as_deref(), Some(custom));
        graph.validate_for_activation().unwrap();
        for target in [
            PromptProviderTarget::OpenAiCompatible,
            PromptProviderTarget::Hunyuan,
        ] {
            assert!(graph.style_reaches_target(target));
            for mode in [PromptMode::Ordinary, PromptMode::PseudoStreaming] {
                for source in ["English", "auto"] {
                    for terminology_rows in [vec![], vec!["VRChat = VRChat".into()]] {
                        let context = TranslationPromptContext {
                            mode,
                            terminology_rows,
                            ..Default::default()
                        };
                        let baseline = original
                            .render(target, "Hello {Alice}", source, "Japanese", &context)
                            .unwrap();
                        let result = graph
                            .render(target, "Hello {Alice}", source, "Japanese", &context)
                            .unwrap();
                        assert_eq!(
                            baseline.messages[0]
                                .content
                                .matches(DEFAULT_TRANSLATION_STYLE)
                                .count(),
                            1
                        );
                        for (actual, expected) in result.messages.iter().zip(baseline.messages) {
                            assert_eq!(actual.role, expected.role);
                            assert_eq!(
                                actual.content,
                                expected.content.replace(DEFAULT_TRANSLATION_STYLE, custom)
                            );
                        }
                        assert!(
                            result
                                .messages
                                .last()
                                .unwrap()
                                .content
                                .ends_with("Hello {Alice}")
                        );
                    }
                }
            }
        }
        for target in [
            PromptProviderTarget::AsrInstruction,
            PromptProviderTarget::AsrContextBias,
        ] {
            let context = AsrPromptContext {
                vocabulary: vec!["VRChat".into()],
                ..Default::default()
            };
            assert!(!graph.style_reaches_target(target));
            assert_eq!(
                graph
                    .render_asr_with_trace(target, "English", "English,Japanese", &context)
                    .unwrap()
                    .render,
                original
                    .render_asr_with_trace(target, "English", "English,Japanese", &context)
                    .unwrap()
                    .render
            );
        }
        // An empty custom style removes preferences without removing model processing rules.
        graph.set_style_text("");
        let output = graph
            .render(
                PromptProviderTarget::Hunyuan,
                "Hello",
                "English",
                "Japanese",
                &TranslationPromptContext::default(),
            )
            .unwrap();
        assert_eq!(
            output.messages[0].content,
            "Translate the following English text into Japanese. Output only the translation, do not output the prompt; do not add explanations.\n\nHello"
        );
    }

    #[test]
    fn saved_styles_roundtrip_and_only_node_text_controls_execution() {
        let library = PromptTemplateLibrary::default();
        let builtin = library.active_graph();
        let mut copy = PromptTemplateLibrary::editable_copy_of(
            library.active_profile().unwrap(),
            "custom-style",
        );
        copy.graph.set_style_text("Use {formal} phrasing.");
        assert!(copy.graph.save_style_preset("  我自己的风格  "));
        assert!(!copy.graph.save_style_preset("   "));
        copy.graph
            .set_style_text("Use {formal} phrasing, with warmth.");
        assert!(copy.graph.save_style_preset("我自己的风格"));
        assert_eq!(
            copy.graph.translation_style.as_ref().unwrap().presets.len(),
            2
        );
        copy.graph.set_style_text("An unsaved draft");
        let json = copy.export_project_json().unwrap();
        let imported = PromptTemplateProfile::import_project_json(&json, "imported-style").unwrap();
        assert_eq!(
            imported.graph.translation_style,
            copy.graph.translation_style
        );
        assert_eq!(
            imported.graph.style_text().as_deref(),
            Some("An unsaved draft")
        );
        assert_eq!(library.active_graph(), builtin);
        let mut normalized = library;
        normalized.profiles.push(imported.clone());
        normalized.normalize();
        assert_eq!(
            normalized
                .profiles
                .iter()
                .find(|p| p.id == imported.id)
                .unwrap()
                .graph
                .translation_style,
            imported.graph.translation_style
        );
    }

    #[test]
    fn binding_is_explicit_and_does_not_rewrite_authored_templates() {
        let mut graph = PromptNodeGraph::builtin_default();
        let style = graph.translation_style.as_ref().unwrap().node_id.clone();
        graph
            .nodes
            .iter_mut()
            .find(|node| node.id == style)
            .unwrap()
            .label = "Renamed freely".into();
        assert_eq!(
            graph.style_text().as_deref(),
            Some(DEFAULT_TRANSLATION_STYLE)
        );
        assert!(!graph.bind_translation_style("openai-explicit-instruction-ordinary"));
        graph
            .nodes
            .iter_mut()
            .find(|node| node.id == style)
            .unwrap()
            .kind = PromptNodeKind::Compose {
            text: "Connected template {0}".into(),
        };
        let before = graph.clone();
        assert!(graph.style_text().is_none());
        assert!(!graph.set_style_text("Don't replace template"));
        assert_eq!(before, graph);
        graph.remove_node(&style);
        assert!(graph.translation_style.is_none());
        let leaf = graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "A custom style".into(),
            [0.0, 0.0],
        );
        assert!(graph.bind_translation_style(&leaf));
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == leaf)
                .unwrap()
                .page,
            PromptNodePage::Shared
        );
        assert_eq!(graph.style_text().as_deref(), Some("A custom style"));
        let mut json = serde_json::to_value(PromptNodeGraph::empty()).unwrap();
        json.as_object_mut().unwrap().remove("translation_style");
        assert!(
            serde_json::from_value::<PromptNodeGraph>(json)
                .unwrap()
                .translation_style
                .is_none()
        );
    }
}
