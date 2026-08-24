use crate::{
    PromptCondition, PromptLink, PromptMessageRole, PromptNode, PromptNodeGraph, PromptNodeKind,
    PromptNodePage, PromptProviderTarget, PromptVariable, TranslationPromptBlock,
};

pub(crate) const BUILTIN_ID: &str = "builtin-default";
pub(crate) const EXPLICIT_REFERENCE_CONTEXT_INSTRUCTION: &str = concat!(
    "Use the provided context to translate the current {0} input into 100% natural, idiomatic {1}.\n\n",
    "First understand the actual meaning of the current {0} input, then use the context to determine references, tone, speaker relationships, and intended meaning. Do not translate word-for-word, preserve {0} sentence structure, or produce translationese.\n\n",
    "The translation should sound like something a native {1} speaker would naturally say or type in Discord, QQ, WeChat, gaming chats, and everyday conversations. Naturally adjust {1} word order, sentence structure, and wording according to the context.\n\n",
    "Preserve the original meaning, tone, emotion, attitude, personality, and level of formality. Do not unnecessarily add, remove, or change the original meaning.\n\n",
    "Use vocabulary and expressions commonly and naturally used in {1}. Do not use non-standard phrasing when a natural {1} expression exists.\n\n",
    "When encountering slang, idioms, internet expressions, or conversational {0}, convey the intended meaning using an expression that {1} speakers would naturally understand and use rather than translating it literally.\n\n",
    "Translate only the current {0} input. Do not translate, repeat, summarize, or explain the context. Unless explicitly requested otherwise, output only the final {1} translation."
);
pub(crate) const AUTO_REFERENCE_CONTEXT_INSTRUCTION: &str = concat!(
    "Use the provided context to translate the current input into the other language among {0} into 100% natural, idiomatic expression.\n\n",
    "First understand the actual meaning of the current input, then use the context to determine references, tone, speaker relationships, and intended meaning. Do not translate word-for-word, preserve original sentence structure, or produce translationese.\n\n",
    "The translation should sound like something a native speaker of the target language would naturally say or type in Discord, QQ, WeChat, gaming chats, and everyday conversations. Naturally adjust target-language word order, sentence structure, and wording according to the context.\n\n",
    "Preserve the original meaning, tone, emotion, attitude, personality, and level of formality. Do not unnecessarily add, remove, or change the original meaning.\n\n",
    "Use vocabulary and expressions commonly and naturally used in the target language. Do not use non-standard phrasing when a natural expression exists.\n\n",
    "When encountering slang, idioms, internet expressions, or conversational speech, convey the intended meaning using an expression that native speakers of the target language would naturally understand and use rather than translating it literally.\n\n",
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
        graph.auto_layout();
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
            "Translate only the current {0} input into natural, idiomatic {1}. Preserve names, numbers, terms, tone, ",
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
            "segment, preserve names, numbers, tone, and intent, and output only the final translation."
        );
        let openai_explicit_instruction = concat!(
            "You are a real-time pseudo-stream speech translator. If the current input is already {0}, output it ",
            "unchanged; otherwise translate only this authoritative current snapshot segment into natural, fluent {0}. ",
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
        ensure_recognition_mode_variables(&mut ordinary);
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
            matches!(
                node.kind,
                PromptNodeKind::Switch {
                    condition: PromptCondition::IsPseudoStreaming
                }
            )
        }) {
            return ordinary;
        }
        ensure_recognition_mode_variables(&mut ordinary);

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
                    kind: PromptNodeKind::Switch {
                        condition: PromptCondition::IsPseudoStreaming,
                    },
                    position: [0.0, 0.0],
                });
                ordinary.links.extend([
                    PromptLink {
                        from: ordinary_source,
                        to: switch_id.clone(),
                        input: 0,
                    },
                    PromptLink {
                        from: pseudo_source,
                        to: switch_id.clone(),
                        input: 1,
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

fn ensure_recognition_mode_variables(graph: &mut PromptNodeGraph) {
    for page in [
        PromptNodePage::OpenAiCompatible,
        PromptNodePage::Hunyuan,
        PromptNodePage::AsrInstruction,
        PromptNodePage::AsrContextBias,
    ] {
        let id = format!("{}-recognition-mode", page_id(page));
        if !graph.nodes.iter().any(|node| node.id == id) {
            graph.nodes.push(PromptNode {
                id,
                label: "RECOGNITION MODE".into(),
                page,
                kind: PromptNodeKind::Variable {
                    variable: PromptVariable::RecognitionMode,
                },
                position: [0.0, 0.0],
            });
        }
    }
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
    if matches!(
        graph.nodes[index].kind,
        PromptNodeKind::Switch {
            condition: PromptCondition::IsPseudoStreaming
        }
    ) {
        return;
    }
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
        kind: PromptNodeKind::Switch {
            condition: PromptCondition::IsPseudoStreaming,
        },
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
    graph.links.push(PromptLink {
        from: ordinary.id.clone(),
        to: id.into(),
        input: 0,
    });
    graph.links.push(PromptLink {
        from: pseudo.id.clone(),
        to: id.into(),
        input: 1,
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
            self.node(id, page, PromptNodeKind::Input { block });
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
        self.switch(
            "openai-reference-handling-rules",
            page,
            "SELECT REFERENCE RULES",
            PromptCondition::SourceIsAuto,
            "openai-reference-explicit-rules",
            "openai-reference-auto-rules",
        );

        self.compose(
            "openai-explicit-instruction",
            page,
            "EXPLICIT SOURCE INSTRUCTION",
            "You are a real-time speech translator. If input is already {0}, output it unchanged. Otherwise translate it into natural, fluent {0}. Output only the translation.",
            &["openai-target-language"],
        );
        self.compose(
            "openai-auto-instruction",
            page,
            "AUTO SOURCE INSTRUCTION",
            "You are a real-time speech translator. The input language is one of the following: {0}. Translate it into the OTHER language from that list. Output only the translation.",
            &["openai-target-language"],
        );
        self.switch(
            "openai-instruction",
            page,
            "SELECT SOURCE INSTRUCTION",
            PromptCondition::SourceIsAuto,
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
        self.switch(
            "openai-user",
            page,
            "SELECT USER MESSAGE",
            PromptCondition::SourceIsAuto,
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
            self.node(id, page, PromptNodeKind::Input { block });
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
        self.switch(
            "hunyuan-reference-handling-rules",
            page,
            "SELECT REFERENCE RULES",
            PromptCondition::SourceIsAuto,
            "hunyuan-reference-explicit-rules",
            "hunyuan-reference-auto-rules",
        );

        self.compose(
            "hunyuan-explicit-instruction",
            page,
            "EXPLICIT SOURCE INSTRUCTION",
            "Translate the following {0} text into natural {1}. Output only the translation, do not output the prompt; do not add explanations.",
            &["hunyuan-source-language", "hunyuan-target-language"],
        );
        self.compose(
            "hunyuan-auto-instruction",
            page,
            "AUTO SOURCE INSTRUCTION",
            "Translate the following text into the other language among {0}. Output only the translation; do not add explanations.",
            &["hunyuan-target-language"],
        );
        self.switch(
            "hunyuan-instruction",
            page,
            "SELECT SOURCE INSTRUCTION",
            PromptCondition::SourceIsAuto,
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
            "{0}\n\n{1}",
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
        self.switch(
            "asr-instruction-source-mode",
            page,
            "SELECT ASR SOURCE MODE",
            PromptCondition::SourceIsAuto,
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
        self.node(id, page, PromptNodeKind::Variable { variable });
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

    fn switch(
        &mut self,
        id: &str,
        page: PromptNodePage,
        label: &str,
        condition: PromptCondition,
        false_source: &str,
        true_source: &str,
    ) {
        self.labeled_node(id, page, label, PromptNodeKind::Switch { condition });
        self.link(false_source, id, 0);
        self.link(true_source, id, 1);
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AsrPromptContext, PromptMessage, PromptMode, PromptTurn, SurroundingSource,
        TranslationPromptContext,
    };

    fn context() -> TranslationPromptContext {
        TranslationPromptContext {
            language_order: vec!["en".into(), "zh".into()],
            terminology_rows: vec!["天使,Mercy".into()],
            recent_turns: vec![PromptTurn {
                turn_id: Some("previous".into()),
                speaker_id: "speaker-01".into(),
                source_language: "en".into(),
                target_language: "zh".into(),
                source_text: "We changed the plan.".into(),
                translated_text: "我们改计划了。".into(),
            }],
            previous_revision: Some(PromptTurn {
                turn_id: Some("current".into()),
                speaker_id: "speaker-01".into(),
                source_language: "en".into(),
                target_language: "zh".into(),
                source_text: "The current window.".into(),
                translated_text: "当前窗口。".into(),
            }),
            surrounding_source: Some(SurroundingSource {
                speaker_id: "speaker-01".into(),
                source_language: "en".into(),
                before: "Before it.".into(),
                after: "After it.".into(),
            }),
            mode: PromptMode::Ordinary,
        }
    }

    fn reference() -> String {
        "# Translation Context\n\n\
## Language Order\n\n\
en,zh\n\n\
## Terminology\n\n\
天使,Mercy\n\n\
## Recent Bilingual History\n\n\
speaker-01 en: We changed the plan.\n\
speaker-01 zh: 我们改计划了。\n\n\
## Previous Revision of Current Speech\n\n\
speaker-01 en: The current window.\n\
speaker-01 zh: 当前窗口。\n\n\
## Current Utterance Context (context only; do not translate)\n\n\
Before current input: speaker-01 en / Before it.\n\
After current input: speaker-01 en / After it."
            .into()
    }

    fn explicit_reference_rules_rendered(source: &str, target: &str) -> String {
        EXPLICIT_REFERENCE_CONTEXT_INSTRUCTION
            .replace("{0}", source)
            .replace("{1}", target)
    }

    fn auto_reference_rules_rendered(target: &str) -> String {
        AUTO_REFERENCE_CONTEXT_INSTRUCTION.replace("{0}", target)
    }

    #[test]
    fn reference_handling_rules_are_canonical() {
        assert_eq!(
            EXPLICIT_REFERENCE_CONTEXT_INSTRUCTION,
            concat!(
                "Use the provided context to translate the current {0} input into 100% natural, idiomatic {1}.\n\n",
                "First understand the actual meaning of the current {0} input, then use the context to determine references, tone, speaker relationships, and intended meaning. Do not translate word-for-word, preserve {0} sentence structure, or produce translationese.\n\n",
                "The translation should sound like something a native {1} speaker would naturally say or type in Discord, QQ, WeChat, gaming chats, and everyday conversations. Naturally adjust {1} word order, sentence structure, and wording according to the context.\n\n",
                "Preserve the original meaning, tone, emotion, attitude, personality, and level of formality. Do not unnecessarily add, remove, or change the original meaning.\n\n",
                "Use vocabulary and expressions commonly and naturally used in {1}. Do not use non-standard phrasing when a natural {1} expression exists.\n\n",
                "When encountering slang, idioms, internet expressions, or conversational {0}, convey the intended meaning using an expression that {1} speakers would naturally understand and use rather than translating it literally.\n\n",
                "Translate only the current {0} input. Do not translate, repeat, summarize, or explain the context. Unless explicitly requested otherwise, output only the final {1} translation."
            )
        );
        assert_eq!(
            AUTO_REFERENCE_CONTEXT_INSTRUCTION,
            concat!(
                "Use the provided context to translate the current input into the other language among {0} into 100% natural, idiomatic expression.\n\n",
                "First understand the actual meaning of the current input, then use the context to determine references, tone, speaker relationships, and intended meaning. Do not translate word-for-word, preserve original sentence structure, or produce translationese.\n\n",
                "The translation should sound like something a native speaker of the target language would naturally say or type in Discord, QQ, WeChat, gaming chats, and everyday conversations. Naturally adjust target-language word order, sentence structure, and wording according to the context.\n\n",
                "Preserve the original meaning, tone, emotion, attitude, personality, and level of formality. Do not unnecessarily add, remove, or change the original meaning.\n\n",
                "Use vocabulary and expressions commonly and naturally used in the target language. Do not use non-standard phrasing when a natural expression exists.\n\n",
                "When encountering slang, idioms, internet expressions, or conversational speech, convey the intended meaning using an expression that native speakers of the target language would naturally understand and use rather than translating it literally.\n\n",
                "Translate only the current input. Do not translate, repeat, summarize, or explain the context. Unless explicitly requested otherwise, output only the final translation."
            )
        );
    }

    #[test]
    fn openai_explicit_with_context_matches_the_canonical_messages() {
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::OpenAiCompatible,
                "Good morning",
                "English",
                "Chinese",
                &context(),
            )
            .unwrap();
        assert_eq!(
            rendered.messages,
            vec![
                PromptMessage {
                    role: PromptMessageRole::System,
                    content: format!(
                        "You are a real-time speech translator. If input is already Chinese, output it unchanged. Otherwise translate it into natural, fluent Chinese. Output only the translation.\n\n{}\n{}",
                        explicit_reference_rules_rendered("English", "Chinese"),
                        reference()
                    ),
                },
                PromptMessage {
                    role: PromptMessageRole::User,
                    content: "Source language: English\nCurrent input:\nGood morning".into(),
                },
            ]
        );
    }

    #[test]
    fn openai_auto_with_context_matches_the_canonical_messages() {
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::OpenAiCompatible,
                "Good morning",
                "auto",
                "Chinese,English",
                &context(),
            )
            .unwrap();
        assert_eq!(
            rendered.messages[0].content,
            format!(
                "You are a real-time speech translator. The input language is one of the following: Chinese,English. Translate it into the OTHER language from that list. Output only the translation.\n\n{}\n{}",
                auto_reference_rules_rendered("Chinese,English"),
                reference()
            )
        );
        assert_eq!(rendered.messages[1].content, "Current input:\nGood morning");
    }

    #[test]
    fn openai_without_context_matches_the_original_messages() {
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::OpenAiCompatible,
                "Good morning",
                "English",
                "Chinese",
                &TranslationPromptContext::default(),
            )
            .unwrap();
        assert_eq!(
            rendered.messages[0].content,
            "You are a real-time speech translator. If input is already Chinese, output it unchanged. Otherwise translate it into natural, fluent Chinese. Output only the translation."
        );
        assert_eq!(
            rendered.messages[1].content,
            "Source language: English\nCurrent input:\nGood morning"
        );
    }

    #[test]
    fn unified_graph_selects_prompt_text_from_runtime_recognition_mode() {
        let graph = PromptNodeGraph::builtin_default();
        let ordinary = graph
            .render_with_trace(
                PromptProviderTarget::OpenAiCompatible,
                "Correct this live tail",
                "English",
                "Chinese",
                &TranslationPromptContext::default(),
            )
            .unwrap();
        let pseudo = graph
            .render_with_trace(
                PromptProviderTarget::OpenAiCompatible,
                "Correct this live tail",
                "English",
                "Chinese",
                &TranslationPromptContext {
                    mode: PromptMode::PseudoStreaming,
                    ..TranslationPromptContext::default()
                },
            )
            .unwrap();

        assert_eq!(
            ordinary
                .trace
                .node("openai-explicit-instruction")
                .unwrap()
                .selected_input,
            Some(0)
        );
        assert_eq!(
            pseudo
                .trace
                .node("openai-explicit-instruction")
                .unwrap()
                .selected_input,
            Some(1)
        );
        assert!(!ordinary.render.messages[0].content.contains("live tail"));
        assert!(pseudo.render.messages[0].content.contains("live tail"));
        assert!(
            pseudo.render.messages[1]
                .content
                .contains("Authoritative current segment")
        );
    }

    #[test]
    fn compose_skips_empty_reference_slots_without_extra_blank_lines() {
        let context = TranslationPromptContext {
            language_order: vec!["en".into(), "zh".into()],
            ..TranslationPromptContext::default()
        };
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::OpenAiCompatible,
                "Good morning",
                "English",
                "Chinese",
                &context,
            )
            .unwrap();

        assert_eq!(
            rendered.messages[0].content,
            format!(
                "You are a real-time speech translator. If input is already Chinese, output it unchanged. Otherwise translate it into natural, fluent Chinese. Output only the translation.\n\n{}\n# Translation Context\n\n## Language Order\n\nen,zh",
                explicit_reference_rules_rendered("English", "Chinese")
            )
        );
    }

    #[test]
    fn hunyuan_explicit_with_context_matches_the_canonical_message() {
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::Hunyuan,
                "Good morning",
                "English",
                "Chinese",
                &context(),
            )
            .unwrap();
        assert_eq!(
            rendered.messages,
            vec![PromptMessage {
                role: PromptMessageRole::User,
                content: format!(
                    "Translate the following English text into natural Chinese. Output only the translation, do not output the prompt; do not add explanations.\n\n{}\n\n--- BEGIN REFERENCE CONTEXT ---\n{}\n--- END REFERENCE CONTEXT ---\n\nCurrent input:\nGood morning",
                    explicit_reference_rules_rendered("English", "Chinese"),
                    reference()
                ),
            }]
        );
    }

    #[test]
    fn hunyuan_auto_with_context_matches_the_canonical_message() {
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::Hunyuan,
                "Good morning",
                "auto",
                "Chinese,English",
                &context(),
            )
            .unwrap();
        assert_eq!(
            rendered.messages[0].content,
            format!(
                "Translate the following text into the other language among Chinese,English. Output only the translation; do not add explanations.\n\n{}\n\n--- BEGIN REFERENCE CONTEXT ---\n{}\n--- END REFERENCE CONTEXT ---\n\nCurrent input:\nGood morning",
                auto_reference_rules_rendered("Chinese,English"),
                reference()
            )
        );
    }

    #[test]
    fn runtime_trace_records_real_outputs_and_the_selected_path() {
        let execution = PromptNodeGraph::builtin_default()
            .render_with_trace(
                PromptProviderTarget::Hunyuan,
                "Then when will you?",
                "English",
                "Chinese",
                &context(),
            )
            .unwrap();

        assert_eq!(
            execution
                .trace
                .node("hunyuan-current-input")
                .unwrap()
                .output,
            "Then when will you?"
        );
        assert!(
            execution
                .trace
                .node("hunyuan-context-recent-turns")
                .unwrap()
                .output
                .contains("We changed the plan.")
        );
        assert_eq!(
            execution.trace.node("hunyuan-user").unwrap().selected_input,
            Some(1)
        );
        assert!(
            execution
                .trace
                .node("hunyuan-request")
                .unwrap()
                .output
                .ends_with("Current input:\nThen when will you?")
        );
        assert!(execution.trace.node("hunyuan-without-context").is_none());
    }

    #[test]
    fn hunyuan_without_context_matches_the_original_message() {
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::Hunyuan,
                "Good morning",
                "English",
                "Chinese",
                &TranslationPromptContext::default(),
            )
            .unwrap();
        assert_eq!(
            rendered.messages[0].content,
            "Translate the following English text into natural Chinese. Output only the translation, do not output the prompt; do not add explanations.\n\nGood morning"
        );
    }

    #[test]
    fn asr_instruction_is_semantic_and_keeps_vocabulary_optional() {
        let graph = PromptNodeGraph::builtin_default();
        let without_context = graph
            .render_asr_with_trace(
                PromptProviderTarget::AsrInstruction,
                "English",
                "English, Chinese",
                &AsrPromptContext::default(),
            )
            .unwrap();
        assert_eq!(
            without_context.render.messages,
            vec![PromptMessage {
                role: PromptMessageRole::System,
                content: "Transcribe the current audio accurately in English. Return only the transcript without translation, explanation, or commentary.".into(),
            }]
        );

        let with_context = graph
            .render_asr_with_trace(
                PromptProviderTarget::AsrInstruction,
                "auto",
                "English, Chinese",
                &AsrPromptContext {
                    vocabulary: vec!["XRTranslate".into(), "VRChat".into()],
                    mode: PromptMode::Ordinary,
                },
            )
            .unwrap();
        let content = &with_context.render.messages[0].content;
        assert!(content.contains("Expected spoken languages are English, Chinese"));
        assert!(content.contains("never repeat it unless it is spoken"));
        assert!(content.ends_with("XRTranslate, VRChat"));
    }

    #[test]
    fn asr_context_bias_contains_terms_without_instruction_text() {
        let rendered = PromptNodeGraph::builtin_default()
            .render_asr_with_trace(
                PromptProviderTarget::AsrContextBias,
                "auto",
                "English, Chinese",
                &AsrPromptContext {
                    vocabulary: vec!["XRTranslate".into(), "VRChat".into()],
                    mode: PromptMode::Ordinary,
                },
            )
            .unwrap();
        assert_eq!(
            rendered.render.messages,
            vec![PromptMessage {
                role: PromptMessageRole::User,
                content: "XRTranslate, VRChat".into(),
            }]
        );
        assert!(!rendered.render.messages[0].content.contains("Transcribe"));
    }

    #[test]
    fn builtin_graph_uses_compose_nodes_instead_of_fragmented_text_nodes() {
        let graph = PromptNodeGraph::builtin_default();
        assert!(graph.nodes.len() <= 85, "{} nodes", graph.nodes.len());
        assert!(
            graph
                .nodes
                .iter()
                .any(|node| matches!(node.kind, PromptNodeKind::Compose { .. }))
        );
    }

    #[test]
    fn builtin_graph_has_one_ordered_request_per_provider_page() {
        let graph = PromptNodeGraph::builtin_default();
        let requests = graph
            .nodes
            .iter()
            .filter_map(|node| match &node.kind {
                PromptNodeKind::Request { target, roles } => Some((node, target, roles)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(requests.len(), 4);
        let openai = requests
            .iter()
            .find(|(_, target, _)| **target == PromptProviderTarget::OpenAiCompatible)
            .unwrap();
        assert_eq!(openai.0.id, "openai-request");
        assert_eq!(openai.0.page, PromptNodePage::OpenAiCompatible);
        assert_eq!(
            openai.2.as_slice(),
            [PromptMessageRole::System, PromptMessageRole::User]
        );

        let hunyuan = requests
            .iter()
            .find(|(_, target, _)| **target == PromptProviderTarget::Hunyuan)
            .unwrap();
        assert_eq!(hunyuan.0.id, "hunyuan-request");
        assert_eq!(hunyuan.0.page, PromptNodePage::Hunyuan);
        assert_eq!(hunyuan.2.as_slice(), [PromptMessageRole::User]);

        let asr_instruction = requests
            .iter()
            .find(|(_, target, _)| **target == PromptProviderTarget::AsrInstruction)
            .unwrap();
        assert_eq!(asr_instruction.0.page, PromptNodePage::AsrInstruction);
        assert_eq!(asr_instruction.2.as_slice(), [PromptMessageRole::System]);

        let asr_context = requests
            .iter()
            .find(|(_, target, _)| **target == PromptProviderTarget::AsrContextBias)
            .unwrap();
        assert_eq!(asr_context.0.page, PromptNodePage::AsrContextBias);
        assert_eq!(asr_context.2.as_slice(), [PromptMessageRole::User]);
    }

    #[test]
    fn provider_page_layouts_do_not_reserve_space_for_hidden_nodes() {
        let graph = PromptNodeGraph::builtin_default();

        for target in [
            PromptProviderTarget::OpenAiCompatible,
            PromptProviderTarget::Hunyuan,
            PromptProviderTarget::AsrInstruction,
            PromptProviderTarget::AsrContextBias,
        ] {
            let visible = graph
                .nodes
                .iter()
                .filter(|node| node.page.is_visible_on(target))
                .collect::<Vec<_>>();
            for (index, first) in visible.iter().enumerate() {
                for second in visible.iter().skip(index + 1) {
                    if first.position[0] != second.position[0] {
                        continue;
                    }
                    let first_bottom = first.position[1] + first.layout_height();
                    let second_bottom = second.position[1] + second.layout_height();
                    assert!(
                        first_bottom <= second.position[1] || second_bottom <= first.position[1],
                        "{} overlaps {} on {target:?}",
                        first.id,
                        second.id
                    );
                }
            }
        }
    }

    #[test]
    fn builtin_composition_nodes_have_semantic_editor_labels() {
        let graph = PromptNodeGraph::builtin_default();
        for (id, label) in [
            ("openai-reference-context", "TRANSLATION CONTEXT"),
            (
                "openai-reference-explicit-rules-ordinary",
                "EXPLICIT REFERENCE RULES / ORDINARY",
            ),
            (
                "openai-reference-auto-rules-ordinary",
                "AUTO REFERENCE RULES / ORDINARY",
            ),
            ("openai-reference-handling-rules", "SELECT REFERENCE RULES"),
            (
                "openai-explicit-instruction-ordinary",
                "EXPLICIT SOURCE INSTRUCTION / ORDINARY",
            ),
            ("openai-system", "SELECT SYSTEM PROMPT"),
            ("hunyuan-with-context", "USER PROMPT WITH CONTEXT"),
        ] {
            assert_eq!(
                graph
                    .nodes
                    .iter()
                    .find(|node| node.id == id)
                    .map(|node| node.label.as_str()),
                Some(label)
            );
        }
    }

    #[test]
    fn reference_rules_are_a_visible_graph_input() {
        let graph = PromptNodeGraph::builtin_default();
        let explicit_rules = graph
            .nodes
            .iter()
            .find(|node| node.id == "openai-reference-explicit-rules-ordinary")
            .unwrap();
        let auto_rules = graph
            .nodes
            .iter()
            .find(|node| node.id == "openai-reference-auto-rules-ordinary")
            .unwrap();
        let switch_rules = graph
            .nodes
            .iter()
            .find(|node| node.id == "openai-reference-handling-rules")
            .unwrap();
        let openai = graph
            .nodes
            .iter()
            .find(|node| node.id == "openai-system-with-context")
            .unwrap();
        let hunyuan = graph
            .nodes
            .iter()
            .find(|node| node.id == "hunyuan-with-context")
            .unwrap();

        assert_eq!(explicit_rules.label, "EXPLICIT REFERENCE RULES / ORDINARY");
        assert_eq!(auto_rules.label, "AUTO REFERENCE RULES / ORDINARY");
        assert_eq!(switch_rules.label, "SELECT REFERENCE RULES");
        assert!(explicit_rules.layout_height() > 142.0);
        assert!(auto_rules.layout_height() > 142.0);
        assert_eq!(
            crate::compose_input_indexes(match &openai.kind {
                PromptNodeKind::Compose { text } => text,
                _ => unreachable!(),
            })
            .unwrap(),
            vec![0, 1, 2]
        );
        assert_eq!(
            crate::compose_input_indexes(match &hunyuan.kind {
                PromptNodeKind::Compose { text } => text,
                _ => unreachable!(),
            })
            .unwrap(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn source_auto_condition_preserves_the_original_case_sensitive_behavior() {
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::OpenAiCompatible,
                "Good morning",
                "AUTO",
                "Chinese",
                &TranslationPromptContext::default(),
            )
            .unwrap();
        assert!(
            rendered.messages[0]
                .content
                .contains("If input is already Chinese")
        );
        assert_eq!(
            rendered.messages[1].content,
            "Source language: AUTO\nCurrent input:\nGood morning"
        );
    }

    #[test]
    fn compose_placeholders_must_be_valid_and_connected() {
        let mut graph = PromptNodeGraph::builtin_default();
        let node = graph
            .nodes
            .iter_mut()
            .find(|node| node.id == "openai-explicit-instruction-ordinary")
            .unwrap();
        node.kind = PromptNodeKind::Compose {
            text: "Translate {5}".into(),
        };
        assert!(graph.validate_for_activation().is_err());

        let mut graph = PromptNodeGraph::builtin_default();
        graph
            .links
            .retain(|link| !(link.to == "openai-explicit-instruction-ordinary" && link.input == 0));
        assert!(graph.validate_for_activation().is_err());
    }

    #[test]
    fn builtin_default_contains_xiaoyv_instructions_and_prompt_suppression() {
        let graph = PromptNodeGraph::builtin_default();

        let hunyuan_explicit = graph
            .nodes
            .iter()
            .find(|node| node.id == "hunyuan-explicit-instruction-ordinary")
            .unwrap();
        match &hunyuan_explicit.kind {
            PromptNodeKind::Compose { text } => {
                assert!(
                    text.contains("do not output the prompt"),
                    "hunyuan-explicit-instruction must contain 'do not output the prompt'"
                );
                assert_eq!(
                    text,
                    "Translate the following {0} text into natural {1}. Output only the translation, do not output the prompt; do not add explanations."
                );
            }
            _ => panic!("hunyuan-explicit-instruction must be a Compose node"),
        }

        let explicit_rules = graph
            .nodes
            .iter()
            .find(|node| node.id == "openai-reference-explicit-rules-ordinary")
            .unwrap();
        match &explicit_rules.kind {
            PromptNodeKind::Compose { text } => {
                assert!(text.contains("100% natural, idiomatic {1}"));
                assert!(
                    text.contains("Discord, QQ, WeChat, gaming chats, and everyday conversations")
                );
                assert!(text.contains(
                    "Unless explicitly requested otherwise, output only the final {1} translation."
                ));
                assert_eq!(text, EXPLICIT_REFERENCE_CONTEXT_INSTRUCTION);
            }
            _ => panic!("openai-reference-explicit-rules must be a Compose node"),
        }

        let auto_rules = graph
            .nodes
            .iter()
            .find(|node| node.id == "openai-reference-auto-rules-ordinary")
            .unwrap();
        match &auto_rules.kind {
            PromptNodeKind::Compose { text } => {
                assert!(text.contains(
                    "into the other language among {0} into 100% natural, idiomatic expression."
                ));
                assert!(
                    text.contains("Discord, QQ, WeChat, gaming chats, and everyday conversations")
                );
                assert!(text.contains(
                    "Unless explicitly requested otherwise, output only the final translation."
                ));
                assert_eq!(text, AUTO_REFERENCE_CONTEXT_INSTRUCTION);
            }
            _ => panic!("openai-reference-auto-rules must be a Compose node"),
        }
    }

    #[test]
    fn reference_handling_rules_adapt_to_source_and_target_languages() {
        let rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::OpenAiCompatible,
                "こんにちは",
                "Japanese",
                "English",
                &context(),
            )
            .unwrap();
        assert!(rendered.messages[0].content.contains("Use the provided context to translate the current Japanese input into 100% natural, idiomatic English."));
        assert!(rendered.messages[0].content.contains("Translate only the current Japanese input. Do not translate, repeat, summarize, or explain the context. Unless explicitly requested otherwise, output only the final English translation."));

        let auto_rendered = PromptNodeGraph::builtin_default()
            .render(
                PromptProviderTarget::OpenAiCompatible,
                "こんにちは",
                "auto",
                "Japanese,English",
                &context(),
            )
            .unwrap();
        assert!(auto_rendered.messages[0].content.contains("Use the provided context to translate the current input into the other language among Japanese,English into 100% natural, idiomatic expression."));
        assert!(auto_rendered.messages[0].content.contains("Translate only the current input. Do not translate, repeat, summarize, or explain the context. Unless explicitly requested otherwise, output only the final translation."));
    }
}
