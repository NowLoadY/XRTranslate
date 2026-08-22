use serde::{Deserialize, Serialize};

use crate::PromptNodeGraph;
use crate::builtin::BUILTIN_ID;

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptTemplateProfile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub graph: PromptNodeGraph,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptGraphProjectFile {
    #[serde(rename = "$schema_guide", default = "default_schema_guide")]
    pub schema_guide: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub graph: PromptNodeGraph,
}

fn default_schema_guide() -> String {
    "X-Translator Prompt Studio Graph Project Guide for AI / Humans:\n\
    1. NODES (nodes: Array):\n\
       - Variable Node: {'type': 'variable', 'variable': 'current_input' | 'source_language' | 'target_language' | 'recognition_context'}\n\
         * 'current_input': Real-time speech transcript sentence to be translated.\n\
         * 'source_language': Source language name (e.g. 'English', 'Japanese').\n\
         * 'target_language': Target language name (e.g. 'Chinese').\n\
         * 'recognition_context': Structured ASR terms rendered as text; it is not the current transcript.\n\
       - Input / Data Block (IMPORTANT: built-in blocks ALREADY include descriptive markdown headers):\n\
         * 'terminology': Renders '## Terminology\\n\\n<matched glossary rows>'\n\
         * 'recent_turns': Renders '## Recent Bilingual History\\n\\n<source/target dialogue turns>'\n\
         * 'language_order': Renders '## Language Order\\n\\n<order list>'\n\
         * 'previous_revision': Renders '## Previous Revision of Current Speech\\n\\n<revision>'\n\
         * 'surrounding_source': Renders '## Current Utterance Context (context only; do not translate)\\n\\n<lines>'\n\
         * 'custom_text': Renders '## Custom Reference Text\\n\\n<custom user text>'\n\
         * NOTE FOR AI DESIGNERS: Because each data block outputs its own '## Header', DO NOT add extra duplicate headers in Compose templates (e.g. write '{0}', not 'Terminology:\\n{0}').\n\
       - Compose Node: {'type': 'compose', 'text': 'Prompt template text with {0}, {1}, etc.'}\n\
         * Placeholders {0}, {1}, {2}... interpolate outputs from incoming links with input: 0, input: 1, etc.\n\
       - Switch Node: {'type': 'switch', 'condition': 'has_reference_context' | 'has_recognition_context' | 'source_is_auto'}\n\
         * Evaluates condition at runtime: routes input: 0 (False branch) or input: 1 (True branch).\n\
       - Request Node: {'type': 'request', 'target': 'open_ai_compatible' | 'hunyuan' | 'asr_instruction' | 'asr_context_bias', 'roles': ['system', 'user']}\n\
         * Final output sink. Translation targets carry LLM messages. 'asr_instruction' carries semantic recognition instructions; 'asr_context_bias' carries lexical context only. Weighted vocabulary is structured provider data and is never rendered by this graph.\n\
    2. LINKS (links: Array):\n\
       - {'from': '<source_node_id>', 'to': '<target_node_id>', 'input': <target_slot_index_integer>}\n\
    3. PROVIDER PAGES:\n\
       - Each node belongs to 'page': 'open_ai_compatible', 'hunyuan', 'asr_instruction', or 'asr_context_bias'. Each delivery graph is an independent DAG pipeline."
        .into()
}

impl PromptTemplateProfile {
    pub fn export_project_json(&self) -> Result<String, String> {
        let project = PromptGraphProjectFile {
            schema_guide: default_schema_guide(),
            name: self.name.clone(),
            description: self.description.clone(),
            graph: self.graph.clone(),
        };
        serde_json::to_string_pretty(&project).map_err(|e| e.to_string())
    }

