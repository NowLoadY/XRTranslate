use std::sync::Mutex;

use xrtranslate_prompt::{PromptExecutionTrace, PromptTarget};

use crate::session_coordinator::TranslationSessionOwner;

type RuntimeTrace = (TranslationSessionOwner, PromptExecutionTrace);

// Desktop tasks and Android background actions use the same current request.
// Traces are transient inspection data, never a source of translation context.
static TRACES: Mutex<(Option<RuntimeTrace>, Option<RuntimeTrace>)> = Mutex::new((None, None));

pub(crate) fn record_runtime_trace(owner: &TranslationSessionOwner, trace: &PromptExecutionTrace) {
    let mut traces = TRACES.lock().unwrap_or_else(|error| error.into_inner());
    let slot = match trace.target {
        PromptTarget::AsrInstruction | PromptTarget::AsrContextBias => &mut traces.0,
        PromptTarget::TranslationInstruction | PromptTarget::ChatMessages => &mut traces.1,
    };
    *slot = Some((owner.clone(), trace.clone()));
}

pub(crate) fn runtime_trace(target: PromptTarget) -> Option<RuntimeTrace> {
    let traces = TRACES.lock().unwrap_or_else(|error| error.into_inner());
    match target {
        PromptTarget::AsrInstruction | PromptTarget::AsrContextBias => traces.0.clone(),
        PromptTarget::TranslationInstruction | PromptTarget::ChatMessages => traces.1.clone(),
    }
}

pub(crate) fn clear_runtime_traces() {
    *TRACES.lock().unwrap_or_else(|error| error.into_inner()) = (None, None);
}
