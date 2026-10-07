use super::{Emit, Event as ShortcutEvent, Shortcut, selected_text};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use windows::Win32::{
    System::{
        Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
        Threading::GetCurrentThreadId,
    },
    UI::{
        Accessibility::{
            CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPatternId,
        },
        Input::KeyboardAndMouse::{
            HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT, RegisterHotKey,
            UnregisterHotKey,
        },
        WindowsAndMessaging::{
            GetMessageW, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_HOTKEY, WM_QUIT,
        },
    },
};

pub(super) fn start(
    key: Shortcut,
    emit: Emit,
) -> Result<(Box<dyn FnOnce() + Send>, std::thread::JoinHandle<()>), String> {
    let thread_id = Arc::new(AtomicU32::new(0));
    let stopped = Arc::new(AtomicBool::new(false));
    let (id, stop) = (thread_id.clone(), stopped.clone());
    let worker = std::thread::Builder::new()
        .name("translation-shortcut".into())
        .spawn(move || unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let mut message = MSG::default();
            let _ = PeekMessageW(&mut message, None, 0, 0, PM_NOREMOVE);
            id.store(GetCurrentThreadId(), Ordering::Release);
            let modifiers = HOT_KEY_MODIFIERS(
                MOD_NOREPEAT.0
                    | if key.control { MOD_CONTROL.0 } else { 0 }
                    | if key.alt { MOD_ALT.0 } else { 0 }
                    | if key.shift { MOD_SHIFT.0 } else { 0 },
            );
            if let Err(error) = RegisterHotKey(None, 1, modifiers, u32::from(key.key)) {
                emit(ShortcutEvent::Unavailable(format!(
                    "This shortcut is unavailable: {error}"
                )));
            } else {
                while !stop.load(Ordering::Acquire) && GetMessageW(&mut message, None, 0, 0).0 > 0 {
                    if message.message == WM_HOTKEY {
                        emit(ShortcutEvent::Captured(selected_text()));
                    }
                }
                let _ = UnregisterHotKey(None, 1);
            }
            if initialized {
                CoUninitialize();
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((
        Box::new(move || {
            stopped.store(true, Ordering::Release);
            let id = thread_id.load(Ordering::Acquire);
            if id != 0 {
                unsafe {
                    let _ = PostThreadMessageW(id, WM_QUIT, Default::default(), Default::default());
                }
            }
        }),
        worker,
    ))
}

pub(super) fn selection() -> Option<String> {
    unsafe {
        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let element = automation.GetFocusedElement().ok()?;
        // Password controls never expose a selection to this feature.
        if element.CurrentIsPassword().ok()?.as_bool() {
            return None;
        }
        let pattern: IUIAutomationTextPattern =
            element.GetCurrentPatternAs(UIA_TextPatternId).ok()?;
        let ranges = pattern.GetSelection().ok()?;
        let mut text = String::new();
        for index in 0..ranges.Length().ok()?.min(16) {
            // Include one character beyond the host limit, allowing UTF-16 surrogate pairs.
            let selected = ranges
                .GetElement(index)
                .ok()?
                .GetText(128_002)
                .ok()?
                .to_string();
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&selected);
        }
        (!text.trim().is_empty()).then_some(text)
    }
}
