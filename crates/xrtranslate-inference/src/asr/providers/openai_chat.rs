use crate::asr::types::parse_language;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use xrtranslate_assets::remote::{AsrResponseFormat, RemoteCapabilities, RemoteConnector};

use crate::{
    AsrOptions, AsrTranscript, AsyncHttpClient, InferenceError, OpenAiCompatibleClient,
    pcm16_mono_16khz_to_wav,
};

#[derive(Clone, Debug)]
pub struct OpenAiAsrAdapter<C> {
    chat: OpenAiCompatibleClient<C>,
    model: String,
    connector: RemoteConnector,
    language_hint: bool,
    response_format: Option<AsrResponseFormat>,
    reasoning_effort: Option<String>,
}

impl<C> OpenAiAsrAdapter<C> {
    pub fn new(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        token: impl Into<String>,
        connector: RemoteConnector,
        capabilities: &RemoteCapabilities,
    ) -> Result<Self, InferenceError> {
        if !matches!(
            connector,
            RemoteConnector::OpenaiTranscription | RemoteConnector::OpenaiAudioChat
        ) {
            return Err(InferenceError::InvalidConfiguration {
                field: "connector",
                message: "expected an OpenAI audio connector".into(),
            });
        }
        let model = model.into().trim().to_owned();
        if model.is_empty() {
            return Err(InferenceError::InvalidConfiguration {
                field: "model",
                message: "must not be empty".into(),
            });
        }
        Ok(Self {
            chat: OpenAiCompatibleClient::with_bearer_token(http, endpoint, token)?,
            model,
            connector,
            language_hint: capabilities.asr_language_hint.unwrap_or(true),
            response_format: capabilities.asr_response_format,
            reasoning_effort: capabilities.reasoning_effort.clone(),
        })
    }
}

impl<C: AsyncHttpClient> OpenAiAsrAdapter<C> {
    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: AsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        let wav = pcm16_mono_16khz_to_wav(pcm)?;
        let language = parse_language(options.language.as_deref())?;
        if self.connector == RemoteConnector::OpenaiTranscription {
            // JSON and temperature zero are the protocol defaults. Keep the
            // mandatory request usable by endpoints with only file/model fields.
            let mut fields = vec![("model".into(), self.model.clone())];
            if let Some(format) = self.response_format {
                fields.push(("response_format".into(), format.as_str().into()));
            }
            if self.language_hint
                && let Some(language) = language
            {
                fields.push(("language".into(), language.base_code().into()));
            }
            if let Some(context) = normalized(&options.context_bias) {
                fields.push(("prompt".into(), context.into()));
            }
            let mut transcript = self.chat.audio_transcription(fields, wav).await?;
            transcript.language = transcript
                .language
                .or_else(|| language.map(|language| language.code().to_owned()));
            return Ok(transcript);
        }
        let audio = STANDARD.encode(wav);
        let instruction = normalized(&options.instruction_prompt);
        let mut messages = Vec::new();
        if let Some(instruction) = instruction {
            messages.push(json!({"role": "system", "content": instruction}));
        }
        messages.push(json!({"role": "user", "content": [{
            "type": "input_audio",
            "input_audio": {"data": audio, "format": "wav"}
        }]}));
        let mut payload = json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0,
            "max_tokens": options.max_tokens.max(1),
            "stream": false
        });
        if let Some(reasoning_effort) = &self.reasoning_effort {
            payload["reasoning_effort"] = json!(reasoning_effort);
        }
        let completion = self.chat.chat_completion(payload).await?;
        Ok(AsrTranscript {
            language: language.map(|language| language.code().to_owned()),
            text: completion.text.trim().to_owned(),
        })
    }
}

fn normalized(value: &Option<String>) -> Option<&str> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
