use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use crate::TranslationPromptBlock;

fn current_schema_version() -> u16 {
    PromptNodeGraph::CURRENT_SCHEMA_VERSION
}

// Keep Prompt Studio auto-layout aligned with the shared graph-editor spacing
// contract. The layer step includes the prompt node/configuration pane width
// plus the shared 144 px horizontal gap.
const AUTO_LAYOUT_LAYER_STEP: f32 = 680.0;
const AUTO_LAYOUT_VERTICAL_GAP: f32 = 56.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptNode {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    #[serde(default)]
    pub page: PromptNodePage,
    pub kind: PromptNodeKind,
    #[serde(default)]
    pub position: [f32; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PromptNodeKind {
    /// Legacy/static data node. Runtime-owned blocks are migrated to
    /// `SystemValue`; only `CustomText` remains valid in current graphs.
    Input {
        block: TranslationPromptBlock,
    },
    /// Legacy runtime variable retained for v6 deserialization and migration.
    Variable {
        variable: PromptVariable,
    },
    SystemValue {
        value: PromptSystemValue,
    },
    ConditionValue {
        condition: PromptCondition,
    },
    /// User-owned boolean value. The node label carries the user's semantic
    /// name while this payload remains a generic typed value.
    BoolValue {
        value: bool,
    },
    /// Converts connected text into a boolean without requiring a host-owned
    /// condition special case.
    TextComparison {
        #[serde(default)]
        operator: PromptTextComparison,
        expected: String,
        #[serde(default)]
        case_sensitive: bool,
    },
    Compose {
        text: String,
    },
    Switch {
        /// v6 compatibility only. Current graphs connect a ConditionValue to
        /// input 0 and serialize this field as absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        condition: Option<PromptCondition>,
    },
    /// Selects one text input branch from the current value of a connected
    /// finite text source. Branch sockets are derived from that source's
    /// declared possible outputs and are never persisted separately.
    TextSwitch,
    Request {
        #[serde(default)]
        target: PromptProviderTarget,
        roles: Vec<PromptMessageRole>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptNodePage {
    #[default]
    Shared,
    OpenAiCompatible,
    Hunyuan,
    AsrInstruction,
    AsrContextBias,
}

impl PromptNodePage {
    pub fn for_target(target: PromptProviderTarget) -> Self {
        match target {
            PromptProviderTarget::OpenAiCompatible => Self::OpenAiCompatible,
            PromptProviderTarget::Hunyuan => Self::Hunyuan,
            PromptProviderTarget::AsrInstruction => Self::AsrInstruction,
            PromptProviderTarget::AsrContextBias => Self::AsrContextBias,
        }
    }

    pub fn is_visible_on(self, target: PromptProviderTarget) -> bool {
        self == Self::Shared || self == Self::for_target(target)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptVariable {
    SourceLanguage,
    TargetLanguage,
    CurrentInput,
    RecognitionContext,
    RecognitionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptSystemValue {
    SourceLanguage,
    TargetLanguage,
    CurrentInput,
    RecognitionContext,
    RecognitionMode,
    LanguageOrder,
    Terminology,
    RecentTurns { limit: Option<usize> },
    PreviousRevision,
    SurroundingSource,
}

impl From<PromptVariable> for PromptSystemValue {
    fn from(variable: PromptVariable) -> Self {
        match variable {
            PromptVariable::SourceLanguage => Self::SourceLanguage,
            PromptVariable::TargetLanguage => Self::TargetLanguage,
            PromptVariable::CurrentInput => Self::CurrentInput,
            PromptVariable::RecognitionContext => Self::RecognitionContext,
            PromptVariable::RecognitionMode => Self::RecognitionMode,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptValueType {
    Text,
    Condition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptTextComparison {
    #[default]
    Equals,
    NotEquals,
    Contains,
    StartsWith,
    EndsWith,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptCondition {
    SourceIsAuto,
    HasReferenceContext,
    HasRecognitionContext,
    IsPseudoStreaming,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptProviderTarget {
    Hunyuan,
    #[default]
    OpenAiCompatible,
    AsrInstruction,
    AsrContextBias,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptGraphDomain {
    Translation,
    Asr,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PromptMessageRole {
    System,
    #[default]
    User,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptLink {
    pub from: String,
    pub to: String,
    pub input: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptNodeGraph {
    #[serde(default = "current_schema_version")]
    pub schema_version: u16,
    #[serde(default)]
    pub nodes: Vec<PromptNode>,
    #[serde(default)]
    pub links: Vec<PromptLink>,
    #[serde(default)]
    pub layout_version: u16,
}

impl PromptNode {
    pub fn layout_height(&self) -> f32 {
        let height = match &self.kind {
            PromptNodeKind::Input {
                block: TranslationPromptBlock::CustomText { text },
            } => content_node_height(text, 122.0, 31),
            PromptNodeKind::Compose { text } => {
                let inputs = crate::compose_input_indexes(text)
                    .map(|inputs| inputs.len())
                    .unwrap_or_default();
                content_node_height(text, 142.0, 43).max(72.0 + inputs as f32 * 25.0)
            }
            PromptNodeKind::Switch { .. } => 149.0,
            PromptNodeKind::TextSwitch => 99.0,
            PromptNodeKind::Request { roles, .. } => 88.0 + roles.len() as f32 * 25.0,
            _ => 84.0,
        };
        height.max(156.0)
    }
}

fn content_node_height(text: &str, minimum: f32, wrap_chars: usize) -> f32 {
    let lines = text
        .lines()
        .map(|line| {
            let characters = line.chars().count().max(1);
            characters.div_ceil(wrap_chars)
        })
        .sum::<usize>()
        .max(1);
    minimum.max(54.0 + lines as f32 * 13.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptGraphError {
    message: String,
}

impl PromptGraphError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl Display for PromptGraphError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PromptGraphError {}

impl PromptNodeGraph {
    pub const CURRENT_SCHEMA_VERSION: u16 = 9;
    pub const CURRENT_LAYOUT_VERSION: u16 = 9;
    pub const MAX_COMPOSE_INPUT_INDEX: u8 = 9;

    pub fn empty() -> Self {
        Self {
            schema_version: Self::CURRENT_SCHEMA_VERSION,
            nodes: Vec::new(),
            links: Vec::new(),
            layout_version: 0,
        }
    }

    pub fn fingerprint(&self) -> String {
        let bytes = serde_json::to_vec(self).unwrap_or_default();
        let hash = bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        format!("{hash:016x}")
    }

    pub fn node_layout_height(&self, id: &str) -> Option<f32> {
        let node = self.nodes.iter().find(|node| node.id == id)?;
        let height = if matches!(node.kind, PromptNodeKind::TextSwitch) {
            99.0 + self.text_switch_cases(id).map_or(0, |cases| cases.len()) as f32 * 25.0
        } else {
            node.layout_height()
        };
        Some(height.max(156.0))
    }

    pub fn add_input(
        &mut self,
        page: PromptNodePage,
        block: TranslationPromptBlock,
        position: [f32; 2],
    ) -> String {
        self.add_node(page, PromptNodeKind::Input { block }, position)
    }

    pub fn add_variable(
        &mut self,
        page: PromptNodePage,
        variable: PromptVariable,
        position: [f32; 2],
    ) -> String {
        self.add_system_value(page, variable.into(), position)
    }

    pub fn add_system_value(
        &mut self,
        page: PromptNodePage,
        value: PromptSystemValue,
        position: [f32; 2],
    ) -> String {
        self.add_node(page, PromptNodeKind::SystemValue { value }, position)
    }

    pub fn add_condition_value(
        &mut self,
        page: PromptNodePage,
        condition: PromptCondition,
        position: [f32; 2],
    ) -> String {
        self.add_node(page, PromptNodeKind::ConditionValue { condition }, position)
    }

    pub fn add_boolean_value(
        &mut self,
        page: PromptNodePage,
        value: bool,
        position: [f32; 2],
    ) -> String {
        self.add_node(page, PromptNodeKind::BoolValue { value }, position)
    }

    pub fn add_text_comparison(
        &mut self,
        page: PromptNodePage,
        operator: PromptTextComparison,
        expected: String,
        case_sensitive: bool,
        position: [f32; 2],
    ) -> String {
        self.add_node(
            page,
            PromptNodeKind::TextComparison {
                operator,
                expected,
                case_sensitive,
            },
            position,
        )
    }

    pub fn add_switch(
        &mut self,
        page: PromptNodePage,
        condition: PromptCondition,
        position: [f32; 2],
    ) -> String {
        let condition_id =
            self.add_condition_value(page, condition, [position[0] - 300.0, position[1]]);
        let switch_id = self.add_conditional_switch(page, position);
        let connected = self.connect(&condition_id, &switch_id, 0);
        debug_assert!(connected);
        switch_id
    }

    pub fn add_conditional_switch(&mut self, page: PromptNodePage, position: [f32; 2]) -> String {
        self.add_node(page, PromptNodeKind::Switch { condition: None }, position)
    }

    pub fn add_text_switch(&mut self, page: PromptNodePage, position: [f32; 2]) -> String {
        self.add_node(page, PromptNodeKind::TextSwitch, position)
    }

    pub fn add_compose(
        &mut self,
        page: PromptNodePage,
        text: String,
        position: [f32; 2],
    ) -> String {
        self.add_node(page, PromptNodeKind::Compose { text }, position)
    }

    pub fn add_request(
        &mut self,
        target: PromptProviderTarget,
        roles: Vec<PromptMessageRole>,
        position: [f32; 2],
    ) -> String {
        self.add_node(
            PromptNodePage::for_target(target),
            PromptNodeKind::Request { target, roles },
            position,
        )
    }

    fn add_node(
        &mut self,
        page: PromptNodePage,
        kind: PromptNodeKind,
        position: [f32; 2],
    ) -> String {
        let prefix = match kind {
            PromptNodeKind::Input { .. } => "input",
            PromptNodeKind::Variable { .. } => "variable",
            PromptNodeKind::SystemValue { .. } => "system-value",
            PromptNodeKind::ConditionValue { .. } => "condition-value",
            PromptNodeKind::BoolValue { .. } => "bool-value",
            PromptNodeKind::TextComparison { .. } => "text-comparison",
            PromptNodeKind::Compose { .. } => "compose",
            PromptNodeKind::Switch { .. } => "switch",
            PromptNodeKind::TextSwitch => "text-switch",
            PromptNodeKind::Request { .. } => "request",
        };
        let id = self.next_id(prefix);
        self.nodes.push(PromptNode {
            id: id.clone(),
            label: default_node_label(&kind),
            page,
            kind,
            position,
        });
        id
    }

    pub fn remove_node(&mut self, id: &str) {
        self.nodes.retain(|node| node.id != id);
        self.links.retain(|link| link.from != id && link.to != id);
    }

    pub fn connect(&mut self, from: &str, to: &str, input: u8) -> bool {
        let old_text_cases = (input == 0).then(|| self.text_switch_cases(to)).flatten();
        if from == to
            || !self.nodes.iter().any(|node| node.id == from)
            || !self.nodes.iter().any(|node| node.id == to)
            || !self.accepts_input(to, input)
            || !self.types_can_connect(from, to, input)
            || self.reaches(to, from)
        {
            return false;
        }

        let from_page = self
            .nodes
            .iter()
            .find(|node| node.id == from)
            .map(|n| n.page)
            .unwrap_or_default();
        let to_page = self
            .nodes
            .iter()
            .find(|node| node.id == to)
            .map(|n| n.page)
            .unwrap_or_default();

        if !pages_can_connect(from_page, to_page) {
            return false;
        }

        if let Some(PromptNode {
            kind: PromptNodeKind::Compose { text },
            ..
        }) = self.nodes.iter_mut().find(|node| node.id == to)
        {
            let is_declared =
                crate::compose_input_indexes(text).is_ok_and(|inputs| inputs.contains(&input));
            if !is_declared {
                append_compose_input(text, input);
            }
        }
        self.links
            .retain(|link| !(link.to == to && link.input == input));
        self.links.push(PromptLink {
            from: from.into(),
            to: to.into(),
            input,
        });
        if input == 0
            && self
                .nodes
                .iter()
                .any(|node| node.id == to && matches!(node.kind, PromptNodeKind::TextSwitch))
        {
            let new_cases = self.text_switch_cases(to).unwrap_or_default();
            self.remap_text_switch_links(to, &old_text_cases.unwrap_or_default(), &new_cases);
        }
        true
    }

    /// Returns the finite text values a node can produce from graph-owned
    /// static metadata. `None` means the output is runtime-dependent or an
    /// upstream branch does not expose a finite set.
    pub fn possible_text_outputs(&self, id: &str) -> Option<Vec<String>> {
        self.possible_text_outputs_inner(id, &mut HashSet::new())
    }

    fn possible_text_outputs_inner(
        &self,
        id: &str,
        visiting: &mut HashSet<String>,
    ) -> Option<Vec<String>> {
        if !visiting.insert(id.to_owned()) {
            return None;
        }
        let node = self.nodes.iter().find(|node| node.id == id)?;
        let values = match &node.kind {
            PromptNodeKind::Input {
                block: TranslationPromptBlock::CustomText { text },
            } => Some(vec![text.clone()]),
            PromptNodeKind::SystemValue {
                value: PromptSystemValue::RecognitionMode,
            }
            | PromptNodeKind::Variable {
                variable: PromptVariable::RecognitionMode,
            } => Some(vec!["ordinary".into(), "pseudo_streaming".into()]),
            PromptNodeKind::Compose { text }
                if crate::compose_input_indexes(text).is_ok_and(|inputs| inputs.is_empty()) =>
            {
                Some(vec![text.clone()])
            }
            PromptNodeKind::Switch { .. } => {
                self.possible_outputs_from_inputs(id, &[1, 2], visiting)
            }
            PromptNodeKind::TextSwitch => {
                let cases = self.text_switch_cases_inner(id, visiting)?;
                let inputs = (1..=cases.len().min(u8::MAX as usize))
                    .map(|input| input as u8)
                    .collect::<Vec<_>>();
                self.possible_outputs_from_inputs(id, &inputs, visiting)
            }
            _ => None,
        };
        visiting.remove(id);
        values.and_then(normalize_text_values)
    }

    fn possible_outputs_from_inputs(
        &self,
        id: &str,
        inputs: &[u8],
        visiting: &mut HashSet<String>,
    ) -> Option<Vec<String>> {
        let mut values = Vec::new();
        for input in inputs {
            let source = self
                .links
                .iter()
                .find(|link| link.to == id && link.input == *input)?
                .from
                .as_str();
            values.extend(self.possible_text_outputs_inner(source, visiting)?);
        }
        normalize_text_values(values)
    }

    /// Returns the selector values that define a TextSwitch's named branch
    /// sockets. A connected text source without finite metadata returns None.
    pub fn text_switch_cases(&self, id: &str) -> Option<Vec<String>> {
        self.text_switch_cases_inner(id, &mut HashSet::new())
    }

    fn text_switch_cases_inner(
        &self,
        id: &str,
        visiting: &mut HashSet<String>,
    ) -> Option<Vec<String>> {
        let node = self.nodes.iter().find(|node| node.id == id)?;
        if !matches!(node.kind, PromptNodeKind::TextSwitch) {
            return None;
        }
        let selector = self
            .links
            .iter()
            .find(|link| link.to == id && link.input == 0)?;
        self.possible_text_outputs_inner(&selector.from, visiting)
            .and_then(normalize_text_values)
    }

    /// Rebinds TextSwitch branch links by their case text after graph-owned
    /// finite metadata changes. This prevents a link from silently selecting a
    /// different branch merely because the new case ordering changed.
    pub fn sync_text_switch_cases(&mut self, previous: &Self) {
        let mut pending = self
            .nodes
            .iter()
            .filter(|node| {
                matches!(node.kind, PromptNodeKind::TextSwitch)
                    && previous.nodes.iter().any(|old| {
                        old.id == node.id && matches!(old.kind, PromptNodeKind::TextSwitch)
                    })
            })
            .map(|node| node.id.clone())
            .collect::<Vec<_>>();
        let mut ordered = Vec::with_capacity(pending.len());
        while !pending.is_empty() {
            let index = pending
                .iter()
                .position(|id| {
                    !pending
                        .iter()
                        .any(|other| other != id && self.reaches(other, id))
                })
                .unwrap_or_default();
            ordered.push(pending.remove(index));
        }

        for id in ordered {
            let old_cases = previous.text_switch_cases(&id).unwrap_or_default();
            let new_cases = self.text_switch_cases(&id).unwrap_or_default();
            self.remap_text_switch_links(&id, &old_cases, &new_cases);
        }
    }

    fn remap_text_switch_links(&mut self, id: &str, old_cases: &[String], new_cases: &[String]) {
        self.links = std::mem::take(&mut self.links)
            .into_iter()
            .filter_map(|mut link| {
                if link.to != id || link.input == 0 {
                    return Some(link);
                }
                let case = old_cases.get(usize::from(link.input - 1))?;
                link.input = new_cases
                    .iter()
                    .position(|candidate| candidate == case)
                    .and_then(|index| u8::try_from(index + 1).ok())?;
                Some(link)
            })
            .collect();
    }

    pub fn compose_input_socket_indexes(&self, id: &str) -> Vec<u8> {
        let Some(PromptNode {
            kind: PromptNodeKind::Compose { text },
            ..
        }) = self.nodes.iter().find(|node| node.id == id)
        else {
            return Vec::new();
        };
        let mut inputs = crate::compose_input_indexes(text).unwrap_or_default();
        for input in self
            .links
            .iter()
            .filter(|link| link.to == id)
            .map(|link| link.input)
        {
            if !inputs.contains(&input) {
                inputs.push(input);
            }
        }
        inputs.sort_unstable();
        inputs.dedup();

        let has_available_input = inputs.iter().any(|input| {
            !self
                .links
                .iter()
                .any(|link| link.to == id && link.input == *input)
        });
        if !has_available_input {
            if let Some(spare) =
                (0..=Self::MAX_COMPOSE_INPUT_INDEX).find(|input| !inputs.contains(input))
            {
                inputs.push(spare);
                inputs.sort_unstable();
            }
        }
        inputs
    }

    /// Upgrades the v6 implicit-runtime graph into the explicit typed dataflow
    /// used by v7. It is idempotent so imported graphs whose version field was
    /// lost can still be repaired safely.
    pub(crate) fn migrate_legacy_dataflow(&mut self) {
        let mut changed = false;
        for node in &mut self.nodes {
            let migrated = match node.kind.clone() {
                PromptNodeKind::Variable { variable } => PromptNodeKind::SystemValue {
                    value: variable.into(),
                },
                PromptNodeKind::Input { block } => match system_value_for_block(&block) {
                    Some(value) => PromptNodeKind::SystemValue { value },
                    None => PromptNodeKind::Input { block },
                },
                kind => kind,
            };
            changed |= migrated != node.kind;
            node.kind = migrated;
        }

        let legacy_switches = self
            .nodes
            .iter_mut()
            .filter_map(|node| {
                let PromptNodeKind::Switch { condition } = &mut node.kind else {
                    return None;
                };
                condition
                    .take()
                    .map(|condition| (node.id.clone(), node.page, node.position, condition))
            })
            .collect::<Vec<_>>();
        for (switch_id, page, position, condition) in legacy_switches {
            changed = true;
            for link in self.links.iter_mut().filter(|link| link.to == switch_id) {
                link.input = link.input.saturating_add(1);
            }
            let condition_id =
                self.add_condition_value(page, condition, [position[0] - 300.0, position[1]]);
            self.links.push(PromptLink {
                from: condition_id,
                to: switch_id,
                input: 0,
            });
        }
        let pseudo_conditions = self
            .nodes
            .iter()
            .filter_map(|node| {
                matches!(
                    node.kind,
                    PromptNodeKind::ConditionValue {
                        condition: PromptCondition::IsPseudoStreaming
                    }
                )
                .then(|| (node.id.clone(), node.page, node.position))
            })
            .collect::<Vec<_>>();
        for (condition_id, page, position) in pseudo_conditions {
            changed = true;
            let mode_id = self
                .nodes
                .iter()
                .find(|node| {
                    node.page == page
                        && matches!(
                            node.kind,
                            PromptNodeKind::SystemValue {
                                value: PromptSystemValue::RecognitionMode
                            }
                        )
                })
                .map(|node| node.id.clone())
                .unwrap_or_else(|| {
                    self.add_system_value(
                        page,
                        PromptSystemValue::RecognitionMode,
                        [position[0] - 300.0, position[1]],
                    )
                });
            let switches = self
                .links
                .iter()
                .filter(|link| link.from == condition_id && link.input == 0)
                .filter_map(|link| {
                    self.nodes
                        .iter()
                        .find(|node| node.id == link.to)
                        .filter(|node| matches!(node.kind, PromptNodeKind::Switch { .. }))
                        .map(|node| node.id.clone())
                })
                .collect::<Vec<_>>();
            if switches.is_empty() {
                if let Some(node) = self.nodes.iter_mut().find(|node| node.id == condition_id) {
                    node.kind = PromptNodeKind::TextComparison {
                        operator: PromptTextComparison::Equals,
                        expected: "pseudo_streaming".into(),
                        case_sensitive: true,
                    };
                }
                self.links
                    .retain(|link| !(link.to == condition_id && link.input == 0));
                self.links.push(PromptLink {
                    from: mode_id,
                    to: condition_id,
                    input: 0,
                });
            } else {
                for switch_id in &switches {
                    if let Some(node) = self.nodes.iter_mut().find(|node| node.id == *switch_id) {
                        node.kind = PromptNodeKind::TextSwitch;
                    }
                }
                self.links.retain(|link| {
                    !(link.from == condition_id && link.input == 0 && switches.contains(&link.to))
                });
                for switch_id in switches {
                    self.links.push(PromptLink {
                        from: mode_id.clone(),
                        to: switch_id,
                        input: 0,
                    });
                }
                self.nodes.retain(|node| node.id != condition_id);
            }
        }
        self.schema_version = Self::CURRENT_SCHEMA_VERSION;
        if changed {
            self.layout_version = 0;
        }
    }

    pub fn output_type(&self, id: &str) -> Option<PromptValueType> {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .map(|node| match node.kind {
                PromptNodeKind::ConditionValue { .. }
                | PromptNodeKind::BoolValue { .. }
                | PromptNodeKind::TextComparison { .. } => PromptValueType::Condition,
                _ => PromptValueType::Text,
            })
    }

    pub fn input_type(&self, id: &str, input: u8) -> Option<PromptValueType> {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .and_then(|node| match &node.kind {
                PromptNodeKind::Compose { .. } => Some(PromptValueType::Text),
                PromptNodeKind::TextComparison { .. } if input == 0 => Some(PromptValueType::Text),
                PromptNodeKind::Switch { .. } if input == 0 => Some(PromptValueType::Condition),
                PromptNodeKind::Switch { .. } if input < 3 => Some(PromptValueType::Text),
                PromptNodeKind::TextSwitch if input == 0 => Some(PromptValueType::Text),
                PromptNodeKind::TextSwitch
                    if self
                        .text_switch_cases(id)
                        .is_some_and(|cases| usize::from(input) <= cases.len()) =>
                {
                    Some(PromptValueType::Text)
                }
                PromptNodeKind::Request { roles, .. } if usize::from(input) < roles.len() => {
                    Some(PromptValueType::Text)
                }
                _ => None,
            })
    }

    fn types_can_connect(&self, from: &str, to: &str, input: u8) -> bool {
        self.output_type(from)
            .zip(self.input_type(to, input))
            .is_some_and(|(output, input)| output == input)
    }

    pub fn validate_for_activation(&self) -> Result<(), PromptGraphError> {
        if self.schema_version != Self::CURRENT_SCHEMA_VERSION {
            return Err(PromptGraphError::new(format!(
                "prompt graph schema {} must be migrated to {}",
                self.schema_version,
                Self::CURRENT_SCHEMA_VERSION
            )));
        }
        let mut ids = HashSet::new();
        for node in &self.nodes {
            if node.id.trim().is_empty() {
                return Err(PromptGraphError::new("prompt node IDs cannot be empty"));
            }
            if !ids.insert(node.id.as_str()) {
                return Err(PromptGraphError::new(format!(
                    "duplicate prompt node ID: {}",
                    node.id
                )));
            }
            if let PromptNodeKind::Compose { text } = &node.kind {
                crate::compose_input_indexes(text).map_err(|error| {
                    PromptGraphError::new(format!("compose node {}: {error}", node.id))
                })?;
            }
            match &node.kind {
                PromptNodeKind::Variable { .. }
                | PromptNodeKind::Input {
                    block:
                        TranslationPromptBlock::LanguageOrder
                        | TranslationPromptBlock::Terminology
                        | TranslationPromptBlock::RecentTurns { .. }
                        | TranslationPromptBlock::PreviousRevision
                        | TranslationPromptBlock::SurroundingSource,
                }
                | PromptNodeKind::Switch { condition: Some(_) } => {
                    return Err(PromptGraphError::new(format!(
                        "prompt node {} uses legacy implicit runtime dataflow",
                        node.id
                    )));
                }
                _ => {}
            }
            if matches!(node.kind, PromptNodeKind::TextSwitch)
                && self.text_switch_cases(&node.id).is_none()
            {
                return Err(PromptGraphError::new(format!(
                    "text switch {} selector does not expose finite possible outputs",
                    node.id
                )));
            }
            if self
                .text_switch_cases(&node.id)
                .is_some_and(|cases| cases.len() > u8::MAX as usize)
            {
                return Err(PromptGraphError::new(format!(
                    "text switch {} exposes too many possible outputs",
                    node.id
                )));
            }
            if let PromptNodeKind::Request { target, roles } = &node.kind {
                if roles.is_empty() || roles.len() > u8::MAX as usize {
                    return Err(PromptGraphError::new(format!(
                        "provider request {} must contain at least one message",
                        node.id
                    )));
                }
                if node.page != PromptNodePage::for_target(*target) {
                    return Err(PromptGraphError::new(format!(
                        "provider request {} is assigned to the wrong page",
                        node.id
                    )));
                }
            }
        }
        let nodes = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect::<HashMap<_, _>>();
        let mut sockets = HashSet::new();
        for link in &self.links {
            if !nodes.contains_key(link.from.as_str()) || !nodes.contains_key(link.to.as_str()) {
                return Err(PromptGraphError::new(
                    "prompt link references a missing node",
                ));
            }
            if link.from == link.to || !self.has_declared_input(&link.to, link.input) {
                return Err(PromptGraphError::new("prompt link uses an invalid socket"));
            }
            if !pages_can_connect(nodes[link.from.as_str()].page, nodes[link.to.as_str()].page) {
                return Err(PromptGraphError::new(
                    "prompt link crosses incompatible provider pages",
                ));
            }
            if self.output_type(&link.from) != self.input_type(&link.to, link.input) {
                return Err(PromptGraphError::new(format!(
                    "prompt link {} -> {} uses incompatible value types",
                    link.from, link.to
                )));
            }
            if !sockets.insert((link.to.as_str(), link.input)) {
                return Err(PromptGraphError::new(
                    "prompt input socket has multiple links",
                ));
            }
            if self.reaches(&link.to, &link.from) {
                return Err(PromptGraphError::new("prompt graph contains a cycle"));
            }
        }
        for node in &self.nodes {
            let required_inputs = match &node.kind {
                PromptNodeKind::Compose { text } => crate::compose_input_indexes(text)?,
                PromptNodeKind::TextComparison { .. } => vec![0],
                PromptNodeKind::Switch { .. } => vec![0, 1, 2],
                PromptNodeKind::TextSwitch => (0..=self
                    .text_switch_cases(&node.id)
                    .map(|cases| cases.len().min(u8::MAX as usize) as u8)
                    .unwrap_or_default())
                    .collect(),
                PromptNodeKind::Request { roles, .. } => (0..roles.len() as u8).collect(),
                _ => continue,
            };
            for input in required_inputs {
                if !self
                    .links
                    .iter()
                    .any(|link| link.to == node.id && link.input == input)
                {
                    return Err(PromptGraphError::new(format!(
                        "node {} input {input} is not connected",
                        node.id
                    )));
                }
            }
        }
        for target in [
            PromptProviderTarget::Hunyuan,
            PromptProviderTarget::OpenAiCompatible,
        ] {
            let outputs = self
                .nodes
                .iter()
                .filter(|node| {
                    matches!(node.kind, PromptNodeKind::Request { target: value, .. } if value == target)
                })
                .collect::<Vec<_>>();
            if outputs.is_empty() {
                return Err(PromptGraphError::new(format!(
                    "prompt graph has no {target:?} provider request"
                )));
            }
            if outputs.len() != 1 {
                return Err(PromptGraphError::new(format!(
                    "prompt graph must have exactly one {target:?} provider request"
                )));
            }
            if !outputs
                .iter()
                .any(|output| self.has_variable_ancestor(&output.id, PromptVariable::CurrentInput))
            {
                return Err(PromptGraphError::new(format!(
                    "the {target:?} prompt must include Current Input"
                )));
            }
        }
        for target in [
            PromptProviderTarget::AsrInstruction,
            PromptProviderTarget::AsrContextBias,
        ] {
            let outputs = self
                .nodes
                .iter()
                .filter(|node| {
                    matches!(node.kind, PromptNodeKind::Request { target: value, .. } if value == target)
                })
                .collect::<Vec<_>>();
            if outputs.len() > 1 {
                return Err(PromptGraphError::new(format!(
                    "prompt graph must have at most one {target:?} provider request"
                )));
            }
            if target == PromptProviderTarget::AsrContextBias
                && outputs.iter().any(|output| {
                    !self.has_variable_ancestor(&output.id, PromptVariable::RecognitionContext)
                })
            {
                return Err(PromptGraphError::new(format!(
                    "the {target:?} prompt must include Recognition Context"
                )));
            }
        }
        Ok(())
    }

    pub fn auto_layout(&mut self) {
        self.auto_layout_shared();
        for page in [
            PromptNodePage::OpenAiCompatible,
            PromptNodePage::Hunyuan,
            PromptNodePage::AsrInstruction,
            PromptNodePage::AsrContextBias,
        ] {
            self.auto_layout_page(page);
        }
        self.layout_version = Self::CURRENT_LAYOUT_VERSION;
    }

    fn auto_layout_shared(&mut self) {
        let ids = self
            .nodes
            .iter()
            .filter(|node| node.page == PromptNodePage::Shared)
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        if ids.is_empty() {
            return;
        }
        let mut layers = ids
            .iter()
            .map(|id| (id.clone(), 0_usize))
            .collect::<HashMap<_, _>>();
        for _ in 0..ids.len() {
            let mut changed = false;
            for link in &self.links {
                if !ids.contains(&link.from) || !ids.contains(&link.to) {
                    continue;
                }
                let next = layers[&link.from].saturating_add(1);
                let entry = layers.entry(link.to.clone()).or_default();
                if *entry < next {
                    *entry = next;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let max_layer = layers.values().copied().max().unwrap_or_default();
        for layer in 0..=max_layer {
            let mut column = ids
                .iter()
                .filter(|id| layers.get(*id) == Some(&layer))
                .cloned()
                .collect::<Vec<_>>();
            if layer == 0 {
                column.sort_by_key(|id| {
                    self.nodes
                        .iter()
                        .find(|node| node.id == *id)
                        .map(node_semantic_rank)
                        .unwrap_or((99, id.clone()))
                });
            } else {
                sort_layer_nodes(&mut column, layer, &self.nodes, &self.links);
            }
            let heights = column
                .iter()
                .filter_map(|id| {
                    self.node_layout_height(id)
                        .map(|height| (id.clone(), height))
                })
                .collect::<HashMap<_, _>>();
            let columns_before_provider = max_layer - layer + 1;
            position_column(
                &mut self.nodes,
                48.0 - columns_before_provider as f32 * AUTO_LAYOUT_LAYER_STEP,
                &column,
                40.0,
                &heights,
            );
        }
    }

    fn auto_layout_page(&mut self, page: PromptNodePage) {
        let page_node_ids: HashSet<String> = self
            .nodes
            .iter()
            .filter(|node| node.page == page || node.page == PromptNodePage::Shared)
            .map(|node| node.id.clone())
            .collect();

        if page_node_ids.is_empty() {
            return;
        }

        let mut layers = page_node_ids
            .iter()
            .map(|id| (id.clone(), 0_usize))
            .collect::<HashMap<_, _>>();

        for _ in 0..page_node_ids.len() {
            let mut changed = false;
            for link in &self.links {
                if !page_node_ids.contains(&link.from) || !page_node_ids.contains(&link.to) {
                    continue;
                }
                let Some(from_layer) = layers.get(&link.from).copied() else {
                    continue;
                };
                let next = from_layer.saturating_add(1);
                let entry = layers.entry(link.to.clone()).or_default();
                if *entry < next {
                    *entry = next;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let max_layer = layers.values().copied().max().unwrap_or(0);

        for layer in 0..=max_layer {
            let mut ids = self
                .nodes
                .iter()
                .filter(|node| node.page == page && layers.get(&node.id) == Some(&layer))
                .map(|node| node.id.clone())
                .collect::<Vec<_>>();

            if layer == 0 {
                ids.sort_by_key(|id| {
                    self.nodes
                        .iter()
                        .find(|n| n.id == *id)
                        .map(node_semantic_rank)
                        .unwrap_or((99, id.clone()))
                });
            } else {
                sort_layer_nodes(&mut ids, layer, &self.nodes, &self.links);
            }

            let heights = ids
                .iter()
                .filter_map(|id| {
                    self.node_layout_height(id)
                        .map(|height| (id.clone(), height))
                })
                .collect::<HashMap<_, _>>();
            position_column(
                &mut self.nodes,
                48.0 + layer as f32 * AUTO_LAYOUT_LAYER_STEP,
                &ids,
                40.0,
                &heights,
            );
        }
    }

    pub(crate) fn accepts_input(&self, id: &str, input: u8) -> bool {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .is_some_and(|node| match node.kind {
                PromptNodeKind::Compose { .. } => {
                    self.compose_input_socket_indexes(id).contains(&input)
                }
                PromptNodeKind::TextComparison { .. } => input == 0,
                PromptNodeKind::Switch { .. } => input < 3,
                PromptNodeKind::TextSwitch => {
                    input == 0
                        || self
                            .text_switch_cases(id)
                            .is_some_and(|cases| usize::from(input) <= cases.len())
                }
                PromptNodeKind::Request { ref roles, .. } => usize::from(input) < roles.len(),
                _ => false,
            })
    }

    fn has_declared_input(&self, id: &str, input: u8) -> bool {
        self.nodes
            .iter()
            .find(|node| node.id == id)
            .is_some_and(|node| match node.kind {
                PromptNodeKind::TextSwitch => {
                    input == 0
                        || self
                            .text_switch_cases(id)
                            .is_some_and(|cases| usize::from(input) <= cases.len())
                }
                _ => base_node_has_declared_input(node, input),
            })
    }

    pub(crate) fn remove_invalid_socket_links(&mut self) {
        let nodes = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect::<HashMap<_, _>>();
        let initially_valid = self
            .links
            .iter()
            .filter(|link| {
                let (Some(source), Some(target)) =
                    (nodes.get(link.from.as_str()), nodes.get(link.to.as_str()))
                else {
                    return false;
                };
                link.from != link.to
                    && base_node_has_declared_input(target, link.input)
                    && Some(node_output_type(source)) == base_node_input_type(target, link.input)
                    && pages_can_connect(source.page, target.page)
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut accepted = Vec::with_capacity(self.links.len());
        let mut sockets = HashSet::new();
        for link in initially_valid {
            if !sockets.insert((link.to.clone(), link.input))
                || links_reach(&accepted, &link.to, &link.from)
            {
                continue;
            }
            accepted.push(link);
        }
        self.links = accepted;
        let declared = self
            .links
            .iter()
            .map(|link| {
                (
                    (link.from.clone(), link.to.clone(), link.input),
                    self.has_declared_input(&link.to, link.input)
                        && self.types_can_connect(&link.from, &link.to, link.input),
                )
            })
            .collect::<HashMap<_, _>>();
        self.links.retain(|link| {
            declared
                .get(&(link.from.clone(), link.to.clone(), link.input))
                .copied()
                .unwrap_or(false)
        });
    }

    pub(crate) fn reaches(&self, start: &str, target: &str) -> bool {
        let mut pending = vec![start.to_owned()];
        let mut visited = HashSet::new();
        while let Some(current) = pending.pop() {
            if current == target {
                return true;
            }
            if !visited.insert(current.clone()) {
                continue;
            }
            pending.extend(
                self.links
                    .iter()
                    .filter(|link| link.from == current)
                    .map(|link| link.to.clone()),
            );
        }
        false
    }

    fn has_variable_ancestor(&self, output: &str, variable: PromptVariable) -> bool {
        let expected: PromptSystemValue = variable.into();
        let mut pending = vec![output];
        let mut visited = HashSet::new();
        while let Some(current) = pending.pop() {
            if !visited.insert(current) {
                continue;
            }
            if self.nodes.iter().find(|node| node.id == current).is_some_and(|node| {
                matches!(node.kind, PromptNodeKind::SystemValue { value } if value == expected)
            }) {
                return true;
            }
            pending.extend(
                self.links
                    .iter()
                    .filter(|link| link.to == current)
                    .map(|link| link.from.as_str()),
            );
        }
        false
    }

    fn next_id(&self, prefix: &str) -> String {
        let mut index = self.nodes.len() + 1;
        loop {
            let candidate = format!("{prefix}-{index}");
            if !self.nodes.iter().any(|node| node.id == candidate) {
                return candidate;
            }
            index += 1;
        }
    }
}

fn links_reach(links: &[PromptLink], start: &str, target: &str) -> bool {
    let mut pending = vec![start];
    let mut visited = HashSet::new();
    while let Some(current) = pending.pop() {
        if current == target {
            return true;
        }
        if !visited.insert(current) {
            continue;
        }
        pending.extend(
            links
                .iter()
                .filter(|link| link.from == current)
                .map(|link| link.to.as_str()),
        );
    }
    false
}

fn normalize_text_values(values: Vec<String>) -> Option<Vec<String>> {
    let mut unique = Vec::with_capacity(values.len());
    for value in values {
        if value.trim().is_empty() {
            return None;
        }
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    (!unique.is_empty()).then_some(unique)
}

fn base_node_has_declared_input(node: &PromptNode, input: u8) -> bool {
    match &node.kind {
        PromptNodeKind::Compose { text } => {
            crate::compose_input_indexes(text).is_ok_and(|inputs| inputs.contains(&input))
        }
        PromptNodeKind::TextComparison { .. } => input == 0,
        PromptNodeKind::Switch { .. } => input < 3,
        PromptNodeKind::TextSwitch => true,
        PromptNodeKind::Request { roles, .. } => usize::from(input) < roles.len(),
        _ => false,
    }
}

fn append_compose_input(text: &mut String, input: u8) {
    if !text.is_empty() {
        if text.ends_with("\n\n") {
            // The existing paragraph boundary already separates the new input.
        } else if text.ends_with('\n') {
            text.push('\n');
        } else {
            text.push_str("\n\n");
        }
    }
    text.push_str(&format!("{{{input}}}"));
}

fn pages_can_connect(source: PromptNodePage, target: PromptNodePage) -> bool {
    source == PromptNodePage::Shared || source == target
}

pub(crate) fn default_node_label(kind: &PromptNodeKind) -> String {
    match kind {
        PromptNodeKind::Input { block } => block.preview_name().into(),
        PromptNodeKind::Variable { variable } => match variable {
            PromptVariable::SourceLanguage => "SOURCE LANGUAGE".into(),
            PromptVariable::TargetLanguage => "TARGET LANGUAGE".into(),
            PromptVariable::CurrentInput => "CURRENT INPUT".into(),
            PromptVariable::RecognitionContext => "RECOGNITION CONTEXT".into(),
            PromptVariable::RecognitionMode => "RECOGNITION MODE".into(),
        },
        PromptNodeKind::SystemValue { value } => system_value_label(*value).into(),
        PromptNodeKind::ConditionValue { condition } => {
            format!("{} CONDITION", condition_label(*condition))
        }
        PromptNodeKind::BoolValue { value } => {
            if *value {
                "TRUE".into()
            } else {
                "FALSE".into()
            }
        }
        PromptNodeKind::TextComparison { .. } => "TEXT COMPARISON".into(),
        PromptNodeKind::Compose { .. } => "COMPOSE TEXT".into(),
        PromptNodeKind::Switch {
            condition: Some(condition),
        } => {
            format!("SELECT {}", condition_label(*condition))
        }
        PromptNodeKind::Switch { condition: None } => "CONDITIONAL SWITCH".into(),
        PromptNodeKind::TextSwitch => "TEXT BRANCH SELECTOR".into(),
        PromptNodeKind::Request { target, .. } => {
            let provider = match target {
                PromptProviderTarget::Hunyuan => "HUNYUAN",
                PromptProviderTarget::OpenAiCompatible => "OPENAI",
                PromptProviderTarget::AsrInstruction => "ASR PROMPT",
                PromptProviderTarget::AsrContextBias => "ASR CONTEXT",
            };
            format!("{provider} REQUEST")
        }
    }
}

fn node_semantic_rank(node: &PromptNode) -> (usize, String) {
    let category = match &node.kind {
        PromptNodeKind::Variable { variable } => match variable {
            PromptVariable::SourceLanguage => 0,
            PromptVariable::TargetLanguage => 1,
            PromptVariable::CurrentInput => 2,
            PromptVariable::RecognitionContext => 3,
            PromptVariable::RecognitionMode => 4,
        },
        PromptNodeKind::Input { block } => match block {
            TranslationPromptBlock::LanguageOrder => 3,
            TranslationPromptBlock::RecentTurns { .. } => 4,
            TranslationPromptBlock::SurroundingSource => 5,
            TranslationPromptBlock::Terminology => 6,
            TranslationPromptBlock::PreviousRevision => 7,
            TranslationPromptBlock::CustomText { .. } => 8,
        },
        PromptNodeKind::SystemValue { value } => match value {
            PromptSystemValue::SourceLanguage => 0,
            PromptSystemValue::TargetLanguage => 1,
            PromptSystemValue::CurrentInput => 2,
            PromptSystemValue::RecognitionContext => 3,
            PromptSystemValue::RecognitionMode => 4,
            PromptSystemValue::LanguageOrder => 3,
            PromptSystemValue::RecentTurns { .. } => 4,
            PromptSystemValue::SurroundingSource => 5,
            PromptSystemValue::Terminology => 6,
            PromptSystemValue::PreviousRevision => 7,
        },
        PromptNodeKind::ConditionValue { .. } => 8,
        PromptNodeKind::BoolValue { .. } => 8,
        PromptNodeKind::TextComparison { .. } => 9,
        PromptNodeKind::Compose { text }
            if text.contains("Reference handling rules") || text.contains("POLICY") =>
        {
            9
        }
        PromptNodeKind::Compose { .. } => 10,
        PromptNodeKind::Switch { .. } => 11,
        PromptNodeKind::TextSwitch => 11,
        PromptNodeKind::Request { .. } => 12,
    };
    (category, node.id.clone())
}

fn node_output_type(node: &PromptNode) -> PromptValueType {
    match node.kind {
        PromptNodeKind::ConditionValue { .. }
        | PromptNodeKind::BoolValue { .. }
        | PromptNodeKind::TextComparison { .. } => PromptValueType::Condition,
        _ => PromptValueType::Text,
    }
}

fn base_node_input_type(node: &PromptNode, input: u8) -> Option<PromptValueType> {
    match &node.kind {
        PromptNodeKind::Compose { .. } => Some(PromptValueType::Text),
        PromptNodeKind::TextComparison { .. } if input == 0 => Some(PromptValueType::Text),
        PromptNodeKind::Switch { .. } if input == 0 => Some(PromptValueType::Condition),
        PromptNodeKind::Switch { .. } if input < 3 => Some(PromptValueType::Text),
        PromptNodeKind::TextSwitch => Some(PromptValueType::Text),
        PromptNodeKind::Request { roles, .. } if usize::from(input) < roles.len() => {
            Some(PromptValueType::Text)
        }
        _ => None,
    }
}

fn system_value_for_block(block: &TranslationPromptBlock) -> Option<PromptSystemValue> {
    Some(match block {
        TranslationPromptBlock::LanguageOrder => PromptSystemValue::LanguageOrder,
        TranslationPromptBlock::Terminology => PromptSystemValue::Terminology,
        TranslationPromptBlock::RecentTurns { limit } => {
            PromptSystemValue::RecentTurns { limit: *limit }
        }
        TranslationPromptBlock::PreviousRevision => PromptSystemValue::PreviousRevision,
        TranslationPromptBlock::SurroundingSource => PromptSystemValue::SurroundingSource,
        TranslationPromptBlock::CustomText { .. } => return None,
    })
}

pub fn system_value_label(value: PromptSystemValue) -> &'static str {
    match value {
        PromptSystemValue::SourceLanguage => "SOURCE LANGUAGE",
        PromptSystemValue::TargetLanguage => "TARGET LANGUAGE",
        PromptSystemValue::CurrentInput => "CURRENT INPUT",
        PromptSystemValue::RecognitionContext => "RECOGNITION CONTEXT",
        PromptSystemValue::RecognitionMode => "RECOGNITION MODE",
        PromptSystemValue::LanguageOrder => "LANGUAGE ORDER",
        PromptSystemValue::Terminology => "TERMINOLOGY",
        PromptSystemValue::RecentTurns { .. } => "RECENT TURNS",
        PromptSystemValue::PreviousRevision => "PREVIOUS REVISION",
        PromptSystemValue::SurroundingSource => "SURROUNDING SOURCE",
    }
}

pub fn condition_label(condition: PromptCondition) -> &'static str {
    match condition {
        PromptCondition::SourceIsAuto => "SOURCE IS AUTO",
        PromptCondition::HasReferenceContext => "HAS REFERENCE CONTEXT",
        PromptCondition::HasRecognitionContext => "HAS RECOGNITION CONTEXT",
        PromptCondition::IsPseudoStreaming => "IS PSEUDO-STREAMING",
    }
}

fn compute_node_barycenter(
    node_id: &str,
    nodes: &[PromptNode],
    links: &[PromptLink],
) -> Option<f32> {
    let mut sum_y = 0.0;
    let mut count = 0.0;
    for link in links {
        if link.to == node_id {
            if let Some(from_node) = nodes.iter().find(|n| n.id == link.from) {
                sum_y += from_node.position[1];
                count += 1.0;
            }
        }
    }
    if count > 0.0 {
        Some(sum_y / count)
    } else {
        None
    }
}

fn sort_layer_nodes(ids: &mut [String], layer: usize, nodes: &[PromptNode], links: &[PromptLink]) {
    if layer == 0 {
        ids.sort_by_key(|id| {
            nodes
                .iter()
                .find(|n| n.id == *id)
                .map(node_semantic_rank)
                .unwrap_or((99, id.clone()))
        });
    } else {
        ids.sort_by(|a, b| {
            let bary_a = compute_node_barycenter(a, nodes, links);
            let bary_b = compute_node_barycenter(b, nodes, links);
            match (bary_a, bary_b) {
                (Some(va), Some(vb)) => va
                    .partial_cmp(&vb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.cmp(b)),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.cmp(b),
            }
        });
    }
}

fn position_column(
    nodes: &mut [PromptNode],
    x: f32,
    ids: &[String],
    mut y: f32,
    heights: &HashMap<String, f32>,
) -> f32 {
    for id in ids {
        if let Some(node) = nodes.iter_mut().find(|node| node.id == *id) {
            node.position = [x, y];
            y += heights
                .get(id)
                .copied()
                .unwrap_or_else(|| node.layout_height())
                + AUTO_LAYOUT_VERTICAL_GAP;
        }
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_nodes_stay_on_their_page_and_shared_nodes_connect_to_both() {
        let mut graph = PromptNodeGraph::empty();
        let shared = graph.add_variable(
            PromptNodePage::Shared,
            PromptVariable::CurrentInput,
            [0.0, 0.0],
        );
        let openai =
            graph.add_compose(PromptNodePage::OpenAiCompatible, "{0}".into(), [300.0, 0.0]);
        let hunyuan = graph.add_compose(PromptNodePage::Hunyuan, "{0}".into(), [300.0, 200.0]);
        let shared_target = graph.add_compose(PromptNodePage::Shared, "{0}".into(), [300.0, 400.0]);

        assert!(graph.connect(&shared, &openai, 0));
        assert!(graph.connect(&shared, &hunyuan, 0));
        assert!(!graph.connect(&openai, &hunyuan, 0));
        assert!(!graph.connect(&openai, &shared_target, 0));
    }

    #[test]
    fn independent_translation_context_accepts_compose_and_input_nodes() {
        let mut graph = PromptNodeGraph::builtin_default();
        let custom_compose = graph.add_compose(
            PromptNodePage::OpenAiCompatible,
            "Custom context: {0}".into(),
            [100.0, 100.0],
        );
        assert!(graph.connect(&custom_compose, "openai-reference-context", 0));

        let hunyuan_compose = graph.add_compose(
            PromptNodePage::Hunyuan,
            "Hunyuan specific context: {0}".into(),
            [200.0, 200.0],
        );
        assert!(graph.connect(&hunyuan_compose, "hunyuan-reference-context", 0));
    }

    #[test]
    fn request_serialization_names_the_node_by_its_actual_role() {
        let graph = PromptNodeGraph::builtin_default();
        let request = graph
            .nodes
            .iter()
            .find(|node| node.id == "openai-request")
            .unwrap();
        let value = serde_json::to_value(request).unwrap();

        assert_eq!(value["kind"]["type"], "request");
        assert_eq!(value["kind"]["roles"][0], "system");
        assert_eq!(value["kind"]["roles"][1], "user");
    }

    #[test]
    fn links_serialize_only_data_flow() {
        let graph = PromptNodeGraph::builtin_default();
        let value = serde_json::to_value(graph).unwrap();

        for link in value["links"].as_array().unwrap() {
            assert!(link.get("from").is_some());
            assert!(link.get("to").is_some());
            assert!(link.get("input").is_some());
            assert!(link.get("newline").is_none());
        }
    }

    #[test]
    fn graph_fingerprint_is_stable_and_covers_graph_content() {
        let graph = PromptNodeGraph::builtin_default();
        let mut changed = graph.clone();
        let PromptNodeKind::Compose { text } = &mut changed
            .nodes
            .iter_mut()
            .find(|node| node.id == "openai-reference-explicit-rules-ordinary")
            .unwrap()
            .kind
        else {
            panic!("expected compose node");
        };
        text.push('!');

        assert_eq!(graph.fingerprint(), graph.clone().fingerprint());
        assert_ne!(graph.fingerprint(), changed.fingerprint());
    }

    #[test]
    fn compose_inputs_grow_with_one_spare_until_ten_are_connected() {
        let mut graph = PromptNodeGraph::empty();
        let source = graph.add_variable(
            PromptNodePage::Shared,
            PromptVariable::CurrentInput,
            [0.0, 0.0],
        );
        let compose = graph.add_compose(PromptNodePage::Shared, "Instruction".into(), [300.0, 0.0]);

        assert_eq!(graph.compose_input_socket_indexes(&compose), vec![0]);
        for input in 0..=PromptNodeGraph::MAX_COMPOSE_INPUT_INDEX {
            assert!(graph.connect(&source, &compose, input));
            let sockets = graph.compose_input_socket_indexes(&compose);
            let expected_count = usize::from(input + 1)
                + usize::from(input < PromptNodeGraph::MAX_COMPOSE_INPUT_INDEX);
            assert_eq!(sockets.len(), expected_count);
        }
        assert!(!graph.connect(&source, &compose, 10));
        assert_eq!(
            graph.compose_input_socket_indexes(&compose),
            (0..=PromptNodeGraph::MAX_COMPOSE_INPUT_INDEX).collect::<Vec<_>>()
        );
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == compose)
                .and_then(|node| match &node.kind {
                    PromptNodeKind::Compose { text } => Some(text.as_str()),
                    _ => None,
                }),
            Some(
                "Instruction\n\n{0}\n\n{1}\n\n{2}\n\n{3}\n\n{4}\n\n{5}\n\n{6}\n\n{7}\n\n{8}\n\n{9}"
            )
        );
    }

    #[test]
    fn an_unconnected_declared_compose_input_is_the_spare() {
        let mut graph = PromptNodeGraph::empty();
        let source = graph.add_variable(
            PromptNodePage::Shared,
            PromptVariable::CurrentInput,
            [0.0, 0.0],
        );
        let compose = graph.add_compose(PromptNodePage::Shared, "{0}".into(), [300.0, 0.0]);

        assert_eq!(graph.compose_input_socket_indexes(&compose), vec![0]);
        assert!(graph.connect(&source, &compose, 0));
        assert_eq!(graph.compose_input_socket_indexes(&compose), vec![0, 1]);
        graph.links.clear();
        assert_eq!(graph.compose_input_socket_indexes(&compose), vec![0]);
    }

    #[test]
    fn validation_rejects_a_compose_link_without_a_text_placeholder() {
        let mut graph = PromptNodeGraph::builtin_default();
        graph.links.push(PromptLink {
            from: "openai-current-input".into(),
            to: "openai-reference-context".into(),
            input: 2,
        });

        assert_eq!(
            graph.validate_for_activation().unwrap_err().to_string(),
            "prompt link uses an invalid socket"
        );
    }

    #[test]
    fn auto_layout_orders_layers_by_barycenter_without_line_crossings() {
        let mut graph = PromptNodeGraph::empty();
        // Top variable
        let top_var = graph.add_variable(
            PromptNodePage::Shared,
            PromptVariable::SourceLanguage,
            [0.0, 0.0],
        );
        // Bottom variable
        let btm_var = graph.add_variable(
            PromptNodePage::Shared,
            PromptVariable::CurrentInput,
            [0.0, 0.0],
        );

        // Target nodes in Layer 1: intentionally add bottom target first, then top target
        let btm_target = graph.add_compose(PromptNodePage::Shared, "Btm: {0}".into(), [0.0, 0.0]);
        let top_target = graph.add_compose(PromptNodePage::Shared, "Top: {0}".into(), [0.0, 0.0]);

        // Connect top_var -> top_target, and btm_var -> btm_target
        assert!(graph.connect(&top_var, &top_target, 0));
        assert!(graph.connect(&btm_var, &btm_target, 0));

        // Perform auto_layout
        graph.auto_layout();

        let top_target_node = graph.nodes.iter().find(|n| n.id == top_target).unwrap();
        let btm_target_node = graph.nodes.iter().find(|n| n.id == btm_target).unwrap();

        // Top target must be placed above bottom target (smaller Y) to prevent edge crossing
        assert!(top_target_node.position[1] < btm_target_node.position[1]);
    }

    #[test]
    fn typed_ports_reject_condition_text_mismatches() {
        let mut graph = PromptNodeGraph::empty();
        let condition = graph.add_condition_value(
            PromptNodePage::Shared,
            PromptCondition::SourceIsAuto,
            [0.0, 0.0],
        );
        let text = graph.add_system_value(
            PromptNodePage::Shared,
            PromptSystemValue::CurrentInput,
            [0.0, 100.0],
        );
        let compose = graph.add_compose(PromptNodePage::Shared, "{0}".into(), [300.0, 0.0]);
        let switch = graph.add_conditional_switch(PromptNodePage::Shared, [300.0, 100.0]);

        assert!(!graph.connect(&condition, &compose, 0));
        assert!(!graph.connect(&text, &switch, 0));
        assert!(graph.connect(&condition, &switch, 0));
        assert!(graph.connect(&text, &switch, 1));
    }

    #[test]
    fn text_switch_derives_finite_outputs_and_rebinds_cases_by_name() {
        assert!(matches!(
            serde_json::from_value::<PromptNodeKind>(serde_json::json!({
                "type": "text_switch",
                "cases": ["legacy-copy"]
            }))
            .unwrap(),
            PromptNodeKind::TextSwitch
        ));
        assert_eq!(
            serde_json::to_value(PromptNodeKind::TextSwitch).unwrap(),
            serde_json::json!({ "type": "text_switch" })
        );
        let mut graph = PromptNodeGraph::empty();
        let mode = graph.add_system_value(
            PromptNodePage::Shared,
            PromptSystemValue::RecognitionMode,
            [0.0, 0.0],
        );
        let first = graph.add_input(
            PromptNodePage::Shared,
            TranslationPromptBlock::CustomText { text: "a".into() },
            [0.0, 0.0],
        );
        let second = graph.add_input(
            PromptNodePage::Shared,
            TranslationPromptBlock::CustomText { text: "b".into() },
            [0.0, 0.0],
        );
        let selector = graph.add_text_switch(PromptNodePage::Shared, [0.0, 0.0]);
        assert!(graph.connect(&mode, &selector, 0));
        assert!(graph.connect(&first, &selector, 1));
        assert!(graph.connect(&second, &selector, 2));

        let output_a = graph.add_input(
            PromptNodePage::Shared,
            TranslationPromptBlock::CustomText { text: "A".into() },
            [0.0, 0.0],
        );
        let output_b = graph.add_input(
            PromptNodePage::Shared,
            TranslationPromptBlock::CustomText { text: "B".into() },
            [0.0, 0.0],
        );
        let downstream = graph.add_text_switch(PromptNodePage::Shared, [0.0, 0.0]);
        assert!(graph.connect(&selector, &downstream, 0));
        assert_eq!(graph.text_switch_cases(&downstream).unwrap(), ["a", "b"]);
        assert!(graph.connect(&output_a, &downstream, 1));
        assert!(graph.connect(&output_b, &downstream, 2));

        let previous = graph.clone();
        for node in &mut graph.nodes {
            if node.id == first {
                node.kind = PromptNodeKind::Input {
                    block: TranslationPromptBlock::CustomText { text: "b".into() },
                };
            } else if node.id == second {
                node.kind = PromptNodeKind::Input {
                    block: TranslationPromptBlock::CustomText { text: "a".into() },
                };
            }
        }
        graph.sync_text_switch_cases(&previous);

        assert_eq!(graph.text_switch_cases(&downstream).unwrap(), ["b", "a"]);
        assert!(
            graph
                .links
                .iter()
                .any(|link| { link.from == output_a && link.to == downstream && link.input == 2 })
        );
        assert!(
            graph
                .links
                .iter()
                .any(|link| { link.from == output_b && link.to == downstream && link.input == 1 })
        );
    }

    #[test]
    fn v6_implicit_runtime_nodes_migrate_to_explicit_typed_wires() {
        let mut graph = PromptNodeGraph {
            schema_version: 6,
            layout_version: 7,
            nodes: vec![
                PromptNode {
                    id: "source".into(),
                    label: String::new(),
                    page: PromptNodePage::Shared,
                    kind: PromptNodeKind::Variable {
                        variable: PromptVariable::CurrentInput,
                    },
                    position: [0.0, 0.0],
                },
                PromptNode {
                    id: "false".into(),
                    label: String::new(),
                    page: PromptNodePage::Shared,
                    kind: PromptNodeKind::Compose {
                        text: "false".into(),
                    },
                    position: [0.0, 100.0],
                },
                PromptNode {
                    id: "true".into(),
                    label: String::new(),
                    page: PromptNodePage::Shared,
                    kind: PromptNodeKind::Compose {
                        text: "true".into(),
                    },
                    position: [0.0, 200.0],
                },
                PromptNode {
                    id: "switch".into(),
                    label: String::new(),
                    page: PromptNodePage::Shared,
                    kind: PromptNodeKind::Switch {
                        condition: Some(PromptCondition::SourceIsAuto),
                    },
                    position: [300.0, 100.0],
                },
            ],
            links: vec![
                PromptLink {
                    from: "false".into(),
                    to: "switch".into(),
                    input: 0,
                },
                PromptLink {
                    from: "true".into(),
                    to: "switch".into(),
                    input: 1,
                },
            ],
        };

        graph.migrate_legacy_dataflow();

        assert_eq!(
            graph.schema_version,
            PromptNodeGraph::CURRENT_SCHEMA_VERSION
        );
        assert_eq!(graph.layout_version, 0);
        assert!(graph.nodes.iter().any(|node| matches!(
            node.kind,
            PromptNodeKind::SystemValue {
                value: PromptSystemValue::CurrentInput
            }
        )));
        assert!(graph.nodes.iter().any(|node| matches!(
            node.kind,
            PromptNodeKind::ConditionValue {
                condition: PromptCondition::SourceIsAuto
            }
        )));
        assert!(
            graph
                .links
                .iter()
                .any(|link| link.to == "switch" && link.input == 0)
        );
        assert!(
            graph
                .links
                .iter()
                .any(|link| { link.from == "false" && link.to == "switch" && link.input == 1 })
        );
        assert!(
            graph
                .links
                .iter()
                .any(|link| { link.from == "true" && link.to == "switch" && link.input == 2 })
        );
    }

    #[test]
    fn auto_layout_uses_shared_vertical_spacing_contract() {
        let mut graph = PromptNodeGraph::empty();
        let first = graph.add_system_value(
            PromptNodePage::Shared,
            PromptSystemValue::SourceLanguage,
            [0.0, 0.0],
        );
        let second = graph.add_system_value(
            PromptNodePage::Shared,
            PromptSystemValue::TargetLanguage,
            [0.0, 0.0],
        );
        graph.auto_layout();
        let first = graph.nodes.iter().find(|node| node.id == first).unwrap();
        let second = graph.nodes.iter().find(|node| node.id == second).unwrap();
        let vertical_distance = (second.position[1] - first.position[1]).abs();
        assert_eq!(
            vertical_distance,
            first.layout_height() + AUTO_LAYOUT_VERTICAL_GAP
        );
    }
}
