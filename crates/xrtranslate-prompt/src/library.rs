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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    Ordinary,
    PseudoStreaming,
}

impl Default for PromptMode {
    fn default() -> Self {
        Self::Ordinary
    }
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
       - System Value Node: {'type': 'system_value', 'value': 'current_input' | 'source_language' | 'target_language' | 'recognition_context' | 'recognition_mode' | ...}\n\
         * 'current_input': Real-time speech transcript sentence to be translated.\n\
         * 'source_language': Source language name (e.g. 'English', 'Japanese').\n\
         * 'target_language': Target language name (e.g. 'Chinese').\n\
         * 'recognition_context': Structured ASR terms rendered as text; it is not the current transcript.\n\
         * 'recognition_mode': Runtime workflow mode, either 'ordinary' or 'pseudo_streaming'.\n\
       - Input / Data Block (IMPORTANT: built-in blocks ALREADY include descriptive markdown headers):\n\
         * 'terminology': Renders '## Terminology\\n\\n<matched glossary rows>'\n\
         * 'recent_turns': Renders '## Recent Bilingual History\\n\\n<source/target dialogue turns>'\n\
         * 'language_order': Renders '## Language Order\\n\\n<order list>'\n\
         * 'previous_revision': Renders '## Previous Revision of Current Speech\\n\\n<revision>'\n\
         * 'surrounding_source': Renders '## Current Utterance Context (context only; do not translate)\\n\\n<lines>'\n\
         * 'custom_text': Renders '## Custom Reference Text\\n\\n<custom user text>'\n\
         * NOTE FOR AI DESIGNERS: Because each data block outputs its own '## Header', DO NOT add extra duplicate headers in Compose templates (e.g. write '{0}', not 'Terminology:\\n{0}').\n\
       - Bool Value Node: {'type': 'bool_value', 'value': true | false}. This is editor-owned data, not a hidden host predicate.\n\
       - Condition Value Node: {'type': 'condition_value', 'condition': 'has_reference_context' | 'has_recognition_context' | ...}. This is a host-owned boolean fact.\n\
       - Text Comparison Node: {'type': 'text_comparison', 'operator': 'equals' | 'not_equals' | 'contains' | 'starts_with' | 'ends_with', 'expected': '<text>', 'case_sensitive': true | false}. Input 0 is TEXT; output is BOOL.\n\
       - Compose Node: {'type': 'compose', 'text': 'Prompt template text with {0}, {1}, etc.'}\n\
         * Placeholders {0}, {1}, {2}... interpolate outputs from incoming links with input: 0, input: 1, etc.\n\
       - Switch Node: {'type': 'switch'}. Input 0 is BOOL, input 1 is FALSE TEXT, and input 2 is TRUE TEXT.\n\
       - Text Switch Node: {'type': 'text_switch'}. Input 0 is a finite TEXT selector. Its named TEXT sockets are derived from the selector source's possible outputs; never write or cache a 'cases' field.\n\
       - Request Node: {'type': 'request', 'target': 'open_ai_compatible' | 'hunyuan' | 'asr_instruction' | 'asr_context_bias', 'roles': ['system', 'user']}\n\
         * Final output sink. Translation targets carry LLM messages. 'asr_instruction' carries semantic recognition instructions; 'asr_context_bias' carries lexical context only. Weighted vocabulary is structured provider data and is never rendered by this graph.\n\
    2. LINKS (links: Array):\n\
       - {'from': '<source_node_id>', 'to': '<target_node_id>', 'input': <target_slot_index_integer>}\n\
    3. PROVIDER PAGES:\n\
       - One graph owns every recognition mode. Connect the recognition_mode System Value directly to a Text Switch; 'ordinary' and 'pseudo_streaming' become its named branches.\n\
       - Each node belongs to 'page': 'shared', 'open_ai_compatible', 'hunyuan', 'asr_instruction', or 'asr_context_bias'. Shared nodes may feed compatible provider pages; each provider page is a DAG pipeline inside the unified graph."
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
        let imported_graph = graph.clone();

