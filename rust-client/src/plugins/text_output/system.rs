//! One system sampler with an explicit owner and read-only output handles.
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Clone, Debug, Default)]
pub struct SystemMetrics {
    pub time_str: String,
    pub cpu_name: String,
    pub cpu_usage: u32,
    pub gpu_name: String,
    pub gpu_usage: u32,
}

/// Cloning a handle never starts a sampler. A default handle is empty.
#[derive(Clone, Default)]
pub struct TextOutputHandle(Arc<parking_lot::Mutex<SystemMetrics>>);

impl TextOutputHandle {
    pub fn snapshot(&self) -> SystemMetrics {
        self.0.lock().clone()
    }
}

pub(super) struct SystemMonitor {
    handle: TextOutputHandle,
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl SystemMonitor {
    pub(super) fn new() -> Self {
        let handle = TextOutputHandle(Arc::new(parking_lot::Mutex::new(SystemMetrics {
            time_str: get_formatted_time(),
            ..SystemMetrics::default()
        })));
        let metrics = handle.0.clone();
        let (stop, stopped) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("text-output-metrics".into())
            .spawn(move || {
                let cpu_name = detect_cpu_name();
                let gpu_name = detect_gpu_name();
                let mut cpu_sampler = CpuUsageSampler::new();
                let mut gpu_sampler = GpuUsageSampler::new();
                loop {
                    let now_str = get_formatted_time();
                    let cpu_usage = cpu_sampler.sample();
                    let gpu_usage = gpu_sampler.as_mut().and_then(GpuUsageSampler::sample);
                    {
                        let mut value = metrics.lock();
                        value.time_str = now_str;
                        value.cpu_name.clone_from(&cpu_name);
                        value.gpu_name.clone_from(&gpu_name);
                        if let Some(usage) = cpu_usage {
                            value.cpu_usage = usage;
                        }
                        if let Some(usage) = gpu_usage {
                            value.gpu_usage = usage;
                        }
                    }
                    if stopped.recv_timeout(Duration::from_secs(1))
                        != Err(mpsc::RecvTimeoutError::Timeout)
                    {
                        break;
                    }
                }
            })
            .map_err(|error| log::warn!("Cannot start text-output sampler: {error}"))
            .ok();
        Self {
            handle,
            stop,
            worker,
        }
    }

    pub(super) fn handle(&self) -> TextOutputHandle {
        self.handle.clone()
    }

    pub(super) fn shutdown(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = self.stop.send(());
            let _ = worker.join();
        }
    }
}

impl Drop for SystemMonitor {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(windows)]
fn get_formatted_time() -> String {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;

    let mut local = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    unsafe { GetLocalTime(&mut local) };
    format!(
        "{:02}:{:02}:{:02}",
        local.wHour, local.wMinute, local.wSecond
    )
}

#[cfg(unix)]
fn get_formatted_time() -> String {
    let timestamp = unsafe { libc::time(std::ptr::null_mut()) };
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    if unsafe { libc::localtime_r(&timestamp, local.as_mut_ptr()) }.is_null() {
        return String::new();
    }
    let local = unsafe { local.assume_init() };
    format!(
        "{:02}:{:02}:{:02}",
        local.tm_hour, local.tm_min, local.tm_sec
    )
}

#[cfg(not(any(windows, unix)))]
fn get_formatted_time() -> String {
    String::new()
}

#[cfg(target_os = "windows")]
type SystemTimes = (u64, u64, u64);

struct CpuUsageSampler {
    #[cfg(target_os = "windows")]
    previous: Option<SystemTimes>,
}

impl CpuUsageSampler {
    fn new() -> Self {
        Self {
            #[cfg(target_os = "windows")]
            previous: read_system_times(),
        }
    }

    fn sample(&mut self) -> Option<u32> {
        #[cfg(target_os = "windows")]
        {
            let current = read_system_times()?;
            let previous = self.previous.replace(current)?;
            let idle = current.0.saturating_sub(previous.0);
            let kernel = current.1.saturating_sub(previous.1);
            let user = current.2.saturating_sub(previous.2);
            let total = kernel.saturating_add(user);
            if total == 0 {
                return None;
            }
            let busy = total.saturating_sub(idle);
            return Some(((busy as f64 * 100.0 / total as f64).round() as u32).min(100));
        }
        #[cfg(not(target_os = "windows"))]
        None
    }
}

#[cfg(target_os = "windows")]
fn read_system_times() -> Option<SystemTimes> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetSystemTimes;

    fn as_u64(value: FILETIME) -> u64 {
        ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
    }

    unsafe {
        let mut idle = std::mem::zeroed();
        let mut kernel = std::mem::zeroed();
        let mut user = std::mem::zeroed();
        (GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0)
            .then(|| (as_u64(idle), as_u64(kernel), as_u64(user)))
    }
}

#[cfg(target_os = "windows")]
struct GpuUsageSampler {
    query: windows_sys::Win32::System::Performance::PDH_HQUERY,
    counter: windows_sys::Win32::System::Performance::PDH_HCOUNTER,
}

