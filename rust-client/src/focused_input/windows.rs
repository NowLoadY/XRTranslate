use std::{
    cell::Cell,
    mem::size_of,
    rc::Rc,
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{CloseHandle, HWND},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoUninitialize,
            },
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
            Variant::{VT_BOOL, VariantClear},
        },
        UI::{
            Accessibility::{
                CUIAutomation8, HWINEVENTHOOK, IUIAutomation2, IUIAutomationElement,
                IUIAutomationTextPattern, IUIAutomationValuePattern, SetWinEventHook,
                UIA_ComboBoxControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
                UIA_IsReadOnlyAttributeId, UIA_TextPatternId, UIA_ValuePatternId, UnhookWinEvent,
            },
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
                KEYEVENTF_UNICODE, SendInput, VK_CONTROL, VK_LBUTTON, VK_LWIN, VK_MBUTTON, VK_MENU,
                VK_RBUTTON, VK_RWIN, VK_SHIFT,
            },
            WindowsAndMessaging::{
                DispatchMessageW, EVENT_OBJECT_FOCUS, EVENT_SYSTEM_FOREGROUND, GA_ROOT,
                GetAncestor, GetForegroundWindow, GetWindowThreadProcessId, MSG, PM_REMOVE,
                PeekMessageW, TranslateMessage, WINEVENT_OUTOFCONTEXT,
            },
        },
    },
    core::PWSTR,
};

pub(crate) fn timestamp() -> u64 {
    unsafe { windows_sys::Win32::System::SystemInformation::GetTickCount64() }
}

thread_local! {
    // OUTOFCONTEXT callbacks run on the installing worker thread, including
    // while UI Automation pumps messages. No plugin locks or COM calls here.
    static LAST_FOCUS_CHANGE: Cell<u64> = const { Cell::new(0) };
}

unsafe extern "system" fn focus_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    _window: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    event_tick: u32,
) {
    let changed_at = event_timestamp(timestamp(), event_tick);
    LAST_FOCUS_CHANGE.with(|last| last.set(last.get().max(changed_at)));
}

fn event_timestamp(now: u64, event_tick: u32) -> u64 {
    // WinEvent carries the low 32 bits of the uptime clock. Lift it into the
    // current epoch, including events queued just before the 49.7-day wrap.
    now.saturating_sub(u64::from((now as u32).wrapping_sub(event_tick)))
}

struct FocusEvents {
    hooks: Vec<HWINEVENTHOOK>,
    started_at: u64,
}

impl FocusEvents {
    fn new() -> Result<Self, String> {
        let mut events = Self {
            hooks: Vec::with_capacity(2),
            started_at: 0,
        };
        for event in [EVENT_SYSTEM_FOREGROUND, EVENT_OBJECT_FOCUS] {
            let hook = unsafe {
                SetWinEventHook(
                    event,
                    event,
                    None,
                    Some(focus_changed),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT,
                )
            };
            if hook.is_invalid() {
                return Err("Could not monitor focused input changes".to_owned());
            }
            events.hooks.push(hook);
        }
        // Results produced before both hooks were installed have no reliable
        // focus history and must be skipped.
        events.started_at = timestamp();
        Ok(events)
    }

    fn permits(&self, produced_at: u64) -> bool {
        // Same-millisecond boundaries are ambiguous; skip them conservatively.
        produced_at > self.started_at && LAST_FOCUS_CHANGE.with(|last| produced_at > last.get())
    }
}

impl Drop for FocusEvents {
    fn drop(&mut self) {
        for hook in self.hooks.drain(..) {
            let _ = unsafe { UnhookWinEvent(hook) };
        }
    }
}

