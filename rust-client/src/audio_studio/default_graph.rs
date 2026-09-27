use super::graph::{
    AudioGraph, AudioLink, AudioNode, AudioNodeKind, AudioProcessor, GraphPosition,
    SystemAudioCapture, SystemCapturePolicy,
};

pub const DEFAULT_AUDIO_GRAPH_ID: &str = "audio-system";

/// The only built-in route: recognition, microphone, application audio, TTS,
/// app microphone output and monitoring, with source gates initially closed.
pub fn default_graph() -> AudioGraph {
    let mut graph = AudioGraph::new(DEFAULT_AUDIO_GRAPH_ID, "Default");
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
                follow_tts: false,
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
        AudioLink::new(
            "recognition-to-gain",
            "recognition-system-audio",
            "gain-rec-sys",
        ),
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
        AudioLink::to_mixer_input("gain-bgm-to-game-mixer", "gain-bgm-game", "game-mixer", 1),
        AudioLink::new("tts-to-gain", "tts", "gain-tts-game"),
        AudioLink::to_mixer_input("gain-tts-to-game-mixer", "gain-tts-game", "game-mixer", 2),
        AudioLink::new("game-mixer-to-limiter", "game-mixer", "game-limiter"),
        AudioLink::new(
            "limiter-to-game-microphone",
            "game-limiter",
            "game-microphone",
        ),
        AudioLink::new("tts-to-monitor", "tts", "tts-monitor"),
    ];
    graph.initialize_source_gates();
    graph
}

pub(super) fn node(id: &str, label: &str, x: f32, y: f32, kind: AudioNodeKind) -> AudioNode {
    AudioNode {
        position: GraphPosition { x, y },
        ..AudioNode::new(id, label, kind)
    }
}

pub(super) fn gain_node(id: &str, label: &str, x: f32, y: f32) -> AudioNode {
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
