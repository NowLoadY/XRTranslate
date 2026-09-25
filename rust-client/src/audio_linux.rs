//! PulseAudio protocol capture, also served by PipeWire's pipewire-pulse.
//! libpulse is loaded at run time so a missing sound server/client library does
//! not prevent the desktop client from starting.
use crate::audio::{AudioApplication, AudioRouteLoopbackTarget, InputDevice};
use libloading::Library;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

type Ptr = *mut c_void;
type SinkCb = unsafe extern "C" fn(Ptr, *const SinkInfo, c_int, Ptr);
type InputCb = unsafe extern "C" fn(Ptr, *const SinkInputInfo, c_int, Ptr);
type ServerCb = unsafe extern "C" fn(Ptr, *const ServerInfo, Ptr);

// These are ABI prefixes of the public libpulse structs. Fields after the last
// value read here are deliberately omitted; libpulse only appends new fields.
#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}
#[repr(C)]
struct ChannelMap {
    channels: u8,
    map: [c_int; 32],
}
#[repr(C)]
struct Cvolume {
    channels: u8,
    values: [u32; 32],
}
#[repr(C)]
struct SinkInfo {
    name: *const c_char,
    index: u32,
    description: *const c_char,
    sample_spec: SampleSpec,
    channel_map: ChannelMap,
    owner_module: u32,
    volume: Cvolume,
    mute: c_int,
    monitor_source: u32,
    monitor_source_name: *const c_char,
}
#[repr(C)]
struct SinkInputInfo {
    index: u32,
    name: *const c_char,
    owner_module: u32,
    client: u32,
    sink: u32,
    sample_spec: SampleSpec,
    channel_map: ChannelMap,
    volume: Cvolume,
    buffer_usec: u64,
    sink_usec: u64,
    resample_method: *const c_char,
    driver: *const c_char,
    mute: c_int,
    proplist: Ptr,
}
#[repr(C)]
struct ServerInfo {
    user_name: *const c_char,
    host_name: *const c_char,
    server_version: *const c_char,
    server_name: *const c_char,
    sample_spec: SampleSpec,
    default_sink_name: *const c_char,
}
#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

macro_rules! symbols {
    ($($name:ident: $ty:ty),+ $(,)?) => {
        struct Api { _library: Library, $($name: $ty),+ }
        impl Api {
            fn load() -> Result<Self, String> {
                // SAFETY: all function signatures below match the stable libpulse C ABI.
                let library = unsafe { Library::new("libpulse.so.0") }
                    .map_err(|error| format!("libpulse.so.0 is unavailable: {error}"))?;
                Ok(Self {
                    $($name: unsafe { *library.get::<$ty>(concat!(stringify!($name), "\0").as_bytes())
                        .map_err(|error| format!("libpulse is missing {}: {error}", stringify!($name)))? }),+,
                    _library: library,
                })
            }
        }
    };
}
symbols! {
    pa_mainloop_new: unsafe extern "C" fn() -> Ptr,
    pa_mainloop_free: unsafe extern "C" fn(Ptr),
    pa_mainloop_get_api: unsafe extern "C" fn(Ptr) -> Ptr,
    pa_mainloop_iterate: unsafe extern "C" fn(Ptr, c_int, *mut c_int) -> c_int,
    pa_context_new: unsafe extern "C" fn(Ptr, *const c_char) -> Ptr,
    pa_context_connect: unsafe extern "C" fn(Ptr, *const c_char, u32, Ptr) -> c_int,
    pa_context_get_state: unsafe extern "C" fn(Ptr) -> c_int,
    pa_context_errno: unsafe extern "C" fn(Ptr) -> c_int,
    pa_strerror: unsafe extern "C" fn(c_int) -> *const c_char,
    pa_context_disconnect: unsafe extern "C" fn(Ptr),
    pa_context_unref: unsafe extern "C" fn(Ptr),
    pa_context_get_sink_info_list: unsafe extern "C" fn(Ptr, SinkCb, Ptr) -> Ptr,
    pa_context_get_sink_input_info_list: unsafe extern "C" fn(Ptr, InputCb, Ptr) -> Ptr,
    pa_context_get_server_info: unsafe extern "C" fn(Ptr, ServerCb, Ptr) -> Ptr,
    pa_proplist_gets: unsafe extern "C" fn(Ptr, *const c_char) -> *const c_char,
    pa_operation_get_state: unsafe extern "C" fn(Ptr) -> c_int,
    pa_operation_unref: unsafe extern "C" fn(Ptr),
    pa_stream_new: unsafe extern "C" fn(Ptr, *const c_char, *const SampleSpec, Ptr) -> Ptr,
    pa_stream_set_monitor_stream: unsafe extern "C" fn(Ptr, u32) -> c_int,
    pa_stream_connect_record: unsafe extern "C" fn(Ptr, *const c_char, *const BufferAttr, u32) -> c_int,
    pa_stream_get_state: unsafe extern "C" fn(Ptr) -> c_int,
    pa_stream_readable_size: unsafe extern "C" fn(Ptr) -> usize,
    pa_stream_peek: unsafe extern "C" fn(Ptr, *mut *const c_void, *mut usize) -> c_int,
    pa_stream_drop: unsafe extern "C" fn(Ptr) -> c_int,
    pa_stream_disconnect: unsafe extern "C" fn(Ptr) -> c_int,
    pa_stream_unref: unsafe extern "C" fn(Ptr),
}

