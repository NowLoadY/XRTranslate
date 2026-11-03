use super::*;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    fn new(reply: impl Fn(String, TcpStream) + Send + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        stream
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut header = Vec::new();
                        while header.len() < 8192 && !header.ends_with(b"\r\n\r\n") {
                            let mut byte = [0];
                            if stream.read(&mut byte).unwrap_or(0) == 0 {
                                break;
                            }
                            header.push(byte[0]);
                        }
                        reply(String::from_utf8_lossy(&header).into_owned(), stream);
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

fn wav(seconds: u32) -> Vec<u8> {
    let size = seconds * 16_000 * 2;
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + size).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(16_000u32.to_le_bytes());
    wav.extend(32_000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(size.to_le_bytes());
    for index in 0..size / 2 {
        wav.extend(((index % 100) as i16 * 100).to_le_bytes());
    }
    wav
}

fn terminal(import: &AudioImportHandle) -> AudioImportEvent {
    loop {
        let event = import
            .events()
            .recv_timeout(Duration::from_secs(3))
            .unwrap();
        if matches!(
            event,
            AudioImportEvent::Completed { .. }
                | AudioImportEvent::Error(_)
                | AudioImportEvent::Stopped { .. }
        ) {
            return event;
        }
    }
}

#[test]
fn online_audio_delivers_frames_before_download_finishes() {
    let (release, wait) = crossbeam_channel::bounded(1);
    let server = Server::new(move |_, mut stream| {
        let wav = wav(8);
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", wav.len()).unwrap();
        stream.write_all(&wav[..128_000]).unwrap();
        wait.recv_timeout(Duration::from_secs(5)).unwrap();
        stream.write_all(&wav[128_000..]).unwrap();
    });
    let (tx, rx) = crossbeam_channel::bounded(4);
    let import = import_audio_url(
        format!("{}/audio.wav", server.url),
        String::new(),
        tx,
        AudioImportOptions::for_recognition(xrtranslate_protocol::AudioPreprocessing::None),
    )
    .unwrap();
    let first = rx
        .recv_timeout(Duration::from_secs(3))
        .expect("audio must arrive before the remaining bytes");
    assert!(first.iter().any(|value| value.abs() > 0.01));
    release.send(()).unwrap();
    let mut frames = first.len();
    while let Ok(chunk) = rx.recv_timeout(Duration::from_secs(3)) {
        frames += chunk.len();
    }
    assert_eq!(frames, 8 * 16_000);
    assert!(matches!(
        terminal(&import),
        AudioImportEvent::Completed { .. }
    ));
}

#[test]
fn truncated_http_audio_fails_instead_of_completing() {
    let server = Server::new(|_, mut stream| {
        let wav = wav(4);
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            wav.len()
        )
        .unwrap();
        stream.write_all(&wav[..70_000]).unwrap();
    });
    let (tx, rx) = crossbeam_channel::bounded(4);
    let import = import_audio_url(
        format!("{}/audio.wav", server.url),
        String::new(),
        tx,
        AudioImportOptions::for_recognition(xrtranslate_protocol::AudioPreprocessing::None),
    )
    .unwrap();
    while rx.recv_timeout(Duration::from_secs(3)).is_ok() {}
    assert!(matches!(terminal(&import), AudioImportEvent::Error(_)));
}

#[test]
fn stalled_network_start_is_cancellable() {
    let (accepted, request) = crossbeam_channel::bounded(1);
    let (release, wait) = crossbeam_channel::bounded(1);
    let server = Server::new(move |_, _stream| {
        accepted.send(()).unwrap();
        let _ = wait.recv_timeout(Duration::from_secs(3));
    });
    let (tx, _rx) = crossbeam_channel::bounded(4);
    let import = import_audio_url(
        format!("{}/audio.mp3", server.url),
        String::new(),
        tx,
        AudioImportOptions::default(),
    )
    .unwrap();
    request.recv_timeout(Duration::from_secs(3)).unwrap();
    let start = Instant::now();
    import.stop();
    assert!(matches!(
        import
            .events()
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        AudioImportEvent::Stopped { .. }
    ));
    assert!(start.elapsed() < Duration::from_secs(1));
    let _ = release.send(());
}

#[test]
fn ranged_audio_seeks_without_replaying_prefix() {
    let server = Server::new(|request, mut stream| {
        let offset: usize = request
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("range: bytes=")
                    .and_then(|value| value.split('-').next())
                    .and_then(|value| value.parse().ok())
            })
            .unwrap_or(0);
        let bytes: Vec<u8> = (0..200).collect();
        write!(stream, "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {offset}-199/200\r\nConnection: close\r\n\r\n", 200 - offset).unwrap();
        stream.write_all(&bytes[offset..]).unwrap();
    });
    let mut source =
        super::http::HttpAudioSource::open(&server.url, "", Arc::new(AtomicBool::new(false)))
            .unwrap();
    let mut bytes = [0; 3];
    source.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, [0, 1, 2]);
    source.seek(SeekFrom::End(-3)).unwrap();
    source.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, [197, 198, 199]);
    source.seek(SeekFrom::Start(5)).unwrap();
    source.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, [5, 6, 7]);
}

