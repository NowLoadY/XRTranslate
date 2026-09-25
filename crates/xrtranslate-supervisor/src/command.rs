use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{LlamaServerSpec, SpecValidationError};

/// A fully materialized, but not yet launched, llama-server command.
///
/// This serializable-in-spirit representation is intentionally separate from
/// [`std::process::Command`] so callers can log it and unit tests can validate
/// argument semantics without launching models.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlamaServerCommand {
    program: PathBuf,
    arguments: Vec<OsString>,
    current_dir: Option<PathBuf>,
    environment: Vec<(OsString, OsString)>,
}

impl LlamaServerCommand {
    /// Builds a command after validating the launch specification.
    pub fn from_spec(spec: &LlamaServerSpec) -> Result<Self, SpecValidationError> {
        spec.validate()?;
        Ok(Self {
            program: spec.executable.clone(),
            arguments: spec.command_args(),
            current_dir: spec.working_directory().map(Path::to_path_buf),
            environment: spec.environment.clone(),
        })
    }

    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    #[must_use]
    pub fn arguments(&self) -> &[OsString] {
        &self.arguments
    }

    #[must_use]
    pub fn current_dir(&self) -> Option<&Path> {
        self.current_dir.as_deref()
    }

    #[must_use]
    pub fn environment(&self) -> &[(OsString, OsString)] {
        &self.environment
    }

    /// Converts the immutable command plan into a standard-library process
    /// command immediately before spawning it.
    #[must_use]
    pub fn as_std_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.arguments);
        if let Some(current_dir) = &self.current_dir {
            command.current_dir(current_dir);
        }
        command.envs(self.environment.iter().cloned());
        command
    }
}

#[cfg(test)]
mod tests {
    use std::{
        ffi::OsString,
        net::Ipv4Addr,
        path::{Path, PathBuf},
    };

    use crate::{
        FlashAttention, GpuLayers, LlamaServerCommand, LlamaServerEndpoint, LlamaServerRole,
        LlamaServerSpec, SpecValidationError,
    };

    fn strings(arguments: &[OsString]) -> Vec<String> {
        arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn audio_chat_command_includes_multimodal_projection_and_alias() {
        let spec = LlamaServerSpec::new(
            LlamaServerRole::Asr,
            "C:/llama/llama-server.exe",
            "C:/models/audio.gguf",
            Some(PathBuf::from("C:/models/audio.mmproj.gguf")),
            "speech-model",
        )
        .with_endpoint(LlamaServerEndpoint::new(Ipv4Addr::LOCALHOST.into(), 8101));

        let command = LlamaServerCommand::from_spec(&spec).expect("valid audio ASR spec");

        assert_eq!(
            command.program(),
            PathBuf::from("C:/llama/llama-server.exe")
        );
        assert_eq!(
            strings(command.arguments()),
            [
                "--host",
                "127.0.0.1",
                "--port",
                "8101",
                "--model",
                "C:/models/audio.gguf",
                "--mmproj",
                "C:/models/audio.mmproj.gguf",
                "--alias",
                "speech-model",
                "--ctx-size",
                "2048",
                "--n-gpu-layers",
                "99",
            ]
        );
    }

    #[test]
    fn text_chat_command_uses_configured_runtime_without_mmproj() {
        let mut spec = LlamaServerSpec::new(
            LlamaServerRole::Translation,
            "C:/llama/llama-server.exe",
            "C:/models/text.gguf",
            None,
            "text-model",
        );
        spec.context_size = 4096;
        spec.parallel_slots = Some(4);
        spec.gpu_layers = GpuLayers::All;
        spec.flash_attention = Some(FlashAttention::Auto);
        spec.extra_args.push(OsString::from("--no-webui"));

        let command = LlamaServerCommand::from_spec(&spec).expect("valid translation spec");
        let arguments = strings(command.arguments());

        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--alias", "text-model"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--ctx-size", "4096"])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--n-gpu-layers", "-1"])
        );
        assert!(arguments.windows(2).any(|pair| pair == ["--parallel", "4"]));
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--flash-attn", "auto"])
        );
        assert!(!arguments.iter().any(|argument| argument == "--mmproj"));
        assert_eq!(arguments.last(), Some(&"--no-webui".to_owned()));
    }

    #[test]
    fn audio_chat_requires_mmproj_before_a_command_is_built() {
        let spec = LlamaServerSpec::new(
            LlamaServerRole::Asr,
            "llama-server",
            "audio.gguf",
            None,
            "speech-model",
        );

        assert_eq!(
            LlamaServerCommand::from_spec(&spec),
            Err(SpecValidationError::MissingMultimodalProjection)
        );
    }

    #[test]
    fn command_keeps_absolute_runtime_and_model_paths_independent_of_cwd() {
        let mut spec = LlamaServerSpec::new(
            LlamaServerRole::Translation,
            "/srv/xrtranslate/runtime/llama.cpp/llama-server",
            "/srv/xrtranslate/models/text.gguf",
            None,
            "text-model",
        );
        spec.working_directory = Some(PathBuf::from("/srv/xrtranslate"));

        let command = LlamaServerCommand::from_spec(&spec).expect("valid absolute spec");

        assert_eq!(
            command.program(),
            Path::new("/srv/xrtranslate/runtime/llama.cpp/llama-server")
        );
        assert_eq!(command.current_dir(), Some(Path::new("/srv/xrtranslate")));
        let arguments = strings(command.arguments());
        assert!(
            arguments
                .windows(2)
                .any(|pair| { pair == ["--model", "/srv/xrtranslate/models/text.gguf"] })
        );
    }
}
