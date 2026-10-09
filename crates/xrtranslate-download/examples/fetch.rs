use xrtranslate_download::{DownloadClient, DownloadSpec};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if arguments.len() != 3 {
        return Err("Usage: fetch URL BYTES DESTINATION".into());
    }
    let spec = DownloadSpec::new("Build resource", &arguments[0], arguments[1].parse()?);
    let destination = std::path::Path::new(&arguments[2]);
    if let Some(directory) = destination.parent() {
        std::fs::create_dir_all(directory)?;
    }
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            DownloadClient::new("XRTranslate-build")?
                .download_to(spec, destination, |_| {})
                .await
        })?;
    Ok(())
}
