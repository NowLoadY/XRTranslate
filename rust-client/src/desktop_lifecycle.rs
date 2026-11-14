//! Windows tray and close confirmation; hiding the window leaves application work running.
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
                FindWindowW, PostMessageW, WM_APP, WM_ENDSESSION, WM_QUERYENDSESSION,
            },
        },
    },
    core::w,
};

const SHOW_APPLICATION: u32 = WM_APP + 1;

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
                    // Let the owner update both native and egui visibility state.
                    let _ = PostMessageW(Some(window), SHOW_APPLICATION, WPARAM(0), LPARAM(0));
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
    _messages: Box<WindowMessages>,
    exit: MenuItem,
    language: UiLanguage,
    actions: Receiver<Action>,
    close: WindowClose,
}

impl Tray {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        icon: &egui::IconData,
        language: UiLanguage,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let ctx = &cc.egui_ctx;
        let (send, actions) = mpsc::channel();
        let messages = WindowMessages::attach(cc, send.clone())?;
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
            _messages: messages,
            exit,
            language,
            actions,
            close: WindowClose::Running,
        })
    }

    pub fn exit(&mut self, ctx: &egui::Context) {
        self.close.exit(ctx);
    }

    pub fn render_close_prompt(&mut self, ctx: &egui::Context, language: UiLanguage) -> bool {
        self.close.render(ctx, language)
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
        self.close.update(ctx);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WindowClose {
    Running,
    Confirming,
    Exiting,
}

impl WindowClose {
    fn exit(&mut self, ctx: &egui::Context) {
        *self = Self::Exiting;
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }

    fn update(&mut self, ctx: &egui::Context) {
        if *self != Self::Exiting && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::CancelClose);
            *self = Self::Confirming;
        }
    }

    /// Takes priority over other dialogs without discarding their pending actions.
    fn render(&mut self, ctx: &egui::Context, language: UiLanguage) -> bool {
        if *self != Self::Confirming {
            return false;
        }
        let response = egui::Modal::new(egui::Id::unique("window_close"))
            .frame(
                egui::Frame::new()
                    .fill(crate::ui::theme::modal_backdrop())
                    .stroke(egui::Stroke::new(1.0, crate::ui::theme::border()))
                    .corner_radius(crate::ui::theme::container_radius(12))
                    .inner_margin(egui::Margin::symmetric(12, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(i18n::tr(language, "Also stop running in the background?"));
                    ui.add_space(4.0);
                    let background = crate::ui::components::secondary_button(
                        ui,
                        i18n::tr(language, "Keep running in background"),
                    )
                    .clicked();
                    let exit = crate::ui::components::primary_button(
                        ui,
                        i18n::tr(language, "Exit completely"),
                    )
                    .clicked();
                    let cancel = ui.add_sized([24.0, 24.0], egui::Button::new("×").frame(false));
                    let cancel = ui_director::egui::Control::button(i18n::tr(language, "Cancel"))
                        .response(ui, cancel)
                        .on_hover_text(i18n::tr(language, "Cancel"));
                    (background, exit, cancel.clicked())
                })
                .inner
            });
        let (background, exit, cancel) = response.inner;
        if exit {
            self.exit(ctx);
        } else if background || cancel || response.should_close() {
            *self = Self::Running;
            if background {
                ctx.send_viewport_cmd_to(
                    egui::ViewportId::ROOT,
                    egui::ViewportCommand::Visible(false),
                );
            }
        }
        true
    }
}

/// Reopening and session shutdown use the same actions as the tray menu.
struct WindowMessages {
    window: HWND,
    send: Sender<Action>,
    ctx: egui::Context,
}

impl WindowMessages {
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
                Some(window_message),
                1,
                &*state as *const Self as usize,
            )
            .ok()?;
        }
        Ok(state)
    }
}

impl Drop for WindowMessages {
    fn drop(&mut self) {
        unsafe {
            let _ = RemoveWindowSubclass(self.window, Some(window_message), 1);
        }
    }
}

unsafe extern "system" fn window_message(
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
    let action = if message == SHOW_APPLICATION {
        Some(Action::Show)
    } else if message == WM_ENDSESSION && wparam.0 != 0 {
        Some(Action::Exit)
    } else {
        None
    };
    if let Some(action) = action {
        // The box stays owned by Tray, and its callback is removed before it is freed.
        let state = unsafe { &*(data as *const WindowMessages) };
        let _ = state.send.send(action);
        state.ctx.request_repaint();
        return LRESULT(0);
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

#[cfg(test)]
mod tests;
