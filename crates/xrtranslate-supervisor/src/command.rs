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
