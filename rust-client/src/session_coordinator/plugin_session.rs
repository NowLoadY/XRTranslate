use super::PluginSessionOwner;

/// Controls which application-wide presentation channels receive a plugin
/// session. Domain subscribers still receive the typed session event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutputPolicy {
    /// Feed the normal history, overlay, TTS and external caption outputs.
    Host,
    /// Keep results out of host presentation and deliver them to subscribers.
    PluginOnly,
}

/// Uniform configuration supplied by a plugin before it uses recognition and
/// translation infrastructure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSessionBinding {
    pub owner: PluginSessionOwner,
    pub output_policy: SessionOutputPolicy,
    pub host_tts: bool,
    pub external_audio_gate: bool,
    pub finish_when_audio_ends: bool,
}

impl PluginSessionBinding {
    pub const fn publish_to_host_outputs(&self) -> bool {
        matches!(self.output_policy, SessionOutputPolicy::Host)
    }
}

/// Implemented by plugins that acquire the translation capability.
/// The infrastructure consumes only this neutral binding, never plugin types.
pub trait TranslationSessionPlugin {
    fn translation_session_binding(&self) -> Option<PluginSessionBinding>;
}