        for node in &mut graph.nodes {
            if node.label.trim().is_empty() {
                node.label = crate::schema::default_node_label(&node.kind);
            }
        }

        let migrate_legacy_shared_nodes = graph.schema_version < 7;
        if graph.schema_version <= PromptNodeGraph::CURRENT_SCHEMA_VERSION {
            graph.migrate_legacy_dataflow();
        }
        if migrate_legacy_shared_nodes {
            migrate_shared_nodes(&mut graph);
        }
        ensure_asr_pages(&mut graph);
        graph =
            PromptNodeGraph::unify_mode_graphs(graph, &PromptNodeGraph::builtin_pseudo_streaming());
        graph.upgrade_known_pseudo_streaming_prompts();
        graph.sync_text_switch_cases(&imported_graph);
        graph.remove_invalid_socket_links();
        graph.restore_translation_style_binding();
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

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PromptTemplateLibrary {
    pub active_id: String,
    pub profiles: Vec<PromptTemplateProfile>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct PromptProfileCollection {
    active_id: String,
    profiles: Vec<PromptTemplateProfile>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
struct SplitDomainCollection {
    translation: PromptProfileCollection,
    asr: PromptProfileCollection,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum PromptTemplateLibraryWire {
    Current(PromptProfileCollection),
    ModeSeparated {
        ordinary: PromptProfileCollection,
        pseudo_streaming: PromptProfileCollection,
    },
    ModeAndDomainSeparated {
        ordinary: SplitDomainCollection,
        pseudo_streaming: SplitDomainCollection,
    },
}

impl<'de> Deserialize<'de> for PromptTemplateLibrary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(
            match PromptTemplateLibraryWire::deserialize(deserializer)? {
                PromptTemplateLibraryWire::Current(collection) => Self {
                    active_id: collection.active_id,
                    profiles: collection.profiles,
                },
                PromptTemplateLibraryWire::ModeSeparated {
                    ordinary,
                    pseudo_streaming,
                } => merge_mode_collections(ordinary, pseudo_streaming),
                PromptTemplateLibraryWire::ModeAndDomainSeparated {
                    ordinary,
                    pseudo_streaming,
                } => merge_split_domain_collections(ordinary, pseudo_streaming),
            },
        )
    }
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
        let stored = std::fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok());
        let Some(mut library) = stored
            .as_ref()
            .and_then(|value| Self::deserialize(value).ok())
        else {
            return Self::default();
        };
        library.normalize();
        // Compare the on-disk shape: deserialization itself can already migrate legacy fields.
        // Round-trip through JSON text to retain its f32 formatting for node positions.
        let current = serde_json::to_string(&library)
            .ok()
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok());
        if current.is_some() && current != stored {
            let _ = library.save_to_dir(runtime_dir);
        }
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
            let stored_graph = profile.graph.clone();
            let stored_positions = profile
                .graph
                .nodes
                .iter()
                .map(|node| (node.id.clone(), node.position))
                .collect::<std::collections::HashMap<_, _>>();
            let migrate_legacy_shared_nodes = profile.graph.schema_version < 7;
            if profile.graph.schema_version <= PromptNodeGraph::CURRENT_SCHEMA_VERSION {
                profile.graph.migrate_legacy_dataflow();
            }
            if profile.graph.schema_version != PromptNodeGraph::CURRENT_SCHEMA_VERSION
                || profile.graph.nodes.is_empty()
            {
                profile.graph = PromptNodeGraph::builtin_default();
            } else {
                if migrate_legacy_shared_nodes {
                    migrate_shared_nodes(&mut profile.graph);
                }
                ensure_asr_pages(&mut profile.graph);
                profile.graph = PromptNodeGraph::unify_mode_graphs(
                    profile.graph.clone(),
                    &PromptNodeGraph::builtin_pseudo_streaming(),
                );
                for node in &mut profile.graph.nodes {
                    if let Some(position) = stored_positions.get(&node.id) {
                        node.position = *position;
                    }
                }
                profile.graph.upgrade_known_pseudo_streaming_prompts();
                profile.graph.sync_text_switch_cases(&stored_graph);
                profile.graph.remove_invalid_socket_links();
                profile.graph.restore_translation_style_binding();
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
        description: "Unified ordinary, pseudo-streaming, translation, and ASR prompt workflow."
            .into(),
        graph: PromptNodeGraph::builtin_default(),
        read_only: true,
    }
}

