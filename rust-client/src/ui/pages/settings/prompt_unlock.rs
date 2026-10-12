use eframe::egui::{Id, Response};
use std::path::Path;

#[derive(Clone, Default)]
struct UnlockGesture {
    activations: u8,
    last_frame: u64,
    last_toggle_frame: Option<u64>,
    unlocked: bool,
}

pub(super) fn observe(response: &Response, beta_enabled: bool, project_root: &Path) {
    let ctx = &response.ctx;
    let id = Id::new("android_prompt_editor_unlock");
    let frame = ctx.cumulative_frame_nr();
    let mut state = ctx.data_mut(|data| data.get_temp::<UnlockGesture>(id).unwrap_or_default());
    if frame > state.last_frame + 1
        || (ctx.input(|input| input.pointer.any_click()) && !response.clicked())
    {
        state = UnlockGesture::default();
    }
    state.last_frame = frame;
    // Count only deliberate off-to-on changes, once even if egui repeats a pass.
    if response.changed() && state.last_toggle_frame != Some(frame) {
        state.last_toggle_frame = Some(frame);
        if beta_enabled && !state.unlocked {
            state.activations += 1;
            if state.activations == 5 {
                state.activations = 0;
                match super::super::prompt_studio::unlock_editor(project_root) {
                    Ok(()) => state.unlocked = true,
                    Err(error) => log::warn!("Could not unlock prompt editor: {error}"),
                }
            }
        }
    }
    ctx.data_mut(|data| data.insert_temp(id, state));
}
