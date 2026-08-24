//! Pure session state for the native XRTranslate runtime.
//!
//! This crate deliberately owns no sockets, audio devices, model runtimes, or
//! tasks.  Runtime adapters submit [`ProtocolEvent`] values and one owner
//! drains [`OutboundEvent`] values in FIFO order.  Keeping that boundary here
//! makes route changes deterministic and lets stale inference results be
//! rejected before they reach OSC, WebSocket, or TTS adapters.

use std::collections::VecDeque;
use std::error::Error;
use std::fmt;

pub mod language;
pub mod text_processing;

pub use language::{
    Script, auto_route_language_pair, detect_text_language, has_substantial_script_evidence,
    observed_scripts,
};
pub use text_processing::{
    RevisableTranscript, TranslationSegmentPair, collapse_asr_split_words,
    ends_at_sentence_boundary, is_filler_segment, is_split_word_pair, remove_asr_stutters,
    remove_transcript_overlap, split_translation_segments, strip_filler_edges,
    strip_filler_edges_for_lang, translation_segment_pairs_for_final_text,
    translation_segment_pairs_for_final_text_with_lang, translation_segment_pairs_for_live_text,
    translation_segment_pairs_for_live_text_with_lang, translation_segment_pairs_for_text,
    translation_segment_pairs_for_text_with_lang, translation_segments_for_text,
    translation_segments_for_text_with_lang,
};

/// Monotonically increasing identity of a source/target language route.
///
/// Any result created for an earlier epoch is stale as soon as the route is
/// changed.  Adapters should capture this value when they start work and put
/// it back on the matching [`ProtocolEvent`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RouteEpoch(u64);

impl RouteEpoch {
    /// Initial route epoch used by a new session.
    pub const INITIAL: Self = Self(0);

    /// Numeric representation for logging or protocol serialization.
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// Monotonically increasing identity of the TTS playback configuration.
///
/// It is separate from [`RouteEpoch`] so disabling TTS immediately invalidates
/// audio which was already being synthesized without changing the text route.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TtsEpoch(u64);

impl TtsEpoch {
    /// Initial TTS epoch used by a new session.
    pub const INITIAL: Self = Self(0);

    /// Numeric representation for logging or protocol serialization.
    pub const fn get(self) -> u64 {
        self.0
    }

    fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// Language identifier kept deliberately open for future locale/model support.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Language(String);

impl Language {
    /// Creates a language identifier from a BCP-47-style tag, such as `en` or
    /// `zh-CN`.
    pub fn new(tag: impl Into<String>) -> Result<Self, LanguageError> {
        let tag = tag.into();
        if tag.trim().is_empty() {
            return Err(LanguageError::Empty);
        }
        if tag != tag.trim() {
            return Err(LanguageError::SurroundingWhitespace);
        }
        Ok(Self(tag))
    }

    /// Borrows the language identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Language {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A language route currently active for the session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LanguageRoute {
    /// Language detected or supplied by ASR.
    pub source: Language,
    /// Translation destination language.
    pub target: Language,
}

impl LanguageRoute {
    /// Creates a route from source to target.
    pub const fn new(source: Language, target: Language) -> Self {
        Self { source, target }
    }
}

/// Configuration with explicit resource limits for a [`SessionEngine`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineConfig {
    /// Maximum completed translation turns retained as model context.
    pub context_capacity: usize,
    /// Maximum messages awaiting the single outbound writer.
    pub outbound_capacity: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            context_capacity: 32,
            outbound_capacity: 128,
        }
    }
}

/// A fixed-capacity FIFO queue that never silently drops an item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundedQueue<T> {
    capacity: usize,
    entries: VecDeque<T>,
}