fn string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: libpulse owns the NUL-terminated string for the callback duration.
    unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
}

#[derive(Clone)]
struct Sink {
    id: String,
    name: String,
    monitor: String,
    index: u32,
}
#[derive(Clone)]
struct Input {
    index: u32,
    sink: u32,
    pid: u32,
    key: String,
    name: String,
}

unsafe extern "C" fn sink_cb(_: Ptr, info: *const SinkInfo, eol: c_int, data: Ptr) {
    if eol != 0 || info.is_null() {
        return;
    }
    // SAFETY: callback runs synchronously within mainloop iteration; data points at its live Vec.
    let sinks = unsafe { &mut *(data as *mut Vec<Sink>) };
    let info = unsafe { &*info };
    let monitor = string(info.monitor_source_name);
    if !monitor.is_empty() {
        sinks.push(Sink {
            id: string(info.name),
            name: string(info.description),
            monitor,
            index: info.index,
        });
    }
}
unsafe extern "C" fn input_cb(_: Ptr, info: *const SinkInputInfo, eol: c_int, data: Ptr) {
    if eol != 0 || info.is_null() {
        return;
    }
    let (inputs, api) = unsafe { &mut *(data as *mut (Vec<Input>, *const Api)) };
    let info = unsafe { &*info };
    let api = unsafe { &**api };
    let property = |key: &'static [u8]| -> String {
        if info.proplist.is_null() {
            return String::new();
        }
        string(unsafe { (api.pa_proplist_gets)(info.proplist, key.as_ptr().cast()) })
    };
    let pid = property(b"application.process.id\0").parse().unwrap_or(0);
    if pid == 0 {
        return;
    }
    let binary = property(b"application.process.binary\0");
    let app_id = property(b"application.id\0");
    let app_name = property(b"application.name\0");
    let key = if !binary.is_empty() {
        format!("binary:{binary}")
    } else if !app_id.is_empty() {
        format!("app:{app_id}")
    } else if pid != 0 {
        format!("pid:{pid}")
    } else {
        return;
    };
    let name = if !app_name.is_empty() {
        app_name
    } else if !binary.is_empty() {
        binary
    } else {
        string(info.name)
    };
    inputs.push(Input {
        index: info.index,
        sink: info.sink,
        pid,
        key,
        name,
    });
}
unsafe extern "C" fn server_cb(_: Ptr, info: *const ServerInfo, data: Ptr) {
    if !info.is_null() {
        *unsafe { &mut *(data as *mut String) } = string(unsafe { (*info).default_sink_name });
    }
}

