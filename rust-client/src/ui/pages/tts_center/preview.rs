use xrtranslate_protocol::tts::{SpeechRequest, VoiceSelection};

pub(super) fn phrase(voice: &str) -> (&str, &str) {
    let language = xrtranslate_assets::remote::preset_voice_language(voice);
    let text = match language {
        "zh" => "你好，很高兴认识你",
        "ja" => "こんにちは、お会いできてうれしいです。",
        "ko" => "안녕하세요, 만나서 반갑습니다.",
        "fr" => "Bonjour, je suis ravi de vous rencontrer.",
        "de" => "Hallo, schön, dich kennenzulernen.",
        "es" => "Hola, mucho gusto en conocerte.",
        "pt" => "Olá, é um prazer conhecer você.",
        "it" => "Ciao, piacere di conoscerti.",
        "ru" => "Здравствуйте, приятно познакомиться.",
        "ar" => "مرحباً، سعيد بلقائك.",
        _ => "Hello, nice to meet you.",
    };
    (language, text)
}

pub(super) fn generate(server: &str, voice: &str) -> Result<(Vec<u8>, u32), String> {
    let (language, text) = phrase(voice);
    let wav = super::transport::bytes(server, "/tts/synthesize", |client, url| {
        client.post(url).json(&SpeechRequest {
            text: text.into(),
            language: language.into(),
            voice: Some(VoiceSelection {
                preset: Some(voice.into()),
                ..Default::default()
            }),
            reference_audio: None,
        })
    })?;
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).map_err(|e| e.to_string())?;
    let spec = reader.spec();
    if spec.channels != 1
        || spec.bits_per_sample != 16
        || spec.sample_format != hound::SampleFormat::Int
        || spec.sample_rate == 0
    {
        return Err("The speech service returned unsupported preview audio.".into());
    }
    let samples = reader
        .samples::<i16>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    if samples.is_empty() {
        return Err("The speech service returned no preview audio.".into());
    }
    Ok((
        samples.into_iter().flat_map(i16::to_le_bytes).collect(),
        spec.sample_rate,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read, Write};

    #[test]
    fn audition_synthesizes_the_greeting_without_selecting_a_voice() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&mut socket);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.starts_with("POST /tts/synthesize "));
            let mut length = 0;
            loop {
                line.clear();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap();
                }
            }
            let mut body = vec![0; length];
            reader.read_exact(&mut body).unwrap();
            let request: SpeechRequest = serde_json::from_slice(&body).unwrap();
            assert_eq!(request.text, "你好，很高兴认识你");
            assert_eq!(request.language, "zh");
            assert_eq!(
                request.voice.unwrap().preset.as_deref(),
                Some("longanlingxin_v3.1")
            );
            assert!(request.reference_audio.is_none());
            let mut wav = std::io::Cursor::new(Vec::new());
            let mut writer = hound::WavWriter::new(
                &mut wav,
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 24000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )
            .unwrap();
            writer.write_sample(123_i16).unwrap();
            writer.finalize().unwrap();
            let wav = wav.into_inner();
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                wav.len()
            )
            .unwrap();
            socket.write_all(&wav).unwrap();
        });
        assert_eq!(
            generate(&server, "longanlingxin_v3.1").unwrap(),
            (vec![123, 0], 24000)
        );
        worker.join().unwrap();
        assert_eq!(phrase("Emily_v3.1"), ("en", "Hello, nice to meet you."));
    }
}
