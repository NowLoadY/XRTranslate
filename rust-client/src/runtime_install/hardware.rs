//! Host GPU capabilities. Detection never downloads runtimes or changes drivers.

use super::parse_version;
use std::{path::PathBuf, process::Command};
use xrtranslate_assets::{ModelAccelerator, ModelHardwareRequirements};

const MIN_CUDA_COMPUTE_CAPABILITY: (u16, u16) = (6, 0);
const MIN_LOCAL_MODEL_VRAM_BYTES: u64 = 1024 * 1024 * 1024;
pub(crate) const NVIDIA_APP_URL: &str = "https://www.nvidia.com/en-us/software/nvidia-app/";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct NvidiaCuda {
    pub(super) gpu: String,
    pub(super) compute_capability: (u16, u16),
    pub(super) driver_cuda: String,
    pub(super) memory_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvailableGpu {
    pub name: String,
    pub raw_name: String,
    pub memory_bytes: u64,
    pub is_cuda: bool,
    pub vulkan_index: Option<u32>,
    pub compute_capability: Option<(u16, u16)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LocalModelAvailability {
    Detecting,
    Available {
        gpu: String,
        memory_bytes: u64,
        cuda_memory_bytes: u64,
        all_gpus: Vec<AvailableGpu>,
    },
    InsufficientVram {
        gpu: String,
        memory_bytes: u64,
        required_bytes: u64,
        all_gpus: Vec<AvailableGpu>,
    },
    Unavailable(String),
}

impl LocalModelAvailability {
    #[must_use]
    pub fn available_gpus(&self) -> &[AvailableGpu] {
        match self {
            Self::Available { all_gpus, .. } | Self::InsufficientVram { all_gpus, .. } => all_gpus,
            _ => &[],
        }
    }

    pub fn supports(&self, hardware: ModelHardwareRequirements) -> bool {
        if hardware.accelerator == ModelAccelerator::Cpu {
            return true;
        }
        matches!(self, Self::Available { memory_bytes, cuda_memory_bytes, .. }
            if (if hardware.accelerator == ModelAccelerator::NvidiaCuda {
                *cuda_memory_bytes
            } else {
                *memory_bytes
            }) >= hardware.minimum_memory_bytes.max(MIN_LOCAL_MODEL_VRAM_BYTES))
    }
}

impl std::fmt::Display for LocalModelAvailability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Detecting => f.write_str("GPU detection in progress"),
            Self::Unavailable(reason) => f.write_str(reason),
            Self::Available {
                gpu, memory_bytes, ..
            }
            | Self::InsufficientVram {
                gpu, memory_bytes, ..
            } => {
                write!(
                    f,
                    "{gpu} · {:.1} GiB",
                    *memory_bytes as f64 / (1024.0 * 1024.0 * 1024.0)
                )
            }
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct VulkanGpu {
    pub gpu: String,
    pub memory_bytes: u64,
    pub index: u32,
}

pub(super) struct Hardware {
    pub nvidia: Option<NvidiaCuda>,
    pub amd: Option<VulkanGpu>,
    pub availability: LocalModelAvailability,
}

impl Hardware {
    pub fn detect(preferred_gpu: Option<&str>) -> Self {
        let cuda = supported_nvidia_cuda_gpus();
        let vulkan = supported_vulkan_gpus();
        let nvidia_list = cuda.as_ref().ok().cloned().unwrap_or_default();
        let vulkan_list = vulkan.as_ref().ok().cloned().unwrap_or_default();

        let (selected_nvidia, selected_vulkan, mut availability) =
            resolve_hardware_selection(&nvidia_list, &vulkan_list, preferred_gpu);

        if let LocalModelAvailability::Unavailable(reason) = &mut availability {
            for error in [cuda.err(), vulkan.err()].into_iter().flatten() {
                reason.push(' ');
                reason.push_str(&error);
            }
        }
        Self {
            nvidia: selected_nvidia,
            amd: selected_vulkan,
            availability,
        }
    }
}

pub(super) fn resolve_hardware_selection(
    nvidia_gpus: &[NvidiaCuda],
    vulkan_gpus: &[VulkanGpu],
    preferred_gpu: Option<&str>,
) -> (
    Option<NvidiaCuda>,
    Option<VulkanGpu>,
    LocalModelAvailability,
) {
    let mut all_gpus = Vec::new();
    for n in nvidia_gpus {
        all_gpus.push(AvailableGpu {
            name: format!("{} (CUDA)", n.gpu),
            raw_name: n.gpu.clone(),
            memory_bytes: n.memory_bytes,
            is_cuda: true,
            vulkan_index: None,
            compute_capability: Some(n.compute_capability),
        });
    }
    for v in vulkan_gpus {
        all_gpus.push(AvailableGpu {
            name: format!("{} (Vulkan)", v.gpu),
            raw_name: v.gpu.clone(),
            memory_bytes: v.memory_bytes,
            is_cuda: false,
            vulkan_index: Some(v.index),
            compute_capability: None,
        });
    }

    if all_gpus.is_empty() {
        return (
            None,
            None,
            LocalModelAvailability::Unavailable(
                "Local GPU models require NVIDIA CUDA or AMD Vulkan 1.2 with 16-bit storage. CPU ONNX models remain available.".into(),
            ),
        );
    }

    let selected = preferred_gpu
        .and_then(|pref| {
            all_gpus.iter().find(|g| {
                g.name == pref
                    || g.name.eq_ignore_ascii_case(pref)
                    || g.raw_name == pref
                    || g.raw_name.eq_ignore_ascii_case(pref)
            })
        })
        .or_else(|| {
            // Priority default:
            // 1. Dedicated NVIDIA CUDA GPU
            all_gpus
                .iter()
                .filter(|g| g.is_cuda)
                .max_by_key(|g| g.memory_bytes)
                // 2. Vulkan GPU with largest memory
                .or_else(|| all_gpus.iter().max_by_key(|g| g.memory_bytes))
        });

    let Some(selected) = selected else {
        return (
            None,
            None,
            LocalModelAvailability::Unavailable(
                "Local GPU models require NVIDIA CUDA or AMD Vulkan 1.2 with 16-bit storage. CPU ONNX models remain available.".into(),
            ),
        );
    };

    if selected.memory_bytes < MIN_LOCAL_MODEL_VRAM_BYTES {
        return (
            None,
            None,
            LocalModelAvailability::InsufficientVram {
                gpu: selected.name.clone(),
                memory_bytes: selected.memory_bytes,
                required_bytes: MIN_LOCAL_MODEL_VRAM_BYTES,
                all_gpus,
            },
        );
    }

    if selected.is_cuda {
        let n = nvidia_gpus
            .iter()
            .find(|n| n.gpu == selected.raw_name)
            .cloned();
        let cuda_memory_bytes = selected.memory_bytes;
        (
            n,
            None,
            LocalModelAvailability::Available {
                gpu: selected.name.clone(),
                memory_bytes: selected.memory_bytes,
                cuda_memory_bytes,
                all_gpus,
            },
        )
    } else {
        let v = vulkan_gpus
            .iter()
            .find(|v| v.gpu == selected.raw_name && Some(v.index) == selected.vulkan_index)
            .or_else(|| {
                vulkan_gpus
                    .iter()
                    .find(|v| Some(v.index) == selected.vulkan_index)
            })
            .or_else(|| vulkan_gpus.iter().find(|v| v.gpu == selected.raw_name))
            .cloned();
        (
            None,
            v,
            LocalModelAvailability::Available {
                gpu: selected.name.clone(),
                memory_bytes: selected.memory_bytes,
                cuda_memory_bytes: 0,
                all_gpus,
            },
        )
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn local_model_availability(
    nvidia: Option<&NvidiaCuda>,
    amd: Option<&VulkanGpu>,
) -> LocalModelAvailability {
    let nvidia_list = nvidia.into_iter().cloned().collect::<Vec<_>>();
    let amd_list = amd.into_iter().cloned().collect::<Vec<_>>();
    let (_, _, availability) = resolve_hardware_selection(&nvidia_list, &amd_list, None);
    availability
}

/// Query the driver directly: no SDK, helper executable or downloaded runtime is
/// needed. Keep the physical index used by GGML_VK_VISIBLE_DEVICES, including
/// adapters of other vendors, so hybrid systems launch on the GPU we checked.
fn supported_vulkan_gpus() -> Result<Vec<VulkanGpu>, String> {
    use ash::vk;
    // SAFETY: the loader outlives the instance; all physical devices originate
    // from it, feature chains are scoped to each call, and no pointers escape.
    unsafe {
        let entry =
            ash::Entry::load().map_err(|error| format!("Vulkan driver unavailable: {error}"))?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_2);
        let instance = entry
            .create_instance(
                &vk::InstanceCreateInfo::default().application_info(&app),
                None,
            )
            .map_err(|error| format!("Cannot initialize Vulkan 1.2: {error}"))?;
        let result = (|| {
            let devices = instance
                .enumerate_physical_devices()
                .map_err(|error| format!("Cannot enumerate Vulkan GPUs: {error}"))?;
            let mut candidates = Vec::new();
            let mut unsupported = Vec::new();
            for (index, device) in devices.into_iter().enumerate() {
                let properties = instance.get_physical_device_properties(device);
                if !matches!(
                    properties.device_type,
                    vk::PhysicalDeviceType::DISCRETE_GPU | vk::PhysicalDeviceType::INTEGRATED_GPU
                ) {
                    continue;
                }
                let name = std::ffi::CStr::from_ptr(properties.device_name.as_ptr())
                    .to_string_lossy()
                    .into_owned();
                let mut storage = vk::PhysicalDevice16BitStorageFeatures::default();
                instance.get_physical_device_features2(
                    device,
                    &mut vk::PhysicalDeviceFeatures2::default().push_next(&mut storage),
                );
                let compute = instance
                    .get_physical_device_queue_family_properties(device)
                    .iter()
                    .any(|queue| {
                        queue.queue_count > 0 && queue.queue_flags.contains(vk::QueueFlags::COMPUTE)
                    });
                if properties.api_version < vk::API_VERSION_1_2
                    || storage.storage_buffer16_bit_access == 0
                    || !compute
                {
                    unsupported.push(name);
                    continue;
                }
                let memory = instance.get_physical_device_memory_properties(device);
                let memory_bytes = memory.memory_heaps[..memory.memory_heap_count as usize]
                    .iter()
                    .filter(|heap| heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
                    .map(|heap| heap.size)
                    .sum();
                candidates.push(VulkanGpu {
                    gpu: name,
                    memory_bytes,
                    index: index as u32,
                });
            }
            if candidates.is_empty() && !unsupported.is_empty() {
                return Err(format!(
                    "Vulkan adapters {} lack the required Vulkan capabilities; update the graphics driver.",
                    unsupported.join(", ")
                ));
            }
            Ok(candidates)
        })();
        instance.destroy_instance(None);
        result
    }
}

#[allow(dead_code)]
pub(super) fn supported_vulkan() -> Result<Option<VulkanGpu>, String> {
    supported_vulkan_gpus().map(|gpus| gpus.into_iter().max_by_key(|gpu| gpu.memory_bytes))
}

pub(super) fn supported_nvidia_cuda_gpus() -> Result<Vec<NvidiaCuda>, String> {
    let Some((program, query)) = run_nvidia_smi(&[
        "--query-gpu=name,compute_cap,memory.total",
        "--format=csv,noheader,nounits",
    ])?
    else {
        return Ok(Vec::new());
    };
    if !query.status.success() {
        return Err(command_failure("Cannot query NVIDIA GPUs", &query));
    }
    let gpus = parse_nvidia_gpu_rows(&String::from_utf8_lossy(&query.stdout))?;
    if gpus.is_empty() {
        return Ok(Vec::new());
    }

    let version_output = crate::child_process::hide_console(&mut Command::new(&program))
        .output()
        .map_err(|error| format!("Cannot run {}: {error}", program.display()))?;
    if !version_output.status.success() {
        return Err(command_failure(
            "Cannot query the NVIDIA driver CUDA version",
            &version_output,
        ));
    }
    let version_text = String::from_utf8_lossy(&version_output.stdout);
    let driver_cuda = cuda_version_from_nvidia_smi(&version_text).ok_or_else(|| {
        "nvidia-smi did not report a parseable CUDA Version or CUDA UMD Version; refusing to silently install the CPU runtime on an NVIDIA system.".to_owned()
    })?;

    let valid_gpus = gpus
        .into_iter()
        .filter(|gpu| gpu.compute_capability >= MIN_CUDA_COMPUTE_CAPABILITY)
        .map(|mut gpu| {
            gpu.driver_cuda = driver_cuda.clone();
            gpu
        })
        .collect();

    Ok(valid_gpus)
}

#[allow(dead_code)]
pub(super) fn supported_nvidia_cuda() -> Result<Option<NvidiaCuda>, String> {
    supported_nvidia_cuda_gpus().map(|gpus| gpus.into_iter().next())
}

pub(super) fn parse_nvidia_gpu_rows(output: &str) -> Result<Vec<NvidiaCuda>, String> {
    let mut gpus = Vec::new();
    let mut invalid = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let columns = line.split(',').map(str::trim).collect::<Vec<_>>();
        let [gpu, capability, memory_mib] = columns.as_slice() else {
            invalid.push(line.to_owned());
            continue;
        };
        let Some(compute_capability) = parse_version(capability) else {
            invalid.push(line.to_owned());
            continue;
        };
        let Ok(memory_mib) = memory_mib.parse::<u64>() else {
            invalid.push(line.to_owned());
            continue;
        };
        gpus.push(NvidiaCuda {
            gpu: (*gpu).to_owned(),
            compute_capability,
            driver_cuda: String::new(),
            memory_bytes: memory_mib.saturating_mul(1024 * 1024),
        });
    }
    if gpus.is_empty() {
        let detail = if invalid.is_empty() {
            "nvidia-smi returned no GPU rows".to_owned()
        } else {
            format!("unparseable rows: {}", invalid.join(" | "))
        };
        Err(format!("Cannot identify an NVIDIA GPU ({detail})."))
    } else {
        Ok(gpus)
    }
}

fn run_nvidia_smi(args: &[&str]) -> Result<Option<(PathBuf, std::process::Output)>, String> {
    let mut candidates = Vec::new();
    if let Some(system_root) = std::env::var_os("SystemRoot") {
        candidates.push(PathBuf::from(system_root).join("System32/nvidia-smi.exe"));
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates
            .push(PathBuf::from(program_files).join("NVIDIA Corporation/NVSMI/nvidia-smi.exe"));
    }
    candidates.push(PathBuf::from("nvidia-smi"));

    for program in candidates {
        match crate::child_process::hide_console(&mut Command::new(&program))
            .args(args)
            .output()
        {
            Ok(output) => return Ok(Some((program, output))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!("Cannot run {}: {error}", program.display()));
            }
        }
    }

    if windows_reports_nvidia_adapter() {
        Err(format!(
            "Windows reports an NVIDIA display adapter, but nvidia-smi could not be found. Reinstall or update the NVIDIA driver with NVIDIA App ({NVIDIA_APP_URL}); the installer will not silently substitute a CPU runtime."
        ))
    } else {
        Ok(None)
    }
}

fn windows_reports_nvidia_adapter() -> bool {
    if !cfg!(target_os = "windows") {
        return false;
    }
    crate::child_process::hide_console(&mut Command::new("reg"))
        .args([
            "query",
            "HKLM\\SYSTEM\\CurrentControlSet\\Control\\Class\\{4d36e968-e325-11ce-bfc1-08002be10318}",
            "/s",
            "/v",
            "DriverDesc",
        ])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| {
            String::from_utf8_lossy(&output.stdout)
                .to_ascii_lowercase()
                .contains("nvidia")
        })
}

fn command_failure(context: &str, output: &std::process::Output) -> String {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if detail.is_empty() {
        format!("{context} (exit status {})", output.status)
    } else {
        format!("{context}: {detail}")
    }
}

pub(super) fn cuda_version_from_nvidia_smi(version_text: &str) -> Option<String> {
    let version = ["CUDA Version: ", "CUDA UMD Version: "]
        .into_iter()
        .find_map(|marker| {
            let start = version_text.find(marker)? + marker.len();
            Some(
                version_text[start..]
                    .split_whitespace()
                    .next()?
                    .trim_end_matches('|'),
            )
        })?;
    parse_version(version)?;
    Some(version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_backend_selection_is_independent_of_host_hardware() {
        let nvidia = NvidiaCuda {
            gpu: "Test GPU".into(),
            compute_capability: (8, 6),
            driver_cuda: "13.3".into(),
            memory_bytes: 4 * 1024 * 1024 * 1024,
        };
        let vulkan = VulkanGpu {
            gpu: "Test GPU".into(),
            memory_bytes: nvidia.memory_bytes,
            index: 2,
        };
        let (cuda, vk, availability) =
            resolve_hardware_selection(&[nvidia.clone()], &[vulkan.clone()], None);
        assert!(cuda.is_some());
        assert!(vk.is_none());
        assert_eq!(availability.available_gpus().len(), 2);

        let (cuda, vk, availability) =
            resolve_hardware_selection(&[nvidia], &[vulkan], Some("Test GPU (Vulkan)"));
        assert!(cuda.is_none());
        assert_eq!(vk.unwrap().index, 2);
        assert!(matches!(
            availability,
            LocalModelAvailability::Available {
                cuda_memory_bytes: 0,
                ..
            }
        ));
        assert_eq!(availability.to_string(), "Test GPU (Vulkan) · 4.0 GiB");
    }
}
