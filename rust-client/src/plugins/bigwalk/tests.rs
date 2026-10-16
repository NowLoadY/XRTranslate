use super::*;
use crate::{client_settings::CaptureSource, plugins::text_output::MessageFormat};
use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
};

fn shared() -> Arc<Shared> {
    Arc::new(Shared {
        state: Mutex::new(State::new(
            &BigWalkSettings {
                enabled: true,
                ..Default::default()
            },
            true,
        )),
        wake: Condvar::new(),
    })
}

fn committed(segment: u32, audio_source: CaptureSource) -> HostOutputEvent<'static> {
    HostOutputEvent::CommittedTranslation {
        stream_id: 1,
        turn_id: "turn",
        segment_index: segment,
        audio_source,
        is_typing: false,
        source: "你好",
        translated: "Hello",
        speaker: "",
        additional_translations: &[],
        asr_only: false,
    }
}

#[test]
fn bilingual_chat_fits_both_languages_without_breaking_graphemes() {
    let text = chat_text(
        &"原文".repeat(200),
        &"👩‍🚀 translated ".repeat(100),
        ["短译文"],
        MessageFormat::BilingualSourceFirst,
        256,
    );
    assert!(text.encode_utf16().count() <= 256);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3);
    assert!(lines[0].ends_with('…') && lines[1].ends_with('…'));
    assert_eq!(lines[2], "短译文");
    assert!(!lines[1].ends_with("\u{200d}…"));
    assert_eq!(
        chat_text(
            "",
            "<b>Hello</b>\r\n world",
            [],
            MessageFormat::TargetOnly,
            256
        ),
        "‹b›Hello‹/b› world"
    );
}

#[test]
fn final_identity_survives_filtering_disconnect_and_cancellation() {
    let shared = shared();
    let publisher = Publisher(shared.clone());
    publisher.on_host_output(committed(0, CaptureSource::Microphone));
    shared.state.lock().snapshot.status = ConnectionStatus::Ready;
    publisher.on_host_output(committed(0, CaptureSource::Microphone));
    publisher.on_host_output(committed(1, CaptureSource::SystemAudio));
    publisher.on_host_output(HostOutputEvent::Previews {
        stream_id: 1,
        is_typing: false,
        entries: &[],
    });
    for index in 2..8 {
        publisher.on_host_output(committed(index, CaptureSource::Microphone));
    }
    assert_eq!(shared.state.lock().pending.len(), MAX_PENDING);
    publisher.on_host_output(HostOutputEvent::StreamEnded(1));
    assert_eq!(shared.state.lock().pending.len(), MAX_PENDING);
    publisher.on_host_output(HostOutputEvent::StreamCancelled(1));
    publisher.on_host_output(committed(2, CaptureSource::Microphone));
    assert!(shared.state.lock().pending.is_empty());
    publisher.on_host_output(committed(8, CaptureSource::Microphone));
    let mut state = shared.state.lock();
    state.expire(Instant::now() + MESSAGE_LIFETIME);
    assert!(state.pending.is_empty());
}

fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !predicate() {
        assert!(Instant::now() < deadline, "bridge state did not settle");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn wire_sends_once_and_discards_queue_on_game_session_change() {
    let shared = shared();
    let publisher = Publisher(shared.clone());
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let stream = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let worker_state = shared.clone();
    let worker = thread::spawn(move || drive_connection(&worker_state, 0, stream));
    server.write_all(b"{\"type\":\"state\",\"protocol\":1,\"session\":\"room-a\",\"ready\":true,\"max_utf16\":256}\n").unwrap();
    wait_for(|| shared.state.lock().snapshot.status == ConnectionStatus::Ready);
    publisher.on_host_output(committed(1, CaptureSource::Microphone));
    let mut reader = BufReader::new(server.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let frame: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(frame["session"], "room-a");
    assert_eq!(frame["text"], "Hello");
    writeln!(
        server,
        "{}",
        serde_json::json!({"type":"result", "id":frame["id"], "status":"sent"})
    )
    .unwrap();
    wait_for(|| shared.state.lock().snapshot.last_sent == "Hello");
    publisher.on_host_output(committed(1, CaptureSource::Microphone));
    assert!(shared.state.lock().pending.is_empty());
    publisher.on_host_output(committed(2, CaptureSource::Microphone));
    assert_eq!(shared.state.lock().pending.len(), 1);
    server.write_all(b"{\"type\":\"state\",\"protocol\":1,\"session\":\"room-b\",\"ready\":true,\"max_utf16\":256}\n").unwrap();
    wait_for(|| shared.state.lock().pending.is_empty());
    shared.state.lock().shutdown = true;
    shared.wake.notify_one();
    assert!(worker.join().unwrap().is_ok());
    line.clear();
    assert_eq!(reader.read_line(&mut line).unwrap(), 0);
}
