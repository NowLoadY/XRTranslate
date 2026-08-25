use super::graph::{
    AudioGraph, AudioLink, AudioNode, AudioNodeKind, AudioProcessor, GraphPosition,
    SystemAudioCapture, SystemCapturePolicy,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioStudioPreset {
    CompleteAudioSystem,
    TranslationSafe,
    VrchatKaraoke,
    TtsToGameMicrophone,
}

impl AudioStudioPreset {
    pub const ALL: [Self; 4] = [
        Self::CompleteAudioSystem,
        Self::TranslationSafe,
        Self::VrchatKaraoke,
        Self::TtsToGameMicrophone,
    ];

    pub const fn stable_id(self) -> &'static str {
        match self {
            Self::CompleteAudioSystem => "complete-audio-system",
            Self::TranslationSafe => "translation-safe",
            Self::VrchatKaraoke => "vrchat-karaoke",
            Self::TtsToGameMicrophone => "tts-to-game-microphone",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::CompleteAudioSystem => "Complete audio system",
            Self::TranslationSafe => "Translation-safe system audio",
            Self::VrchatKaraoke => "Karaoke / shared microphone",
            Self::TtsToGameMicrophone => "TTS to app microphone",
        }
    }
}

pub fn graph_for_preset(preset: AudioStudioPreset) -> AudioGraph {
    match preset {
        AudioStudioPreset::CompleteAudioSystem => complete_audio_system(),
        AudioStudioPreset::TranslationSafe => translation_safe(),
        AudioStudioPreset::VrchatKaraoke => vrchat_karaoke(),
        AudioStudioPreset::TtsToGameMicrophone => tts_to_game_microphone(),
    }
}

fn complete_audio_system() -> AudioGraph {
    let mut graph = AudioGraph::new(
        AudioStudioPreset::CompleteAudioSystem.stable_id(),
        AudioStudioPreset::CompleteAudioSystem.display_name(),
    );
    graph.nodes = vec![
        node(
            "recognition-system-audio",
            "Recognition system audio",
            40.0,
            40.0,
            AudioNodeKind::SystemAudio {
                capture: SystemAudioCapture::Endpoint {
                    device_id: None,
                    capture_policy: SystemCapturePolicy::SuppressDuringOwnTts,
                },
            },
        ),
        gain_node("gain-rec-sys", "Sys Gain", 340.0, 55.0),
        node(
            "asr-input-mixer",
            "Recognition inputs",
            480.0,
            120.0,
            AudioNodeKind::Mixer,
        ),
        node("asr", "ASR", 820.0, 120.0, AudioNodeKind::AsrTap),
        node(
            "microphone",
            "Microphone",
            40.0,
            290.0,
            AudioNodeKind::Microphone { device_id: None },
        ),
        gain_node("gain-mic-asr", "Mic Gain", 340.0, 190.0),
        gain_node("gain-mic-game", "Mic Gain", 340.0, 310.0),
        node(
            "bgm",
            "BGM / application audio",
            40.0,
            520.0,
            AudioNodeKind::SystemAudio {
                capture: SystemAudioCapture::Application {
                    application: None,
                    resolved_process_id: None,
                },
            },
        ),
        gain_node("gain-bgm-game", "BGM Gain", 340.0, 530.0),
        node("tts", "TTS", 40.0, 790.0, AudioNodeKind::TextToSpeech),
        gain_node("gain-tts-game", "TTS Gain", 340.0, 710.0),
        node(
            "game-mixer",
            "Voice + BGM + TTS",
            480.0,
            360.0,
            AudioNodeKind::Mixer,
        ),
        node(
            "game-limiter",
            "Virtual microphone limiter",
            872.0,
            360.0,
            AudioNodeKind::Processing {
                processor: AudioProcessor::Limiter { ceiling_db: -1.0 },
            },
        ),
        node(
            "game-microphone",
            "App microphone output",
            1264.0,
            330.0,
            AudioNodeKind::GameMicrophoneOutput {
                device_id: None,
                voicemeeter_bus: None,
            },
        ),
        node(
            "tts-monitor",
            "TTS monitor output",
            480.0,
            790.0,
            AudioNodeKind::MonitorOutput { device_id: None },
        ),
    ];
    graph.links = vec![
        AudioLink::new("recognition-to-gain", "recognition-system-audio", "gain-rec-sys"),
        AudioLink::to_mixer_input(
            "gain-rec-sys-to-asr-mixer",
            "gain-rec-sys",
            "asr-input-mixer",
            0,
        ),
        AudioLink::new("mic-to-gain-asr", "microphone", "gain-mic-asr"),
        AudioLink::to_mixer_input(
            "gain-mic-asr-to-asr-mixer",
            "gain-mic-asr",
            "asr-input-mixer",
            1,
        ),
        AudioLink::new_with_enabled("asr-mixer-to-asr", "asr-input-mixer", "asr", false),
        AudioLink::new("mic-to-gain-game", "microphone", "gain-mic-game"),
        AudioLink::to_mixer_input(
            "gain-mic-game-to-game-mixer",
            "gain-mic-game",
            "game-mixer",
            0,
        ),
        AudioLink::new("bgm-to-gain", "bgm", "gain-bgm-game"),
        AudioLink::to_mixer_input(
            "gain-bgm-to-game-mixer",
            "gain-bgm-game",
            "game-mixer",
            1,
        ),
        AudioLink::new("tts-to-gain", "tts", "gain-tts-game"),
        AudioLink::to_mixer_input(
            "gain-tts-to-game-mixer",
            "gain-tts-game",
            "game-mixer",
            2,
        ),
        AudioLink::new("game-mixer-to-limiter", "game-mixer", "game-limiter"),
        AudioLink::new(
            "limiter-to-game-microphone",
            "game-limiter",
            "game-microphone",
        ),
        AudioLink::new("tts-to-monitor", "tts", "tts-monitor"),
    ];
    graph
}

