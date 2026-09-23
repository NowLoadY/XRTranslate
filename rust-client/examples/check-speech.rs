//! Replay a recorded speech sample through the native speak pipeline.

use std::{
    env, fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use futures::{SinkExt, StreamExt};
use serde_json::json;
use tokio::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use xrtranslate_assets::{ModelAssetId, manifest_for};
use xrtranslate_config::AppConfig;
use xrtranslate_prompt::PromptTemplateLibrary;
use xrtranslate_protocol::{AsrResultKind, PromptGraphSet, ServerEvent, VoiceClonePhase};

const INPUT_RATE: u32 = 16_000;
const CHUNK_BYTES: usize = 3_200;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let input = args.next().ok_or(
        "usage: check-speech <16k-mono-pcm16.wav> <source-lang> <target-lang> [output.wav]",
    )?;
    let source = args.next().ok_or("missing source language")?;
    let target = args.next().ok_or("missing target language")?;
    let output = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/check-speech.wav"));
    if args.next().is_some() {
        return Err("too many arguments".into());
    }

    let config_path = Path::new("config.json");
    let config = AppConfig::from_path_with_user_config(config_path, Path::new("."))?;
    let sample_rate = if config.tts.provider == "none" {
        eprintln!(
            "TTS provider is none; checking recognition and translation only. Select a TTS provider in XRTranslate settings to check synthesis."
        );
        None
    } else {
        Some(tts_sample_rate(&config).ok_or("configured TTS provider has no output sample rate")?)
    };
    let pcm = read_wav_pcm(Path::new(&input))?;
    let settings: serde_json::Value = fs::read("runtime/rust-client-settings.json")
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let recognition = &settings["microphone_recognition"];
    let background_noise = recognition["background_noise"].as_f64().unwrap_or(0.3);
    let pause_tolerance = recognition["pause_tolerance"].as_f64().unwrap_or(0.4);
    let vad_threshold = background_noise.clamp(0.2, 0.8);
    let vad_silence_ms = (240.0 + pause_tolerance.clamp(0.0, 1.0) * 960.0).round() as u32;
    let prompt_graphs = PromptGraphSet {
        graph: PromptTemplateLibrary::load_from_dir(Path::new("runtime")).active_graph(),
    };
    let url = settings["server_url"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| format!("ws://127.0.0.1:{}/ws", config.server.port));
    let (mut socket, _) = connect_async(&url).await.map_err(|error| {
        format!(
            "cannot connect to {url}: {error}; start a translation session in XRTranslate first"
        )
    })?;

    send(
        &mut socket,
        json!({
            "action": "session_config", "sample_rate": INPUT_RATE,
            "source_lang": &source, "target_lang": &target,
            "prompt_graphs": prompt_graphs
        }),
    )
    .await?;
    send(
        &mut socket,
        json!({
            "event": "config_audio", "sample_rate": INPUT_RATE,
            "source_lang": source, "target_lang": target,
            "audio_source": "microphone", "continuous_recognition": false,
            "vad_threshold": vad_threshold, "vad_silence_ms": vad_silence_ms,
            "workload": "realtime"
        }),
    )
    .await?;
    if sample_rate.is_some() {
        send(
            &mut socket,
            json!({"action": "toggle_feature", "feature": "tts", "enabled": true}),
        )
        .await?;
        send(&mut socket, json!({"action": "begin_voice_clone"})).await?;
    }
    send(
        &mut socket,
        json!({"event": "turn_started", "turn_id": "recorded-speech"}),
    )
    .await?;

    let started = Instant::now();
    let deadline = started + Duration::from_secs(180);
    let (mut writer, mut reader) = socket.split();
    let sender = tokio::spawn(async move {
        for chunk in pcm.chunks(CHUNK_BYTES) {
            writer.send(Message::Binary(chunk.to_vec().into())).await?;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        writer
            .send(Message::Text(
                json!({"event": "input_ended"}).to_string().into(),
            ))
            .await
    });

    let mut recognized = 0;
    let mut translated = 0;
    let mut clone_ready = false;
    let mut failure = None;
    let mut tts_pcm = Vec::new();
    loop {
        let message = match tokio::time::timeout_at(deadline, reader.next()).await {
            Ok(Some(Ok(message))) => message,
            result => {
                if let Some(rate) = sample_rate
                    && !tts_pcm.is_empty()
                {
                    write_wav(&output, rate, &tts_pcm)?;
                    eprintln!("Saved incomplete TTS audio: {}", output.display());
                }
                return Err(format!("backend closed before pipeline drain: {result:?}").into());
            }
        };
        match message {
            Message::Binary(bytes) => tts_pcm.extend_from_slice(&bytes),
            Message::Text(text) => match serde_json::from_str::<ServerEvent>(&text)? {
                ServerEvent::AsrResult(result) if result.kind == AsrResultKind::Final => {
                    recognized += usize::from(!result.text.trim().is_empty());
                    println!(
                        "ASR +{:.2}s: {}",
                        started.elapsed().as_secs_f64(),
                        result.text
                    );
                }
                ServerEvent::TranslationReady(result) => {
                    translated += usize::from(!result.translated_text.trim().is_empty());
                    println!(
                        "Translation +{:.2}s (ASR {}ms, translation {}ms, queue {}ms): {}",
                        started.elapsed().as_secs_f64(),
                        result.metrics.asr_ms,
                        result.metrics.mt_ms,
                        result.metrics.queue_ms,
                        result.translated_text
                    );
                }
                ServerEvent::VoiceCloneState(state) => match state.state {
                    VoiceClonePhase::Collecting => clone_ready = false,
                    VoiceClonePhase::Ready => {
                        clone_ready = true;
                        println!("Voice ready +{:.2}s", started.elapsed().as_secs_f64());
                    }
                    VoiceClonePhase::Failed => {
                        clone_ready = false;
                        let message = format!(
                            "voice registration failed: {}",
                            state.message.as_deref().unwrap_or("unknown error")
                        );
                        eprintln!("{message}");
                        failure = Some(message);
                    }
                    _ => {}
                },
                ServerEvent::TtsFinished(_) => {
                    println!("TTS chunk +{:.2}s", started.elapsed().as_secs_f64());
                }
                ServerEvent::Error(error) => {
                    let message = format!("backend: {}", error.message);
                    eprintln!("{message}");
                    failure = Some(message);
                }
                ServerEvent::PipelineDrained(_) => break,
                _ => {}
            },
            _ => {}
        }
    }
    sender.await??;
    if let Some(message) = failure {
        return Err(message.into());
    }

    if recognized == 0 || translated == 0 {
        return Err(
            "no completed recognition and translation; check backend output and the sample speech"
                .into(),
        );
    }
    let Some(sample_rate) = sample_rate else {
        return Err("TTS was not checked because its provider is none; configure one in XRTranslate settings and rerun".into());
    };
    if tts_pcm.is_empty() {
        return Err(format!(
            "no synthesized audio (voice ready: {clone_ready}); check TTS model, target language, and voice sample length"
        ).into());
    }
    if tts_pcm
        .chunks_exact(2)
        .all(|sample| i16::from_le_bytes([sample[0], sample[1]]) == 0)
    {
        return Err("synthesized audio is silent".into());
    }
    write_wav(&output, sample_rate, &tts_pcm)?;
    println!(
        "Completed in {:.2}s; {} ASR result(s), {} translation(s), TTS: {}",
        started.elapsed().as_secs_f64(),
        recognized,
        translated,
        output.display()
    );
    Ok(())
}

async fn send<S>(socket: &mut S, value: serde_json::Value) -> Result<(), Box<dyn std::error::Error>>
where
    S: SinkExt<Message> + Unpin,
    S::Error: std::error::Error + 'static,
{
    socket.send(Message::Text(value.to_string().into())).await?;
    Ok(())
}

fn tts_sample_rate(config: &AppConfig) -> Option<u32> {
    let provider = config.tts.provider_config(&config.tts.provider)?;
    provider
        .get("model_asset")
        .and_then(|value| value.as_str())
        .and_then(ModelAssetId::from_config_key)
        .and_then(|asset| manifest_for(asset).audio_output)
        .map(|audio| audio.sample_rate_hz)
        .or_else(|| {
            provider
                .get("sample_rate")
                .and_then(|value| value.as_u64())
                .and_then(|rate| u32::try_from(rate).ok())
        })
}

fn read_wav_pcm(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let wav = fs::read(path)?;
    if wav.len() < 12 || &wav[..4] != b"RIFF" || &wav[8..12] != b"WAVE" {
        return Err("input must be a RIFF WAV file".into());
    }
    let mut offset = 12;
    let mut format_ok = false;
    let mut data = None;
    while offset + 8 <= wav.len() {
        let len = u32::from_le_bytes(wav[offset + 4..offset + 8].try_into()?) as usize;
        let start = offset + 8;
        let end = start.checked_add(len).ok_or("invalid WAV chunk length")?;
        if end > wav.len() {
            return Err("truncated WAV chunk".into());
        }
        match &wav[offset..offset + 4] {
            b"fmt " if len >= 16 => {
                let codec = u16::from_le_bytes(wav[start..start + 2].try_into()?);
                let channels = u16::from_le_bytes(wav[start + 2..start + 4].try_into()?);
                let rate = u32::from_le_bytes(wav[start + 4..start + 8].try_into()?);
                let bits = u16::from_le_bytes(wav[start + 14..start + 16].try_into()?);
                format_ok = codec == 1 && channels == 1 && rate == INPUT_RATE && bits == 16;
            }
            b"data" => data = Some(wav[start..end].to_vec()),
            _ => {}
        }
        offset = end + (len & 1);
    }
    let pcm = data.ok_or("WAV has no data chunk")?;
    if !format_ok || pcm.is_empty() || pcm.len() % 2 != 0 {
        return Err("input must contain 16 kHz mono PCM16 audio".into());
    }
    Ok(pcm)
}

fn write_wav(path: &Path, sample_rate: u32, pcm: &[u8]) -> io::Result<()> {
    let data_len = u32::try_from(pcm.len()).map_err(io::Error::other)?;
    let riff_len = data_len
        .checked_add(36)
        .ok_or_else(|| io::Error::other("WAV too long"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::File::create(path)?;
    file.write_all(b"RIFF")?;
    file.write_all(&riff_len.to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?;
    file.write_all(&sample_rate.to_le_bytes())?;
    file.write_all(&(sample_rate * 2).to_le_bytes())?;
    file.write_all(&2_u16.to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_len.to_le_bytes())?;
    file.write_all(pcm)
}