    pub fn import_project_json(
        content: &str,
        new_id: impl Into<String>,
    ) -> Result<PromptTemplateProfile, String> {
        let (name, description, mut graph) = if let Ok(project) =
            serde_json::from_str::<PromptGraphProjectFile>(content)
        {
            (project.name, project.description, project.graph)
        } else if let Ok(profile) = serde_json::from_str::<PromptTemplateProfile>(content) {
            (profile.name, profile.description, profile.graph)
        } else if let Ok(raw_graph) = serde_json::from_str::<PromptNodeGraph>(content) {
            ("Imported Graph".to_string(), String::new(), raw_graph)
        } else {
            return Err(
                    "Failed to parse prompt graph JSON. Please ensure it follows the Prompt Graph schema."
                        .into(),
                );
        };

        if graph.nodes.is_empty() {
            return Err("Imported prompt graph contains no nodes.".into());
        }

        for node in &mut graph.nodes {
            if node.label.trim().is_empty() {
                node.label = crate::schema::default_node_label(&node.kind);
            }
        }

        graph.schema_version = PromptNodeGraph::CURRENT_SCHEMA_VERSION;
        migrate_shared_nodes(&mut graph);
        ensure_asr_pages(&mut graph);
        graph.auto_layout();

        if let Err(err) = graph.validate_for_activation() {
            return Err(format!("Invalid graph structure: {err}"));
        }

        Ok(PromptTemplateProfile {
            id: new_id.into(),
            name: if name.trim().is_empty() {
                "Imported Graph".into()
            } else {
                name
            },
            description,
            graph,
            read_only: false,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptTemplateLibrary {
    pub active_id: String,
    pub profiles: Vec<PromptTemplateProfile>,
}

impl Default for PromptTemplateLibrary {
    fn default() -> Self {
        Self {
            active_id: BUILTIN_ID.into(),
            profiles: vec![builtin_default_profile()],
        }
    }
}

impl PromptTemplateLibrary {
    pub const FILE_NAME: &'static str = "prompt-studio.json";

    pub fn load_from_dir(runtime_dir: &Path) -> Self {
        let path = runtime_dir.join(Self::FILE_NAME);
        let mut library = std::fs::read_to_string(path)
            .ok()
            .and_then(|contents| serde_json::from_str::<Self>(&contents).ok())
            .unwrap_or_default();
        library.normalize();
        library
    }

    pub fn save_to_dir(&self, runtime_dir: &Path) -> Result<(), String> {
        let _ = std::fs::create_dir_all(runtime_dir);
        let path = runtime_dir.join(Self::FILE_NAME);
        let mut normalized = self.clone();
        normalized.normalize();
        let contents = serde_json::to_string_pretty(&normalized).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("{contents}\n")).map_err(|e| e.to_string())
    }

    pub fn normalize(&mut self) {
        self.profiles
            .retain(|profile| !profile.id.trim().is_empty());

        for profile in &mut self.profiles {
            if profile.id == BUILTIN_ID {
                continue;
            }
            if profile.graph.schema_version != PromptNodeGraph::CURRENT_SCHEMA_VERSION
                || profile.graph.nodes.is_empty()
            {
                profile.graph = PromptNodeGraph::builtin_default();
            } else {
                migrate_shared_nodes(&mut profile.graph);
                ensure_asr_pages(&mut profile.graph);
            }
            profile.read_only = false;
        }

        if let Some(profile) = self
            .profiles
            .iter_mut()
            .find(|profile| profile.id == BUILTIN_ID)
        {
            *profile = builtin_default_profile();
        } else {
            self.profiles.insert(0, builtin_default_profile());
        }

        if !self
            .profiles
            .iter()
            .any(|profile| profile.id == self.active_id)
        {
            self.active_id = BUILTIN_ID.into();
        }
    }

    pub fn active_profile(&self) -> Option<&PromptTemplateProfile> {
        self.profiles
            .iter()
            .find(|profile| profile.id == self.active_id)
            .or_else(|| self.profiles.first())
    }

    pub fn active_graph(&self) -> PromptNodeGraph {
        self.active_profile()
            .map(|profile| profile.graph.clone())
            .unwrap_or_default()
    }

    pub fn is_builtin_id(id: &str) -> bool {
        id == BUILTIN_ID
    }

    pub fn editable_copy_of(
        profile: &PromptTemplateProfile,
        id: impl Into<String>,
    ) -> PromptTemplateProfile {
        let mut copy = profile.clone();
        copy.id = id.into();
        copy.name = format!("{} (copy)", profile.name);
        copy.read_only = false;
        copy
    }
}

fn builtin_default_profile() -> PromptTemplateProfile {
    PromptTemplateProfile {
        id: BUILTIN_ID.into(),
        name: "Built-in Default".into(),
        description: "The original provider prompts with configurable translation context.".into(),
        graph: PromptNodeGraph::builtin_default(),
        read_only: true,
    }
}

fn migrate_shared_nodes(graph: &mut PromptNodeGraph) {
    use crate::{PromptLink, PromptNodePage};
    if !graph
        .nodes
        .iter()
        .any(|node| node.page == PromptNodePage::Shared)
    {
        return;
    }

    let mut new_nodes = Vec::new();
    let mut new_links = Vec::new();

    for node in &graph.nodes {
        if node.page == PromptNodePage::Shared {
            let mut openai_node = node.clone();
            openai_node.id = format!("openai-{}", node.id);
            openai_node.page = PromptNodePage::OpenAiCompatible;
            new_nodes.push(openai_node);

            let mut hunyuan_node = node.clone();
            hunyuan_node.id = format!("hunyuan-{}", node.id);
            hunyuan_node.page = PromptNodePage::Hunyuan;
            new_nodes.push(hunyuan_node);
        } else {
            new_nodes.push(node.clone());
        }
    }

    for link in &graph.links {
        let from_node = graph.nodes.iter().find(|n| n.id == link.from);
        let to_node = graph.nodes.iter().find(|n| n.id == link.to);
        let from_shared = from_node.is_some_and(|n| n.page == PromptNodePage::Shared);
        let to_shared = to_node.is_some_and(|n| n.page == PromptNodePage::Shared);

        if from_shared && to_shared {
            new_links.push(PromptLink {
                from: format!("openai-{}", link.from),
                to: format!("openai-{}", link.to),
                input: link.input,
            });
            new_links.push(PromptLink {
                from: format!("hunyuan-{}", link.from),
                to: format!("hunyuan-{}", link.to),
                input: link.input,
            });
        } else if from_shared {
            let target_page = to_node
                .map(|n| n.page)
                .unwrap_or(PromptNodePage::OpenAiCompatible);
            let prefix = match target_page {
                PromptNodePage::Hunyuan => "hunyuan",
                _ => "openai",
            };
            new_links.push(PromptLink {
                from: format!("{prefix}-{}", link.from),
                to: link.to.clone(),
                input: link.input,
            });
        } else if to_shared {
            let source_page = from_node
                .map(|n| n.page)
                .unwrap_or(PromptNodePage::OpenAiCompatible);
            let prefix = match source_page {
                PromptNodePage::Hunyuan => "hunyuan",
                _ => "openai",
            };
            new_links.push(PromptLink {
                from: link.from.clone(),
                to: format!("{prefix}-{}", link.to),
                input: link.input,
            });
        } else {
            new_links.push(link.clone());
        }
    }

    graph.nodes = new_nodes;
    graph.links = new_links;
    graph.auto_layout();
}

/// Adds the canonical ASR prompt/context pages to translation graphs saved by
/// older releases without changing their OpenAI or Hunyuan paths.
fn ensure_asr_pages(graph: &mut PromptNodeGraph) {
    use crate::{PromptNodeKind, PromptNodePage, PromptProviderTarget};
    use std::collections::{HashMap, HashSet};

    let canonical = PromptNodeGraph::builtin_default();
    for (page, target) in [
        (
            PromptNodePage::AsrInstruction,
            PromptProviderTarget::AsrInstruction,
        ),
        (
            PromptNodePage::AsrContextBias,
            PromptProviderTarget::AsrContextBias,
        ),
    ] {
        let exists = graph.nodes.iter().any(|node| {
            matches!(node.kind, PromptNodeKind::Request { target: value, .. } if value == target)
        });
        if exists {
            continue;
        }
        let source_nodes = canonical
            .nodes
            .iter()
            .filter(|node| node.page == page)
            .cloned()
            .collect::<Vec<_>>();
        let source_ids = source_nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        let mut used_ids = graph
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        let mut id_map = HashMap::new();
        for mut node in source_nodes {
            let source_id = node.id.clone();
            if used_ids.contains(&node.id) {
                let mut suffix = 1_u32;
                loop {
                    let candidate = format!("{source_id}-migrated-{suffix}");
                    if !used_ids.contains(&candidate) {
                        node.id = candidate;
                        break;
                    }
                    suffix += 1;
                }
            }
            used_ids.insert(node.id.clone());
            id_map.insert(source_id, node.id.clone());
            graph.nodes.push(node);
        }
        graph
            .links
            .extend(canonical.links.iter().filter_map(|link| {
                if !(source_ids.contains(&link.from) && source_ids.contains(&link.to)) {
                    return None;
                }
                Some(crate::PromptLink {
                    from: id_map.get(&link.from)?.clone(),
                    to: id_map.get(&link.to)?.clone(),
                    input: link.input,
                })
            }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_restores_the_canonical_builtin() {
        let mut library = PromptTemplateLibrary::default();
        library.profiles[0].graph = PromptNodeGraph::empty();
        library.profiles[0].read_only = false;
        library.normalize();
        assert_eq!(library.profiles[0], builtin_default_profile());
    }

    #[test]
    fn asr_page_migration_renames_canonical_ids_that_collide_with_custom_nodes() {
        let canonical = PromptNodeGraph::builtin_default();
        let mut graph = canonical.clone();
        graph.nodes.retain(|node| {
            !matches!(
                node.page,
                crate::PromptNodePage::AsrInstruction | crate::PromptNodePage::AsrContextBias
            )
        });
        let retained = graph
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<std::collections::HashSet<_>>();
        graph
            .links
            .retain(|link| retained.contains(&link.from) && retained.contains(&link.to));
        let old_id = graph.nodes[0].id.clone();
        graph.nodes[0].id = "asr-instruction-source-language".into();
        for link in &mut graph.links {
            if link.from == old_id {
                link.from = graph.nodes[0].id.clone();
            }
            if link.to == old_id {
                link.to = graph.nodes[0].id.clone();
            }
        }

        ensure_asr_pages(&mut graph);

        let unique = graph
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), graph.nodes.len());
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| node.id == "asr-instruction-source-language-migrated-1")
        );
        graph.validate_for_activation().unwrap();
    }

    #[test]
    fn template_profiles_do_not_serialize_visual_color_configuration() {
        let value = serde_json::to_value(PromptTemplateLibrary::default()).unwrap();
        assert!(value["profiles"][0].get("accent").is_none());
    }

    #[test]
    fn prompt_library_saves_and_loads_from_dedicated_file() {
        let temp_dir = std::env::temp_dir().join(format!(
            "xrt_prompt_lib_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);

        let mut library = PromptTemplateLibrary::default();
        let mut custom =
            PromptTemplateLibrary::editable_copy_of(&library.profiles[0], "custom-test-profile");
        custom.name = "Custom Test Profile".into();
        library.profiles.push(custom);
        library.active_id = "custom-test-profile".into();

        library.save_to_dir(&temp_dir).unwrap();
        assert!(temp_dir.join(PromptTemplateLibrary::FILE_NAME).exists());

        let loaded = PromptTemplateLibrary::load_from_dir(&temp_dir);
        assert_eq!(loaded.active_id, "custom-test-profile");
        assert_eq!(loaded.profiles.len(), 2);
        assert_eq!(loaded.profiles[1].name, "Custom Test Profile");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn export_and_import_project_json_round_trips_cleanly() {
        let profile = builtin_default_profile();
        let exported = profile.export_project_json().unwrap();

        assert!(exported.contains("\"$schema_guide\":"));
        assert!(exported.contains("\"name\": \"Built-in Default\""));
        assert!(exported.contains("\"openai-request\""));
        assert!(exported.contains("\"hunyuan-request\""));

        let imported =
            PromptTemplateProfile::import_project_json(&exported, "imported-test-id").unwrap();
        assert_eq!(imported.id, "imported-test-id");
        assert_eq!(imported.name, "Built-in Default");
        assert!(!imported.read_only);
        assert_eq!(imported.graph.nodes.len(), profile.graph.nodes.len());
        assert_eq!(imported.graph.links.len(), profile.graph.links.len());
    }

    #[test]
    fn import_project_json_rejects_empty_or_malformed_input() {
        assert!(PromptTemplateProfile::import_project_json("{}", "test-id").is_err());
        assert!(PromptTemplateProfile::import_project_json("not json", "test-id").is_err());
    }
}
