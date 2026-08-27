use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

use crate::{
    AsrTranscript, AsrVocabularyBias, AsyncHttpClient, InferenceError, OpenAiCompatibleClient,
    openai::{non_streaming_chat_payload, remove_completion_markers},
    pcm16_mono_16khz_to_wav,
};

/// Options for one Qwen3-ASR completion request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Qwen3AsrOptions {
    /// Language name understood by Qwen3-ASR (for example `English` or
    /// `Chinese`). An empty value asks the model to infer the language.
    pub language: Option<String>,
    /// Official Qwen3-ASR recognition context. This is sent as the system
    /// message content, exactly as the Python/vLLM implementation does.
    pub context_bias: Option<String>,
    /// Lexical terms to include in the official context field. Qwen3-ASR does
    /// not expose weighted hotwords, so weights are intentionally ignored.
    pub vocabulary_bias: Vec<AsrVocabularyBias>,
    /// Retained for provider-neutral API compatibility. Qwen3-ASR does not
    /// accept semantic instruction prompts; callers should use context_bias.
    pub instruction_prompt: Option<String>,
    /// Maximum generated transcript tokens.
    pub max_tokens: u32,
}

impl Default for Qwen3AsrOptions {
    fn default() -> Self {
        Self {
            language: None,
            context_bias: None,
            vocabulary_bias: Vec::new(),
            instruction_prompt: None,
            max_tokens: 128,
        }
    }
}

