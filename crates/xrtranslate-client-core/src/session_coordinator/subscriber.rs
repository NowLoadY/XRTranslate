use super::{TranslationEvent, TranslationSessionOwner};
/// Read-only observer for the generic recognition/translation event stream.
///
/// Implementations must return quickly. Any storage or blocking work belongs
/// on a plugin-owned worker queue.
pub trait SessionEventSubscriber: Send + Sync {
    fn on_translation_event(&self, owner: &TranslationSessionOwner, event: &TranslationEvent);

    /// Domain consumers opt into events from their own task identity.
    fn accepts_owner(&self, _owner: &super::TranslationSessionOwner) -> bool {
        true
    }
}
