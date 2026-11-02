//! Manual acceptance: log in with XRTranslate, then supply a non-private test
//! utterance as 16 kHz mono s16le PCM. Never prints session credentials.
use std::{path::PathBuf, time::Instant};
use xrtranslate_connections::AccountStore;
use xrtranslate_inference::{AsrOptions, DoubaoWebAsrAdapter};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: account_asr_smoke PROJECT_ROOT PCM16_FILE".into());
    }
    let root = PathBuf::from(&args[0]);
    let pcm = std::fs::read(&args[1])?;
    let duration = pcm.len() as f64 / 32000.0;
    let adapter = DoubaoWebAsrAdapter::new(
        "wss://ws-samantha.doubao.com/samantha/audio/asr",
        "doubao-web-speech",
        AccountStore::for_project_root(root),
    )?;
    let started = Instant::now();
    let frames =
        futures_util::stream::unfold((pcm.as_slice(), true), |(remaining, first)| async move {
            if remaining.is_empty() {
                return None;
            }
            if !first {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let end = remaining.len().min(3200);
            Some((remaining[..end].to_vec(), (&remaining[end..], false)))
        });
    let result = adapter
        .transcribe_stream(
            frames,
            AsrOptions {
                language: Some("zh".into()),
                ..Default::default()
            },
            |snapshot| {
                println!(
                    "preview_seconds={:.2} text={snapshot}",
                    started.elapsed().as_secs_f64()
                );
                async {}
            },
        )
        .await;
    println!(
        "audio_seconds={duration:.2} request_seconds={:.2}",
        started.elapsed().as_secs_f64()
    );
    match result {
        Ok(transcript) => println!("completed_transcript={}", transcript.text),
        Err(error) => {
            eprintln!("{error}");
            return Err("Account ASR acceptance failed".into());
        }
    }
    Ok(())
}