fn node(id: &str, label: &str, x: f32, y: f32, kind: AudioNodeKind) -> AudioNode {
    AudioNode {
        position: GraphPosition { x, y },
        ..AudioNode::new(id, label, kind)
    }
}

fn gain_node(id: &str, label: &str, x: f32, y: f32) -> AudioNode {
    node(
        id,
        label,
        x,
        y,
        AudioNodeKind::Processing {
            processor: AudioProcessor::Gain { gain_db: 0.0 },
        },
    )
}

fn translation_safe() -> AudioGraph {
    let mut graph = AudioGraph::new(
        AudioStudioPreset::TranslationSafe.stable_id(),
        AudioStudioPreset::TranslationSafe.display_name(),
    );
    graph.nodes = vec![
        node(
            "system-audio",
            "System audio",
            40.0,
            60.0,
            AudioNodeKind::SystemAudio {
                capture: SystemAudioCapture::Endpoint {
                    device_id: None,
                    capture_policy: SystemCapturePolicy::SuppressDuringOwnTts,
                },
            },
        ),
        gain_node("gain-system-asr", "Sys Gain", 340.0, 75.0),
        node(
            "asr-input-mixer",
            "Recognition inputs",
            480.0,
            60.0,
            AudioNodeKind::Mixer,
        ),
        node("asr", "ASR", 820.0, 60.0, AudioNodeKind::AsrTap),
        node("tts", "TTS", 40.0, 330.0, AudioNodeKind::TextToSpeech),
        node(
            "monitor",
            "Monitor output",
            480.0,
            330.0,
            AudioNodeKind::MonitorOutput { device_id: None },
        ),
    ];
    graph.links = vec![
        AudioLink::new("system-to-gain", "system-audio", "gain-system-asr"),
        AudioLink::to_mixer_input(
            "gain-system-to-asr-mixer",
            "gain-system-asr",
            "asr-input-mixer",
            0,
        ),
        AudioLink::new_with_enabled("asr-mixer-to-asr", "asr-input-mixer", "asr", false),
        AudioLink::new("tts-to-monitor", "tts", "monitor"),
    ];
    graph
}