struct Client {
    api: Api,
    mainloop: Ptr,
    context: Ptr,
}
impl Client {
    fn connect() -> Result<Self, String> {
        let api = Api::load()?;
        let mainloop = unsafe { (api.pa_mainloop_new)() };
        if mainloop.is_null() {
            return Err("cannot create PulseAudio mainloop".into());
        }
        let context = unsafe {
            (api.pa_context_new)(
                (api.pa_mainloop_get_api)(mainloop),
                c"XRTranslate audio capture".as_ptr(),
            )
        };
        if context.is_null() {
            unsafe { (api.pa_mainloop_free)(mainloop) };
            return Err("cannot create PulseAudio context".into());
        }
        let mut client = Self {
            api,
            mainloop,
            context,
        };
        if unsafe { (client.api.pa_context_connect)(context, ptr::null(), 0, ptr::null_mut()) } < 0
        {
            return Err(client.error("cannot connect to the PulseAudio/PipeWire server"));
        }
        client.wait_until(Duration::from_secs(3), |client| {
            match unsafe { (client.api.pa_context_get_state)(client.context) } {
                4 => Some(Ok(())),
                5 | 6 => Some(Err(client.error("audio server connection failed"))),
                _ => None,
            }
        })?;
        Ok(client)
    }
    fn error(&self, prefix: &str) -> String {
        let code = unsafe { (self.api.pa_context_errno)(self.context) };
        format!(
            "{prefix}: {}",
            string(unsafe { (self.api.pa_strerror)(code) })
        )
    }
    fn iterate(&self) -> Result<(), String> {
        if unsafe { (self.api.pa_mainloop_iterate)(self.mainloop, 0, ptr::null_mut()) } < 0 {
            Err(self.error("audio server mainloop failed"))
        } else {
            Ok(())
        }
    }
    fn wait_until<T>(
        &mut self,
        timeout: Duration,
        mut check: impl FnMut(&Self) -> Option<Result<T, String>>,
    ) -> Result<T, String> {
        let deadline = Instant::now() + timeout;
        loop {
            self.iterate()?;
            if let Some(result) = check(self) {
                return result;
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for the audio server".into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn operation(&mut self, operation: Ptr) -> Result<(), String> {
        if operation.is_null() {
            return Err(self.error("audio server query failed"));
        }
        let result = self.wait_until(Duration::from_secs(3), |client| {
            match unsafe { (client.api.pa_operation_get_state)(operation) } {
                0 => None,
                1 => Some(Ok(())),
                _ => Some(Err(client.error("audio server query was cancelled"))),
            }
        });
        unsafe { (self.api.pa_operation_unref)(operation) };
        result
    }
    fn sinks(&mut self) -> Result<Vec<Sink>, String> {
        let mut sinks = Vec::new();
        let operation = unsafe {
            (self.api.pa_context_get_sink_info_list)(
                self.context,
                sink_cb,
                (&mut sinks as *mut Vec<Sink>).cast(),
            )
        };
        self.operation(operation)?;
        Ok(sinks)
    }
    fn inputs(&mut self) -> Result<Vec<Input>, String> {
        let mut context: (Vec<Input>, *const Api) = (Vec::new(), &self.api as *const Api);
        let operation = unsafe {
            (self.api.pa_context_get_sink_input_info_list)(
                self.context,
                input_cb,
                (&mut context as *mut (Vec<Input>, *const Api)).cast(),
            )
        };
        self.operation(operation)?;
        Ok(context.0)
    }
    fn default_sink(&mut self) -> Result<String, String> {
        let mut name = String::new();
        let operation = unsafe {
            (self.api.pa_context_get_server_info)(
                self.context,
                server_cb,
                (&mut name as *mut String).cast(),
            )
        };
        self.operation(operation)?;
        if name.is_empty() {
            Err("no default playback device is available".into())
        } else {
            Ok(name)
        }
    }
    fn open_stream(&mut self, monitor: &str, sink_input: Option<u32>) -> Result<Stream, String> {
        let spec = SampleSpec {
            format: if cfg!(target_endian = "little") { 5 } else { 6 },
            rate: 48_000,
            channels: 2,
        };
        let raw = unsafe {
            (self.api.pa_stream_new)(
                self.context,
                c"XRTranslate capture".as_ptr(),
                &spec,
                ptr::null_mut(),
            )
        };
        if raw.is_null() {
            return Err(self.error("cannot create audio capture stream"));
        }
        let stream = Stream {
            raw,
            api: &self.api,
        };
        if let Some(index) = sink_input {
            if unsafe { (self.api.pa_stream_set_monitor_stream)(raw, index) } < 0 {
                return Err(self.error("cannot select application audio stream"));
            }
        }
        let name = CString::new(monitor).map_err(|_| "invalid monitor source name")?;
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: 48_000 * 2 * 4 / 50,
        };
        if unsafe { (self.api.pa_stream_connect_record)(raw, name.as_ptr(), &attr, 0x2000) } < 0 {
            return Err(self.error("cannot connect audio capture stream"));
        }
        self.wait_until(Duration::from_secs(3), |client| {
            match unsafe { (client.api.pa_stream_get_state)(raw) } {
                2 => Some(Ok(())),
                3 | 4 => Some(Err(client.error("audio capture stream failed"))),
                _ => None,
            }
        })?;
        Ok(stream)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            (self.api.pa_context_disconnect)(self.context);
            (self.api.pa_context_unref)(self.context);
            (self.api.pa_mainloop_free)(self.mainloop);
        }
    }
}
struct Stream {
    raw: Ptr,
    api: *const Api,
}
impl Stream {
    fn read(&mut self, pending: &mut VecDeque<f32>) -> Result<(), String> {
        let api = unsafe { &*self.api };
        while unsafe { (api.pa_stream_readable_size)(self.raw) } > 0 {
            let mut data = ptr::null();
            let mut bytes = 0;
            if unsafe { (api.pa_stream_peek)(self.raw, &mut data, &mut bytes) } < 0 {
                return Err("audio capture read failed".into());
            }
            if bytes == 0 {
                break;
            }
            if !data.is_null() {
                // 48 kHz interleaved stereo float, negotiated by libpulse.
                for frame in unsafe { std::slice::from_raw_parts(data.cast::<f32>(), bytes / 4) }
                    .chunks_exact(2)
                {
                    pending.push_back((frame[0] + frame[1]) * 0.5);
                }
            } else {
                pending.extend(std::iter::repeat_n(0.0, bytes / 8));
            }
            if unsafe { (api.pa_stream_drop)(self.raw) } < 0 {
                return Err("audio capture buffer release failed".into());
            }
            // Bound latency if the consumer is temporarily slower than the server.
            if pending.len() > 48_000 / 2 {
                pending.drain(..pending.len() - 48_000 / 2);
            }
        }
        Ok(())
    }
    fn state(&self) -> c_int {
        unsafe { ((*self.api).pa_stream_get_state)(self.raw) }
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            ((*self.api).pa_stream_disconnect)(self.raw);
            ((*self.api).pa_stream_unref)(self.raw);
        }
    }
}

