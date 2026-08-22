use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::json;

use crate::{
    AsrTranscript, AsyncHttpClient, InferenceError, OpenAiCompatibleClient, pcm16_mono_16khz_to_wav,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OpenAiAsrOptions {
    pub language: Option<String>,
    pub instruction_prompt: Option<String>,
    pub max_tokens: u32,
}

#[derive(Clone, Debug)]
pub struct OpenAiAsrAdapter<C> {
    chat: OpenAiCompatibleClient<C>,
    model: String,
}

impl<C> OpenAiAsrAdapter<C> {
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
        Ok(Self {
            chat: OpenAiCompatibleClient::with_bearer_token(http, endpoint, token)?,
            model,
        })
    }
}

impl<C: AsyncHttpClient> OpenAiAsrAdapter<C> {
    pub async fn transcribe_pcm16(
        &self,
        pcm: &[u8],
        options: OpenAiAsrOptions,
    ) -> Result<AsrTranscript, InferenceError> {
        let audio = STANDARD.encode(pcm16_mono_16khz_to_wav(pcm)?);
        let language = normalized(&options.language);
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
            language: language.map(str::to_owned),
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

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::{HttpRequest, HttpResponse, TransportError};

    #[derive(Default)]
    struct RecordingHttpClient {
        requests: Mutex<Vec<HttpRequest>>,
    }

    impl AsyncHttpClient for RecordingHttpClient {
        async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
            self.requests.lock().unwrap().push(request);
            Ok(HttpResponse {
                status: 200,
                body: r#"{"choices":[{"message":{"content":"hello"}}]}"#.into(),
            })
        }
    }

    #[tokio::test]
    async fn uses_openai_audio_contract_without_qwen_prefill() {
        let adapter = OpenAiAsrAdapter::with_bearer_token(
            RecordingHttpClient::default(),
            "https://api.openai.com/v1/chat/completions",
            "gpt-4o-transcribe",
            "secret",
        )
        .unwrap();
        let transcript = adapter
            .transcribe_pcm16(
                &[0, 0],
                OpenAiAsrOptions {
                    language: Some("English".into()),
                    instruction_prompt: Some("Transcribe names accurately: XRTranslate.".into()),
                    max_tokens: 256,
                },
            )
            .await
            .unwrap();
        assert_eq!(transcript.text, "hello");
        let request = adapter
            .chat
            .into_inner()
            .requests
            .into_inner()
            .unwrap()
            .remove(0);
        assert_eq!(request.body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(
            request.body["messages"][0]["content"],
            "Transcribe names accurately: XRTranslate."
        );
        assert_eq!(
            request.body["messages"][1]["content"][0]["type"],
            "input_audio"
        );
        assert!(!request.body.to_string().contains("<asr_text>"));
        assert!(!request.body.to_string().contains("Use this context"));
    }
}
