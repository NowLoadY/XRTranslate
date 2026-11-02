//! Use the existing MPV runtime for HLS/other network formats. PCM flows through
//! a loopback socket, so no complete download or growing temporary WAV is needed.
use std::{
    ffi::CString,
    io::{self, Read},
    net::{Shutdown, TcpListener, TcpStream},
    sync::atomic::{AtomicBool, AtomicU64},
    time::{Duration, Instant},
};

use crossbeam_channel::Sender;

use super::{
    stream::{ChunkSink, check_cancelled},
    types::{AudioFileInfo, AudioImportError, AudioImportEvent, AudioImportOptions},
};

struct Decoder {
    handle: *mut libmpv_sys::mpv_handle,
    socket: Option<TcpStream>,
}

impl Drop for Decoder {
    fn drop(&mut self) {
        // Release an encoder blocked on backpressure before joining MPV's threads.
        if let Some(socket) = &self.socket {
            let _ = socket.shutdown(Shutdown::Both);
        }
        unsafe {
            libmpv_sys::mpv_terminate_destroy(self.handle);
        }
    }
}

impl Decoder {
    fn set(&self, key: &str, value: &str) -> Result<(), AudioImportError> {
        let key = CString::new(key).map_err(|e| AudioImportError::InvalidOptions(e.to_string()))?;
        let value =
            CString::new(value).map_err(|e| AudioImportError::InvalidOptions(e.to_string()))?;
        check(unsafe {
            libmpv_sys::mpv_set_option_string(self.handle, key.as_ptr(), value.as_ptr())
        })
    }
}

fn check(code: i32) -> Result<(), AudioImportError> {
    if code < 0 {
        let error = unsafe { std::ffi::CStr::from_ptr(libmpv_sys::mpv_error_string(code)) }
            .to_string_lossy();
        Err(AudioImportError::Decode(format!("MPV: {error}")))
    } else {
        Ok(())
    }
}

pub(super) fn decode(
    url: &str,
    proxy: &str,
    audio_tx: Sender<Vec<f32>>,
    options: AudioImportOptions,
    stop: &AtomicBool,
    sent: &AtomicU64,
    events: &Sender<AudioImportEvent>,
) -> Result<u64, AudioImportError> {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let handle = unsafe { libmpv_sys::mpv_create() };
    if handle.is_null() {
        return Err(AudioImportError::Decode(
            "Could not initialize MPV stream decoder".into(),
        ));
    }
    let mut decoder = Decoder {
        handle,
        socket: None,
    };
    for (key, value) in [
        ("config", "no"),
        ("load-scripts", "no"),
        ("ytdl", "no"),
        ("video", "no"),
        ("vo", "null"),
        ("of", "f32le"),
        ("oac", "pcm_f32le"),
        ("untimed", "yes"),
        ("cache", "no"),
        ("network-timeout", "15"),
        ("demuxer-max-bytes", "33554432"),
        ("msg-level", "all=error"),
    ] {
        decoder.set(key, value)?;
    }
    decoder.set("o", &format!("tcp://{}", listener.local_addr()?))?;
    decoder.set("audio-samplerate", &options.output_sample_rate.to_string())?;
    decoder.set(
        "audio-channels",
        if options.output_channels == 2 {
            "stereo"
        } else {
            "mono"
        },
    )?;
    if let Some(track) = options.audio_track {
        decoder.set("aid", &track.to_string())?;
    }
    if !options.recognition_channels.is_empty() {
        let selected = &options.recognition_channels;
        let filter = if options.output_channels == 1 {
            let terms = selected
                .iter()
                .map(|index| format!("c{index}"))
                .collect::<Vec<_>>();
            format!("pan=mono|c0<{}", terms.join("+"))
        } else if let [index] = selected.as_slice() {
            format!("pan=stereo|c0=c{index}|c1=c{index}")
        } else {
            // Mute excluded inputs before MPV downmixes, preserving the source layout.
            let terms = selected
                .iter()
                .map(|index| format!("eq(ch,{index})"))
                .collect::<Vec<_>>();
            format!("aeval='if({},val(ch),0)':c=same", terms.join("+"))
        };
        decoder.set("af", &format!("lavfi=[{filter}]"))?;
    }
    if !proxy.trim().is_empty() {
        decoder.set("http-proxy", proxy)?;
    }
    check(unsafe { libmpv_sys::mpv_initialize(handle) })?;
    let command = CString::new("loadfile").unwrap();
    let url_value =
        CString::new(url).map_err(|e| AudioImportError::InvalidOptions(e.to_string()))?;
    let mut args = [command.as_ptr(), url_value.as_ptr(), std::ptr::null()];
    check(unsafe { libmpv_sys::mpv_command_async(handle, 0, args.as_mut_ptr()) })?;

    let mut sink = ChunkSink::new(
        audio_tx,
        options.chunk_frames,
        options.output_sample_rate,
        options.output_channels,
        options.pacing,
        stop,
        sent,
    );
    let mut gate = options
        .gate_threshold_db
        .map(|db| crate::audio_processing::NoiseGate::new(db, options.output_sample_rate));
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 32 * 1024];
    let mut last_data = Instant::now();
    let mut ended = false;
    loop {
        check_cancelled(stop)?;
        let event = unsafe { libmpv_sys::mpv_wait_event(handle, 0.0) };
        if !event.is_null() {
            let event = unsafe { &*event };
            if event.event_id == libmpv_sys::mpv_event_id_MPV_EVENT_END_FILE {
                let end = unsafe { &*(event.data as *const libmpv_sys::mpv_event_end_file) };
                check(end.error)?;
                ended = true;
            }
        }
        if decoder.socket.is_none() {
            match listener.accept() {
                Ok((socket, _)) => {
                    socket.set_nonblocking(true)?;
                    decoder.socket = Some(socket);
                    let _ = events.send(AudioImportEvent::Started(AudioFileInfo {
                        path: url.into(),
                        codec: "MPV".into(),
                        source_sample_rate: options.output_sample_rate,
                        source_channels: options.output_channels,
                        total_source_frames: None,
                        duration: None,
                        output_sample_rate: options.output_sample_rate,
                    }));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if ended {
                        return Err(AudioImportError::Decode("Stream contains no audio".into()));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        if let Some(socket) = &mut decoder.socket {
            match socket.read(&mut buffer) {
                Ok(0) if ended => break,
                Ok(0) => {} // Observe END_FILE before accepting EOF as successful.
                Ok(count) => {
                    last_data = Instant::now();
                    bytes.extend_from_slice(&buffer[..count]);
                    let aligned =
                        bytes.len() / (4 * options.output_channels) * (4 * options.output_channels);
                    let mut samples = bytes[..aligned]
                        .chunks_exact(4)
                        .map(|chunk| f32::from_le_bytes(chunk.try_into().unwrap()))
                        .collect::<Vec<_>>();
                    bytes.drain(..aligned);
                    if let Some(gate) = &mut gate {
                        gate.process(&mut samples);
                    }
                    sink.push(&samples)?;
                    continue;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    // Encoding can retain the output socket after END_FILE. All
                    // queued bytes have been read once a completed stream blocks.
                    if ended {
                        break;
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        if last_data.elapsed() > Duration::from_secs(30) {
            return Err(AudioImportError::Decode(
                "Online audio stream timed out".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    sink.finish()?;
    if sink.sent_frames == 0 || !bytes.is_empty() {
        return Err(AudioImportError::Decode(
            "Stream ended without complete audio frames".into(),
        ));
    }
    Ok(sink.sent_frames)
}
