use super::{
    Track,
    events::State,
    log::{self, Reader},
};
use serde_json::{Value, json};
use std::{fs, io::Write};

fn track() -> Track {
    Track {
        title: "测试歌曲".into(),
        artists: vec!["歌手".into()],
        album: String::new(),
    }
}

fn event(at: u64, name: &str, values: &[Value]) -> String {
    let args = std::iter::once(json!(name))
        .chain(values.iter().cloned())
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    format!("[42:7:1115/053507:{at}:INFO:app.cpp(1231)] [2026-11-15 05:35:07] 【playing】,{args}\n")
}

fn metadata(at: u64, id: &str, title: &str) -> String {
    // The actual log can append non-JSON `undefined` after the useful payload.
    let mut row = event(
        at,
        "checkPlayPrivilege",
        &[json!({"id":id,"name":title,"duration":300000,"artists":[{"name":"歌手"}]})],
    );
    row.pop();
    row.push_str(",\"scene\",undefined\n");
    row
}

fn loaded(at: u64, id: &str) -> String {
    event(
        at,
        "native播放资源load完成，开始播放",
        &[json!({"songId":id})],
    )
}

fn status(at: u64, playing: u64, play_id: &str) -> String {
    event(at, "native播放state", &[json!(playing), json!(play_id)])
}

fn initial() -> String {
    metadata(900, "1", "测试歌曲") + &loaded(1000, "1") + &status(1010, 1, "1_first")
}

fn replay(text: &str) -> State {
    let mut state = State::default();
    for row in text.lines() {
        state.push(row);
    }
    state
}

#[test]
fn reconstructs_play_pause_resume_seek_and_same_song_repeat() {
    let track = track();
    let mut state = replay(&initial());
    assert_eq!(state.sample(&track, true, 6000).unwrap().position_ms, 5000);
    state.push(&status(6000, 2, "1_first"));
    assert_eq!(
        state.sample(&track, false, 20_000).unwrap().position_ms,
        5000
    );
    assert!(state.sample(&track, true, 20_000).is_none());
    state.push(&event(20_000, "setPlayingPosition", &[json!(82.113)]));
    assert_eq!(
        state.sample(&track, false, 21_000).unwrap().position_ms,
        82_113
    );
    state.push(&status(25_000, 1, "1_first"));
    state.push(&status(26_000, 1, "1_first"));
    assert_eq!(
        state.sample(&track, true, 27_000).unwrap().position_ms,
        84_113
    );
    state.push(&event(28_000, "setPlayingPosition", &[json!(12.5)]));
    assert_eq!(
        state.sample(&track, true, 29_000).unwrap().position_ms,
        13_500
    );
    state.push(&loaded(30_000, "1"));
    state.push(&status(30_010, 1, "1_repeat"));
    state.push(&status(31_000, 2, "1_first"));
    assert_eq!(
        state.sample(&track, true, 32_000).unwrap().position_ms,
        2000
    );
}

#[test]
fn only_current_track_and_native_clock_can_supply_progress() {
    let track = track();
    let mut state = replay(&initial());
    let mut unrelated = track.clone();
    unrelated.artists = vec!["另一歌手".into()];
    assert!(state.sample(&unrelated, true, 2000).is_none());
    assert!(state.sample(&track, true, 999).is_none());
    assert!(state.sample(&track, true, 304_000).is_none());
    state.push(&metadata(5000, "2", "新歌"));
    assert!(state.sample(&track, true, 6000).is_none());
    let mut next = track.clone();
    next.title = "新歌".into();
    state.push(&status(5010, 2, "1_first"));
    state.push(&loaded(5020, "1"));
    assert!(state.sample(&next, true, 6000).is_none());
    state.push(&loaded(5100, "2"));
    state.push(&status(5110, 1, "2_next"));
    assert_eq!(state.sample(&next, true, 6100).unwrap().position_ms, 1000);
}

#[test]
fn resume_without_a_load_or_seek_never_invents_song_start() {
    let track = track();
    let mut state = replay(&metadata(1000, "1", "测试歌曲"));
    state.push(&status(2000, 1, "1_first"));
    assert!(state.sample(&track, true, 3000).is_none());
    for invalid in [json!(-1), json!(400), json!("82.1"), Value::Null] {
        state.push(&event(3000, "setPlayingPosition", &[invalid]));
        assert!(state.sample(&track, true, 4000).is_none());
    }
    state.push(&event(4000, "setPlayingPosition", &[json!(80)]));
    assert_eq!(
        state.sample(&track, true, 4500).unwrap().position_ms,
        80_500
    );
    state.push(&status(5000, 0, "1_first"));
    assert!(state.sample(&track, true, 5500).is_none());
}

#[test]
fn incremental_log_handles_split_utf8_and_truncation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cloudmusic.elog");
    let text = initial();
    let bytes = log::encode(text.as_bytes());
    let split = text.find("测试").unwrap() + 1;
    fs::write(&path, &bytes[..split]).unwrap();
    let mut reader = Reader::new(path.clone());
    reader.refresh().unwrap();
    let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&bytes[split..]).unwrap();
    file.write_all(&log::encode(status(6000, 2, "1_first").as_bytes()))
        .unwrap();
    reader.refresh().unwrap();
    assert_eq!(
        reader
            .state
            .sample(&track(), false, 10_000)
            .unwrap()
            .position_ms,
        5000
    );
    drop(file);
    fs::write(&path, log::encode(b"new log\n")).unwrap();
    reader.refresh().unwrap();
    assert!(reader.state.sample(&track(), false, 10_000).is_none());
}

#[test]
fn bounded_startup_skips_partial_history_and_reads_latest_song() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cloudmusic.elog");
    let mut data = vec![log::encode(b"x")[0]; 5 * 1024 * 1024];
    data.extend(log::encode(format!("\n{}", initial()).as_bytes()));
    fs::write(&path, data).unwrap();
    let mut reader = Reader::new(path);
    reader.refresh().unwrap();
    assert_eq!(
        reader
            .state
            .sample(&track(), true, 5000)
            .unwrap()
            .position_ms,
        4000
    );
}

#[test]
fn log_rotation_with_equal_length_cannot_reuse_the_previous_clock() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cloudmusic.elog");
    fs::write(&path, log::encode(initial().as_bytes())).unwrap();
    let mut reader = Reader::new(path.clone());
    reader.refresh().unwrap();
    assert!(reader.state.sample(&track(), true, 2000).is_some());
    fs::rename(&path, directory.path().join("old.elog")).unwrap();
    let replacement = initial().replace("测试歌曲", "替换歌曲");
    assert_eq!(replacement.len(), initial().len());
    fs::write(&path, log::encode(replacement.as_bytes())).unwrap();
    reader.refresh().unwrap();
    assert!(reader.state.sample(&track(), true, 2000).is_none());
    let mut next = track();
    next.title = "替换歌曲".into();
    assert_eq!(
        reader.state.sample(&next, true, 2000).unwrap().position_ms,
        1000
    );
}
