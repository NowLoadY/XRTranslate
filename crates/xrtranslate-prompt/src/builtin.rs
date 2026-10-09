mod bilingual;

use crate::{
    PromptCondition, PromptLink, PromptMessageRole, PromptNode, PromptNodeGraph, PromptNodeKind,
    PromptNodePage, PromptProviderTarget, PromptSystemValue, PromptVariable,
    TranslationPromptBlock,
};

pub(crate) const BUILTIN_ID: &str = "builtin-default";
pub(crate) const EXPLICIT_REFERENCE_CONTEXT_INSTRUCTION: &str = concat!(
    "Use the provided context to translate the current {0} input into {1}.\n\n",
    "First understand the actual meaning of the current {0} input, then use the context to determine references, tone, speaker relationships, and intended meaning.\n\n",
    "Preserve the original meaning. Do not unnecessarily add, remove, or change the original meaning.\n\n",
    "Translate only the current {0} input. Do not translate, repeat, summarize, or explain the context. Unless explicitly requested otherwise, output only the final {1} translation."
);
pub(crate) const AUTO_REFERENCE_CONTEXT_INSTRUCTION: &str = concat!(
    "Use the provided context to translate the current input into the other language among {0}.\n\n",
    "First understand the actual meaning of the current input, then use the context to determine references, tone, speaker relationships, and intended meaning.\n\n",
    "Preserve the original meaning. Do not unnecessarily add, remove, or change the original meaning.\n\n",
    "Translate only the current input. Do not translate, repeat, summarize, or explain the context. Unless explicitly requested otherwise, output only the final translation."
);

pub(crate) const LEGACY_PSEUDO_HUNYUAN_EXPLICIT_INSTRUCTION: &str = concat!(
    "You are a real-time pseudo-stream speech translator. If the current input is already {0}, output it ",
    "unchanged; otherwise translate only this authoritative current snapshot segment into natural, fluent {0}. ",
    "The input may revise a previous live tail. Do not repeat stable prefix text, copy the previous revision, ",
    "or add content missing from the current input. Output only the translation."
);

const PSEUDO_HUNYUAN_EXPLICIT_INSTRUCTION: &str = concat!(
    "Translate the AUTHORITATIVE CURRENT SEGMENT from {0} into {1}. ",
    "The output must be in {1}; never return a corrected or paraphrased {0} sentence. ",
    "Translate the current segment completely as it is now. Do not repeat a stable prefix or add text from an ",
    "older revision. Return only the translation, without explanations."
);

impl PromptNodeGraph {
    pub fn builtin_default() -> Self {
        let ordinary = Self::builtin_ordinary();
        let pseudo_streaming = Self::builtin_pseudo_streaming();
        Self::unify_mode_graphs(ordinary, &pseudo_streaming)
    }

    pub(crate) fn builtin_ordinary() -> Self {
        let mut builder = GraphBuilder::default();
        builder.build_openai_flow();
        builder.build_hunyuan_flow();
        builder.build_asr_instruction_flow();
        builder.build_asr_context_bias_flow();
        let mut graph = builder.finish();
        graph.add_builtin_translation_style();
        graph
    }

