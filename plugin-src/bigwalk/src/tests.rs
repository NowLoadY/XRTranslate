use super::*;
use std::{
    io::{BufRead, BufReader},
    net::TcpListener,
};
use xrtranslate_plugin::translation::{CaptureSource, MessageFormat};

fn shared() -> Arc<Shared> {
    Arc::new(Shared {
        state: Mutex::new(State::new(&BigWalkSettings::default(), true)),
        wake: Condvar::new(),
    })
}

fn committed(segment: u32, audio_source: CaptureSource) -> Value {
    serde_json::to_value(Completion {
        stream: 1,
        turn: "turn".into(),
        segment,
        audio_source,
        is_typing: false,
        source: "你好".into(),
        translation: "Hello".into(),
        speaker: String::new(),
        additional: Vec::new(),
    })
    .unwrap()
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
    publisher
        .event(
            "translation.committed",
            committed(0, CaptureSource::Microphone),
        )
        .unwrap();
    shared.state.lock().snapshot.status = ConnectionStatus::Ready;
    publisher
        .event(
            "translation.committed",
            committed(0, CaptureSource::Microphone),
        )
        .unwrap();
    publisher
        .event(
            "translation.committed",
            committed(1, CaptureSource::SystemAudio),
        )
        .unwrap();
    publisher.event("translation.preview", Value::Null).unwrap();
    for index in 2..8 {
        publisher
            .event(
                "translation.committed",
                committed(index, CaptureSource::Microphone),
            )
            .unwrap();
    }
    assert_eq!(shared.state.lock().pending.len(), MAX_PENDING);
    publisher
        .event("translation.ended", serde_json::json!({"stream":1}))
        .unwrap();
    assert_eq!(shared.state.lock().pending.len(), MAX_PENDING);
    publisher
        .event("translation.cancelled", serde_json::json!({"stream":1}))
        .unwrap();
    publisher
        .event(
            "translation.committed",
            committed(2, CaptureSource::Microphone),
        )
        .unwrap();
    assert!(shared.state.lock().pending.is_empty());
    publisher
        .event(
            "translation.committed",
            committed(8, CaptureSource::Microphone),
        )
        .unwrap();
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
    publisher
        .event(
            "translation.committed",
            committed(1, CaptureSource::Microphone),
        )
        .unwrap();
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
    publisher
        .event(
            "translation.committed",
            committed(1, CaptureSource::Microphone),
        )
        .unwrap();
    assert!(shared.state.lock().pending.is_empty());
    publisher
        .event(
            "translation.committed",
            committed(2, CaptureSource::Microphone),
        )
        .unwrap();
    assert_eq!(shared.state.lock().pending.len(), 1);
    server.write_all(b"{\"type\":\"state\",\"protocol\":1,\"session\":\"room-b\",\"ready\":true,\"max_utf16\":256}\n").unwrap();
    wait_for(|| shared.state.lock().pending.is_empty());
    shared.state.lock().shutdown = true;
    shared.wake.notify_one();
    assert!(worker.join().unwrap().is_ok());
    line.clear();
    assert_eq!(reader.read_line(&mut line).unwrap(), 0);
}