pub fn available_devices() -> Result<Vec<InputDevice>, String> {
    let mut client = Client::connect()?;
    Ok(client
        .sinks()?
        .into_iter()
        .map(|sink| InputDevice {
            id: sink.id,
            name: sink.name,
        })
        .collect())
}
pub fn available_applications() -> Result<Vec<AudioApplication>, String> {
    let mut client = Client::connect()?;
    let mut apps = BTreeMap::<String, AudioApplication>::new();
    for input in client.inputs()? {
        let entry = apps
            .entry(input.key.clone())
            .or_insert_with(|| AudioApplication {
                id: input.key,
                name: input.name,
                process_id: input.pid,
                active: true,
            });
        if entry.process_id == 0 {
            entry.process_id = input.pid;
        }
    }
    Ok(apps.into_values().collect())
}
pub fn device_config(device_id: &str) -> Result<(), String> {
    let mut client = Client::connect()?;
    let id = if device_id.is_empty() {
        client.default_sink()?
    } else {
        device_id.to_owned()
    };
    if client.sinks()?.iter().any(|sink| sink.id == id) {
        Ok(())
    } else {
        Err(format!("playback device '{id}' is no longer available"))
    }
}

pub fn run_capture(
    target: &AudioRouteLoopbackTarget,
    stop: Arc<AtomicBool>,
    ready: &std::sync::mpsc::SyncSender<Result<(), String>>,
    mut deliver: impl FnMut(Vec<f32>),
) -> Result<(), String> {
    let mut client = Client::connect()?;
    let mut sinks = client.sinks()?;
    let mut streams = HashMap::<u32, (Stream, VecDeque<f32>, u32)>::new();
    let app_key = match target {
        AudioRouteLoopbackTarget::Endpoint { device_id } => {
            let id = if device_id.is_empty() {
                client.default_sink()?
            } else {
                device_id.clone()
            };
            let sink = sinks
                .iter()
                .find(|sink| sink.id == id)
                .ok_or_else(|| format!("playback device '{id}' is no longer available"))?;
            streams.insert(
                u32::MAX,
                (
                    client.open_stream(&sink.monitor, None)?,
                    VecDeque::new(),
                    sink.index,
                ),
            );
            None
        }
        AudioRouteLoopbackTarget::Application { process_id, .. } => {
            let inputs = client.inputs()?;
            let key = inputs
                .iter()
                .find(|input| input.pid == *process_id)
                .map(|input| input.key.clone())
                .ok_or("the selected application's audio stream is no longer available")?;
            for input in inputs.iter().filter(|input| input.key == key) {
                if let Some(sink) = sinks.iter().find(|sink| sink.index == input.sink) {
                    streams.insert(
                        input.index,
                        (
                            client.open_stream(&sink.monitor, Some(input.index))?,
                            VecDeque::new(),
                            sink.index,
                        ),
                    );
                }
            }
            if streams.is_empty() {
                return Err("the selected application's playback device is unavailable".into());
            }
            Some(key)
        }
    };
    let _ = ready.send(Ok(()));
    let mut last_scan = Instant::now();
    let mut last_emit = Instant::now();
    while !stop.load(Ordering::Acquire) {
        client.iterate()?;
        for (stream, pending, _) in streams.values_mut() {
            if matches!(stream.state(), 3 | 4) {
                continue;
            }
            stream.read(pending)?;
        }
        if let Some(key) = &app_key {
            if last_scan.elapsed() >= Duration::from_secs(1) {
                let inputs = client.inputs()?;
                sinks = client.sinks()?;
                streams.retain(|index, (stream, _, sink_index)| {
                    stream.state() == 2
                        && inputs.iter().any(|input| {
                            input.index == *index && input.key == *key && input.sink == *sink_index
                        })
                });
                for input in inputs.iter().filter(|input| &input.key == key) {
                    if streams.contains_key(&input.index) {
                        continue;
                    }
                    if let Some(sink) = sinks.iter().find(|sink| sink.index == input.sink) {
                        match client.open_stream(&sink.monitor, Some(input.index)) {
                            Ok(stream) => {
                                streams.insert(input.index, (stream, VecDeque::new(), sink.index));
                            }
                            Err(error) => log::warn!(
                                "Application audio stream could not be captured: {error}"
                            ),
                        }
                    }
                }
                last_scan = Instant::now();
            }
        } else if streams.values().any(|(stream, _, _)| stream.state() != 2) {
            return Err("system audio capture stream disconnected".into());
        }
        // One 20 ms timeline for all streams of the selected application.
        if last_emit.elapsed() >= Duration::from_millis(20) {
            let mut mixed = vec![0.0; 960];
            for (_, pending, _) in streams.values_mut() {
                for sample in &mut mixed {
                    if let Some(value) = pending.pop_front() {
                        *sample += value;
                    }
                }
            }
            if !streams.is_empty() {
                deliver(mixed);
            }
            last_emit += Duration::from_millis(20);
            if last_emit.elapsed() > Duration::from_millis(100) {
                last_emit = Instant::now();
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

#[cfg(test)]
mod live_tests {
    use super::*;
    #[test]
    #[ignore = "requires a running PulseAudio or pipewire-pulse desktop session"]
    fn enumerate_live_audio_server() {
        let devices = available_devices().expect("audio server should enumerate playback devices");
        assert!(!devices.is_empty(), "audio server has no playback sinks");
        let apps = available_applications().expect("audio server should enumerate sink inputs");
        eprintln!(
            "devices: {:?}",
            devices.iter().map(|d| (&d.id, &d.name)).collect::<Vec<_>>()
        );
        eprintln!("applications: {apps:?}");
        device_config("").expect("default playback sink should be capturable");
    }

    #[test]
    #[ignore = "requires an active PulseAudio playback client; set XRTRANSLATE_AUDIO_SMOKE_PID"]
    fn capture_live_application_and_system_audio() {
        let pid: u32 = std::env::var("XRTRANSLATE_AUDIO_SMOKE_PID")
            .expect("set XRTRANSLATE_AUDIO_SMOKE_PID to a playing PulseAudio client")
            .parse()
            .unwrap();
        let mut targets = vec![AudioRouteLoopbackTarget::Endpoint {
            device_id: String::new(),
        }];
        if pid != 0 {
            targets.push(AudioRouteLoopbackTarget::Application {
                process_id: pid,
                application_name: "test tone".into(),
            });
        }
        for target in targets {
            let stop = Arc::new(AtomicBool::new(false));
            let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
            let worker_stop = Arc::clone(&stop);
            let worker = std::thread::spawn(move || {
                let mut peak = 0.0f32;
                run_capture(&target, worker_stop, &ready_tx, |samples| {
                    for sample in samples {
                        peak = peak.max(sample.abs());
                    }
                })
                .expect("capture should run");
                peak
            });
            ready_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("capture should start")
                .expect("capture should connect");
            std::thread::sleep(Duration::from_millis(600));
            stop.store(true, Ordering::Release);
            let peak = worker.join().unwrap();
            eprintln!("capture peak: {peak}");
            assert!(peak > 0.001, "capture returned only silence");
        }
    }
}