    /// Builds the legacy pseudo-stream branch source used when constructing
    /// and migrating the unified mode-aware graph.
    pub fn builtin_pseudo_streaming() -> Self {
        let mut graph = Self::builtin_ordinary();
        let explicit_rules = concat!(
            "You are translating one segment from a pseudo-streaming speech snapshot. ",
            "The Current input is the authoritative text for this segment. It may replace an earlier live tail, ",
            "so translate the current input in full and correct any changed recognition.\n\n",
            "Previous Revision of Current Speech is only an older hypothesis of the same speech. ",
            "Use it to keep wording stable when meaning is unchanged, but never copy text or meaning that is absent ",
            "from the current input. Do not resurrect content removed from the current snapshot.\n\n",
            "Stable prefix segments may already be visible in floating subtitles or OSC. Do not repeat or translate ",
            "those segments; output only this current segment. Surrounding source, recent turns, and terminology are ",
            "reference context only and are never part of the translation payload.\n\n",
            "Translate only the current {0} input into {1}. Preserve names, numbers, terms, ",
            "and intent unless the current source clearly corrects them. Output only the final translation."
        );
        let auto_rules = concat!(
            "You are translating one segment from a pseudo-streaming speech snapshot. ",
            "The Current input is the authoritative text for this segment and can revise a previous live tail. ",
            "Infer the source language from the current input and translate it into the other configured language among {0}.\n\n",
            "Previous Revision of Current Speech is an older hypothesis only. It may guide correction and stable ",
            "wording, but it is not additional input: never copy absent text, repeat a stable prefix, or resurrect ",
            "content that disappeared from the current snapshot.\n\n",
            "Surrounding source, recent turns, and terminology are reference context only. Translate only the current ",
            "segment, preserve names, numbers, and intent, and output only the final translation."
        );
        let openai_explicit_instruction = concat!(
            "You are a real-time pseudo-stream speech translator. If the current input is already {0}, output it ",
            "unchanged; otherwise translate only this authoritative current snapshot segment into {0}. ",
            "The input may revise a previous live tail. Do not repeat stable prefix text, copy the previous revision, ",
            "or add content missing from the current input. Output only the translation."
        );
        let hunyuan_explicit_instruction = PSEUDO_HUNYUAN_EXPLICIT_INSTRUCTION;
        let auto_instruction = concat!(
            "You are a real-time pseudo-stream speech translator. The current input is authoritative and may revise ",
            "a previous live tail. Its language is one of the following: {0}. Translate only this current segment into ",
            "the OTHER language from that list. Do not repeat stable prefix text or resurrect old-revision content. ",
            "Output only the translation."
        );
        let explicit_user =
            "Source language: {0}\nAuthoritative current segment (translate only this):\n{1}";
        let auto_user = "Authoritative current segment (translate only this):\n{0}";
        let asr_explicit = concat!(
            "Transcribe the entire current audio window accurately in {0}. This window may overlap an earlier ",
            "window: preserve every word audible now, including repeated overlap, but do not infer or combine text ",
            "from any earlier window. Do not translate. Return only the transcript."
        );
        let asr_auto = concat!(
            "Transcribe the entire current audio window accurately. The spoken language is one of {0}. This window ",
            "may overlap an earlier window: preserve every word audible now, including repeated overlap, but do not ",
            "infer or combine text from any earlier window. Do not translate. Return only the transcript."
        );
        let asr_with_context = concat!(
            "{0}\n\nRecognition vocabulary (spelling aid only; include a term only when it is audible):\n{1}"
        );
        for (prefix, rules, instruction, auto_rules, auto_instruction, explicit_user, auto_user) in [
            (
                "openai",
                explicit_rules,
                openai_explicit_instruction,
                auto_rules,
                auto_instruction,
                explicit_user,
                auto_user,
            ),
            (
                "hunyuan",
                explicit_rules,
                hunyuan_explicit_instruction,
                auto_rules,
                auto_instruction,
                explicit_user,
                auto_user,
            ),
        ] {
            replace_compose_text(
                &mut graph,
                &format!("{prefix}-reference-explicit-rules"),
                rules,
            );
            replace_compose_text(
                &mut graph,
                &format!("{prefix}-reference-auto-rules"),
                auto_rules,
            );
            replace_compose_text(
                &mut graph,
                &format!("{prefix}-explicit-instruction"),
                instruction,
            );
            replace_compose_text(
                &mut graph,
                &format!("{prefix}-auto-instruction"),
                auto_instruction,
            );
            replace_compose_text(
                &mut graph,
                &format!("{prefix}-explicit-user"),
                explicit_user,
            );
            replace_compose_text(&mut graph, &format!("{prefix}-auto-user"), auto_user);
        }
        replace_compose_text(&mut graph, "asr-instruction-explicit", asr_explicit);
        replace_compose_text(&mut graph, "asr-instruction-auto", asr_auto);
        replace_compose_text(&mut graph, "asr-instruction-with-context", asr_with_context);
        graph
    }

    pub fn unify_mode_graphs(mut ordinary: Self, pseudo_streaming: &Self) -> Self {
        ensure_recognition_mode_value(&mut ordinary);
        for id in [
            "openai-reference-explicit-rules",
            "openai-reference-auto-rules",
            "openai-explicit-instruction",
            "openai-auto-instruction",
            "openai-explicit-user",
            "openai-auto-user",
            "hunyuan-reference-explicit-rules",
            "hunyuan-reference-auto-rules",
            "hunyuan-explicit-instruction",
            "hunyuan-auto-instruction",
            "hunyuan-explicit-user",
            "hunyuan-auto-user",
            "asr-instruction-explicit",
            "asr-instruction-auto",
            "asr-instruction-with-context",
        ] {
            let Some(pseudo_text) = pseudo_streaming.nodes.iter().find_map(|node| {
                (node.id == id)
                    .then_some(&node.kind)
                    .and_then(|kind| match kind {
                        PromptNodeKind::Compose { text } => Some(text.as_str()),
                        _ => None,
                    })
            }) else {
                continue;
            };
            split_compose_by_mode(&mut ordinary, id, pseudo_text);
        }
        ordinary.auto_layout();
        ordinary
    }

