use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct SystemMetrics {
    pub time_str: String,
    pub cpu_name: String,
    pub cpu_usage: u32,
    pub gpu_name: String,
    pub gpu_usage: u32,
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self {
            time_str: String::new(),
            cpu_name: String::new(),
            cpu_usage: 0,
            gpu_name: String::new(),
            gpu_usage: 0,
        }
    }
}

pub struct SystemMonitor {
    metrics: Arc<parking_lot::Mutex<SystemMetrics>>,
}

impl SystemMonitor {
    pub fn new() -> Self {
        let metrics = Arc::new(parking_lot::Mutex::new(SystemMetrics::default()));
        let metrics_clone = Arc::clone(&metrics);

        let cpu_name = detect_cpu_name();
        let gpu_name = detect_gpu_name();

        std::thread::Builder::new()
            .name("sys-monitor".into())
            .spawn(move || {
                let mut cpu_sampler = CpuUsageSampler::new();
                let mut gpu_sampler = GpuUsageSampler::new();
                loop {
                    let now_str = get_formatted_time();
                    let cpu_usage = cpu_sampler.sample();
                    let gpu_usage = gpu_sampler.as_mut().and_then(GpuUsageSampler::sample);

                    {
                        let mut guard = metrics_clone.lock();
                        guard.time_str = now_str;
                        guard.cpu_name = cpu_name.clone();
                        guard.gpu_name = gpu_name.clone();
                        if let Some(cpu_usage) = cpu_usage {
                            guard.cpu_usage = cpu_usage;
                        }
                        if let Some(gpu_usage) = gpu_usage {
                            guard.gpu_usage = gpu_usage;
                        }
                    }

                    std::thread::sleep(Duration::from_millis(1000));
                }
            })
            .ok();

        Self { metrics }
    }

    pub fn snapshot(&self) -> SystemMetrics {
        let mut m = self.metrics.lock().clone();
        if m.time_str.is_empty() {
            m.time_str = get_formatted_time();
        }
        if m.cpu_name.is_empty() {
            m.cpu_name = detect_cpu_name();
        }
        if m.gpu_name.is_empty() {
            m.gpu_name = detect_gpu_name();
        }
        m
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
    format!("{:02}:{:02}:{:02}", local.wHour, local.wMinute, local.wSecond)
}

#[cfg(not(windows))]
fn get_formatted_time() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() % 86_400);
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3_600,
        (seconds / 60) % 60,
        seconds % 60
    )
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

#[cfg(test)]
mod tests {
    #[test]
    fn formatted_time_is_current_and_well_formed() {
        let value = super::get_formatted_time();
        let parts = value.split(':').collect::<Vec<_>>();
        assert_eq!(parts.len(), 3);
        assert!(parts.iter().all(|part| part.len() == 2));
        assert!(parts.iter().all(|part| part.parse::<u8>().is_ok()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn gpu_engine_instances_from_different_processes_share_an_engine_key() {
        let first = "pid_120_luid_0x00000000_0x0000A123_phys_0_eng_2_engtype_Compute_0";
        let second = "pid_456_luid_0x00000000_0x0000A123_phys_0_eng_2_engtype_Compute_0";
        let other = "pid_456_luid_0x00000000_0x0000A123_phys_0_eng_3_engtype_Copy";

        assert_eq!(super::gpu_engine_key(first), super::gpu_engine_key(second));
        assert_ne!(super::gpu_engine_key(first), super::gpu_engine_key(other));
    }
}