impl<T> BoundedQueue<T> {
    /// Makes an empty queue with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::with_capacity(capacity),
        }
    }

    /// Returns the configured maximum number of entries.
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the current number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns whether the queue has no entries.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Appends an item or reports that no capacity remains.
    pub fn push(&mut self, entry: T) -> Result<(), QueueFull> {
        if self.entries.len() == self.capacity {
            return Err(QueueFull {
                capacity: self.capacity,
            });
        }
        self.entries.push_back(entry);
        Ok(())
    }

    /// Removes and returns the oldest entry.
    pub fn pop(&mut self) -> Option<T> {
        self.entries.pop_front()
    }

    /// Removes every queued entry.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Retains only entries selected by `keep`.
    pub fn retain(&mut self, mut keep: impl FnMut(&T) -> bool) {
        self.entries.retain(|entry| keep(entry));
    }
}

/// Error returned when a [`BoundedQueue`] has reached capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueFull {
    /// The configured maximum number of entries.
    pub capacity: usize,
}

impl fmt::Display for QueueFull {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "bounded queue is full (capacity: {})",
            self.capacity
        )
    }
}

impl Error for QueueFull {}

/// A completed source/translation pair retained as translator context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranslationTurn {
    /// Text recognized from the speaker.
    pub source_text: String,
    /// Final translation sent to listeners.
    pub translated_text: String,
}

impl TranslationTurn {
    /// Creates one completed translation turn.
    pub fn new(source_text: impl Into<String>, translated_text: impl Into<String>) -> Self {
        Self {
            source_text: source_text.into(),
            translated_text: translated_text.into(),
        }
    }
}

/// Bounded history passed to a translation implementation as conversational context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TranslationContext {
    turns: VecDeque<TranslationTurn>,
    capacity: usize,
}

impl TranslationContext {
    /// Makes an empty context with a bounded turn count.
    pub fn new(capacity: usize) -> Self {
        Self {
            turns: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Number of retained turns.
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Whether no completed turn is retained.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Iterates from the oldest to newest retained turn.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &TranslationTurn> {
        self.turns.iter()
    }

    /// Clears every retained turn, for example after a language-route change.
    pub fn clear(&mut self) {
        self.turns.clear();
    }

    fn push(&mut self, turn: TranslationTurn) {
        if self.capacity == 0 {
            return;
        }
        if self.turns.len() == self.capacity {
            let _ = self.turns.pop_front();
        }
        self.turns.push_back(turn);
    }
}

/// Event supplied by a local inference or input adapter.
///
/// This is intentionally local to the engine crate during the migration.  Once
/// `xrtranslate-protocol` is stable, its wire representation can convert into
/// this type at the transport boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolEvent {
    /// A recognized source-text update.
    RecognizedText {
        /// Route active when ASR work began.
        route_epoch: RouteEpoch,
        /// Recognized source text.
        text: String,
        /// Whether the segment is complete.
        is_final: bool,
    },
    /// A translated text update.
    TranslatedText {
        /// Route active when translation work began.
        route_epoch: RouteEpoch,
        /// Source segment used for this translation.
        source_text: String,
        /// Translated output.
        translated_text: String,
        /// Whether the segment is complete and suitable for context retention.
        is_final: bool,
    },
    /// PCM audio produced by a TTS adapter.
    TtsAudio {
        /// Route active when synthesis work began.
        route_epoch: RouteEpoch,
        /// TTS configuration active when synthesis work began.
        tts_epoch: TtsEpoch,
        /// PCM payload owned by the event.
        pcm: Vec<u8>,
    },
}

impl ProtocolEvent {
    /// Epoch attached to this event by its producing adapter.
    pub const fn route_epoch(&self) -> RouteEpoch {
        match self {
            Self::RecognizedText { route_epoch, .. }
            | Self::TranslatedText { route_epoch, .. }
            | Self::TtsAudio { route_epoch, .. } => *route_epoch,
        }
    }
}

/// Sequenced event that the session's sole outbound adapter must write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundEvent {
    /// Strictly increasing sequence assigned by one [`SessionEngine`].
    pub sequence: u64,
    /// Epoch which was current when this event was accepted.
    pub route_epoch: RouteEpoch,
    /// Content that may be written by an OSC, WebSocket, or audio adapter.
    pub payload: OutboundPayload,
}

