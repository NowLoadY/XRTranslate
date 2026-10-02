//! Keep keyboard occlusion separate from the native drawing surface.
use eframe::egui::{Context, Id, RawInput, output::IMEOutput};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicI32, Ordering},
};

static KEYBOARD_BOTTOM: AtomicI32 = AtomicI32::new(0);
static CONTEXT: Mutex<Option<Context>> = Mutex::new(None);

pub fn install(ctx: &Context) {
    *CONTEXT.lock().unwrap() = Some(ctx.clone());
    ctx.on_end_pass(
        "android_ime",
        Arc::new(|ui| {
            let key = Id::new("android_ime_output");
            let focused = ui.memory(|memory| memory.focused());
            let previous = ui.data(|data| data.get_temp::<(Id, IMEOutput)>(key));
            let current = ui.ctx().output(|output| output.ime);
            let retained = match (focused, current, previous) {
                (Some(id), Some(mut ime), _) => {
                    ime.should_interrupt_composition = false;
                    Some((id, ime))
                }
                (Some(id), None, Some((previous_id, ime))) if id == previous_id => {
                    // A focused editor can be clipped for one pass while its
                    // scroll area moves above the keyboard. Keep IME active.
                    ui.ctx().output_mut(|output| output.ime = Some(ime));
                    Some((id, ime))
                }
                _ => None,
            };
            ui.data_mut(|data| {
                if let Some(retained) = retained {
                    data.insert_temp(key, retained);
                } else {
                    data.remove::<(Id, IMEOutput)>(key);
                }
            });
        }),
    );
}

pub fn apply_input(ctx: &Context, input: &mut RawInput) {
    let scale = input.viewport().native_pixels_per_point.unwrap_or(1.0) * ctx.zoom_factor();
    if let Some(rect) = &mut input.screen_rect {
        let bottom = KEYBOARD_BOTTOM.load(Ordering::Acquire).max(0) as f32 / scale;
        rect.max.y = (rect.max.y - bottom).max(rect.min.y + 1.0);
    }
}

pub fn uninstall() {
    *CONTEXT.lock().unwrap() = None;
    KEYBOARD_BOTTOM.store(0, Ordering::Release);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_org_xrtranslate_app_MainActivity_keyboardInsetChanged(
    _env: jni::JNIEnv,
    _class: jni::objects::JClass,
    bottom: jni::sys::jint,
) {
    if KEYBOARD_BOTTOM.swap(bottom, Ordering::AcqRel) != bottom
        && let Some(ctx) = CONTEXT.lock().unwrap().as_ref()
    {
        ctx.request_repaint();
    }
}
