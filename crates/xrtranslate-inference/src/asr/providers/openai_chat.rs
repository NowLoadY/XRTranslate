use crate::asr::types::parse_language;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;
use xrtranslate_assets::remote::RemoteConnector;

use crate::{
    AsrOptions, AsrTranscript, AsyncHttpClient, InferenceError, OpenAiCompatibleClient,
    pcm16_mono_16khz_to_wav,
};

#[derive(Clone, Debug)]
pub struct OpenAiAsrAdapter<C> {
    chat: OpenAiCompatibleClient<C>,
    model: String,
    connector: RemoteConnector,
}

impl<C> OpenAiAsrAdapter<C> {
    pub fn new(
        http: C,
        endpoint: impl Into<String>,
        model: impl Into<String>,
        token: impl Into<String>,
        connector: RemoteConnector,
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
            let mut fields = vec![
                ("model".into(), self.model.clone()),
                ("response_format".into(), "json".into()),
                ("temperature".into(), "0".into()),
            ];
            if let Some(language) = language {
                fields.push(("language".into(), language.base_code().into()));
            }
            if let Some(context) = normalized(&options.context_bias) {
                fields.push(("prompt".into(), context.into()));
            }
            let text = self.chat.audio_transcription(fields, wav).await?;
            return Ok(AsrTranscript {
                language: language.map(|language| language.code().to_owned()),
                text: text.trim().to_owned(),
            });
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
        let payload = json!({
            "model": self.model,
            "messages": messages,
            "temperature": 0,
            "max_tokens": options.max_tokens.max(1),
            "stream": false
        });
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