/// Payload available to the single outbound writer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboundPayload {
    /// A recognized source-text update.
    RecognizedText { text: String, is_final: bool },
    /// A translated-text update.
    TranslatedText {
        source_text: String,
        translated_text: String,
        is_final: bool,
    },
    /// TTS PCM audio.
    TtsAudio { pcm: Vec<u8> },
}

#[derive(Debug)]
struct OutboundWriter {
    next_sequence: u64,
    events: BoundedQueue<OutboundEvent>,
}

impl OutboundWriter {
    fn new(capacity: usize) -> Self {
        Self {
            next_sequence: 0,
            events: BoundedQueue::new(capacity),
        }
    }

    fn enqueue(
        &mut self,
        route_epoch: RouteEpoch,
        payload: OutboundPayload,
    ) -> Result<(), QueueFull> {
        let event = OutboundEvent {
            sequence: self.next_sequence,
            route_epoch,
            payload,
        };
        self.events.push(event)?;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        Ok(())
    }

    fn drain(&mut self) -> impl Iterator<Item = OutboundEvent> + '_ {
        std::iter::from_fn(|| self.events.pop())
    }

    fn discard_audio(&mut self) {
        self.events
            .retain(|event| !matches!(event.payload, OutboundPayload::TtsAudio { .. }));
    }
}

/// All rejected events leave session state unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EventRejected {
    /// The event was started on a no-longer-current route.
    StaleRoute {
        /// Epoch provided by the event.
        event_epoch: RouteEpoch,
        /// Route epoch required by the session.
        current_epoch: RouteEpoch,
    },
    /// TTS was disabled, so audio cannot be written.
    TtsDisabled,
    /// Audio was produced for a prior TTS configuration.
    StaleTts {
        /// Epoch provided by the event.
        event_epoch: TtsEpoch,
        /// TTS epoch required by the session.
        current_epoch: TtsEpoch,
    },
    /// The single outbound writer cannot accept more work yet.
    OutboundQueueFull(QueueFull),
}

impl fmt::Display for EventRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleRoute {
                event_epoch,
                current_epoch,
            } => write!(
                formatter,
                "event route epoch {} is stale; current epoch is {}",
                event_epoch.get(),
                current_epoch.get()
            ),
            Self::TtsDisabled => formatter.write_str("TTS is disabled"),
            Self::StaleTts {
                event_epoch,
                current_epoch,
            } => write!(
                formatter,
                "audio TTS epoch {} is stale; current epoch is {}",
                event_epoch.get(),
                current_epoch.get()
            ),
            Self::OutboundQueueFull(error) => error.fmt(formatter),
        }
    }
}

impl Error for EventRejected {}

/// Error returned when constructing a [`Language`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LanguageError {
    /// A language identifier cannot be empty.
    Empty,
    /// Whitespace is ambiguous and therefore forbidden around a language tag.
    SurroundingWhitespace,
}

impl fmt::Display for LanguageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("language tag cannot be empty"),
            Self::SurroundingWhitespace => {
                formatter.write_str("language tag cannot have surrounding whitespace")
            }
        }
    }
}

impl Error for LanguageError {}

/// No-I/O session state and the sole producer of outbound events.
///
/// `&mut self` is required for both [`Self::submit`] and
/// [`Self::drain_outbound`], giving the runtime one serialized point where
/// events are accepted and written.  A transport may have many inference
/// workers, but only its session owner should call these methods.
#[derive(Debug)]
pub struct SessionEngine {
    route: LanguageRoute,
    route_epoch: RouteEpoch,
    tts_enabled: bool,
    tts_epoch: TtsEpoch,
    context: TranslationContext,
    outbound: OutboundWriter,
}

impl SessionEngine {
    /// Creates one session with the supplied route and explicit limits.
    pub fn new(route: LanguageRoute, config: EngineConfig) -> Self {
        Self {
            route,
            route_epoch: RouteEpoch::INITIAL,
            tts_enabled: false,
            tts_epoch: TtsEpoch::INITIAL,
            context: TranslationContext::new(config.context_capacity),
            outbound: OutboundWriter::new(config.outbound_capacity),
        }
    }

