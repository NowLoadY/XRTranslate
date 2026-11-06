use super::*;
use std::time::{Duration, Instant};

// Exercise actual libmpv notifications without a display or audio device.
pub fn player() -> MpvBackend {
    let mpv = MpvHandle::create().unwrap();
    for (name, value) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("vo", "null"),
        ("ao", "null"),
        ("pause", "yes"),
        ("keep-open", "always"),
        ("msg-level", "all=no"),
    ] {
        mpv.set_option_string(name, value).unwrap();
    }
    mpv.initialize().unwrap();
    mpv.observe_properties().unwrap();
    MpvBackend {
        #[cfg(any(windows, target_os = "linux"))]
        video: None,
        mpv,
        status: PlaybackStatus::Stopped,
        snapshot: PlaybackSnapshot::default(),
        error: None,
        active_channels: Vec::new(),
        audio_selection: Default::default(),
        audio_loading: Default::default(),
        applied_audio: None,
        audio_reloads: 0,
        audio_resume: None,
        loading: false,
        subtitle_style: Default::default(),
        subtitle_text: String::new(),
    }
}

#[track_caller]
fn wait_until(player: &mut MpvBackend, condition: impl Fn(&MpvBackend) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        player.tick();
        if condition(player) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "state did not settle: {:?}, position {}, duration {}, error {:?}",
            player.status,
            player.get_time_ms(),
            player.get_duration_ms(),
            player.error
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub fn silent_wav(path: &std::path::Path) {
    let data_size = 8_000u32 * 2 * 2 * 4; // Four seconds of stereo PCM16.
    let mut data = Vec::new();
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + data_size).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&8_000u32.to_le_bytes());
    data.extend_from_slice(&32_000u32.to_le_bytes());
    data.extend_from_slice(&4u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&data_size.to_le_bytes());
    data.resize(data.len() + data_size as usize, 0);
    std::fs::write(path, data).unwrap();
}

#[test]
#[ignore = "requires the installed libmpv playback runtime"]
fn observed_playback_survives_reload_seek_and_eof() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("silent.wav");
    silent_wav(&source);
    let mut player = player();
    player.load_local_file(source.clone()).unwrap();
    wait_until(&mut player, |p| {
        p.get_duration_ms() == 4_000
            && p.get_audio_channel_count() == Some(2)
            && p.get_status() == PlaybackStatus::Paused
    });

    // An unchanged pause value need not be emitted again after a new load.
    player.stop();
    player.load_local_file(source).unwrap();
    player.play();
    wait_until(&mut player, |p| {
        p.get_status() == PlaybackStatus::Playing && p.get_time_ms() > 200
    });
    player.pause();
    wait_until(&mut player, |p| {
        // Confirm that the core has handled pause before reopening the file.
        let mut paused = 0i32;
        let error = unsafe {
            libmpv_sys::mpv_get_property(
                p.mpv.handle,
                c"pause".as_ptr(),
                libmpv_sys::mpv_format_MPV_FORMAT_FLAG,
                &mut paused as *mut _ as *mut _,
            )
        };
        error >= 0 && paused != 0
    });
    player.stop();
    player
        .load_local_file(directory.path().join("silent.wav"))
        .unwrap();
    wait_until(&mut player, |p| {
        p.get_status() == PlaybackStatus::Paused
            && p.get_duration_ms() == 4_000
            && p.get_audio_channel_count() == Some(2)
            // The UI can already show the requested state. Wait for the new
            // file to actually replace the old paused position before seeking.
            && p.mpv.get_property_f64("time-pos").is_some_and(|time| time < 0.01)
    });
    player.seek(3_500);
    player.play();
    wait_until(&mut player, |p| {
        p.get_status() == PlaybackStatus::Stopped && p.get_time_ms() >= 3_500
    });
    player.play();
    wait_until(&mut player, |p| {
        p.get_status() == PlaybackStatus::Playing && (100..1_000).contains(&p.get_time_ms())
    });
}

