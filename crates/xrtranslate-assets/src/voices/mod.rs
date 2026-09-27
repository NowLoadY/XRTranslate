//! Curated reference recordings, independent of any provider's clone format.
//! Only these explicit resources are embedded and exported into release resources.

mod library;
pub use library::{MAX_REFERENCE_SECONDS, VoiceCard, VoiceCatalog};

pub struct VoiceReference {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub language: &'static str,
    /// Canonical 44-byte RIFF/WAV header followed by mono PCM16 at 16 kHz.
    pub wav: &'static [u8],
    pub transcript: &'static str,
    pub source_url: &'static str,
    pub credit: &'static str,
    pub source_notice: &'static str,
    pub license: &'static str,
}

pub const SAMPLE_RATE: u32 = 16_000;

impl VoiceReference {
    /// The curated WAV files contain only the fixed PCM format and data chunks.
    /// Borrow their samples for preview without decoding or keeping a second copy.
    pub fn pcm16(&self) -> &'static [u8] {
        &self.wav[44..]
    }
}

pub static BUILTIN_VOICES: &[VoiceReference] = &[
    VoiceReference {
        id: "miku",
        name: "Miku",
        description: "Clear, bright Miku timbre. Singing reference.",
        language: "ja",
        wav: include_bytes!("../../resources/voices/miku/reference.wav"),
        transcript: include_str!("../../resources/voices/miku/reference.txt"),
        source_url: "https://commons.wikimedia.org/w/index.php?title=File:Vocaloid_example_%22Kimigayo%22_-_MIKU_V4X_Original_EVEC_01.flac&oldid=1228613916",
        credit: "Rose Abrams · CC BY 4.0",
        source_notice: include_str!("../../resources/voices/miku/SOURCE.md"),
        license: include_str!("../../resources/voices/miku/LICENSE"),
    },
    VoiceReference {
        credit: "OpenVoice · MyShell · MIT",
        source_notice: include_str!("../../resources/voices/cute/SOURCE.md"),
        license: include_str!("../../resources/voices/cute/LICENSE"),
        id: "cute",
        name: "Cute",
        description: "Bright, playful and a little mischievous.",
        language: "en",
        wav: include_bytes!("../../resources/voices/cute/reference.wav"),
        transcript: include_str!("../../resources/voices/cute/reference.txt"),
        source_url: "https://github.com/myshell-ai/OpenVoice/blob/74a1d147b17a8c3092dd5430504bd83ef6c7eb23/resources/demo_speaker1.mp3",
    },
];

pub fn builtin(id: &str) -> Option<&'static VoiceReference> {
    BUILTIN_VOICES.iter().find(|voice| voice.id == id)
}