    /// Current source/target language route.
    pub fn route(&self) -> &LanguageRoute {
        &self.route
    }

    /// Current route epoch to attach when new ASR/MT/TTS work begins.
    pub const fn route_epoch(&self) -> RouteEpoch {
        self.route_epoch
    }

    /// Current TTS epoch to attach when new synthesis begins.
    pub const fn tts_epoch(&self) -> TtsEpoch {
        self.tts_epoch
    }

    /// Whether synthesized audio may currently be emitted.
    pub const fn tts_enabled(&self) -> bool {
        self.tts_enabled
    }

    /// Completed translations retained for a subsequent MT request.
    pub fn context(&self) -> &TranslationContext {
        &self.context
    }

    /// Replaces the route, invalidating in-flight work and clearing context.
    ///
    /// Reapplying an identical route is a no-op so callers may synchronize
    /// settings safely without invalidating active work.
    pub fn set_route(&mut self, route: LanguageRoute) -> RouteEpoch {
        if self.route != route {
            self.route = route;
            self.route_epoch = self.route_epoch.next();
            self.context.clear();
        }
        self.route_epoch
    }

    /// Enables or disables TTS.
    ///
    /// A state change increments [`TtsEpoch`].  Disabling also removes audio
    /// that had been accepted but has not yet reached the outbound writer.
    pub fn set_tts_enabled(&mut self, enabled: bool) -> TtsEpoch {
        if self.tts_enabled != enabled {
            self.tts_enabled = enabled;
            self.tts_epoch = self.tts_epoch.next();
            if !enabled {
                self.outbound.discard_audio();
            }
        }
        self.tts_epoch
    }

    /// Validates one inference/input result and queues it for the only
    /// outbound writer.
    ///
    /// On failure, the event is rejected without altering the context or
    /// sequence.  A full outbound queue applies backpressure rather than
    /// silently discarding a visible translation.
    pub fn submit(&mut self, event: ProtocolEvent) -> Result<(), EventRejected> {
        if event.route_epoch() != self.route_epoch {
            return Err(EventRejected::StaleRoute {
                event_epoch: event.route_epoch(),
                current_epoch: self.route_epoch,
            });
        }

        match event {
            ProtocolEvent::RecognizedText { text, is_final, .. } => self
                .outbound
                .enqueue(
                    self.route_epoch,
                    OutboundPayload::RecognizedText { text, is_final },
                )
                .map_err(EventRejected::OutboundQueueFull),
            ProtocolEvent::TranslatedText {
                source_text,
                translated_text,
                is_final,
                ..
            } => {
                let payload = OutboundPayload::TranslatedText {
                    source_text: source_text.clone(),
                    translated_text: translated_text.clone(),
                    is_final,
                };
                self.outbound
                    .enqueue(self.route_epoch, payload)
                    .map_err(EventRejected::OutboundQueueFull)?;
                if is_final {
                    self.context
                        .push(TranslationTurn::new(source_text, translated_text));
                }
                Ok(())
            }
            ProtocolEvent::TtsAudio { tts_epoch, pcm, .. } => {
                if tts_epoch != self.tts_epoch {
                    return Err(EventRejected::StaleTts {
                        event_epoch: tts_epoch,
                        current_epoch: self.tts_epoch,
                    });
                }
                if !self.tts_enabled {
                    return Err(EventRejected::TtsDisabled);
                }
                self.outbound
                    .enqueue(self.route_epoch, OutboundPayload::TtsAudio { pcm })
                    .map_err(EventRejected::OutboundQueueFull)
            }
        }
    }

