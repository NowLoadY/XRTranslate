use super::{Emit, Event as ShortcutEvent, Shortcut, selected_text};
use std::sync::Arc;
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{self, ConnectionExt, ModMask},
    },
};

type Worker = (Box<dyn FnOnce() + Send>, std::thread::JoinHandle<()>);
pub(super) fn start(key: Shortcut, emit: Emit) -> Result<Worker, String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return portal(key, emit);
    }
    let (connection, screen) = x11rb::connect(None).map_err(|e| e.to_string())?;
    let root = connection.setup().roots[screen].root;
    let first = connection.setup().min_keycode;
    let mapping = connection
        .get_keyboard_mapping(first, connection.setup().max_keycode - first + 1)
        .map_err(|e| e.to_string())?
        .reply()
        .map_err(|e| e.to_string())?;
    let keycode = mapping
        .keysyms
        .chunks(usize::from(mapping.keysyms_per_keycode))
        .position(|keys| {
            keys.contains(&u32::from(key.key.to_ascii_lowercase()))
                || keys.contains(&u32::from(key.key))
        })
        .map(|index| first + index as u8)
        .ok_or("Shortcut key is unavailable")?;
    let mut modifiers = ModMask::from(0u16);
    if key.control {
        modifiers |= ModMask::CONTROL;
    }
    if key.alt {
        modifiers |= ModMask::M1;
    }
    if key.shift {
        modifiers |= ModMask::SHIFT;
    }
    for lock in [
        ModMask::from(0u16),
        ModMask::LOCK,
        ModMask::M2,
        ModMask::LOCK | ModMask::M2,
    ] {
        connection
            .grab_key(
                false,
                root,
                modifiers | lock,
                keycode,
                xproto::GrabMode::ASYNC,
                xproto::GrabMode::ASYNC,
            )
            .map_err(|e| e.to_string())?
            .check()
            .map_err(|_| "This shortcut is already in use. Choose another shortcut.".to_owned())?;
    }
    let window = connection.generate_id().map_err(|e| e.to_string())?;
    connection
        .create_window(
            0,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            xproto::WindowClass::INPUT_ONLY,
            0,
            &xproto::CreateWindowAux::default(),
        )
        .map_err(|e| e.to_string())?
        .check()
        .map_err(|e| e.to_string())?;
    connection.flush().map_err(|e| e.to_string())?;
    let connection = Arc::new(connection);
    let stop_connection = connection.clone();
    let worker = std::thread::Builder::new()
        .name("translation-shortcut".into())
        .spawn(move || {
            let mut last_release = 0;
            while let Ok(event) = connection.wait_for_event() {
                match event {
                    Event::KeyRelease(event) if event.detail == keycode => {
                        last_release = event.time
                    }
                    Event::KeyPress(event)
                        if event.detail == keycode && event.time != last_release =>
                    {
                        emit(ShortcutEvent::Captured(selected_text()));
                    }
                    Event::ClientMessage(event) if event.window == window => break,
                    _ => {}
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((
        Box::new(move || {
            let event = xproto::ClientMessageEvent::new(32, window, x11rb::NONE, [0u32; 5]);
            let _ = stop_connection.send_event(false, window, xproto::EventMask::NO_EVENT, event);
            let _ = stop_connection.flush();
        }),
        worker,
    ))
}

fn portal(key: Shortcut, emit: Emit) -> Result<Worker, String> {
    let (cancel, stopped) = futures::channel::oneshot::channel::<()>();
    let worker = std::thread::Builder::new()
        .name("translation-shortcut".into())
        .spawn(move || {
            let mut active_session = None;
            let run = async {
                use ashpd::desktop::{
                    CreateSessionOptions,
                    global_shortcuts::{GlobalShortcuts, NewShortcut},
                };
                use futures::StreamExt;
                let portal = GlobalShortcuts::new().await.map_err(|e| e.to_string())?;
                let session = portal
                    .create_session(CreateSessionOptions::default())
                    .await
                    .map_err(|e| e.to_string())?;
                active_session = Some(session);
                let session = active_session.as_ref().unwrap();
                let mut trigger = String::new();
                for (enabled, modifier) in [
                    (key.control, "CTRL+"),
                    (key.alt, "ALT+"),
                    (key.shift, "SHIFT+"),
                ] {
                    if enabled {
                        trigger.push_str(modifier);
                    }
                }
                trigger.push(char::from(key.key));
                let shortcut_id = uuid::Uuid::new_v4().to_string();
                let mut events = portal
                    .receive_activated()
                    .await
                    .map_err(|e| e.to_string())?;
                portal
                    .bind_shortcuts(
                        session,
                        &[NewShortcut::new(
                            shortcut_id.as_str(),
                            "Translate selected or copied text",
                        )
                        .preferred_trigger(trigger.as_str())],
                        None,
                        Default::default(),
                    )
                    .await
                    .map_err(|e| e.to_string())?
                    .response()
                    .map_err(|e| e.to_string())?;
                while let Some(event) = events.next().await {
                    if event.shortcut_id() == shortcut_id {
                        emit(ShortcutEvent::Captured(selected_text()));
                    }
                }
                Ok::<(), String>(())
            };
            let error = match futures::executor::block_on(futures::future::select(
                Box::pin(run),
                Box::pin(stopped),
            )) {
                futures::future::Either::Left((result, _)) => result.err(),
                futures::future::Either::Right(_) => None,
            };
            if let Some(session) = active_session {
                let _ = futures::executor::block_on(session.close());
            }
            if let Some(error) = error {
                emit(ShortcutEvent::Unavailable(format!(
                    "Global shortcut unavailable: {error}"
                )));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((
        Box::new(move || {
            let _ = cancel.send(());
        }),
        worker,
    ))
}