fn drain_focus_events() -> bool {
    let mut message = MSG::default();
    // Keep even an unusually busy message queue from blocking cancellation.
    for _ in 0..256 {
        if !unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            return true;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    false
}

// Rc keeps the COM apartment alive until its last element is dropped and makes
// both public types thread-affine, even if a Windows interface happens to be Send.
struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(crate) struct Platform {
    focus_events: FocusEvents,
    automation: IUIAutomation2,
    own_executable: String,
    apartment: Rc<Apartment>,
}

pub(crate) struct Target {
    element: IUIAutomationElement,
    foreground: HWND,
    produced_at: u64,
    _apartment: Rc<Apartment>,
}

impl Platform {
    pub(crate) fn new() -> Result<Self, String> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| format!("Could not initialize focused input: {error}"))?;
        let apartment = Rc::new(Apartment);
        let automation: IUIAutomation2 =
            unsafe { CoCreateInstance(&CUIAutomation8, None, CLSCTX_INPROC_SERVER) }
                .map_err(|error| format!("Could not initialize Windows UI Automation: {error}"))?;
        // A hung application must not keep the output worker waiting for the
        // default multi-second UI Automation timeouts.
        (|| unsafe {
            automation.SetConnectionTimeout(200)?;
            automation.SetTransactionTimeout(200)?;
            automation.SetAutoSetFocus(false)?;
            Ok::<_, windows::core::Error>(())
        })()
        .map_err(|error| format!("Could not configure Windows UI Automation: {error}"))?;
        let own_executable = std::env::current_exe()
            .map_err(|error| {
                format!("Could not identify the input application's process: {error}")
            })?
            .to_string_lossy()
            .into_owned();
        Ok(Self {
            focus_events: FocusEvents::new()?,
            automation,
            own_executable,
            apartment,
        })
    }

    pub(crate) fn poll_events(&self) {
        drain_focus_events();
    }

    /// Returns only enabled, writable, non-password text fields outside this
    /// executable whose current focus predates the completed translation.
    pub(crate) fn focused_target_since(&self, produced_at: u64) -> Result<Option<Target>, String> {
        if !drain_focus_events() || !self.focus_events.permits(produced_at) {
            return Ok(None);
        }
        let foreground = unsafe { GetForegroundWindow() };
        if foreground.is_invalid()
            || input_gesture_in_progress()
            || !self.external_window(foreground)
        {
            return Ok(None);
        }
        // Disappearing controls, empty focus, and inaccessible applications are
        // ordinary reasons to skip a result, not errors that should spam the UI.
        let Ok(element) = (unsafe { self.automation.GetFocusedElement() }) else {
            return Ok(None);
        };
        if !self.editable(&element)
            || !self.belongs_to_window(&element, foreground)
            || !unsafe { element.CurrentHasKeyboardFocus() }.is_ok_and(|value| value.as_bool())
            || unsafe { GetForegroundWindow() } != foreground
            || !drain_focus_events()
            || !self.focus_events.permits(produced_at)
        {
            return Ok(None);
        }
        Ok(Some(Target {
            element,
            foreground,
            produced_at,
            _apartment: Rc::clone(&self.apartment),
        }))
    }

    /// Inserts at the current caret/selection without changing the clipboard or
    /// adding an Enter key. `false` means no input was sent; discard the result.
    /// An error may follow partial delivery: discard that item to avoid duplicates.
    /// The permit is acquired after UI Automation calls and held through the
    /// final focus check and input, synchronizing cancellation with delivery.
    pub(crate) fn insert_if<G>(
        &self,
        target: &Target,
        text: &str,
        permit: impl FnOnce() -> Option<G>,
    ) -> Result<bool, String> {
        let inputs = unicode_inputs(text);
        if inputs.is_empty() {
            return Ok(true);
        }
        let Some(current) = self.focused_target_since(target.produced_at)? else {
            return Ok(false);
        };
        if current.foreground != target.foreground
            || !unsafe {
                self.automation
                    .CompareElements(&target.element, &current.element)
            }
            .is_ok_and(|same| same.as_bool())
            || input_gesture_in_progress()
            || unsafe { GetForegroundWindow() } != target.foreground
        {
            return Ok(false);
        }
        let Some(_permit) = permit() else {
            return Ok(false);
        };
        // The permit can briefly wait for a concurrent queue update. Recheck
        // the cheap OS state after acquiring it and immediately before injection.
        if !drain_focus_events()
            || !self.focus_events.permits(target.produced_at)
            || input_gesture_in_progress()
            || unsafe { GetForegroundWindow() } != target.foreground
        {
            return Ok(false);
        }
        // One batch prevents user key/mouse input from interleaving its UTF-16
        // key pairs. Do not steal focus or retry after any partial insertion.
        let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) } as usize;
        if sent != inputs.len() {
            return Err(format!(
                "Windows accepted {sent}/{} text input events; the target may require elevated permission",
                inputs.len()
            ));
        }
        Ok(true)
    }

    fn external_window(&self, window: HWND) -> bool {
        let mut process_id = 0;
        unsafe { GetWindowThreadProcessId(window, Some(&mut process_id)) };
        self.external_process(process_id)
    }

    fn external_process(&self, process_id: u32) -> bool {
        if process_id == 0 || process_id == std::process::id() {
            return false;
        }
        let Ok(process) =
            (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
        else {
            return false;
        };
        let mut buffer = vec![0; 32768];
        let mut length = buffer.len() as u32;
        let result = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        };
        let _ = unsafe { CloseHandle(process) };
        result.is_ok()
            && !same_executable(
                &String::from_utf16_lossy(&buffer[..length as usize]),
                &self.own_executable,
            )
    }

    fn editable(&self, element: &IUIAutomationElement) -> bool {
        unsafe {
            element
                .CurrentHasKeyboardFocus()
                .is_ok_and(|value| value.as_bool())
                && element
                    .CurrentProcessId()
                    .is_ok_and(|pid| self.external_process(pid as u32))
                && writable_text(element)
        }
    }

    fn belongs_to_window(&self, element: &IUIAutomationElement, foreground: HWND) -> bool {
        // Browser/WinUI controls can be windowless and run in a different process
        // from their foreground frame. Verify ancestry, not just matching PIDs.
        let started = Instant::now();
        let Ok(walker) = (unsafe { self.automation.RawViewWalker() }) else {
            return false;
        };
        let mut ancestor = element.clone();
        for _ in 0..32 {
            if started.elapsed() > Duration::from_millis(200) {
                return false;
            }
            let Ok(window) = (unsafe { ancestor.CurrentNativeWindowHandle() }) else {
                return false;
            };
            if !window.is_invalid() {
                return window == foreground
                    || unsafe { GetAncestor(window, GA_ROOT) } == foreground;
            }
            let Ok(parent) = (unsafe { walker.GetParentElement(&ancestor) }) else {
                return false;
            };
            ancestor = parent;
        }
        false
    }
}