fn vrchat_karaoke() -> AudioGraph {
    let mut graph = AudioGraph::new(
        AudioStudioPreset::VrchatKaraoke.stable_id(),
        AudioStudioPreset::VrchatKaraoke.display_name(),
    );
    graph.nodes = vec![
        node(
            "microphone",
            "Microphone",
            40.0,
            40.0,
            AudioNodeKind::Microphone { device_id: None },
        ),
        gain_node("gain-mic", "Mic Gain", 340.0, 55.0),
        node(
            "bgm",
            "BGM / system audio",
            40.0,
            270.0,
            AudioNodeKind::SystemAudio {
                capture: SystemAudioCapture::Endpoint {
                    device_id: None,
                    capture_policy: SystemCapturePolicy::AllEndpointAudio,
                },
            },
        ),
        gain_node("gain-bgm", "BGM Gain", 340.0, 285.0),
        node("mixer", "Voice + BGM", 480.0, 90.0, AudioNodeKind::Mixer),
        node(
            "limiter",
            "Output limiter",
            872.0,
            90.0,
            AudioNodeKind::Processing {
                processor: AudioProcessor::Limiter { ceiling_db: -1.0 },
            },
        ),
        node(
            "game-microphone",
            "App microphone output",
            1264.0,
            60.0,
            AudioNodeKind::GameMicrophoneOutput {
                device_id: None,
                voicemeeter_bus: None,
            },
        ),
    ];
    graph.links = vec![
        AudioLink::new("mic-to-gain", "microphone", "gain-mic"),
        AudioLink::to_mixer_input("gain-mic-to-mixer", "gain-mic", "mixer", 0),
        AudioLink::new("bgm-to-gain", "bgm", "gain-bgm"),
        AudioLink::to_mixer_input("gain-bgm-to-mixer", "gain-bgm", "mixer", 1),
        AudioLink::new("mixer-to-limiter", "mixer", "limiter"),
        AudioLink::new("limiter-to-game", "limiter", "game-microphone"),
    ];
    graph
}

