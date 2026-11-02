use std::{fs, io};

use crate::{ModelAssetId, ModelFileRole, ResolvedModelAssets};

/// Prepare the built-in denoiser offline, including its required attribution.
/// Use the resolved model directory for both source runs and packaged installs.
pub fn ensure_bundled_gtcrn(assets: &ResolvedModelAssets) -> io::Result<()> {
    let asset = assets.asset(ModelAssetId::GtcrnOnnx);
    for (role, bytes) in [
        (
            ModelFileRole::Weights,
            include_bytes!("../../../assets/gtcrn/gtcrn_simple.onnx").as_slice(),
        ),
        (
            ModelFileRole::License,
            include_bytes!("../../../assets/gtcrn/LICENSE").as_slice(),
        ),
    ] {
        let path = asset
            .file_path(role)
            .expect("bundled GTCRN file in catalog");
        let result = (|| -> io::Result<()> {
            match fs::read(&path) {
                Ok(existing) if existing == bytes => return Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            fs::create_dir_all(asset.directory())?;
            let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
            let result = fs::write(&temporary, bytes).and_then(|()| fs::rename(&temporary, &path));
            if result.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            result
        })();
        result.map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("Cannot prepare {}: {error}", path.display()),
            )
        })?;
    }
    Ok(())
}
