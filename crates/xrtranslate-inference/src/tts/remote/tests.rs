use super::*;
use crate::HttpResponse;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use xrtranslate_assets::remote::remote_model;

#[derive(Clone, Default)]
struct SpeechHttp {
    requests: Arc<Mutex<Vec<HttpRequest>>>,
    responses: Arc<Mutex<VecDeque<(HttpResponse, Vec<u8>)>>>,
}

impl SpeechHttp {
    fn respond(&self, status: u16, bytes: Vec<u8>, retry_after: Option<Duration>) {
        self.responses.lock().unwrap().push_back((
            HttpResponse {
                status,
                body: String::new(),
                retry_after,
            },
            bytes,
        ));
    }
}

impl AsyncHttpClient for SpeechHttp {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        self.requests.lock().unwrap().push(request);
        let (mut response, bytes) = self.responses.lock().unwrap().pop_front().unwrap();
        response.body = String::from_utf8(bytes).unwrap();
        Ok(response)
    }

    async fn execute_streaming<F, Fut>(
        &self,
        request: HttpRequest,
        mut on_chunk: F,
    ) -> Result<HttpResponse, TransportError>
    where
        F: FnMut(Vec<u8>) -> Fut + Send,
        Fut: Future<Output = Result<ControlFlow<()>, TransportError>> + Send,
    {
        self.requests.lock().unwrap().push(request);
        let (mut response, bytes) = self.responses.lock().unwrap().pop_front().unwrap();
        if (200..300).contains(&response.status) {
            for chunk in bytes.chunks(7) {
                if on_chunk(chunk.to_vec()).await?.is_break() {
                    break;
                }
            }
        } else {
            response.body = String::from_utf8(bytes).unwrap();
        }
        Ok(response)
    }
}

fn adapter(
    http: SpeechHttp,
    connector: RemoteConnector,
    model: &str,
) -> RemoteTtsAdapter<SpeechHttp> {
    RemoteTtsAdapter::new(
        http,
        "https://speech.example.test/synthesize".into(),
        "test-key".into(),
        remote_model(connector, model).unwrap(),
    )
}