#[cfg(feature = "mpv")]
#[test]
fn mpv_network_audio_applies_channel_selection_before_downmixing() {
    let server = Server::new(|_, mut stream| {
        let mut audio = wav(2);
        // One second of stereo: silent left channel, audible right channel.
        audio[22..24].copy_from_slice(&2u16.to_le_bytes());
        audio[28..32].copy_from_slice(&64_000u32.to_le_bytes());
        audio[32..34].copy_from_slice(&4u16.to_le_bytes());
        for frame in audio[44..].chunks_exact_mut(4) {
            frame[..2].copy_from_slice(&0i16.to_le_bytes());
            frame[2..].copy_from_slice(&8_192i16.to_le_bytes());
        }
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", audio.len()).unwrap();
        stream.write_all(&audio).unwrap();
    });
    for (channels, selected) in [(1, vec![0]), (1, vec![1]), (2, vec![1]), (2, vec![0, 1])] {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let import = import_audio_url(
            format!("{}/stereo.wav", server.url),
            String::new(),
            tx,
            AudioImportOptions {
                output_channels: channels,
                audio_track: Some(1),
                recognition_channels: selected.clone(),
                gate_threshold_db: None,
                pacing: AudioImportPacing::AsFastAsPossible,
                ..Default::default()
            },
        )
        .unwrap();
        let mut samples = Vec::new();
        while let Ok(chunk) = rx.recv_timeout(Duration::from_secs(3)) {
            samples.extend(chunk);
        }
        assert!(matches!(
            terminal(&import),
            AudioImportEvent::Completed { .. }
        ));
        // MPV can pad the final codec block; it must still end near one second.
        assert!((16_000..=17_024).contains(&(samples.len() / channels)));
        let left_audible = selected == [1];
        assert_eq!(
            samples.iter().step_by(channels).any(|s| s.abs() > 0.1),
            left_audible
        );
        if channels == 2 {
            assert!(samples.iter().skip(1).step_by(2).any(|s| s.abs() > 0.1));
        }
    }
}

/// Real-source discovery and decoding check, opt in with XRT_AUDIO_CHECK_URL.
/// Never runs in offline CI.
#[test]
#[ignore = "requires an explicitly selected public audio URL"]
fn public_online_audio() {
    let url = std::env::var("XRT_AUDIO_CHECK_URL").unwrap();
    let episodes = super::discovery::resolve(&url, "", Arc::new(AtomicBool::new(false))).unwrap();
    let episode = &episodes[0];
    eprintln!("Resolved {:?}: {}", episode.kind, episode.title);
    let (tx, rx) = crossbeam_channel::bounded(4);
    let import = import_audio_url(
        episode.url.clone(),
        String::new(),
        tx,
        AudioImportOptions {
            gate_threshold_db: None,
            ..Default::default()
        },
    )
    .unwrap();
    let mut samples = Vec::new();
    while samples.len() < 16_000 * 15 {
        samples.extend(
            rx.recv_timeout(Duration::from_secs(45))
                .unwrap_or_else(|error| {
                    panic!(
                        "{error}; {:?}",
                        import.events().try_iter().collect::<Vec<_>>()
                    )
                }),
        );
    }
    import.stop();
    assert!(samples.iter().any(|sample| sample.abs() > 0.01));
    eprintln!("Decoded {} samples at 16 kHz", samples.len());
}