fn merge_mode_collections(
    ordinary: PromptProfileCollection,
    pseudo_streaming: PromptProfileCollection,
) -> PromptTemplateLibrary {
    let active_id = ordinary.active_id.clone();
    let fallback_pseudo = PromptNodeGraph::builtin_pseudo_streaming();
    let active_pseudo = pseudo_streaming
        .profiles
        .iter()
        .find(|profile| profile.id == pseudo_streaming.active_id)
        .map(|profile| &profile.graph)
        .unwrap_or(&fallback_pseudo);
    let mut profiles = ordinary.profiles;
    for profile in &mut profiles {
        profile.graph =
            PromptNodeGraph::merge_complete_mode_graphs(profile.graph.clone(), active_pseudo);
    }
    append_preserved_profiles(&mut profiles, pseudo_streaming.profiles, "pseudo-streaming");
    PromptTemplateLibrary {
        active_id,
        profiles,
    }
}

fn merge_split_domain_collections(
    ordinary: SplitDomainCollection,
    pseudo_streaming: SplitDomainCollection,
) -> PromptTemplateLibrary {
    let mut ordinary_collection = ordinary.translation;
    merge_asr_pages(&mut ordinary_collection.profiles, &ordinary.asr);
    let mut pseudo_collection = pseudo_streaming.translation;
    merge_asr_pages(&mut pseudo_collection.profiles, &pseudo_streaming.asr);
    merge_mode_collections(ordinary_collection, pseudo_collection)
}

fn merge_asr_pages(
    translation_profiles: &mut [PromptTemplateProfile],
    asr: &PromptProfileCollection,
) {
    let active_asr = asr
        .profiles
        .iter()
        .find(|profile| profile.id == asr.active_id)
        .or_else(|| asr.profiles.first())
        .map(|profile| &profile.graph);
    for profile in translation_profiles {
        let Some(asr) = asr
            .profiles
            .iter()
            .find(|asr| asr.id == profile.id)
            .map(|asr| &asr.graph)
            .or(active_asr)
        else {
            continue;
        };
        profile.graph.replace_provider_pages(
            asr,
            &[
                crate::PromptNodePage::AsrInstruction,
                crate::PromptNodePage::AsrContextBias,
            ],
        );
    }
}

fn append_preserved_profiles(
    profiles: &mut Vec<PromptTemplateProfile>,
    incoming: Vec<PromptTemplateProfile>,
    prefix: &str,
) {
    for mut profile in incoming.into_iter().filter(|profile| !profile.read_only) {
        if profiles.iter().any(|existing| existing.id == profile.id) {
            profile.id = format!("{prefix}-{}", profile.id);
        }
        profile.name = format!("{} ({prefix})", profile.name);
        let mut ordinary = PromptNodeGraph::builtin_ordinary();
        ordinary.replace_provider_pages(
            &profile.graph,
            &[
                crate::PromptNodePage::AsrInstruction,
                crate::PromptNodePage::AsrContextBias,
            ],
        );
        profile.graph = PromptNodeGraph::merge_complete_mode_graphs(ordinary, &profile.graph);
        profiles.push(profile);
    }
}