    /// Drains queued output in strict sequence order for the session's one
    /// outbound adapter.
    pub fn drain_outbound(&mut self) -> impl Iterator<Item = OutboundEvent> + '_ {
        self.outbound.drain()
    }

    /// Number of outbound events waiting for that adapter.
    pub fn pending_outbound(&self) -> usize {
        self.outbound.events.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn language(tag: &str) -> Language {
        Language::new(tag).expect("test language tag must be valid")
    }

    fn engine() -> SessionEngine {
        SessionEngine::new(
            LanguageRoute::new(language("ja"), language("en")),
            EngineConfig {
                context_capacity: 2,
                outbound_capacity: 4,
            },
        )
    }

    #[test]
    fn language_switch_rejects_event_from_old_route_epoch() {
        let mut engine = engine();
        let old_epoch = engine.route_epoch();

        let current_epoch = engine.set_route(LanguageRoute::new(language("ja"), language("zh-CN")));

        assert_ne!(old_epoch, current_epoch);
        assert_eq!(
            engine.submit(ProtocolEvent::RecognizedText {
                route_epoch: old_epoch,
                text: "old result".into(),
                is_final: true,
            }),
            Err(EventRejected::StaleRoute {
                event_epoch: old_epoch,
                current_epoch,
            })
        );
        assert_eq!(engine.pending_outbound(), 0);
    }

    #[test]
    fn language_switch_clears_translation_context() {
        let mut engine = engine();
        let epoch = engine.route_epoch();
        engine
            .submit(ProtocolEvent::TranslatedText {
                route_epoch: epoch,
                source_text: "こんにちは".into(),
                translated_text: "Hello".into(),
                is_final: true,
            })
            .expect("current route event must be accepted");
        assert_eq!(engine.context().len(), 1);

        engine.set_route(LanguageRoute::new(language("ja"), language("zh-CN")));

        assert!(engine.context().is_empty());
    }

    #[test]
    fn disabling_tts_rejects_audio_created_before_disable() {
        let mut engine = engine();
        engine.set_tts_enabled(true);
        let route_epoch = engine.route_epoch();
        let old_tts_epoch = engine.tts_epoch();

        let current_tts_epoch = engine.set_tts_enabled(false);

        assert_ne!(old_tts_epoch, current_tts_epoch);
        assert_eq!(
            engine.submit(ProtocolEvent::TtsAudio {
                route_epoch,
                tts_epoch: old_tts_epoch,
                pcm: vec![0, 1, 2],
            }),
            Err(EventRejected::StaleTts {
                event_epoch: old_tts_epoch,
                current_epoch: current_tts_epoch,
            })
        );
        assert_eq!(engine.pending_outbound(), 0);
    }

    #[test]
    fn completed_translations_use_bounded_context_and_outbound_is_fifo() {
        let mut engine = engine();
        let epoch = engine.route_epoch();
        for index in 0..3 {
            engine
                .submit(ProtocolEvent::TranslatedText {
                    route_epoch: epoch,
                    source_text: format!("source-{index}"),
                    translated_text: format!("translated-{index}"),
                    is_final: true,
                })
                .expect("queue has room");
        }

        let context = engine.context().iter().collect::<Vec<_>>();
        assert_eq!(context.len(), 2);
        assert_eq!(context[0].source_text, "source-1");
        assert_eq!(context[1].source_text, "source-2");

        let events = engine.drain_outbound().collect::<Vec<_>>();
        assert_eq!(
            events
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn bounded_queue_applies_backpressure_without_advancing_context() {
        let mut engine = SessionEngine::new(
            LanguageRoute::new(language("ja"), language("en")),
            EngineConfig {
                context_capacity: 2,
                outbound_capacity: 1,
            },
        );
        let epoch = engine.route_epoch();
        engine
            .submit(ProtocolEvent::TranslatedText {
                route_epoch: epoch,
                source_text: "first".into(),
                translated_text: "one".into(),
                is_final: true,
            })
            .expect("first event fits");

        assert_eq!(
            engine.submit(ProtocolEvent::TranslatedText {
                route_epoch: epoch,
                source_text: "second".into(),
                translated_text: "two".into(),
                is_final: true,
            }),
            Err(EventRejected::OutboundQueueFull(QueueFull { capacity: 1 }))
        );
        assert_eq!(engine.context().len(), 1);
    }
}
