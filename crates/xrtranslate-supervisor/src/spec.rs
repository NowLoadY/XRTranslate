use std::{
    ffi::OsString,
    fmt,
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    time::Duration,
};

/// The capability served by one local llama.cpp process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LlamaServerRole {
    Asr,
    Translation,
}

/// GPU-layer policy passed to llama.cpp's `--n-gpu-layers` option.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuLayers {
    /// Keep all layers on the CPU.
    None,
    /// Offload exactly this many layers.
    Count(u32),
    /// Ask llama.cpp to offload every layer that the backend supports.
    All,
}

impl GpuLayers {
    fn command_value(self) -> String {
        match self {
            Self::None => "0".to_owned(),
            Self::Count(count) => count.to_string(),
            Self::All => "-1".to_owned(),
        }
    }
}

/// Flash-attention policy forwarded to llama.cpp.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlashAttention {
    Auto,
    On,
    Off,
}

impl FlashAttention {
    fn command_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }
}

/// The address on which a locally managed llama-server listens.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlamaServerEndpoint {
    pub host: IpAddr,
    pub port: u16,
}

impl LlamaServerEndpoint {
    #[must_use]
    pub const fn new(host: IpAddr, port: u16) -> Self {
        Self { host, port }
    }

    /// Builds an HTTP URL without requiring a URL-parsing dependency.
    #[must_use]
    pub fn url(&self, path: &str) -> String {
        let path = if path.starts_with('/') {
            path.to_owned()
        } else {
            format!("/{path}")
        };
        let host = match self.host {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        };
        format!("http://{host}:{}{path}", self.port)
    }
}

impl fmt::Display for LlamaServerEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.host, self.port)
    }
}

/// Configuration required to start one `llama-server` process.
///
/// The type is intentionally independent from `xrtranslate-config`.  The
/// integration layer can convert persisted user settings into this validated
/// runtime specification without making this low-level crate depend on the
/// configuration format.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlamaServerSpec {
    pub role: LlamaServerRole,
    pub executable: PathBuf,
    pub model: PathBuf,
    pub mmproj: Option<PathBuf>,
    pub endpoint: LlamaServerEndpoint,
    pub model_alias: String,
    pub context_size: u32,
    pub gpu_layers: GpuLayers,
    pub parallel_slots: Option<u16>,
    pub flash_attention: Option<FlashAttention>,
    pub working_directory: Option<PathBuf>,
    pub environment: Vec<(OsString, OsString)>,
    pub extra_args: Vec<OsString>,
    pub startup_timeout: Duration,
}

impl LlamaServerSpec {
    /// Creates a capability-level server plan. The model card supplies the
    /// alias, required files and optional launch settings.
    #[must_use]
    pub fn new(
        role: LlamaServerRole,
        executable: impl Into<PathBuf>,
        model: impl Into<PathBuf>,
        mmproj: Option<PathBuf>,
        model_alias: impl Into<String>,
    ) -> Self {
        Self {
            role,
            executable: executable.into(),
            model: model.into(),
            mmproj,
            endpoint: LlamaServerEndpoint::new(
                IpAddr::V4(Ipv4Addr::UNSPECIFIED),
                match role {
                    LlamaServerRole::Asr => 8001,
                    LlamaServerRole::Translation => 8002,
                },
            ),
            model_alias: model_alias.into(),
            context_size: 2048,
            gpu_layers: GpuLayers::Count(99),
            parallel_slots: None,
            flash_attention: None,
            working_directory: None,
            environment: Vec::new(),
            extra_args: Vec::new(),
            startup_timeout: Duration::from_secs(90),
        }
    }

    /// Returns a copy configured to listen on a specific local interface.
    #[must_use]
    pub fn with_endpoint(mut self, endpoint: LlamaServerEndpoint) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Ensures that the configuration is structurally safe to turn into a
    /// child-process invocation.  Deliberately does not check filesystem
    /// existence so callers can construct a plan before model download.
    pub fn validate(&self) -> Result<(), SpecValidationError> {
        if self.executable.as_os_str().is_empty() {
            return Err(SpecValidationError::MissingExecutable);
        }
        if self.model.as_os_str().is_empty() {
            return Err(SpecValidationError::MissingModel);
        }
        if self.endpoint.port == 0 {
            return Err(SpecValidationError::InvalidPort);
        }
        if self.context_size == 0 {
            return Err(SpecValidationError::InvalidContextSize);
        }
        if self.parallel_slots == Some(0) {
            return Err(SpecValidationError::InvalidParallelSlots);
        }
        if self.model_alias.trim().is_empty() {
            return Err(SpecValidationError::MissingModelAlias);
        }
        match self.role {
            LlamaServerRole::Asr if self.mmproj.is_none() => {
                Err(SpecValidationError::MissingMultimodalProjection)
            }
            LlamaServerRole::Translation if self.mmproj.is_some() => {
                Err(SpecValidationError::UnexpectedMultimodalProjection)
            }
            _ => Ok(()),
        }
    }

    pub(crate) fn command_args(&self) -> Vec<OsString> {
        let mut arguments = vec![
            OsString::from("--host"),
            OsString::from(self.endpoint.host.to_string()),
            OsString::from("--port"),
            OsString::from(self.endpoint.port.to_string()),
            OsString::from("--model"),
            self.model.clone().into_os_string(),
        ];

        if let Some(mmproj) = &self.mmproj {
            arguments.extend([OsString::from("--mmproj"), mmproj.clone().into_os_string()]);
        }

        arguments.extend([
            OsString::from("--alias"),
            OsString::from(&self.model_alias),
            OsString::from("--ctx-size"),
            OsString::from(self.context_size.to_string()),
            OsString::from("--n-gpu-layers"),
            OsString::from(self.gpu_layers.command_value()),
        ]);

        if let Some(parallel_slots) = self.parallel_slots {
            arguments.extend([
                OsString::from("--parallel"),
                OsString::from(parallel_slots.to_string()),
            ]);
        }
        if let Some(flash_attention) = self.flash_attention {
            arguments.extend([
                OsString::from("--flash-attn"),
                OsString::from(flash_attention.command_value()),
            ]);
        }
        arguments.extend(self.extra_args.iter().cloned());
        arguments
    }

    pub(crate) fn working_directory(&self) -> Option<&Path> {
        self.working_directory.as_deref()
    }
}

/// A configuration error that can be shown before attempting to spawn a
/// process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpecValidationError {
    MissingExecutable,
    MissingModel,
    InvalidPort,
    InvalidContextSize,
    InvalidParallelSlots,
    MissingModelAlias,
    MissingMultimodalProjection,
    UnexpectedMultimodalProjection,
}

impl fmt::Display for SpecValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::MissingExecutable => "llama-server executable path is empty",
            Self::MissingModel => "GGUF model path is empty",
            Self::InvalidPort => "llama-server port must be non-zero",
            Self::InvalidContextSize => "llama-server context size must be non-zero",
            Self::InvalidParallelSlots => "llama-server parallel slots must be non-zero",
            Self::MissingModelAlias => "llama-server model alias is empty",
            Self::MissingMultimodalProjection => "audio chat ASR requires an mmproj GGUF file",
            Self::UnexpectedMultimodalProjection => {
                "text translation does not accept a multimodal projection file"
            }
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for SpecValidationError {}
