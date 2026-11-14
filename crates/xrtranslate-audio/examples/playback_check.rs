//! Manual output-device check: cargo run --example playback_check --release -- clip.mp3
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use xrtranslate_audio::AudioPlayer;

fn main() -> Result<(), String> {
    for path in std::env::args_os().skip(1).map(PathBuf::from) {
        let mut player = AudioPlayer::new()?;
        player.load_local_file(path.clone())?;
        player.play();
        let deadline = Instant::now() + Duration::from_secs(5);
        while player.get_time_ms() < 250 {
            if let Some(error) = player.take_error() {
                return Err(error);
            }
            if Instant::now() >= deadline {
                return Err("Audio output did not advance".into());
            }
            thread::sleep(Duration::from_millis(10));
        }
        player.stop();
        thread::sleep(Duration::from_millis(50));
        let stopped = player.get_time_ms();
        thread::sleep(Duration::from_millis(100));
        assert_eq!(
            player.get_time_ms(),
            stopped,
            "Playback continued after stop"
        );
        player.play();
        let before = Instant::now();
        drop(player);
        assert!(
            before.elapsed() < Duration::from_millis(250),
            "Closing playback stalled"
        );
        println!("Output, stop and close: {}", path.display());
    }
    Ok(())
}