/// Qwen3-ASR adapter backed by a local llama-server or remote DashScope endpoint.
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

    /// Creates an audio-chat ASR adapter for a remote OpenAI-compatible
    /// endpoint. The payload remains the same multimodal chat contract; only
    /// transport authentication differs from the local llama-server route.
    pub fn with_bearer_token(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, InferenceError> {
        let model = model.into().trim().to_owned();
        if model.is_empty() {
            return Err(InferenceError::InvalidConfiguration {
                field: "model",
                message: "must not be empty".into(),
            });
        }
        let chat = OpenAiCompatibleClient::with_bearer_token(http, endpoint, token)?;
        Ok(Self { chat, model })
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
        options: Qwen3AsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
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
        if let Some(language) = normalized_optional(&options.language) {
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
            normalized_optional(&options.language),
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

#[must_use]
pub fn is_probable_asr_hallucination(
    text: &str,
    sample_count: usize,
    sample_rate: u32,
    instruction_prompt: Option<&str>,
    echo_candidates: &[String],
) -> bool {
    let text = text.trim();
    if text.is_empty() || sample_rate == 0 {
        return false;
    }

    let lowercase = text.to_lowercase();
    let had_instruction_prompt =
        instruction_prompt.is_some_and(|instruction| !instruction.trim().is_empty());
    if lowercase.contains("# asr lexicon")
        || lowercase.contains("asr_lexicon")
        || lowercase.contains("<asr_context")
        || (had_instruction_prompt && lowercase.starts_with("vocabulary:"))
        || (had_instruction_prompt
            && (lowercase.contains("# asr context")
                || lowercase.contains("## language order")
                || lowercase.contains("## terminology")
                || lowercase.contains("## recent bilingual history")))
        || (had_instruction_prompt
            && text.starts_with('{')
            && (lowercase.contains("\"kind\"") || lowercase.contains("\"terms\"")))
    {
        return true;
    }

    let normalized_output = normalized_words(text);
    if spoken_unit_count(text) >= 2
        && echo_candidates
            .iter()
            .map(|candidate| normalized_words(candidate))
            .any(|candidate| !candidate.is_empty() && candidate == normalized_output)
    {
        return true;
    }

    let seconds = sample_count as f64 / f64::from(sample_rate);
    let maximum_units = ((seconds * 7.0).ceil() as usize + 1).max(4);
    spoken_unit_count(text) > maximum_units
}

fn normalized_words(text: &str) -> String {
    let mut normalized = String::new();
    let mut previous_was_space = true;
    for character in text.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            normalized.push(character);
            previous_was_space = false;
        } else if !previous_was_space {
            normalized.push(' ');
            previous_was_space = true;
        }
    }
    normalized.trim().to_owned()
}

fn spoken_unit_count(text: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for character in text.chars() {
        if is_logographic_or_syllabic(character) {
            count += 1;
            in_word = false;
        } else if character.is_alphanumeric() || character == '\'' {
            if !in_word {
                count += 1;
                in_word = true;
            }
        } else {
            in_word = false;
        }
    }
    count
}

fn is_logographic_or_syllabic(character: char) -> bool {
    matches!(
        character as u32,
        0x3400..=0x4DBF
            | 0x4E00..=0x9FFF
            | 0xF900..=0xFAFF
            | 0x3040..=0x30FF
            | 0xAC00..=0xD7AF
    )
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
        language: detected_language.or(forced_language).map(str::to_owned),
        text: remove_completion_markers(transcript),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use base64::{Engine as _, engine::general_purpose::STANDARD};

    use super::*;
    use crate::{HttpRequest, HttpResponse, TransportError};

    #[derive(Default)]
    struct RecordingHttpClient {
        requests: Mutex<Vec<HttpRequest>>,
        responses: Mutex<Vec<Result<HttpResponse, TransportError>>>,
    }

    impl RecordingHttpClient {
        fn respond_with(&self, response: HttpResponse) {
            self.responses.lock().unwrap().push(Ok(response));
        }
    }

    impl AsyncHttpClient for RecordingHttpClient {
        async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.requests.lock().unwrap().push(request);
            self.responses.lock().unwrap().remove(0)
        }
    }

    #[tokio::test]
    async fn qwen3_request_wraps_pcm_in_wav_and_uses_input_audio() {
        let http = RecordingHttpClient::default();
        http.respond_with(HttpResponse {
            status: 200,
            body: r#"{"choices":[{"message":{"content":"language English<asr_text>Hello <|im_end|>"}}]}"#.into(),
        });
        let adapter = Qwen3AsrAdapter::new(
            http,
            "http://127.0.0.1:8001/v1/chat/completions",
            "qwen3-asr",
        )
        .unwrap();

        let result = adapter
            .transcribe_pcm16(
                &[1, 0, 2, 0],
                Qwen3AsrOptions {
                    language: Some("English".into()),
                    context_bias: Some("Names: Codex".into()),
                    vocabulary_bias: vec![AsrVocabularyBias {
                        text: "VRChat".into(),
                        weight: 4,
                    }],
                    instruction_prompt: Some("Do not translate".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(result.text, "Hello");
        assert_eq!(result.language.as_deref(), Some("English"));
        let http = adapter.chat.into_inner();
        let request = http.requests.lock().unwrap().pop().unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "http://127.0.0.1:8001/v1/chat/completions");
        assert_eq!(request.body["model"], "qwen3-asr");
        assert_eq!(request.body["temperature"], 0.0);
        assert_eq!(request.body["stream"], false);
        assert_eq!(request.body["messages"].as_array().unwrap().len(), 3);
        assert_eq!(request.body["messages"][0]["role"], "system");
        assert_eq!(
            request.body["messages"][0]["content"],
            "Names: Codex, VRChat"
        );
        assert_eq!(
            request.body["messages"][1]["content"][0]["type"],
            "input_audio"
        );
        assert_eq!(
            request.body["messages"][1]["content"][0]["input_audio"]["format"],
            "wav"
        );
        let encoded = request.body["messages"][1]["content"][0]["input_audio"]["data"]
            .as_str()
            .unwrap();
        let wav = STANDARD.decode(encoded).unwrap();
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[44..], &[1, 0, 2, 0]);
        assert_eq!(
            request.body["messages"][1]["content"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(request.body["messages"][2]["role"], "assistant");
        assert_eq!(
            request.body["messages"][2]["content"],
            "language English<asr_text>"
        );
        assert!(!request.body.to_string().contains("Do not translate"));
    }

    #[tokio::test]
    async fn qwen3_http_failure_is_structured() {
        let http = RecordingHttpClient::default();
        http.respond_with(HttpResponse {
            status: 503,
            body: "temporarily unavailable".into(),
        });
        let adapter = Qwen3AsrAdapter::new(
            http,
            "http://127.0.0.1:8001/v1/chat/completions",
            "qwen3-asr",
        )
        .unwrap();
        let error = adapter
            .transcribe_pcm16(&[0, 0], Qwen3AsrOptions::default())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            InferenceError::HttpStatus { status: 503, .. }
        ));
        assert!(error.to_string().contains("temporarily unavailable"));
    }

    #[test]
    fn qwen3_marker_only_output_becomes_an_empty_transcript() {
        // A silent audio smoke test on llama.cpp returns this prefix without
        // transcript content. It must not become an OSC subtitle.
        assert_eq!(
            parse_asr_transcript("language None<asr_text>", None),
            AsrTranscript {
                language: None,
                text: String::new(),
            }
        );
    }

    #[test]
    fn forced_language_is_retained_when_completion_only_contains_text() {
        assert_eq!(
            parse_asr_transcript("こんにちは", Some("Japanese")),
            AsrTranscript {
                language: Some("Japanese".into()),
                text: "こんにちは".into(),
            }
        );
    }

    #[test]
    fn quality_gate_uses_audio_duration_instead_of_expected_words() {
        assert!(!is_probable_asr_hallucination(
            "hello",
            6_400,
            16_000,
            None,
            &[],
        ));
        assert!(is_probable_asr_hallucination(
            "Independent transcription of current audio",
            6_400,
            16_000,
            None,
            &[],
        ));
        assert!(is_probable_asr_hallucination(
            r#"{"kind":"asr_lexicon","terms":[]}"#,
            16_000,
            16_000,
            None,
            &[],
        ));
        assert!(is_probable_asr_hallucination(
            "## Recent Bilingual History\nen: hello\nzh: 你好",
            16_000,
            16_000,
            Some("Vocabulary: hello"),
            &[],
        ));
        assert!(is_probable_asr_hallucination(
            "你们玩什么游戏？",
            32_000,
            16_000,
            Some("Vocabulary: Overwatch"),
            &["你们玩什么游戏？".into()],
        ));
        assert!(!is_probable_asr_hallucination(
            "Overwatch",
            16_000,
            16_000,
            Some("Vocabulary: Overwatch"),
            &[],
        ));
    }
}
