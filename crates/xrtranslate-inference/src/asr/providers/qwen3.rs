use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use xrtranslate_engine::language::SupportedLanguage;

use crate::asr::types::parse_language;

use crate::{
    AsrOptions, AsrTranscript, AsrVocabularyBias, AsyncHttpClient, InferenceError,
    OpenAiCompatibleClient,
    openai::{non_streaming_chat_payload, remove_completion_markers},
    pcm16_mono_16khz_to_wav,
};

/// Qwen3-ASR adapter backed by a local llama-server endpoint.
#[derive(Debug, Clone)]
pub struct Qwen3AsrAdapter<C> {
    chat: OpenAiCompatibleClient<C>,
    model: String,
}

impl<C> Qwen3AsrAdapter<C> {
    pub fn new(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        let model = model.into().trim().to_owned();
        if model.is_empty() {
            return Err(InferenceError::InvalidConfiguration {
                field: "model",
                message: "must not be empty".into(),
            });
        }
        Ok(Self {
            chat: OpenAiCompatibleClient::new(http, endpoint)?,
            model,
        })
    }

    pub fn endpoint(&self) -> &str {
        self.chat.endpoint()
    }
}

impl<C: AsyncHttpClient> Qwen3AsrAdapter<C> {
    /// Sends a complete VAD-delimited PCM16/16kHz turn to Qwen3-ASR.
    ///
    /// The raw PCM is always converted to WAV before base64 encoding. The
    /// resulting `input_audio` content part follows the OpenAI multimodal
    /// chat-completions shape accepted by current llama-server builds.
    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: AsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        let language = parse_language(options.language.as_deref())?;
        let wav = pcm16_mono_16khz_to_wav(pcm)?;
        let encoded_wav = STANDARD.encode(wav);

        let mut messages = Vec::new();
        if let Some(context) = qwen3_context(&options.context_bias, &options.vocabulary_bias) {
            messages.push(json!({"role": "system", "content": context}));
        }
        let content = vec![json!({
            "type": "input_audio",
            "input_audio": {"data": encoded_wav, "format": "wav"}
        })];
        messages.push(json!({
            "role": "user",
            "content": content
        }));
        if let Some(language) = language {
            // Qwen3's prefill uses names and a single Chinese speech token.
            let language = if language.base_code() == "zh" {
                "Chinese"
            } else {
                language.name()
            };
            messages.push(json!({
                "role": "assistant",
                "content": format!("language {language}<asr_text>"),
            }));
        }

        let payload = non_streaming_chat_payload(
            &self.model,
            serde_json::Value::Array(messages),
            0.0,
            options.max_tokens,
        );
        let completion = self.chat.chat_completion(payload).await?;
        Ok(parse_asr_transcript(
            &completion.text,
            language.map(SupportedLanguage::code),
        ))
    }
}

fn normalized_optional(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn qwen3_context(
    context_bias: &Option<String>,
    vocabulary_bias: &[AsrVocabularyBias],
) -> Option<String> {
    let mut terms = Vec::new();
    if let Some(context) = normalized_optional(context_bias) {
        terms.extend(
            context
                .split(',')
                .map(str::trim)
                .filter(|term| !term.is_empty())
                .map(str::to_owned),
        );
    }
    for term in vocabulary_bias.iter().map(|term| term.text.trim()) {
        if !term.is_empty() && !terms.iter().any(|existing| existing == term) {
            terms.push(term.to_owned());
        }
    }
    (!terms.is_empty()).then(|| terms.join(", "))
}

fn parse_asr_transcript(text: &str, forced_language: Option<&str>) -> AsrTranscript {
    let (detected_language, transcript) = text.rsplit_once("<asr_text>").map_or_else(
        || (None, text),
        |(metadata, transcript)| {
            let language = metadata
                .trim()
                .strip_prefix("language ")
                .map(str::trim)
                .filter(|language| !language.is_empty() && !language.eq_ignore_ascii_case("none"));
            (language, transcript)
        },
    );
    AsrTranscript {
        language: detected_language.or(forced_language).and_then(|labels| {
            let codes = labels
                .split(',')
                .filter_map(SupportedLanguage::parse)
                .map(SupportedLanguage::code)
                .collect::<Vec<_>>();
            (!codes.is_empty()).then(|| codes.join(","))
        }),
        text: remove_completion_markers(transcript),
    }
}
