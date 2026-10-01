//! Explicit native model installation and verification entrypoint.
//!
//! This tool is intentionally separate from `xrtranslate-backend`: serving
//! audio must never surprise a user by consuming gigabytes of network traffic
//! or mutating the active model directory.

use std::path::PathBuf;

use clap::{
    Parser, Subcommand,
    builder::{PossibleValue, PossibleValuesParser},
};
use xrtranslate_assets::{
    MODEL_ASSET_CATALOG, ModelAssetId, ModelAssetsConfig, NativeModelInstaller, ResolvedModelAssets,
};
use xrtranslate_config::{AppConfig, RuntimeLayout};

#[derive(Debug, Parser)]
#[command(
    name = "xrtranslate-installer",
    version,
    about = "Install or verify native XRTranslate GGUF models"
)]
struct Arguments {
    /// Path to the compatibility config.json file.
    #[arg(long, default_value = "config.json")]
    config: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Prepare verified shared application resources for packaging.
    PrepareResources {
        #[arg(long)]
        output: PathBuf,
        #[arg(long, default_value = "target/resource-downloads")]
        cache: PathBuf,
    },
    /// Download one immutable model package, verify it, and atomically enable it.
    Install {
        #[arg(value_parser = package_value_parser())]
        package: String,
    },
    /// Read and hash installed packages without changing any active files.
    Verify {
        #[arg(value_parser = package_value_parser())]
        package: Option<String>,
    },
}

fn package_value_parser() -> PossibleValuesParser {
    PossibleValuesParser::new(
        MODEL_ASSET_CATALOG
            .iter()
            .map(|manifest| PossibleValue::new(manifest.id.as_str())),
    )
}

fn package_id(package: &str) -> ModelAssetId {
    ModelAssetId::from_config_key(package)
        .expect("clap package parser only accepts catalog model asset ids")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Arguments::parse();
    let config = AppConfig::from_path(&args.config)?;
    let project_root = args
        .config
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut asset_config = ModelAssetsConfig::with_directory_overrides(
        config.model_manager.models_directory.clone(),
        config.model_manager.qwen3_asr_gguf_directory.clone(),
        config.model_manager.hunyuan_mt_gguf_directory.clone(),
    );
    for key in config.active_native_model_assets() {
        if let Some(id) = ModelAssetId::from_config_key(&key) {
            asset_config.select_asset(id);
        }
    }
    let assets = asset_config.resolve(project_root);

    match args.command {
        Command::PrepareResources { output, cache } => {
            prepare_resources(&config, &output, &cache).await?
        }
        Command::Install { package } => install(assets, package_id(&package)).await?,
        Command::Verify { package } => {
            verify(&assets, package.as_deref().map(package_id))?;
        }
    }
    Ok(())
}

async fn install(
    assets: ResolvedModelAssets,
    package: ModelAssetId,
) -> Result<(), Box<dyn std::error::Error>> {
    let installer = NativeModelInstaller::new(assets)?;
    let mut last_file = "";
    let mut last_reported = 0_u64;
    let installed = installer
        .install(package, |progress| {
            if progress.relative_path != last_file {
                last_file = progress.relative_path;
                last_reported = 0;
            }
            const REPORT_INTERVAL: u64 = 64 * 1024 * 1024;
            if progress.downloaded_bytes == progress.total_bytes
                || progress.downloaded_bytes >= last_reported.saturating_add(REPORT_INTERVAL)
            {
                eprintln!(
                    "{}: {}/{} MiB",
                    progress.relative_path,
                    progress.downloaded_bytes / (1024 * 1024),
                    progress.total_bytes / (1024 * 1024)
                );
                last_reported = progress.downloaded_bytes;
            }
        })
        .await?;
    println!(
        "Installed and verified {} at {}",
        package,
        installed.display()
    );
    Ok(())
}

fn verify(
    assets: &ResolvedModelAssets,
    package: Option<ModelAssetId>,
) -> Result<(), Box<dyn std::error::Error>> {
    let preflight = assets.verify_integrity();
    let diagnostics = preflight
        .diagnostics()
        .iter()
        .filter(|diagnostic| package.is_none_or(|id| diagnostic.asset_id == id))
        .collect::<Vec<_>>();
    if diagnostics.is_empty() {
        println!("Installed model files match the native manifest.");
        return Ok(());
    }
    let message = diagnostics
        .iter()
        .map(|diagnostic| format!("- {diagnostic}"))
        .collect::<Vec<_>>()
        .join("\n");
    Err(message.into())
}

async fn prepare_resources(
    config: &AppConfig,
    output: &std::path::Path,
    cache: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use xrtranslate_download::{DownloadClient, DownloadSpec};
    let client = DownloadClient::new("XRTranslate-build")?;
    let mut included = std::collections::HashSet::new();
    for asset in config.model_manager.resolved_bundled_models() {
        if !included.insert(asset.relative_path.clone()) {
            continue;
        }
        let relative = std::path::Path::new(&asset.relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err("Invalid resource path".into());
        }
        let destination = output.join(relative);
        let (bytes, digest) = match asset.relative_path.as_str() {
            RuntimeLayout::VAD_MODEL_PATH => (
                RuntimeLayout::VAD_MODEL_BYTES,
                RuntimeLayout::VAD_MODEL_SHA256,
            ),
            RuntimeLayout::DENOISE_MODEL_PATH => (
                RuntimeLayout::DENOISE_MODEL_BYTES,
                RuntimeLayout::DENOISE_MODEL_SHA256,
            ),
            RuntimeLayout::SPEAKER_MODEL_PATH => (
                RuntimeLayout::SPEAKER_MODEL_BYTES,
                RuntimeLayout::SPEAKER_MODEL_SHA256,
            ),
            _ => return Err("Unknown bundled resource".into()),
        };
        if destination.is_file() && destination.metadata()?.len() == bytes {
            let content = std::fs::read(&destination)?;
            if format!("{:x}", Sha256::digest(&content)) == digest {
                continue;
            }
        }
        std::fs::create_dir_all(destination.parent().ok_or("Resource directory missing")?)?;
        let spec = DownloadSpec::verified(&asset.label, &asset.url, asset.bytes, &asset.sha256);
        if let Some(archive_path) = &asset.archive_path {
            std::fs::create_dir_all(&cache)?;
            let archive = cache.join(format!("{}.zip", asset.name));
            client.download_to(spec, &archive, |_| {}).await?;
            let mut zip = zip::ZipArchive::new(std::fs::File::open(&archive)?)?;
            let file = zip.by_name(archive_path)?;
            if file.size() != bytes {
                return Err("Bundled resource size does not match".into());
            }
            let mut data = Vec::new();
            file.take(bytes + 1).read_to_end(&mut data)?;
            if data.len() as u64 != bytes || format!("{:x}", Sha256::digest(&data)) != digest {
                return Err("Bundled resource checksum does not match".into());
            }
            std::fs::write(&destination, data)?;
        } else {
            client.download_to(spec, &destination, |_| {}).await?;
        }
    }
    Ok(())
}