fn migrate_shared_nodes(graph: &mut PromptNodeGraph) {
    use crate::{PromptLink, PromptNodePage};
    use std::collections::{HashMap, HashSet};

    if !graph
        .nodes
        .iter()
        .any(|node| node.page == PromptNodePage::Shared)
    {
        return;
    }

    let pages = [
        PromptNodePage::OpenAiCompatible,
        PromptNodePage::Hunyuan,
        PromptNodePage::AsrInstruction,
        PromptNodePage::AsrContextBias,
    ];
    let mut used_ids = graph
        .nodes
        .iter()
        .filter(|node| node.page != PromptNodePage::Shared)
        .map(|node| node.id.clone())
        .collect::<HashSet<_>>();
    let mut shared_ids = HashMap::new();
    let mut new_nodes = graph
        .nodes
        .iter()
        .filter(|node| node.page != PromptNodePage::Shared)
        .cloned()
        .collect::<Vec<_>>();
    let mut new_links = Vec::new();

    for &page in &pages {
        for node in graph
            .nodes
            .iter()
            .filter(|node| node.page == PromptNodePage::Shared)
        {
            let prefix = match page {
                PromptNodePage::OpenAiCompatible => "openai",
                PromptNodePage::Hunyuan => "hunyuan",
                PromptNodePage::AsrInstruction => "asr-instruction",
                PromptNodePage::AsrContextBias => "asr-context-bias",
                PromptNodePage::Shared => unreachable!(),
            };
            let preferred = format!("{prefix}-{}", node.id);
            let mut id = preferred.clone();
            let mut suffix = 2_u32;
            while !used_ids.insert(id.clone()) {
                id = format!("{preferred}-{suffix}");
                suffix += 1;
            }
            let mut cloned = node.clone();
            cloned.id = id.clone();
            cloned.page = page;
            shared_ids.insert((page, node.id.clone()), id);
            new_nodes.push(cloned);
        }
    }

    for link in &graph.links {
        let from_node = graph.nodes.iter().find(|n| n.id == link.from);
        let to_node = graph.nodes.iter().find(|n| n.id == link.to);
        let from_shared = from_node.is_some_and(|n| n.page == PromptNodePage::Shared);
        let to_shared = to_node.is_some_and(|n| n.page == PromptNodePage::Shared);

        if from_shared && to_shared {
            for &page in &pages {
                new_links.push(PromptLink {
                    from: shared_ids[&(page, link.from.clone())].clone(),
                    to: shared_ids[&(page, link.to.clone())].clone(),
                    input: link.input,
                });
            }
        } else if from_shared {
            let Some(target_page) = to_node.map(|node| node.page) else {
                continue;
            };
            new_links.push(PromptLink {
                from: shared_ids[&(target_page, link.from.clone())].clone(),
                to: link.to.clone(),
                input: link.input,
            });
        } else if to_shared {
            let Some(source_page) = from_node.map(|node| node.page) else {
                continue;
            };
            new_links.push(PromptLink {
                from: link.from.clone(),
                to: shared_ids[&(source_page, link.to.clone())].clone(),
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
        let mut source_ids = canonical
            .nodes
            .iter()
            .filter(|node| node.page == page)
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        let mut pending = source_ids.iter().cloned().collect::<Vec<_>>();
        while let Some(to) = pending.pop() {
            for link in canonical.links.iter().filter(|link| link.to == to) {
                let Some(source) = canonical.nodes.iter().find(|node| node.id == link.from) else {
                    continue;
                };
                if source.page == PromptNodePage::Shared && source_ids.insert(source.id.clone()) {
                    pending.push(source.id.clone());
                }
            }
        }
        let source_nodes = canonical
            .nodes
            .iter()
            .filter(|node| source_ids.contains(&node.id))
            .cloned()
            .collect::<Vec<_>>();
        let mut used_ids = graph
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        let mut id_map = HashMap::new();
        for mut node in source_nodes {
            let source_id = node.id.clone();
            if node.page == PromptNodePage::Shared
                && let Some(existing) = graph.nodes.iter().find(|existing| {
                    existing.id == node.id && existing.page == PromptNodePage::Shared
                })
            {
                id_map.insert(source_id, existing.id.clone());
                continue;
            }
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