    /// Promotes two complete legacy mode graphs into one graph without
    /// assuming built-in node IDs. Each pseudo-streaming DAG is namespaced and
    /// selected immediately before the matching provider request.
    pub fn merge_complete_mode_graphs(mut ordinary: Self, pseudo_streaming: &Self) -> Self {
        use std::collections::{HashMap, HashSet};

        if ordinary.nodes.iter().any(|node| {
            matches!(node.kind, PromptNodeKind::TextSwitch)
                && ordinary
                    .text_switch_cases(&node.id)
                    .is_some_and(|cases| cases.iter().any(|value| value == "pseudo_streaming"))
        }) {
            return ordinary;
        }
        let recognition_mode_value = ensure_recognition_mode_value(&mut ordinary);

        let mut used_ids = ordinary
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        let mut pseudo_ids = HashMap::new();
        for node in pseudo_streaming
            .nodes
            .iter()
            .filter(|node| !matches!(node.kind, PromptNodeKind::Request { .. }))
        {
            let id = unique_mode_node_id(&mut used_ids, &format!("{}-pseudo-streaming", node.id));
            let mut cloned = node.clone();
            cloned.id = id.clone();
            cloned.label = format!("{} / PSEUDO-STREAMING", cloned.label);
            pseudo_ids.insert(node.id.clone(), id);
            ordinary.nodes.push(cloned);
        }

        for link in &pseudo_streaming.links {
            let (Some(from), Some(to)) = (pseudo_ids.get(&link.from), pseudo_ids.get(&link.to))
            else {
                continue;
            };
            ordinary.links.push(PromptLink {
                from: from.clone(),
                to: to.clone(),
                input: link.input,
            });
        }

        let requests = ordinary
            .nodes
            .iter()
            .filter_map(|node| match &node.kind {
                PromptNodeKind::Request { target, roles } => {
                    Some((node.id.clone(), node.page, *target, roles.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for (request_id, page, target, roles) in requests {
            let Some((pseudo_request_id, pseudo_roles)) =
                pseudo_streaming
                    .nodes
                    .iter()
                    .find_map(|node| match &node.kind {
                        PromptNodeKind::Request {
                            target: pseudo_target,
                            roles,
                        } if *pseudo_target == target => Some((node.id.as_str(), roles)),
                        _ => None,
                    })
            else {
                continue;
            };
            if roles != *pseudo_roles {
                continue;
            }

            for input in 0..roles.len() as u8 {
                let ordinary_source = ordinary
                    .links
                    .iter()
                    .find(|link| link.to == request_id && link.input == input)
                    .map(|link| link.from.clone());
                let pseudo_source = pseudo_streaming
                    .links
                    .iter()
                    .find(|link| link.to == pseudo_request_id && link.input == input)
                    .and_then(|link| pseudo_ids.get(&link.from))
                    .cloned();
                let (Some(ordinary_source), Some(pseudo_source)) = (ordinary_source, pseudo_source)
                else {
                    continue;
                };

                ordinary
                    .links
                    .retain(|link| !(link.to == request_id && link.input == input));
                let switch_id = unique_mode_node_id(
                    &mut used_ids,
                    &format!("{request_id}-recognition-mode-{input}"),
                );
                ordinary.nodes.push(PromptNode {
                    id: switch_id.clone(),
                    label: "SELECT RECOGNITION MODE".into(),
                    page,
                    kind: PromptNodeKind::TextSwitch,
                    position: [0.0, 0.0],
                });
                ordinary.links.extend([
                    PromptLink {
                        from: recognition_mode_value.clone(),
                        to: switch_id.clone(),
                        input: 0,
                    },
                    PromptLink {
                        from: ordinary_source,
                        to: switch_id.clone(),
                        input: 1,
                    },
                    PromptLink {
                        from: pseudo_source,
                        to: switch_id.clone(),
                        input: 2,
                    },
                    PromptLink {
                        from: switch_id,
                        to: request_id.clone(),
                        input,
                    },
                ]);
            }
        }

        ordinary.auto_layout();
        ordinary
    }

    pub fn replace_provider_pages(&mut self, source: &Self, pages: &[PromptNodePage]) {
        use std::collections::{HashMap, HashSet};

        let removed_ids = self
            .nodes
            .iter()
            .filter(|node| pages.contains(&node.page))
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();
        self.nodes.retain(|node| !pages.contains(&node.page));
        self.links
            .retain(|link| !removed_ids.contains(&link.from) && !removed_ids.contains(&link.to));
        let mut used_ids = self
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<HashSet<_>>();

        for &page in pages {
            let mut selected = source
                .nodes
                .iter()
                .filter(|node| node.page == page)
                .map(|node| node.id.clone())
                .collect::<HashSet<_>>();
            let mut pending = selected.iter().cloned().collect::<Vec<_>>();
            while let Some(to) = pending.pop() {
                for link in source.links.iter().filter(|link| link.to == to) {
                    let Some(node) = source.nodes.iter().find(|node| node.id == link.from) else {
                        continue;
                    };
                    if matches!(node.page, PromptNodePage::Shared)
                        && selected.insert(node.id.clone())
                    {
                        pending.push(node.id.clone());
                    }
                }
            }

            let mut ids = HashMap::new();
            for node in source
                .nodes
                .iter()
                .filter(|node| selected.contains(&node.id))
            {
                let preferred = if node.page == PromptNodePage::Shared {
                    format!("{}-{}", page_id(page), node.id)
                } else {
                    node.id.clone()
                };
                let id = unique_mode_node_id(&mut used_ids, &preferred);
                let mut cloned = node.clone();
                cloned.id = id.clone();
                cloned.page = page;
                ids.insert(node.id.clone(), id);
                self.nodes.push(cloned);
            }
            self.links.extend(source.links.iter().filter_map(|link| {
                Some(PromptLink {
                    from: ids.get(&link.from)?.clone(),
                    to: ids.get(&link.to)?.clone(),
                    input: link.input,
                })
            }));
        }
    }

    /// Upgrades the shipped pseudo-streaming Hunyuan prompt whose target-only
    /// placeholder was historically connected to the source-language socket.
    /// User-authored prompt text is left untouched.
    pub(crate) fn upgrade_known_pseudo_streaming_prompts(&mut self) {
        let id = "hunyuan-explicit-instruction-pseudo-streaming";
        let should_upgrade = self.nodes.iter().any(|node| {
            node.id == id
                && matches!(
                    &node.kind,
                    PromptNodeKind::Compose { text }
                        if text == LEGACY_PSEUDO_HUNYUAN_EXPLICIT_INSTRUCTION
                )
        });
        if !should_upgrade {
            return;
        }

        replace_compose_text(self, id, PSEUDO_HUNYUAN_EXPLICIT_INSTRUCTION);
        self.links.retain(|link| link.to != id);
        self.links.extend([
            PromptLink {
                from: "hunyuan-source-language".into(),
                to: id.into(),
                input: 0,
            },
            PromptLink {
                from: "hunyuan-target-language".into(),
                to: id.into(),
                input: 1,
            },
        ]);
    }
}

fn unique_mode_node_id(used: &mut std::collections::HashSet<String>, preferred: &str) -> String {
    if used.insert(preferred.to_owned()) {
        return preferred.to_owned();
    }
    for suffix in 2_u32.. {
        let candidate = format!("{preferred}-{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn ensure_recognition_mode_value(graph: &mut PromptNodeGraph) -> String {
    const MODE_ID: &str = "shared-recognition-mode";
    if !graph.nodes.iter().any(|node| node.id == MODE_ID) {
        graph.nodes.push(PromptNode {
            id: MODE_ID.into(),
            label: "RECOGNITION MODE".into(),
            page: PromptNodePage::Shared,
            kind: PromptNodeKind::SystemValue {
                value: PromptSystemValue::RecognitionMode,
            },
            position: [0.0, 0.0],
        });
    }
    MODE_ID.into()
}

fn page_id(page: PromptNodePage) -> &'static str {
    match page {
        PromptNodePage::Shared => "shared",
        PromptNodePage::OpenAiCompatible => "openai",
        PromptNodePage::Hunyuan => "hunyuan",
        PromptNodePage::AsrInstruction => "asr-instruction",
        PromptNodePage::AsrContextBias => "asr-context-bias",
    }
}

fn split_compose_by_mode(graph: &mut PromptNodeGraph, id: &str, pseudo_text: &str) {
    let Some(index) = graph.nodes.iter().position(|node| node.id == id) else {
        return;
    };
    let PromptNodeKind::Compose {
        text: ordinary_text,
    } = &graph.nodes[index].kind
    else {
        return;
    };
    if ordinary_text == pseudo_text {
        return;
    }

    let page = graph.nodes[index].page;
    let mut ordinary = graph.nodes[index].clone();
    ordinary.id = format!("{id}-ordinary");
    ordinary.label = format!("{} / ORDINARY", ordinary.label);
    let mut pseudo = ordinary.clone();
    pseudo.id = format!("{id}-pseudo-streaming");
    pseudo.label = ordinary.label.replace(" / ORDINARY", " / PSEUDO-STREAMING");
    pseudo.kind = PromptNodeKind::Compose {
        text: pseudo_text.into(),
    };
    graph.nodes[index] = PromptNode {
        id: id.into(),
        label: "SELECT RECOGNITION MODE".into(),
        page,
        kind: PromptNodeKind::TextSwitch,
        position: [0.0, 0.0],
    };

    let incoming = graph
        .links
        .iter()
        .filter(|link| link.to == id)
        .cloned()
        .collect::<Vec<_>>();
    graph.links.retain(|link| link.to != id);
    for link in incoming {
        graph.links.push(PromptLink {
            from: link.from.clone(),
            to: ordinary.id.clone(),
            input: link.input,
        });
        graph.links.push(PromptLink {
            from: link.from,
            to: pseudo.id.clone(),
            input: link.input,
        });
    }
    let condition_id = ensure_recognition_mode_value(graph);
    graph.links.push(PromptLink {
        from: condition_id,
        to: id.into(),
        input: 0,
    });
    graph.links.push(PromptLink {
        from: ordinary.id.clone(),
        to: id.into(),
        input: 1,
    });
    graph.links.push(PromptLink {
        from: pseudo.id.clone(),
        to: id.into(),
        input: 2,
    });
    graph.nodes.push(ordinary);
    graph.nodes.push(pseudo);
}

fn replace_compose_text(graph: &mut PromptNodeGraph, id: &str, text: &str) {
    if let Some(node) = graph.nodes.iter_mut().find(|node| node.id == id) {
        if let PromptNodeKind::Compose { text: node_text } = &mut node.kind {
            *node_text = text.into();
        }
    }
}

impl Default for PromptNodeGraph {
    fn default() -> Self {
        Self::builtin_default()
    }
}

fn system_value_from_block(block: TranslationPromptBlock) -> PromptSystemValue {
    match block {
        TranslationPromptBlock::LanguageOrder => PromptSystemValue::LanguageOrder,
        TranslationPromptBlock::Terminology => PromptSystemValue::Terminology,
        TranslationPromptBlock::RecentTurns { limit } => PromptSystemValue::RecentTurns { limit },
        TranslationPromptBlock::PreviousRevision => PromptSystemValue::PreviousRevision,
        TranslationPromptBlock::SurroundingSource => PromptSystemValue::SurroundingSource,
        TranslationPromptBlock::CustomText { .. } => {
            unreachable!("custom text is not a host system value")
        }
    }
}

#[derive(Default)]
struct GraphBuilder {
    nodes: Vec<PromptNode>,
    links: Vec<PromptLink>,
}

impl GraphBuilder {
    fn build_openai_flow(&mut self) {
        let page = PromptNodePage::OpenAiCompatible;
        self.variable(
            "openai-source-language",
            page,
            PromptVariable::SourceLanguage,
        );
        self.variable(
            "openai-target-language",
            page,
            PromptVariable::TargetLanguage,
        );
        self.variable("openai-current-input", page, PromptVariable::CurrentInput);
        self.text_comparison(
            "openai-source-is-auto",
            page,
            "SOURCE LANGUAGE IS AUTO",
            "openai-source-language",
            "auto",
        );

        for (id, block) in [
            (
                "openai-context-language-order",
                TranslationPromptBlock::LanguageOrder,
            ),
            (
                "openai-context-terminology",
                TranslationPromptBlock::Terminology,
            ),
            (
                "openai-context-recent-turns",
                TranslationPromptBlock::RecentTurns { limit: None },
            ),
            (
                "openai-context-previous-revision",
                TranslationPromptBlock::PreviousRevision,
            ),
            (
                "openai-context-surrounding-source",
                TranslationPromptBlock::SurroundingSource,
            ),
        ] {
            self.node(
                id,
                page,
                PromptNodeKind::SystemValue {
                    value: system_value_from_block(block),
                },
            );
        }

        self.compose(
            "openai-reference-sections",
            page,
            "ASSEMBLE CONTEXT SECTIONS",
            "{0}\n\n{1}\n\n{2}\n\n{3}\n\n{4}",
            &[
                "openai-context-language-order",
                "openai-context-terminology",
                "openai-context-recent-turns",
                "openai-context-previous-revision",
                "openai-context-surrounding-source",
            ],
        );
        self.compose(
            "openai-reference-context",
            page,
            "TRANSLATION CONTEXT",
            "# Translation Context\n\n{0}",
            &["openai-reference-sections"],
        );
        self.compose(
            "openai-reference-explicit-rules",
            page,
            "EXPLICIT REFERENCE RULES",
            EXPLICIT_REFERENCE_CONTEXT_INSTRUCTION,
            &["openai-source-language", "openai-target-language"],
        );
        self.compose(
            "openai-reference-auto-rules",
            page,
            "AUTO REFERENCE RULES",
            AUTO_REFERENCE_CONTEXT_INSTRUCTION,
            &["openai-target-language"],
        );
        self.switch_on(
            "openai-reference-handling-rules",
            page,
            "SELECT REFERENCE RULES",
            "openai-source-is-auto",
            "openai-reference-explicit-rules",
            "openai-reference-auto-rules",
        );

        self.compose(
            "openai-explicit-instruction",
            page,
            "EXPLICIT SOURCE INSTRUCTION",
            "You are a real-time speech translator. If input is already {0}, output it unchanged. Otherwise translate it into {0}. Output only the translation.",
            &["openai-target-language"],
        );
        self.compose(
            "openai-auto-instruction",
            page,
            "AUTO SOURCE INSTRUCTION",
            "You are a real-time speech translator. The input language is one of the following: {0}. Translate it into the OTHER language from that list. Output only the translation.",
            &["openai-target-language"],
        );
        self.switch_on(
            "openai-instruction",
            page,
            "SELECT SOURCE INSTRUCTION",
            "openai-source-is-auto",
            "openai-explicit-instruction",
            "openai-auto-instruction",
        );

        self.compose(
            "openai-system-with-context",
            page,
            "SYSTEM PROMPT WITH CONTEXT",
            "{0}\n\n{1}\n{2}",
            &[
                "openai-instruction",
                "openai-reference-handling-rules",
                "openai-reference-context",
            ],
        );
        self.switch(
            "openai-system",
            page,
            "SELECT SYSTEM PROMPT",
            PromptCondition::HasReferenceContext,
            "openai-instruction",
            "openai-system-with-context",
        );
        self.compose(
            "openai-explicit-user",
            page,
            "EXPLICIT SOURCE MESSAGE",
            "Source language: {0}\nCurrent input:\n{1}",
            &["openai-source-language", "openai-current-input"],
        );
        self.compose(
            "openai-auto-user",
            page,
            "AUTO SOURCE MESSAGE",
            "Current input:\n{0}",
            &["openai-current-input"],
        );
        self.switch_on(
            "openai-user",
            page,
            "SELECT USER MESSAGE",
            "openai-source-is-auto",
            "openai-explicit-user",
            "openai-auto-user",
        );
        self.output(
            "openai-request",
            PromptProviderTarget::OpenAiCompatible,
            &[
                (PromptMessageRole::System, "openai-system"),
                (PromptMessageRole::User, "openai-user"),
            ],
        );
    }

    fn build_hunyuan_flow(&mut self) {
        let page = PromptNodePage::Hunyuan;
        self.variable(
            "hunyuan-source-language",
            page,
            PromptVariable::SourceLanguage,
        );
        self.variable(
            "hunyuan-target-language",
            page,
            PromptVariable::TargetLanguage,
        );
        self.variable("hunyuan-current-input", page, PromptVariable::CurrentInput);
        self.text_comparison(
            "hunyuan-source-is-auto",
            page,
            "SOURCE LANGUAGE IS AUTO",
            "hunyuan-source-language",
            "auto",
        );

        for (id, block) in [
            (
                "hunyuan-context-language-order",
                TranslationPromptBlock::LanguageOrder,
            ),
            (
                "hunyuan-context-terminology",
                TranslationPromptBlock::Terminology,
            ),
            (
                "hunyuan-context-recent-turns",
                TranslationPromptBlock::RecentTurns { limit: None },
            ),
            (
                "hunyuan-context-previous-revision",
                TranslationPromptBlock::PreviousRevision,
            ),
            (
                "hunyuan-context-surrounding-source",
                TranslationPromptBlock::SurroundingSource,
            ),
        ] {
            self.node(
                id,
                page,
                PromptNodeKind::SystemValue {
                    value: system_value_from_block(block),
                },
            );
        }

        self.compose(
            "hunyuan-reference-sections",
            page,
            "ASSEMBLE CONTEXT SECTIONS",
            "{0}\n\n{1}\n\n{2}\n\n{3}\n\n{4}",
            &[
                "hunyuan-context-language-order",
                "hunyuan-context-terminology",
                "hunyuan-context-recent-turns",
                "hunyuan-context-previous-revision",
                "hunyuan-context-surrounding-source",
            ],
        );
        self.compose(
            "hunyuan-reference-context",
            page,
            "TRANSLATION CONTEXT",
            "# Translation Context\n\n{0}",
            &["hunyuan-reference-sections"],
        );
        self.compose(
            "hunyuan-reference-explicit-rules",
            page,
            "EXPLICIT REFERENCE RULES",
            EXPLICIT_REFERENCE_CONTEXT_INSTRUCTION,
            &["hunyuan-source-language", "hunyuan-target-language"],
        );
        self.compose(
            "hunyuan-reference-auto-rules",
            page,
            "AUTO REFERENCE RULES",
            AUTO_REFERENCE_CONTEXT_INSTRUCTION,
            &["hunyuan-target-language"],
        );
        self.switch_on(
            "hunyuan-reference-handling-rules",
            page,
            "SELECT REFERENCE RULES",
            "hunyuan-source-is-auto",
            "hunyuan-reference-explicit-rules",
            "hunyuan-reference-auto-rules",
        );

        self.compose(
            "hunyuan-explicit-instruction",
            page,
            "EXPLICIT SOURCE INSTRUCTION",
            "Translate the following {0} text into {1}. Output only the translation, do not output the prompt; do not add explanations.",
            &["hunyuan-source-language", "hunyuan-target-language"],
        );
        self.compose(
            "hunyuan-auto-instruction",
            page,
            "AUTO SOURCE INSTRUCTION",
            "Translate the following text into the other language among {0}. Output only the translation; do not add explanations.",
            &["hunyuan-target-language"],
        );
        self.switch_on(
            "hunyuan-instruction",
            page,
            "SELECT SOURCE INSTRUCTION",
            "hunyuan-source-is-auto",
            "hunyuan-explicit-instruction",
            "hunyuan-auto-instruction",
        );

        self.compose(
            "hunyuan-with-context",
            page,
            "USER PROMPT WITH CONTEXT",
            "{0}\n\n{1}\n\n--- BEGIN REFERENCE CONTEXT ---\n{2}\n--- END REFERENCE CONTEXT ---\n\nCurrent input:\n{3}",
            &[
                "hunyuan-instruction",
                "hunyuan-reference-handling-rules",
                "hunyuan-reference-context",
                "hunyuan-current-input",
            ],
        );
        self.compose(
            "hunyuan-without-context",
            page,
            "USER PROMPT WITHOUT CONTEXT",
            "{0}\n\nCurrent input:\n{1}",
            &["hunyuan-instruction", "hunyuan-current-input"],
        );
        self.switch(
            "hunyuan-user",
            page,
            "SELECT USER PROMPT",
            PromptCondition::HasReferenceContext,
            "hunyuan-without-context",
            "hunyuan-with-context",
        );
        self.output(
            "hunyuan-request",
            PromptProviderTarget::Hunyuan,
            &[(PromptMessageRole::User, "hunyuan-user")],
        );
    }

    fn build_asr_instruction_flow(&mut self) {
        let page = PromptNodePage::AsrInstruction;
        self.variable(
            "asr-instruction-source-language",
            page,
            PromptVariable::SourceLanguage,
        );
        self.variable(
            "asr-instruction-expected-languages",
            page,
            PromptVariable::TargetLanguage,
        );
        self.variable(
            "asr-instruction-recognition-context",
            page,
            PromptVariable::RecognitionContext,
        );
        self.text_comparison(
            "asr-instruction-source-is-auto",
            page,
            "SOURCE LANGUAGE IS AUTO",
            "asr-instruction-source-language",
            "auto",
        );
        self.compose(
            "asr-instruction-explicit",
            page,
            "EXPLICIT ASR INSTRUCTION",
            "Transcribe the current audio accurately in {0}. Return only the transcript without translation, explanation, or commentary.",
            &["asr-instruction-source-language"],
        );
        self.compose(
            "asr-instruction-auto",
            page,
            "AUTO ASR INSTRUCTION",
            "Transcribe the current audio accurately. Expected spoken languages are {0}. Detect which language is actually spoken; both listed languages are equally valid, so do not prefer the first language in the list. Do not translate the speech. Return only the transcript without explanation or commentary.",
            &["asr-instruction-expected-languages"],
        );
        self.switch_on(
            "asr-instruction-source-mode",
            page,
            "SELECT ASR SOURCE MODE",
            "asr-instruction-source-is-auto",
            "asr-instruction-explicit",
            "asr-instruction-auto",
        );
        self.compose(
            "asr-instruction-with-context",
            page,
            "ASR PROMPT WITH RECOGNITION CONTEXT",
            "{0}\n\nUse the following recognition context only to improve spelling and term accuracy; never repeat it unless it is spoken:\n{1}",
            &[
                "asr-instruction-source-mode",
                "asr-instruction-recognition-context",
            ],
        );
        self.switch(
            "asr-instruction-prompt",
            page,
            "SELECT ASR CONTEXT MODE",
            PromptCondition::HasRecognitionContext,
            "asr-instruction-source-mode",
            "asr-instruction-with-context",
        );
        self.output(
            "asr-instruction-request",
            PromptProviderTarget::AsrInstruction,
            &[(PromptMessageRole::System, "asr-instruction-prompt")],
        );
    }

    fn build_asr_context_bias_flow(&mut self) {
        let page = PromptNodePage::AsrContextBias;
        self.variable(
            "asr-context-bias-terms",
            page,
            PromptVariable::RecognitionContext,
        );
        self.output(
            "asr-context-bias-request",
            PromptProviderTarget::AsrContextBias,
            &[(PromptMessageRole::User, "asr-context-bias-terms")],
        );
    }

    fn node(&mut self, id: &str, page: PromptNodePage, kind: PromptNodeKind) {
        let label = crate::schema::default_node_label(&kind);
        self.labeled_node(id, page, &label, kind);
    }

    fn labeled_node(&mut self, id: &str, page: PromptNodePage, label: &str, kind: PromptNodeKind) {
        self.nodes.push(PromptNode {
            id: id.into(),
            label: label.into(),
            page,
            kind,
            position: [0.0, 0.0],
        });
    }

    fn variable(&mut self, id: &str, page: PromptNodePage, variable: PromptVariable) {
        self.node(
            id,
            page,
            PromptNodeKind::SystemValue {
                value: variable.into(),
            },
        );
    }

    fn compose(
        &mut self,
        id: &str,
        page: PromptNodePage,
        label: &str,
        text: &str,
        sources: &[&str],
    ) {
        self.labeled_node(
            id,
            page,
            label,
            PromptNodeKind::Compose { text: text.into() },
        );
        for (input, source) in sources.iter().enumerate() {
            self.link(source, id, input as u8);
        }
    }

    fn text_comparison(
        &mut self,
        id: &str,
        page: PromptNodePage,
        label: &str,
        source: &str,
        expected: &str,
    ) {
        self.labeled_node(
            id,
            page,
            label,
            PromptNodeKind::TextComparison {
                operator: crate::PromptTextComparison::Equals,
                expected: expected.into(),
                case_sensitive: true,
            },
        );
        self.link(source, id, 0);
    }

    fn switch_on(
        &mut self,
        id: &str,
        page: PromptNodePage,
        label: &str,
        condition_source: &str,
        false_source: &str,
        true_source: &str,
    ) {
        self.labeled_node(id, page, label, PromptNodeKind::Switch { condition: None });
        self.link(condition_source, id, 0);
        self.link(false_source, id, 1);
        self.link(true_source, id, 2);
    }

    fn switch(
        &mut self,
        id: &str,
        page: PromptNodePage,
        label: &str,
        condition: PromptCondition,
        false_source: &str,
        true_source: &str,
    ) {
        let condition_id = format!("{id}-condition");
        self.node(
            &condition_id,
            page,
            PromptNodeKind::ConditionValue { condition },
        );
        self.switch_on(id, page, label, &condition_id, false_source, true_source);
    }

    fn output(
        &mut self,
        id: &str,
        target: PromptProviderTarget,
        messages: &[(PromptMessageRole, &str)],
    ) {
        self.nodes.push(PromptNode {
            id: id.into(),
            label: crate::schema::default_node_label(&PromptNodeKind::Request {
                target,
                roles: messages.iter().map(|(role, _)| *role).collect(),
            }),
            page: PromptNodePage::for_target(target),
            kind: PromptNodeKind::Request {
                target,
                roles: messages.iter().map(|(role, _)| *role).collect(),
            },
            position: [0.0, 0.0],
        });
        for (input, (_, source)) in messages.iter().enumerate() {
            self.link(source, id, input as u8);
        }
    }

    fn link(&mut self, from: &str, to: &str, input: u8) {
        self.links.push(PromptLink {
            from: from.into(),
            to: to.into(),
            input,
        });
    }

    fn finish(self) -> PromptNodeGraph {
        PromptNodeGraph {
            schema_version: PromptNodeGraph::CURRENT_SCHEMA_VERSION,
            nodes: self.nodes,
            links: self.links,
            layout_version: 0,
            translation_style: None,
        }
    }
}