fn wav(sample_rate: u32) -> Vec<u8> {
    let mut wav = crate::pcm16_mono_16khz_to_wav(&[0, 0, 1, 0, 255, 255, 0, 0]).unwrap();
    wav[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    wav[28..32].copy_from_slice(&(sample_rate * 2).to_le_bytes());
    wav
}

async fn synthesize(
    adapter: &RemoteTtsAdapter<SpeechHttp>,
    voice: &str,
    language: &str,
) -> Result<Vec<SynthesizedPcm>, InferenceError> {
    let mut chunks = Vec::new();
    adapter
        .synthesize("Hello, world.", voice, language, |chunk| {
            chunks.push(chunk);
            std::future::ready(Ok(()))
        })
        .await?;
    Ok(chunks)
}

#[tokio::test]
async fn qwen_reference_uses_the_active_workspace_and_can_be_cleaned_up() {
    let http = SpeechHttp::default();
    let voice = "qwen-audio-3.1-tts-plus-xrtmedia-test";
    for body in [
        json!({"output":{"voice_id":voice}}),
        json!({"output":{"status":"OK"}}),
    ] {
        http.respond(200, body.to_string().into_bytes(), None);
    }
    let adapter = adapter(
        http.clone(),
        RemoteConnector::DashscopeSpeech,
        "qwen-audio-3.1-tts-plus",
    );
    let reference = crate::pcm16_mono_16khz_to_wav(&vec![0; 16000 * 2 * 6]).unwrap();
    assert_eq!(
        adapter.create_reference_voice(&reference).await.unwrap(),
        voice
    );
    http.respond(
        200,
        json!({"output":{"finish_reason":"stop","audio":{"url":"https://audio.example.test/pcm"}}})
            .to_string()
            .into_bytes(),
        None,
    );
    http.respond(200, vec![0, 0, 1, 0], None);
    assert!(!synthesize(&adapter, voice, "zh").await.unwrap().is_empty());
    http.respond(200, b"{}".to_vec(), None);
    adapter.delete_reference_voice(voice).await;
    let requests = http.requests.lock().unwrap();
    assert_eq!(
        requests[0].url,
        "https://speech.example.test/api/v1/services/audio/tts/customization"
    );
    assert_eq!(
        requests[0].body["input"]["target_model"],
        "qwen-audio-3.1-tts-plus"
    );
    assert!(
        requests[0].body["input"]["audio"]
            .as_str()
            .unwrap()
            .starts_with("data:audio/wav;base64,")
    );
    assert_eq!(requests[2].body["input"]["voice"], voice);
    assert_eq!(requests[2].body["input"]["language_hints"], json!(["zh"]));
    assert_eq!(
        requests[2].body["input"]["instruction"],
        "Speak in Chinese. Read only the provided text, without translating it or adding content."
    );
    assert_eq!(
        requests.last().unwrap().body["input"]["action"],
        "delete_voice"
    );
}

#[tokio::test]
async fn qwen_rejects_short_references_and_removes_failed_enrollments() {
    let http = SpeechHttp::default();
    let adapter = adapter(
        http.clone(),
        RemoteConnector::DashscopeSpeech,
        "qwen-audio-3.1-tts-plus",
    );
    assert!(adapter.create_reference_voice(&wav(24000)).await.is_err());
    assert!(http.requests.lock().unwrap().is_empty());
    for body in [
        json!({"output":{"voice_id":"qwen-audio-3.1-tts-plus-xrtmedia-test"}}),
        json!({"output":{"status":"UNDEPLOYED"}}),
        json!({}),
    ] {
        http.respond(200, body.to_string().into_bytes(), None);
    }
    let reference = crate::pcm16_mono_16khz_to_wav(&vec![0; 16000 * 2 * 6]).unwrap();
    assert!(adapter.create_reference_voice(&reference).await.is_err());
    assert_eq!(
        http.requests.lock().unwrap().last().unwrap().body["input"]["action"],
        "delete_voice"
    );
}

#[tokio::test]
async fn qwen_flash_has_more_voices_and_checks_the_selected_voices_language() {
    let http = SpeechHttp::default();
    let adapter = adapter(
        http.clone(),
        RemoteConnector::DashscopeSpeech,
        "qwen-audio-3.1-tts-flash",
    );
    assert_eq!(adapter.capabilities().voices.len(), 88);
    assert_eq!(
        xrtranslate_assets::remote::preset_voice_language("qiaoxiaojiao_v3.1"),
        "zh"
    );
    assert_eq!(
        xrtranslate_assets::remote::preset_voice_language("Emily_v3.1"),
        "en"
    );
    assert!(synthesize(&adapter, "Emily_v3.1", "zh").await.is_err());
    assert!(http.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn minimax_returns_complete_audio_with_selected_voice_and_language() {
    let http = SpeechHttp::default();
    let encoded = wav(24000)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    http.respond(
        200,
        json!({"base_resp":{"status_code":0}, "data":{"status":2, "audio":encoded}})
            .to_string()
            .into_bytes(),
        None,
    );
    let adapter = adapter(
        http.clone(),
        RemoteConnector::MinimaxSpeech,
        "speech-2.8-hd",
    );
    let voice = &adapter.capabilities().voices[0];
    let chunks = synthesize(&adapter, voice, "zh-TW").await.unwrap();
    assert_eq!(chunks, [decode_pcm16_wav(&wav(24000)).unwrap()]);
    let requests = http.requests.lock().unwrap();
    let request = &requests[0];
    assert_eq!(request.body["stream"], false);
    assert_eq!(request.body["voice_setting"]["voice_id"], *voice);
    assert_eq!(request.body["language_boost"], "Chinese");
    assert_eq!(request.body["audio_setting"]["format"], "wav");
    assert_eq!(request.body["audio_setting"]["sample_rate"], 24000);
    assert!(
        request
            .headers
            .contains(&("authorization".into(), "Bearer test-key".into()))
    );
}

#[tokio::test]
async fn minimax_rate_limits_wait_but_invalid_keys_and_empty_balances_do_not() {
    for (code, expected) in [(1002, 429), (1045, 429), (1004, 401), (1008, 402)] {
        let http = SpeechHttp::default();
        let wait = Some(Duration::from_secs(90));
        http.respond(
            200,
            json!({"base_resp":{"status_code":code, "status_msg":"service error"}})
                .to_string()
                .into_bytes(),
            wait,
        );
        let adapter = adapter(http, RemoteConnector::MinimaxSpeech, "speech-2.8-turbo");
        let error = synthesize(&adapter, &adapter.capabilities().voices[0], "en")
            .await
            .unwrap_err();
        assert!(
            matches!(error, InferenceError::HttpStatus { status, retry_after, .. } if status == expected && retry_after == wait)
        );
    }
}

#[tokio::test]
async fn minimax_rejects_incomplete_and_malformed_audio() {
    for (status, audio) in [(1, "1234"), (2, "123"), (2, "zz"), (2, "")] {
        let http = SpeechHttp::default();
        http.respond(
            200,
            json!({"base_resp":{"status_code":0}, "data":{"status":status, "audio":audio}})
                .to_string()
                .into_bytes(),
            None,
        );
        let adapter = adapter(http, RemoteConnector::MinimaxSpeech, "speech-2.8-hd");
        assert!(matches!(
            synthesize(&adapter, &adapter.capabilities().voices[0], "en").await,
            Err(InferenceError::InvalidResponse { .. })
        ));
    }
}

#[tokio::test]
async fn dashscope_downloads_pcm_without_forwarding_credentials() {
    let http = SpeechHttp::default();
    http.respond(200, json!({"output":{"finish_reason":"stop","audio":{"url":"https://audio.example.test/result.wav?signature=private"}}}).to_string().into_bytes(), None);
    let pcm = decode_pcm16_wav(&wav(48000)).unwrap();
    http.respond(200, pcm.bytes.clone(), None);
    let adapter = adapter(
        http.clone(),
        RemoteConnector::DashscopeSpeech,
        "qwen-audio-3.1-tts-plus",
    );
    assert_eq!(
        synthesize(&adapter, "longanlingxin_v3.1", "ja-JP")
            .await
            .unwrap(),
        [pcm]
    );
    let requests = http.requests.lock().unwrap();
    assert_eq!(requests[0].body["input"]["language_hints"], json!(["ja"]));
    assert_eq!(
        requests[0].body["input"]["instruction"],
        "Speak in Japanese. Read only the provided text, without translating it or adding content."
    );
    assert_eq!(requests[0].body["input"]["format"], "pcm");
    assert_eq!(requests[0].body["input"]["sample_rate"], 48000);
    assert_eq!(requests[0].body["input"]["voice"], "longanlingxin_v3.1");
    assert_eq!(requests[1].method, "GET");
    assert!(requests[1].headers.is_empty());
    assert!(requests[1].body.is_null());
}

#[tokio::test]
async fn dashscope_rejects_empty_or_incomplete_pcm() {
    for bytes in [vec![], vec![0, 0, 1]] {
        let http = SpeechHttp::default();
        http.respond(200, json!({"output":{"finish_reason":"stop","audio":{"url":"https://audio.example.test/result.pcm"}}}).to_string().into_bytes(), None);
        http.respond(200, bytes, None);
        let adapter = adapter(
            http,
            RemoteConnector::DashscopeSpeech,
            "qwen-audio-3.1-tts-plus",
        );
        assert!(matches!(
            synthesize(&adapter, "longanlingxin_v3.1", "zh").await,
            Err(InferenceError::InvalidAudio { .. })
        ));
    }
}

#[tokio::test]
async fn dashscope_does_not_download_unfinished_or_invalid_results() {
    for (reason, url) in [
        ("length", "https://audio.example.test/result.wav"),
        ("stop", "file:///private.wav"),
        ("stop", ""),
    ] {
        let http = SpeechHttp::default();
        http.respond(
            200,
            json!({"output":{"finish_reason":reason,"audio":{"url":url}}})
                .to_string()
                .into_bytes(),
            None,
        );
        let adapter = adapter(
            http.clone(),
            RemoteConnector::DashscopeSpeech,
            "qwen-audio-3.1-tts-plus",
        );
        assert!(matches!(
            synthesize(&adapter, "longanlingxin_v3.1", "zh").await,
            Err(InferenceError::InvalidResponse { .. })
        ));
        assert_eq!(http.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn openai_speech_reuses_binary_audio_for_domestic_and_existing_providers() {
    for (model, voice) in [
        (
            "FunAudioLLM/CosyVoice2-0.5B",
            "FunAudioLLM/CosyVoice2-0.5B:anna",
        ),
        ("canopylabs/orpheus-v1-english", "troy"),
    ] {
        let http = SpeechHttp::default();
        http.respond(200, wav(24000), None);
        let adapter = adapter(http.clone(), RemoteConnector::OpenaiSpeech, model);
        assert_eq!(
            synthesize(&adapter, voice, "en").await.unwrap(),
            [decode_pcm16_wav(&wav(24000)).unwrap()]
        );
        let requests = http.requests.lock().unwrap();
        assert_eq!(requests[0].body["voice"], voice);
        assert_eq!(requests[0].body["response_format"], "wav");
    }
}

#[tokio::test]
async fn gemini_keeps_streaming_audio_and_rate_limit_delays() {
    let http = SpeechHttp::default();
    let event = json!({"event_type":"step.delta", "delta":{"type":"audio", "data":STANDARD.encode([0,0,1,0]),"mime_type":"audio/l16"}});
    http.respond(200, format!("data: {event}\n\ndata: {{\"event_type\":\"interaction.completed\",\"interaction\":{{\"status\":\"completed\"}}}}\n\n").into_bytes(), None);
    let adapter = adapter(
        http.clone(),
        RemoteConnector::GeminiSpeech,
        "gemini-3.8-flash-tts",
    );
    assert_eq!(
        synthesize(&adapter, "Kore", "zh").await.unwrap()[0].bytes,
        [0, 0, 1, 0]
    );
    http.respond(
        429,
        b"Rate limit exceeded".to_vec(),
        Some(Duration::from_secs(31882)),
    );
    assert!(
        matches!(synthesize(&adapter, "Kore", "zh").await, Err(InferenceError::HttpStatus {status:429, retry_after:Some(delay), ..}) if delay.as_secs() == 31882)
    );
}