fn tts_to_game_microphone() -> AudioGraph {
    let mut graph = AudioGraph::new(
        AudioStudioPreset::TtsToGameMicrophone.stable_id(),
        AudioStudioPreset::TtsToGameMicrophone.display_name(),
    );
    graph.nodes = vec![
        node(
            "microphone",
            "Microphone",
            40.0,
            40.0,
            AudioNodeKind::Microphone { device_id: None },
        ),
        gain_node("gain-mic-mixer", "Mic Gain", 340.0, 55.0),
        gain_node("gain-mic-asr", "Mic Gain", 340.0, 190.0),
        node("tts", "TTS", 40.0, 270.0, AudioNodeKind::TextToSpeech),
        gain_node("gain-tts-mixer", "TTS Gain", 340.0, 285.0),
        node("mixer", "Voice + TTS", 480.0, 90.0, AudioNodeKind::Mixer),
        node(
            "asr-input-mixer",
            "Recognition inputs",
            480.0,
            300.0,
            AudioNodeKind::Mixer,
        ),
        node("asr", "ASR", 820.0, 300.0, AudioNodeKind::AsrTap),
        node(
            "limiter",
            "Output limiter",
            872.0,
            90.0,
            AudioNodeKind::Processing {
                processor: AudioProcessor::Limiter { ceiling_db: -1.0 },
            },
        ),
        node(
            "game-microphone",
            "App microphone output",
            1264.0,
            60.0,
            AudioNodeKind::GameMicrophoneOutput {
                device_id: None,
                voicemeeter_bus: None,
            },
        ),
    ];
    graph.links = vec![
        AudioLink::new("mic-to-gain-mixer", "microphone", "gain-mic-mixer"),
        AudioLink::to_mixer_input("gain-mic-to-mixer", "gain-mic-mixer", "mixer", 0),
        AudioLink::new("mic-to-gain-asr", "microphone", "gain-mic-asr"),
        AudioLink::to_mixer_input(
            "gain-mic-to-asr-mixer",
            "gain-mic-asr",
            "asr-input-mixer",
            0,
        ),
        AudioLink::new_with_enabled("asr-mixer-to-asr", "asr-input-mixer", "asr", false),
        AudioLink::new("tts-to-gain-mixer", "tts", "gain-tts-mixer"),
        AudioLink::to_mixer_input("gain-tts-to-mixer", "gain-tts-mixer", "mixer", 1),
        AudioLink::new("mixer-to-limiter", "mixer", "limiter"),
        AudioLink::new("limiter-to-game", "limiter", "game-microphone"),
    ];
    graph
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_preset_is_valid() {
        for preset in AudioStudioPreset::ALL {
            let graph = graph_for_preset(preset);
            assert!(
                graph.validate().is_valid(),
                "{preset:?}: {:?}",
                graph.validate().issues
            );
            assert_eq!(
                graph
                    .nodes
                    .iter()
                    .filter(|node| matches!(node.kind, AudioNodeKind::AsrTap))
                    .count(),
                usize::from(preset != AudioStudioPreset::VrchatKaraoke),
                "{preset:?}"
            );
        }
    }

    #[test]
    fn complete_default_exposes_recognition_game_mix_and_tts_monitor_branches() {
        let graph = graph_for_preset(AudioStudioPreset::CompleteAudioSystem);

        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "recognition-system-audio" && link.to.node_id.0 == "gain-rec-sys"
        }));
        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "gain-rec-sys" && link.to.node_id.0 == "asr-input-mixer"
        }));
        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "microphone" && link.to.node_id.0 == "gain-mic-asr"
        }));
        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "gain-mic-asr" && link.to.node_id.0 == "asr-input-mixer"
        }));
        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "asr-input-mixer" && link.to.node_id.0 == "asr"
        }));
        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "game-limiter" && link.to.node_id.0 == "game-microphone"
        }));
        assert!(
            graph
                .links
                .iter()
                .any(|link| { link.from.node_id.0 == "tts" && link.to.node_id.0 == "tts-monitor" })
        );
        assert!(graph.nodes.iter().any(|node| {
            node.id.0 == "bgm"
                && matches!(
                    node.kind,
                    AudioNodeKind::SystemAudio {
                        capture: SystemAudioCapture::Application {
                            application: None,
                            ..
                        }
                    }
                )
        }));
    }

    #[test]
    fn translation_safe_never_routes_tts_into_asr() {
        let graph = graph_for_preset(AudioStudioPreset::TranslationSafe);
        assert!(
            !graph
                .links
                .iter()
                .any(|link| { link.from.node_id.0 == "tts" && link.to.node_id.0 == "asr" })
        );
        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "system-audio" && link.to.node_id.0 == "gain-system-asr"
        }));
        assert!(graph.nodes.iter().any(|node| {
            matches!(
                node.kind,
                AudioNodeKind::SystemAudio {
                    capture: SystemAudioCapture::Endpoint {
                        capture_policy: SystemCapturePolicy::SuppressDuringOwnTts,
                        ..
                    }
                }
            )
        }));
    }

    #[test]
    fn karaoke_routes_system_bgm_into_the_game_microphone_path() {
        let graph = graph_for_preset(AudioStudioPreset::VrchatKaraoke);
        assert!(
            graph
                .links
                .iter()
                .any(|link| link.from.node_id.0 == "bgm" && link.to.node_id.0 == "gain-bgm")
        );
        assert!(
            graph
                .links
                .iter()
                .any(|link| link.from.node_id.0 == "gain-bgm" && link.to.node_id.0 == "mixer")
        );
        assert!(graph.nodes.iter().any(|node| {
            node.id.0 == "bgm"
                && matches!(
                    node.kind,
                    AudioNodeKind::SystemAudio {
                        capture: SystemAudioCapture::Endpoint {
                            capture_policy: SystemCapturePolicy::AllEndpointAudio,
                            ..
                        }
                    }
                )
        }));
        assert_eq!(
            graph
                .nodes
                .iter()
                .filter(|node| node.kind.is_sink())
                .count(),
            1
        );
    }

    #[test]
    fn tts_conversation_preset_keeps_a_live_asr_session_for_direct_text_turns() {
        let graph = graph_for_preset(AudioStudioPreset::TtsToGameMicrophone);
        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "microphone" && link.to.node_id.0 == "gain-mic-asr"
        }));
        assert!(graph.links.iter().any(|link| {
            link.from.node_id.0 == "gain-mic-asr"
                && link.to.node_id.0 == "asr-input-mixer"
                && link.enabled
        }));
    }
}
