//! Windows owns the tray menu; hiding the window leaves application work running.
use crate::i18n::{self, UiLanguage};
use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::sync::mpsc::{self, Receiver, Sender};
use tray_icon::{
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    menu::{Menu, MenuEvent, MenuItem},
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
        },
        System::Threading::CreateMutexW,
        UI::{
            Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
            WindowsAndMessaging::{
                FindWindowW, SW_RESTORE, SetForegroundWindow, ShowWindow, WM_ENDSESSION,
                WM_QUERYENDSESSION,
            },
        },
    },
    core::w,
};

pub(crate) struct Instance(HANDLE);

impl Instance {
    /// A shortcut reopens the running application instead of starting a second backend.
    pub fn acquire() -> windows::core::Result<Option<Self>> {
        unsafe {
            let handle = CreateMutexW(None, false, w!("Local\\XRTranslate.Desktop"))?;
            let exists = GetLastError() == ERROR_ALREADY_EXISTS;
            let instance = Self(handle);
            if exists {
                if let Ok(window) = FindWindowW(None, w!("XRTranslate")) {
                    let _ = ShowWindow(window, SW_RESTORE);
                    let _ = SetForegroundWindow(window);
                }
                Ok(None)
            } else {
                Ok(Some(instance))
            }
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

enum Action {
    Show,
    Exit,
}

pub(crate) struct Tray {
    _icon: TrayIcon,
    _shutdown: Box<Shutdown>,
    exit: MenuItem,
    language: UiLanguage,
    actions: Receiver<Action>,
    exiting: bool,
}

impl Tray {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        icon: &egui::IconData,
        language: UiLanguage,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let ctx = &cc.egui_ctx;
        let (send, actions) = mpsc::channel();
        let shutdown = Shutdown::attach(cc, send.clone())?;
        let show = MenuItem::new("XRTranslate", true, None);
        let exit = MenuItem::new(i18n::tr(language, "Exit"), true, None);
        let menu = Menu::with_items(&[&show, &exit])?;
        let icon = TrayIconBuilder::new()
            .with_tooltip("XRTranslate")
            .with_icon(tray_icon::Icon::from_rgba(
                icon.rgba.clone(),
                icon.width,
                icon.height,
            )?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;
        let tray_send = send.clone();
        let tray_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = tray_send.send(Action::Show);
                tray_ctx.request_repaint();
            }
        }));
        let ctx = ctx.clone();
        let show_id = show.id().clone();
        let exit_id = exit.id().clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let action = if event.id == show_id {
                Action::Show
            } else if event.id == exit_id {
                Action::Exit
            } else {
                return;
            };
            let _ = send.send(action);
            ctx.request_repaint();
        }));
        Ok(Self {
            _icon: icon,
            _shutdown: shutdown,
            exit,
            language,
            actions,
            exiting: false,
        })
    }

    pub fn exit(&mut self, ctx: &egui::Context) {
        self.exiting = true;
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    pub fn update(&mut self, ctx: &egui::Context, language: UiLanguage) {
        if self.language != language {
            self.exit.set_text(i18n::tr(language, "Exit"));
            self.language = language;
        }
        while let Ok(action) = self.actions.try_recv() {
            match action {
                Action::Show => {
                    for command in [
                        egui::ViewportCommand::Visible(true),
                        egui::ViewportCommand::Minimized(false),
                        egui::ViewportCommand::Focus,
                    ] {
                        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, command);
                    }
                }
                Action::Exit => self.exit(ctx),
            }
        }
        if !self.exiting && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
    }
}

/// Restart Manager and Windows sign-out request an actual exit, not close-to-tray.
struct Shutdown {
    window: HWND,
    send: Sender<Action>,
    ctx: egui::Context,
}

impl Shutdown {
    fn attach(
        cc: &eframe::CreationContext<'_>,
        send: Sender<Action>,
    ) -> Result<Box<Self>, Box<dyn std::error::Error>> {
        let RawWindowHandle::Win32(handle) = cc.window_handle()?.as_raw() else {
            return Err("Windows window handle is unavailable".into());
        };
        let state = Box::new(Self {
            window: HWND(handle.hwnd.get() as _),
            send,
            ctx: cc.egui_ctx.clone(),
        });
        unsafe {
            SetWindowSubclass(
                state.window,
                Some(session_message),
                1,
                &*state as *const Self as usize,
            )
            .ok()?;
        }
        Ok(state)
    }
}

impl Drop for Shutdown {
    fn drop(&mut self) {
        unsafe {
            let _ = RemoveWindowSubclass(self.window, Some(session_message), 1);
        }
    }
}

unsafe extern "system" fn session_message(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if message == WM_QUERYENDSESSION {
        return LRESULT(1);
    }
    if message == WM_ENDSESSION && wparam.0 != 0 {
        // The box stays owned by Tray, and its callback is removed before it is freed.
        let state = unsafe { &*(data as *const Shutdown) };
        let _ = state.send.send(Action::Exit);
        state.ctx.request_repaint();
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}