fn writable_text(element: &IUIAutomationElement) -> bool {
    // Read-only and password checks are deliberately conservative. We do not
    // guess from a text-shaped window or send keys to an arbitrary game.
    unsafe {
        if !element
            .CurrentIsEnabled()
            .is_ok_and(|value| value.as_bool())
            || !element
                .CurrentIsPassword()
                .is_ok_and(|value| !value.as_bool())
        {
            return false;
        }
        let Ok(control_type) = element.CurrentControlType() else {
            return false;
        };
        if ![
            UIA_EditControlTypeId,
            UIA_DocumentControlTypeId,
            UIA_ComboBoxControlTypeId,
        ]
        .contains(&control_type)
        {
            return false;
        }
        if let Ok(value) =
            element.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
        {
            return value
                .CurrentIsReadOnly()
                .is_ok_and(|readonly| !readonly.as_bool());
        }
        // Rich text editors commonly expose only TextPattern. A real BOOL
        // false is required; mixed/unsupported attributes are not writable.
        let Ok(text) = element.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
        else {
            return false;
        };
        let Ok(range) = text.DocumentRange() else {
            return false;
        };
        let Ok(mut readonly) = range.GetAttributeValue(UIA_IsReadOnlyAttributeId) else {
            return false;
        };
        let writable = readonly.Anonymous.Anonymous.vt == VT_BOOL
            && !readonly.Anonymous.Anonymous.Anonymous.boolVal.as_bool();
        let _ = VariantClear(&mut readonly);
        writable
    }
}

fn same_executable(left: &str, right: &str) -> bool {
    left.trim_start_matches(r"\\?\")
        .eq_ignore_ascii_case(right.trim_start_matches(r"\\?\"))
}

fn input_gesture_in_progress() -> bool {
    [
        VK_CONTROL, VK_MENU, VK_SHIFT, VK_LWIN, VK_RWIN, VK_LBUTTON, VK_RBUTTON, VK_MBUTTON,
    ]
    .into_iter()
    .any(|key| unsafe { GetAsyncKeyState(i32::from(key.0)) } < 0)
}

fn unicode_inputs(text: &str) -> Vec<INPUT> {
    // Preserve the completed translation exactly, including its whitespace.
    text.encode_utf16()
        .flat_map(|unit| {
            [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP].map(|flags| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wScan: unit,
                        dwFlags: flags,
                        ..Default::default()
                    },
                },
            })
        })
        .collect()
}
