mod onnx;
mod vlm;

use std::{
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

use image::RgbImage;
use xrtranslate_assets::{
    ModelAssetId, ModelAssetsConfig, ModelCapability, ModelFileRole, ModelRuntime,
};
use xrtranslate_config::{AppConfig, NativeRuntimeBackend, NativeRuntimeSelection, RuntimeLayout};
use xrtranslate_supervisor::{FlashAttention, GpuLayers, LlamaServerRole, LlamaServerSpec};

/// A local image-to-text engine. Capture, scheduling and translation stay with
/// their owners; loading and recognition run on the OCR worker.
pub(crate) struct OcrRuntime(Engine);

enum Engine {
    Onnx(onnx::OnnxOcr),
    Vision(vlm::VisionOcr),
}

impl OcrRuntime {
    pub(crate) fn load(
        project_root: &Path,
        config: &AppConfig,
        cancelled: &AtomicBool,
    ) -> Result<Self, String> {
        check_cancelled(cancelled)?;
        let provider = config.ocr.provider.as_str();
        if provider == "none" {
            return Err("Enable an OCR model in setup first.".into());
        }
        let key = config
            .ocr
            .provider_config(provider)
            .and_then(|settings| settings.get("model_asset"))
            .and_then(serde_json::Value::as_str)
            .ok_or("Select an OCR model in setup first.")?;
        let id = ModelAssetId::from_config_key(key).ok_or("Unknown OCR model.")?;
        let assets = ModelAssetsConfig::with_directory_overrides(
            config.model_manager.models_directory.clone(),
            None,
            None,
        )
        .resolve_selected(project_root);
        let asset = assets.asset(id);
        let manifest = asset.manifest();
        if manifest.capability != ModelCapability::Ocr || manifest.provider != provider {
            return Err("The selected model does not match the OCR provider.".into());
        }
        let file = |role| {
            asset
                .file_path(role)
                .filter(|path| path.is_file())
                .ok_or_else(|| {
                    "OCR model files are missing. Complete the model setup first.".to_owned()
                })
        };
        let layout = RuntimeLayout::for_config(project_root, &config.model_manager);
        let engine = match manifest.runtime {
            Some(ModelRuntime::PaddleOcrOnnx) => Engine::Onnx(onnx::OnnxOcr::load(
                &layout.onnx_cpu_core_library(),
                &file(ModelFileRole::TextDetectionGraph)?,
                &file(ModelFileRole::TextRecognitionGraph)?,
                &file(ModelFileRole::ModelConfig)?,
            )?),
            Some(ModelRuntime::LlamaVisionChat {
                model_alias,
                extra_args,
            }) => {
                let mut spec = LlamaServerSpec::new(
                    LlamaServerRole::Ocr,
                    layout.resolve_llama_server_path(&config.model_manager.llama_server_path),
                    file(ModelFileRole::Weights)?,
                    Some(file(ModelFileRole::MultimodalProjection)?),
                    model_alias,
                );
                spec.context_size = 4096;
                spec.parallel_slots = Some(1);
                spec.flash_attention = Some(FlashAttention::Auto);
                spec.extra_args.extend(extra_args.iter().map(Into::into));
                apply_managed_runtime(&mut spec, &layout)?;
                Engine::Vision(vlm::VisionOcr::load(spec, cancelled)?)
            }
            _ => return Err("This model does not provide a supported OCR runtime.".into()),
        };
        check_cancelled(cancelled)?;
        Ok(Self(engine))
    }

    pub(crate) fn recognize(
        &mut self,
        image: RgbImage,
        cancelled: &AtomicBool,
    ) -> Result<String, String> {
        check_cancelled(cancelled)?;
        if image.width() == 0
            || image.height() == 0
            || u64::from(image.width()) * u64::from(image.height()) > 16_777_216
        {
            return Err("The OCR capture area is empty or too large.".into());
        }
        let text = match &mut self.0 {
            Engine::Onnx(model) => model.recognize(image),
            Engine::Vision(model) => model.recognize(image, cancelled),
        }?;
        check_cancelled(cancelled)?;
        Ok(text)
    }
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        Err("OCR stopped.".into())
    } else {
        Ok(())
    }
}

fn apply_managed_runtime(spec: &mut LlamaServerSpec, layout: &RuntimeLayout) -> Result<(), String> {
    let path = layout.native_runtime_selection_file();
    if !path.is_file() {
        return Ok(());
    }
    let selection: NativeRuntimeSelection =
        serde_json::from_slice(&std::fs::read(&path).map_err(|error| error.to_string())?)
            .map_err(|error| error.to_string())?;
    if selection.schema_version != 1 {
        return Err("The selected OCR runtime needs to be updated in setup.".into());
    }
    let runtime = layout.resolve_native_runtime_selection(&selection);
    match runtime.llama_cpp_backend {
        Some(NativeRuntimeBackend::Cpu) => {
            spec.gpu_layers = GpuLayers::None;
            spec.extra_args.extend([
                "--device".into(),
                "none".into(),
                "--no-mmproj-offload".into(),
            ]);
        }
        Some(NativeRuntimeBackend::Vulkan) => {
            let device = runtime
                .vulkan_device
                .ok_or("Select an OCR graphics device in setup first.")?;
            spec.environment
                .push(("GGML_VK_VISIBLE_DEVICES".into(), device.to_string().into()));
            spec.extra_args
                .extend(["--device".into(), "Vulkan0".into()]);
        }
        _ => {
            if let Some(directory) = runtime.cuda_bin_dir {
                let variable = if cfg!(windows) {
                    "PATH"
                } else {
                    "LD_LIBRARY_PATH"
                };
                let mut paths = vec![directory];
                if let Some(existing) = std::env::var_os(variable) {
                    paths.extend(std::env::split_paths(&existing));
                }
                spec.environment.push((
                    variable.into(),
                    std::env::join_paths(paths).map_err(|error| error.to_string())?,
                ));
            }
        }
    }
    Ok(())
}
