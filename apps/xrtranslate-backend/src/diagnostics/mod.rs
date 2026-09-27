//! Explicit real-model diagnostics, separate from the serving path and unit tests.

mod matrix;
mod results;
mod roundtrip;

use crate::Arguments;
use std::path::PathBuf;
use xrtranslate_config::AppConfig;

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    /// Run text -> translation -> TTS -> ASR -> back translation, then exit.
    Roundtrip(roundtrip::Options),
    /// Print installed ASR/translation/TTS packages and language capabilities as JSON.
    Models,
    /// Run all installed model combinations compatible with the input languages.
    RoundtripMatrix(matrix::Options),
}

pub(crate) async fn run(
    args: &Arguments,
    command: &Command,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Roundtrip(options) => roundtrip::run(args, options).await,
        Command::RoundtripMatrix(options) => matrix::run(args, options),
        Command::Models => {
            let (root, config) = load_config(args)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "models": config.installed_models(&root), "runtime_verified": false
                }))?
            );
            Ok(())
        }
    }
}

fn load_config(args: &Arguments) -> Result<(PathBuf, AppConfig), String> {
    let root = std::path::absolute(
        args.config
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(std::path::Path::new(".")),
    )
    .map_err(|e| e.to_string())?;
    let config =
        AppConfig::from_path_with_user_config(&args.config, &root).map_err(|e| e.to_string())?;
    Ok((root, config))
}