#[cfg(target_os = "windows")]
impl GpuUsageSampler {
    fn new() -> Option<Self> {
        use windows_sys::Win32::System::Performance::{
            PDH_HCOUNTER, PDH_HQUERY, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
            PdhOpenQueryW,
        };

        unsafe {
            let mut query: PDH_HQUERY = std::ptr::null_mut();
            if PdhOpenQueryW(std::ptr::null(), 0, &mut query) != 0 {
                return None;
            }
            let path = "\\GPU Engine(*)\\Utilization Percentage\0"
                .encode_utf16()
                .collect::<Vec<_>>();
            let mut counter: PDH_HCOUNTER = std::ptr::null_mut();
            if PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            if PdhCollectQueryData(query) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            Some(Self { query, counter })
        }
    }

    fn sample(&mut self) -> Option<u32> {
        use std::collections::HashMap;
        use windows_sys::Win32::System::Performance::{
            PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W,
            PDH_FMT_DOUBLE, PDH_MORE_DATA, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
        };

        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let mut byte_count = 0;
            let mut item_count = 0;
            let status = PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut byte_count,
                &mut item_count,
                std::ptr::null_mut(),
            );
            if status != PDH_MORE_DATA || byte_count == 0 || item_count == 0 {
                return None;
            }

            let word_size = std::mem::size_of::<usize>();
            let mut buffer = vec![0usize; (byte_count as usize).div_ceil(word_size)];
            let items = buffer.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
            if PdhGetFormattedCounterArrayW(
                self.counter,
                PDH_FMT_DOUBLE,
                &mut byte_count,
                &mut item_count,
                items,
            ) != 0
            {
                return None;
            }

            let mut engines = HashMap::<String, f64>::new();
            for item in std::slice::from_raw_parts(items, item_count as usize) {
                if !matches!(
                    item.FmtValue.CStatus,
                    PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA
                ) {
                    continue;
                }
                let name = wide_ptr_to_string(item.szName);
                let Some(engine) = gpu_engine_key(&name) else {
                    continue;
                };
                let usage = item.FmtValue.Anonymous.doubleValue;
                if usage.is_finite() && usage >= 0.0 {
                    *engines.entry(engine).or_default() += usage;
                }
            }
            let usage = engines.values().copied().fold(0.0_f64, f64::max);
            Some(usage.round().clamp(0.0, 100.0) as u32)
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for GpuUsageSampler {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Performance::PdhCloseQuery(self.query);
        }
    }
}

#[cfg(target_os = "windows")]
unsafe fn wide_ptr_to_string(value: *const u16) -> String {
    if value.is_null() {
        return String::new();
    }
    let mut length = 0;
    unsafe {
        while *value.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(value, length))
    }
}

#[cfg(target_os = "windows")]
fn gpu_engine_key(instance: &str) -> Option<String> {
    let lower = instance.to_ascii_lowercase();
    let luid = lower.find("luid_")?;
    let engine = lower.find("_eng_")?;
    let engine_type = lower.find("_engtype_").unwrap_or(lower.len());
    (luid < engine && engine < engine_type).then(|| lower[luid..engine_type].to_owned())
}

#[cfg(not(target_os = "windows"))]
struct GpuUsageSampler;

#[cfg(not(target_os = "windows"))]
impl GpuUsageSampler {
    fn new() -> Option<Self> {
        None
    }

    fn sample(&mut self) -> Option<u32> {
        None
    }
}

fn detect_cpu_name() -> String {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        if let Ok(output) = crate::child_process::hide_console(&mut Command::new("reg"))
            .args([
                "query",
                "HKLM\\HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
                "/v",
                "ProcessorNameString",
            ])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                if line.contains("ProcessorNameString")
                    && let Some(pos) = line.find("REG_SZ")
                {
                    let name = line[pos + 6..].trim();
                    if !name.is_empty() {
                        return clean_hardware_name(name);
                    }
                }
            }
        }
    }
    String::new()
}

fn detect_gpu_name() -> String {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        for index in ["0000", "0001", "0002"] {
            let key = format!(
                "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Class\\{{4d36e968-e325-11ce-bfc1-08002be10318}}\\{index}"
            );
            if let Ok(output) = crate::child_process::hide_console(&mut Command::new("reg"))
                .args(["query", &key, "/v", "DriverDesc"])
                .output()
            {
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains("DriverDesc")
                        && let Some(pos) = line.find("REG_SZ")
                    {
                        let name = line[pos + 6..].trim();
                        if !name.is_empty() {
                            return clean_hardware_name(name);
                        }
                    }
                }
            }
        }
    }
    String::new()
}

#[cfg(target_os = "windows")]
fn clean_hardware_name(raw: &str) -> String {
    let name = raw
        .replace("(R)", "")
        .replace("(TM)", "")
        .replace("CPU @ ", "")
        .replace("  ", " ");

    let mut parts = Vec::new();
    for p in name.split_whitespace() {
        if p.ends_with("GHz") || p.ends_with("MHz") || p == "@" {
            continue;
        }
        parts.push(p);
    }
    let cleaned = parts.join(" ");
    if cleaned.len() > 18 {
        cleaned
            .replace("NVIDIA GeForce ", "")
            .replace("AMD Radeon ", "")
    } else {
        cleaned
    }
}