#[test]
#[ignore = "requires the installed libmpv playback runtime"]
fn asynchronous_load_failure_reaches_the_controller() {
    let directory = tempfile::tempdir().unwrap();
    let mut player = player();
    player
        .load_local_file(directory.path().join("missing.wav"))
        .unwrap();
    wait_until(&mut player, |p| p.error.is_some());
    assert_eq!(player.get_status(), PlaybackStatus::Stopped);
    assert!(!player.take_error().unwrap().is_empty());
    assert!(player.take_error().is_none());
}

#[test]
#[ignore = "requires the installed libmpv playback runtime"]
fn mixed_tracks_share_playback_seek_and_reload() {
    use crate::playback::PlaybackAudio;
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.wav");
    let background = directory.path().join("background.wav");
    let speech = directory.path().join("speech.wav");
    for path in [&source, &background, &speech] {
        silent_wav(path);
    }
    let mut player = player();
    player.load_local_file(source.clone()).unwrap();
    let mix = PlaybackAudio::Mix(vec![
        PlaybackAudio::External(background.clone()),
        PlaybackAudio::External(speech.clone()),
    ]);
    player.select_audio(mix.clone()).unwrap();
    wait_until(&mut player, |p| {
        p.snapshot
            .audio_tracks
            .iter()
            .filter(|t| t.selected)
            .count()
            == 2
    });
    assert!(
        player
            .snapshot
            .audio_tracks
            .iter()
            .filter(|t| t.selected)
            .all(|t| t.external.is_some())
    );
    assert!(player.error.is_none(), "{:?}", player.error);
    player.seek(1_000);
    player.play();
    wait_until(&mut player, |p| (1_100..2_000).contains(&p.get_time_ms()));
    player.pause();
    player.refresh_audio().unwrap();
    // Allow asynchronous reloads, including the track-list changes they generate.
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
        player.tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(player.error.is_none(), "{:?}", player.error);
    assert_eq!(
        player
            .snapshot
            .audio_tracks
            .iter()
            .filter(|t| t.selected)
            .count(),
        2
    );
    assert_eq!(player.get_status(), PlaybackStatus::Paused);
    player
        .select_audio(PlaybackAudio::External(speech))
        .unwrap();
    wait_until(&mut player, |p| {
        p.snapshot
            .audio_tracks
            .iter()
            .filter(|t| t.selected)
            .count()
            == 1
    });
    assert_eq!(player.get_status(), PlaybackStatus::Paused);
    player.select_audio(mix).unwrap();
    wait_until(&mut player, |p| {
        p.snapshot
            .audio_tracks
            .iter()
            .filter(|t| t.selected)
            .count()
            == 2
    });
    assert_eq!(
        player.snapshot.audio_tracks.len(),
        3,
        "switching must reuse external tracks"
    );
    assert_eq!(player.get_status(), PlaybackStatus::Paused);
    player.play();
    wait_until(&mut player, |p| {
        p.get_status() == PlaybackStatus::Playing && p.get_time_ms() > 1_300
    });
    player.refresh_audio().unwrap();
    wait_until(&mut player, |p| {
        p.audio_resume.is_none()
            && p.get_status() == PlaybackStatus::Playing
            && p.get_time_ms() > 1_500
    });
    player.refresh_audio().unwrap();
    player.pause();
    player.seek(2_500);
    wait_until(&mut player, |p| {
        p.audio_resume.is_none()
            && p.get_status() == PlaybackStatus::Paused
            && (2_400..2_700).contains(&p.get_time_ms())
    });
    player.select_audio(PlaybackAudio::Mix(Vec::new())).unwrap();
    wait_until(&mut player, |p| {
        p.snapshot.audio_tracks.iter().all(|t| !t.selected)
    });
    player.load_local_file(source).unwrap();
    player.select_audio(PlaybackAudio::Original(None)).unwrap();
    wait_until(&mut player, |p| {
        p.snapshot.audio_tracks.len() == 1 && p.snapshot.audio_tracks[0].selected
    });
    assert!(player.error.is_none(), "{:?}", player.error);
}