#[test]
#[ignore = "requires a public live source and an isolated configured backend"]
fn online_meeting_recognizes_translates_and_persists() {
    use crate::{
        client_settings::CaptureSource,
        network::{self, SessionConfig, SessionEvent},
        plugins::meeting::{
            MeetingInputRequest, MeetingPlugin, MeetingStartRequest, store::MeetingStatus,
        },
        session_coordinator::{
            SessionEventSubscriber, TranslationEvent, TranslationEventAdapter, TranslationInput,
            TranslationOutcome, TranslationSessionPlugin,
        },
    };
    use xrtranslate_engine::language::LanguageSelection;
    use xrtranslate_prompt::TaskContext;
    let url = std::env::var("XRT_AUDIO_CHECK_URL").unwrap();
    let server = std::env::var("XRT_AUDIO_CHECK_SERVER").unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut plugin = MeetingPlugin::open(directory.path());
    let id = plugin
        .controller
        .create(&MeetingStartRequest {
            name: "Online audio verification".into(),
            source_language: "en".into(),
            target_language: "zh".into(),
            input: MeetingInputRequest::OnlineAudio { url: url.clone() },
        })
        .unwrap();
    assert!(plugin.controller.begin_capture(&id));
    let languages = LanguageSelection::parse_with_options("en", "zh", None, false).unwrap();
    plugin.event_sink.begin_translation(languages);
    plugin.event_sink.begin_sessions(1, true);
    let owner = crate::session_coordinator::TranslationSessionOwner::Plugin(
        plugin.translation_session_binding().unwrap().owner,
    );
    let config = |languages, finish| SessionConfig {
        input_sample_rate: 16_000,
        input_channels: 1,
        save_separated_audio: false,
        preprocessing: Some(xrtranslate_protocol::AudioPreprocessing::None),
        context: TaskContext::default(),
        server_url: server.clone(),
        languages,
        external_audio_gate: Default::default(),
        publish_to_host_outputs: false,
        tts: None,
        egui_ctx: None,
        vad_threshold: 0.3,
        vad_silence_ms: 600,
        continuous_recognition: false,
        audio_source: CaptureSource::SystemAudio,
        finish_when_audio_ends: finish,
        workload: xrtranslate_protocol::InferenceWorkload::Realtime,
        prompt_graphs: None,
    };
    let (tx, rx) = crossbeam_channel::bounded(4);
    let (events_tx, events) = crossbeam_channel::unbounded();
    let recognition = network::start_session(
        rx,
        events_tx,
        config(
            LanguageSelection::parse_with_options("en", "zh", None, true).unwrap(),
            true,
        ),
    );
    let import = import_audio_url(
        url,
        String::new(),
        tx,
        AudioImportOptions {
            gate_threshold_db: None,
            ..Default::default()
        },
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut adapter = TranslationEventAdapter::default();
    let mut started = None;
    let mut stopped = false;
    let mut finished = false;
    while !finished && Instant::now() < deadline {
        for event in import.events().try_iter() {
            match event {
                AudioImportEvent::Started(_) => started = Some(Instant::now()),
                AudioImportEvent::Error(error) => panic!("{error}"),
                _ => {}
            }
        }
        if !stopped && started.is_some_and(|start| start.elapsed() >= Duration::from_secs(15)) {
            import.stop();
            plugin.event_sink.request_finish();
            stopped = true;
        }
        if let Ok(event) = events.recv_timeout(Duration::from_millis(50)) {
            for event in adapter.push(&event, recognition.stream_id()) {
                assert!(
                    !matches!(
                        &event,
                        TranslationEvent::Finished {
                            outcome: TranslationOutcome::Failed(_),
                            ..
                        }
                    ),
                    "{event:?}"
                );
                finished |= matches!(
                    &event,
                    TranslationEvent::Finished {
                        outcome: TranslationOutcome::Completed,
                        ..
                    }
                );
                plugin.event_sink.on_translation_event(&owner, &event);
            }
        }
    }
    import.stop();
    assert!(finished, "recognition did not drain");
    while plugin.controller.active_meeting_id().is_some() && Instant::now() < deadline {
        if let Some(task) = plugin.event_sink.next_translation_task() {
            let owner = task.owner();
            let TranslationInput::Text(text) = task.input else {
                panic!("expected text translation");
            };
            let (tx, rx) = crossbeam_channel::unbounded();
            let session = network::start_text_session(tx, config(task.languages, false));
            let mut adapter = TranslationEventAdapter::default();
            let mut received = std::collections::BTreeSet::new();
            loop {
                assert!(Instant::now() < deadline, "translation did not finish");
                let event = rx.recv_timeout(Duration::from_secs(30)).unwrap();
                if matches!(event, SessionEvent::Connected) {
                    let (source, target) = task.languages.wire();
                    session
                        .translate_text(
                            &text,
                            Some(source),
                            Some(target),
                            task.context.clone(),
                            task.text_workload,
                        )
                        .unwrap();
                }
                let mut complete = false;
                if let SessionEvent::Translation {
                    revisable: false,
                    segment_index,
                    segment_count,
                    ..
                } = &event
                {
                    received.insert(*segment_index);
                    complete = received.len() == (*segment_count).max(1) as usize;
                }
                for event in adapter.push(&event, session.stream_id()) {
                    assert!(
                        !matches!(
                            &event,
                            TranslationEvent::Finished {
                                outcome: TranslationOutcome::Failed(_),
                                ..
                            }
                        ),
                        "{event:?}"
                    );
                    plugin.event_sink.on_translation_event(&owner, &event);
                }
                if complete {
                    plugin.event_sink.on_translation_event(
                        &owner,
                        &TranslationEvent::Finished {
                            stream_id: session.stream_id(),
                            outcome: TranslationOutcome::Completed,
                        },
                    );
                    session.disconnect();
                    break;
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    let bundle = plugin.controller.store.open_meeting(&id).unwrap();
    assert_eq!(bundle.meeting.status, MeetingStatus::Ended);
    assert!(!bundle.segments.is_empty(), "no speech recognized");
    assert!(bundle.segments.iter().all(|segment| {
        segment
            .translated_text
            .as_deref()
            .is_some_and(|text| !text.is_empty())
    }));
    eprintln!(
        "Saved {} recognized and translated meeting segments; status {:?}",
        bundle.segments.len(),
        bundle.meeting.status
    );
}
