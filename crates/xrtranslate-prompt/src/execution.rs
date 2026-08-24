use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::context::render_block;
use crate::{
    AsrPromptContext, PromptCondition, PromptGraphError, PromptMessageRole, PromptMode,
    PromptNodeGraph, PromptNodeKind, PromptProviderTarget, PromptSystemValue, PromptTextComparison,
    PromptVariable, TranslationPromptContext,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptMessage {
    pub role: PromptMessageRole,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRender {
    pub messages: Vec<PromptMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptExecution {
    pub render: PromptRender,
    pub trace: PromptExecutionTrace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptExecutionTrace {
    pub target: PromptProviderTarget,
    #[serde(default)]
    pub graph_fingerprint: String,
    pub nodes: Vec<PromptNodeTrace>,
}

impl PromptExecutionTrace {
    pub fn node(&self, id: &str) -> Option<&PromptNodeTrace> {
        self.nodes.iter().find(|node| node.node_id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptNodeTrace {
    pub node_id: String,
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_input: Option<u8>,
}

struct ExecutionContext<'a> {
    source_text: &'a str,
    source_language: &'a str,
    target_language: &'a str,
    recognition_context: &'a str,
    has_recognition_context: bool,
    mode: PromptMode,
    reference: &'a TranslationPromptContext,
}

#[derive(Clone)]
enum ExecutionValue {
    Text(String),
    Condition(bool),
}

impl ExecutionValue {
    fn trace_output(&self) -> String {
        match self {
            Self::Text(value) => value.clone(),
            Self::Condition(value) => value.to_string(),
        }
    }
}

fn render_system_value(value: PromptSystemValue, context: &ExecutionContext<'_>) -> Option<String> {
    Some(match value {
        PromptSystemValue::SourceLanguage => context.source_language.to_owned(),
        PromptSystemValue::TargetLanguage => context.target_language.to_owned(),
        PromptSystemValue::CurrentInput => context.source_text.to_owned(),
        PromptSystemValue::RecognitionContext => context.recognition_context.to_owned(),
        PromptSystemValue::RecognitionMode => match context.mode {
            PromptMode::Ordinary => "ordinary".into(),
            PromptMode::PseudoStreaming => "pseudo_streaming".into(),
        },
        PromptSystemValue::LanguageOrder => {
            return render_block(
                &crate::TranslationPromptBlock::LanguageOrder,
                context.reference,
            );
        }
        PromptSystemValue::Terminology => {
            return render_block(
                &crate::TranslationPromptBlock::Terminology,
                context.reference,
            );
        }
        PromptSystemValue::RecentTurns { limit } => {
            return render_block(
                &crate::TranslationPromptBlock::RecentTurns { limit },
                context.reference,
            );
        }
        PromptSystemValue::PreviousRevision => {
            return render_block(
                &crate::TranslationPromptBlock::PreviousRevision,
                context.reference,
            );
        }
        PromptSystemValue::SurroundingSource => {
            return render_block(
                &crate::TranslationPromptBlock::SurroundingSource,
                context.reference,
            );
        }
    })
}

fn evaluate_condition(condition: PromptCondition, context: &ExecutionContext<'_>) -> bool {
    match condition {
        PromptCondition::SourceIsAuto => context.source_language == "auto",
        PromptCondition::HasReferenceContext => context.reference.has_reference_context(),
        PromptCondition::HasRecognitionContext => context.has_recognition_context,
        PromptCondition::IsPseudoStreaming => context.mode == PromptMode::PseudoStreaming,
    }
}

fn compare_text(
    value: &str,
    expected: &str,
    operator: PromptTextComparison,
    case_sensitive: bool,
) -> bool {
    let (value, expected) = if case_sensitive {
        (value.to_owned(), expected.to_owned())
    } else {
        (value.to_lowercase(), expected.to_lowercase())
    };
    match operator {
        PromptTextComparison::Equals => value == expected,
        PromptTextComparison::NotEquals => value != expected,
        PromptTextComparison::Contains => value.contains(&expected),
        PromptTextComparison::StartsWith => value.starts_with(&expected),
        PromptTextComparison::EndsWith => value.ends_with(&expected),
    }
}

impl PromptNodeGraph {
    pub fn render(
        &self,
        target: PromptProviderTarget,
        source_text: &str,
        source_language: &str,
        target_language: &str,
        reference: &TranslationPromptContext,
    ) -> Result<PromptRender, PromptGraphError> {
        self.render_with_trace(
            target,
            source_text,
            source_language,
            target_language,
            reference,
        )
        .map(|execution| execution.render)
    }

    pub fn render_with_trace(
        &self,
        target: PromptProviderTarget,
        source_text: &str,
        source_language: &str,
        target_language: &str,
        reference: &TranslationPromptContext,
    ) -> Result<PromptExecution, PromptGraphError> {
        self.render_with_trace_internal(
            target,
            source_text,
            source_language,
            target_language,
            "",
            false,
            reference,
            true,
        )
    }

    /// Renders one ASR prompt delivery path. Recognition vocabulary is kept
    /// distinct from post-ASR translation input and may be empty while the
    /// graph's fixed recognition instruction remains useful.
    pub fn render_asr_with_trace(
        &self,
        target: PromptProviderTarget,
        source_language: &str,
        expected_languages: &str,
        context: &AsrPromptContext,
    ) -> Result<PromptExecution, PromptGraphError> {
        if !matches!(
            target,
            PromptProviderTarget::AsrInstruction | PromptProviderTarget::AsrContextBias
        ) {
            return Err(PromptGraphError::new(
                "ASR rendering requires an ASR provider target",
            ));
        }
        let recognition_context = context.recognition_context_text();
        let reference = TranslationPromptContext {
            mode: context.mode,
            ..TranslationPromptContext::default()
        };
        self.render_with_trace_internal(
            target,
            "",
            source_language,
            expected_languages,
            &recognition_context,
            context.has_recognition_context(),
            &reference,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn render_with_trace_internal(
        &self,
        target: PromptProviderTarget,
        source_text: &str,
        source_language: &str,
        target_language: &str,
        recognition_context: &str,
        has_recognition_context: bool,
        reference: &TranslationPromptContext,
        require_source_text: bool,
    ) -> Result<PromptExecution, PromptGraphError> {
        self.validate_for_activation()?;
        if require_source_text && source_text.trim().is_empty() {
            return Err(PromptGraphError::new("source text cannot be empty"));
        }
        if source_language.trim().is_empty() {
            return Err(PromptGraphError::new("source language cannot be empty"));
        }
        if target_language.trim().is_empty() {
            return Err(PromptGraphError::new("target language cannot be empty"));
        }

        let context = ExecutionContext {
            source_text: source_text.trim(),
            source_language: source_language.trim(),
            target_language: target_language.trim(),
            recognition_context: recognition_context.trim(),
            has_recognition_context,
            mode: reference.mode,
            reference,
        };
        let request = self
            .nodes
            .iter()
            .find(|node| {
                matches!(node.kind, PromptNodeKind::Request { target: value, .. } if value == target)
            })
            .ok_or_else(|| PromptGraphError::new("provider request is missing"))?;
        let PromptNodeKind::Request { roles, .. } = &request.kind else {
            unreachable!();
        };

        let mut cache = HashMap::new();
        let mut traced = HashMap::new();
        let mut messages = Vec::new();
        for (input, role) in roles.iter().copied().enumerate() {
            let link = self
                .links
                .iter()
                .find(|link| link.to == request.id && usize::from(link.input) == input)
                .ok_or_else(|| PromptGraphError::new("provider request input is not connected"))?;
            let mut visiting = HashSet::new();
            let Some(ExecutionValue::Text(content)) =
                self.evaluate(&link.from, &context, &mut cache, &mut traced, &mut visiting)
            else {
                return Err(PromptGraphError::new(format!(
                    "provider request {} message {input} has no content",
                    request.id
                )));
            };
            if content.trim().is_empty() {
                return Err(PromptGraphError::new(format!(
                    "provider request {} message {input} is empty",
                    request.id
                )));
            }
            messages.push(PromptMessage { role, content });
        }
        traced.insert(
            request.id.clone(),
            PromptNodeTrace {
                node_id: request.id.clone(),
                output: messages
                    .iter()
                    .map(|message| format!("{:?}:\n{}", message.role, message.content))
                    .collect::<Vec<_>>()
                    .join("\n\n"),
                selected_input: None,
            },
        );
        let nodes = self
            .nodes
            .iter()
            .filter_map(|node| traced.remove(&node.id))
            .collect();
        Ok(PromptExecution {
            render: PromptRender { messages },
            trace: PromptExecutionTrace {
                target,
                graph_fingerprint: self.fingerprint(),
                nodes,
            },
        })
    }

    pub fn compose_preview(&self, target: PromptProviderTarget) -> Option<PromptRender> {
        let request = self
            .nodes
            .iter()
            .find(|node| {
                matches!(node.kind, PromptNodeKind::Request { target: value, .. } if value == target)
            })?;
        let PromptNodeKind::Request { roles, .. } = &request.kind else {
            return None;
        };
        let mut cache = HashMap::new();
        let messages = roles
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(input, role)| {
                let link = self
                    .links
                    .iter()
                    .find(|link| link.to == request.id && usize::from(link.input) == input)?;
                let mut visiting = HashSet::new();
                self.evaluate_preview(&link.from, &mut cache, &mut visiting)
                    .map(|content| PromptMessage { role, content })
            })
            .collect::<Vec<_>>();
        (!messages.is_empty()).then_some(PromptRender { messages })
    }

    pub fn compose_request_preview(&self, request_id: &str) -> Option<String> {
        let request = self.nodes.iter().find(|node| node.id == request_id)?;
        let PromptNodeKind::Request { roles, .. } = &request.kind else {
            return None;
        };
        let mut cache = HashMap::new();
        let messages = roles
            .iter()
            .copied()
            .enumerate()
            .filter_map(|(input, role)| {
                let link = self
                    .links
                    .iter()
                    .find(|link| link.to == request.id && usize::from(link.input) == input)?;
                let mut visiting = HashSet::new();
                self.evaluate_preview(&link.from, &mut cache, &mut visiting)
                    .map(|content| format!("{role:?}:\n{content}"))
            })
            .collect::<Vec<_>>();
        (!messages.is_empty()).then(|| messages.join("\n\n"))
    }

    fn evaluate(
        &self,
        id: &str,
        context: &ExecutionContext<'_>,
        cache: &mut HashMap<String, Option<ExecutionValue>>,
        traced: &mut HashMap<String, PromptNodeTrace>,
        visiting: &mut HashSet<String>,
    ) -> Option<ExecutionValue> {
        if let Some(value) = cache.get(id) {
            return value.clone();
        }
        if !visiting.insert(id.into()) {
            return None;
        }
        let node = self.nodes.iter().find(|node| node.id == id)?;
        let mut selected_input = None;
        let value = match &node.kind {
            PromptNodeKind::Input { block } => {
                render_block(block, context.reference).map(ExecutionValue::Text)
            }
            PromptNodeKind::Variable { variable } => Some(ExecutionValue::Text(match variable {
                PromptVariable::SourceLanguage => context.source_language.to_owned(),
                PromptVariable::TargetLanguage => context.target_language.to_owned(),
                PromptVariable::CurrentInput => context.source_text.to_owned(),
                PromptVariable::RecognitionContext => context.recognition_context.to_owned(),
                PromptVariable::RecognitionMode => match context.mode {
                    PromptMode::Ordinary => "ordinary",
                    PromptMode::PseudoStreaming => "pseudo_streaming",
                }
                .to_owned(),
            })),
            PromptNodeKind::SystemValue { value } => {
                render_system_value(*value, context).map(ExecutionValue::Text)
            }
            PromptNodeKind::ConditionValue { condition } => Some(ExecutionValue::Condition(
                evaluate_condition(*condition, context),
            )),
            PromptNodeKind::BoolValue { value } => Some(ExecutionValue::Condition(*value)),
            PromptNodeKind::TextComparison {
                operator,
                expected,
                case_sensitive,
            } => {
                let input = self
                    .links
                    .iter()
                    .find(|link| link.to == id && link.input == 0)
                    .and_then(|link| self.evaluate(&link.from, context, cache, traced, visiting));
                match input {
                    Some(ExecutionValue::Text(value)) => Some(ExecutionValue::Condition(
                        compare_text(&value, expected, *operator, *case_sensitive),
                    )),
                    _ => None,
                }
            }
            PromptNodeKind::Compose { text } => {
                crate::template::render_compose_text(text, |input| {
                    let link = self
                        .links
                        .iter()
                        .find(|link| link.to == id && link.input == input)?;
                    match self.evaluate(&link.from, context, cache, traced, visiting) {
                        Some(ExecutionValue::Text(value)) => Some(value),
                        None => Some(String::new()),
                        Some(ExecutionValue::Condition(_)) => None,
                    }
                })
                .ok()
                .filter(|value| !value.is_empty())
                .map(ExecutionValue::Text)
            }
            PromptNodeKind::Switch { .. } => {
                let condition_link = self
                    .links
                    .iter()
                    .find(|link| link.to == id && link.input == 0)?;
                let ExecutionValue::Condition(condition) =
                    self.evaluate(&condition_link.from, context, cache, traced, visiting)?
                else {
                    return None;
                };
                let input = if condition { 2 } else { 1 };
                selected_input = Some(input);
                self.links
                    .iter()
                    .find(|link| link.to == id && link.input == input)
                    .and_then(|link| self.evaluate(&link.from, context, cache, traced, visiting))
            }
            PromptNodeKind::TextSwitch => {
                let selector_link = self
                    .links
                    .iter()
                    .find(|link| link.to == id && link.input == 0)?;
                let ExecutionValue::Text(selector) =
                    self.evaluate(&selector_link.from, context, cache, traced, visiting)?
                else {
                    return None;
                };
                let cases = self.text_switch_cases(id)?;
                let input = cases.iter().position(|case| case == &selector)? as u8 + 1;
                selected_input = Some(input);
                self.links
                    .iter()
                    .find(|link| link.to == id && link.input == input)
                    .and_then(|link| self.evaluate(&link.from, context, cache, traced, visiting))
            }
            PromptNodeKind::Request { .. } => None,
        };
        visiting.remove(id);
        cache.insert(id.into(), value.clone());
        traced.insert(
            id.into(),
            PromptNodeTrace {
                node_id: id.into(),
                output: value
                    .as_ref()
                    .map(ExecutionValue::trace_output)
                    .unwrap_or_default(),
                selected_input,
            },
        );
        value
    }

    fn evaluate_preview(
        &self,
        id: &str,
        cache: &mut HashMap<String, Option<String>>,
        visiting: &mut HashSet<String>,
    ) -> Option<String> {
        if let Some(value) = cache.get(id) {
            return value.clone();
        }
        if !visiting.insert(id.into()) {
            return None;
        }
        let node = self.nodes.iter().find(|node| node.id == id)?;
        let value = match &node.kind {
            PromptNodeKind::Input { block } => Some(format!("[{}]", block.preview_name())),
            PromptNodeKind::Variable { variable } => Some(format!("[{variable:?}]")),
            PromptNodeKind::SystemValue { value } => Some(format!("[SYSTEM: {value:?}]")),
            PromptNodeKind::ConditionValue { condition } => {
                Some(format!("[CONDITION: {condition:?}]"))
            }
            PromptNodeKind::BoolValue { value } => Some(value.to_string()),
            PromptNodeKind::TextComparison {
                operator, expected, ..
            } => Some(format!("[TEXT {operator:?} {expected:?}]")),
            PromptNodeKind::Compose { text } => {
                crate::template::render_compose_text(text, |input| {
                    let Some(link) = self
                        .links
                        .iter()
                        .find(|link| link.to == id && link.input == input)
                    else {
                        return Some(format!("[Input {input}: unconnected]"));
                    };
                    Some(
                        self.evaluate_preview(&link.from, cache, visiting)
                            .unwrap_or_default(),
                    )
                })
                .ok()
                .filter(|value| !value.is_empty())
            }
            PromptNodeKind::Switch { .. } => {
                let condition = self
                    .links
                    .iter()
                    .find(|link| link.to == id && link.input == 0)
                    .and_then(|link| self.evaluate_preview(&link.from, cache, visiting))
                    .unwrap_or_else(|| "(unconnected condition)".into());
                let mut branch = |input| {
                    self.links
                        .iter()
                        .find(|link| link.to == id && link.input == input)
                        .and_then(|link| self.evaluate_preview(&link.from, cache, visiting))
                        .unwrap_or_else(|| "(unconnected)".into())
                };
                Some(format!(
                    "IF {condition} {{ FALSE: {} | TRUE: {} }}",
                    branch(1),
                    branch(2)
                ))
            }
            PromptNodeKind::TextSwitch => {
                let selector = self
                    .links
                    .iter()
                    .find(|link| link.to == id && link.input == 0)
                    .and_then(|link| self.evaluate_preview(&link.from, cache, visiting))
                    .unwrap_or_else(|| "(unconnected text value)".into());
                let branches = self
                    .text_switch_cases(id)
                    .unwrap_or_default()
                    .iter()
                    .enumerate()
                    .map(|(index, case)| {
                        let value = self
                            .links
                            .iter()
                            .find(|link| link.to == id && usize::from(link.input) == index + 1)
                            .and_then(|link| self.evaluate_preview(&link.from, cache, visiting))
                            .unwrap_or_else(|| "(unconnected)".into());
                        format!("{case:?}: {value}")
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                Some(format!("MATCH {selector} {{ {branches} }}"))
            }
            PromptNodeKind::Request { .. } => None,
        };
        visiting.remove(id);
        cache.insert(id.into(), value.clone());
        value
    }
}
