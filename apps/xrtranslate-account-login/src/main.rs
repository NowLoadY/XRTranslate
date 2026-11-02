#![cfg_attr(windows, windows_subsystem = "windows")]

// A separate process keeps GTK/WebView event loops and browser dependencies
// out of the egui client and the audio backend. It exists only while signing in.
#[cfg(not(target_os = "android"))]
fn main() {
    std::process::exit(run().unwrap_or(3));
}

#[cfg(target_os = "android")]
fn main() {} // Android uses the host Activity's WebView.

#[cfg(not(target_os = "android"))]
fn run() -> Result<i32, Box<dyn std::error::Error>> {
    use std::{
        path::PathBuf,
        time::{Duration, Instant},
    };
    use tao::{
        event::{Event, WindowEvent},
        event_loop::{ControlFlow, EventLoopBuilder},
        platform::run_return::EventLoopExtRunReturn,
        window::WindowBuilder,
    };
    use wry::WebViewBuilder;
    use xrtranslate_connections::{AccountProvider, AccountStore, login};

    enum LoginEvent {
        Capture(String),
        Navigate(String),
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() == 1 && args[0] == "--version" {
        println!("xrtranslate-account-login {}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }
    if args.len() != 2 {
        return Ok(64);
    }
    let Some(provider) = AccountProvider::ALL
        .iter()
        .copied()
        .find(|provider| args[0] == provider.id())
    else {
        return Ok(64);
    };
    let root = PathBuf::from(&args[1]);
    let accounts = AccountStore::for_project_root(root);
    let spec = login::spec(provider).ok_or("This account uses system-browser authorization")?;
    let mut event_loop = EventLoopBuilder::<LoginEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title(format!("XRTranslate · {} · {}", spec.label, spec.origin))
        .with_inner_size(tao::dpi::LogicalSize::new(1100.0, 760.0))
        .build(&event_loop)?;
    let popup_proxy = event_loop.create_proxy();
    let builder = WebViewBuilder::new()
        .with_url(spec.url)
        .with_user_agent(spec.user_agent)
        .with_incognito(true)
        .with_devtools(false)
        .with_initialization_script(spec.initialization_script)
        .with_navigation_handler(move |url| spec.allows_navigation(&url))
        .with_new_window_req_handler(move |url, _| {
            if spec.allows_navigation(&url) {
                let _ = popup_proxy.send_event(LoginEvent::Navigate(url));
            }
            wry::NewWindowResponse::Deny
        })
        .with_download_started_handler(|_, _| false)
        .with_permission_handler(|_| wry::PermissionResponse::Deny);
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        // Tao already installs a child in GtkWindow. Attach to that box;
        // GtkWindow rejects a second child and otherwise leaves a blank window.
        let container = window
            .default_vbox()
            .ok_or("Login window container unavailable")?;
        match builder.build_gtk(container) {
            Ok(view) => view,
            Err(_) => return Ok(5),
        }
    };
    #[cfg(not(target_os = "linux"))]
    let webview = match builder.build(&window) {
        Ok(view) => view,
        Err(_) => return Ok(5),
    };

    let proxy = event_loop.create_proxy();
    let mut next_poll = Instant::now();
    let mut pending = false;
    let mut evaluated_at = Instant::now();
    let mut exit_code = 2; // closing the window is cancellation, never success
    event_loop.run_return(|event, _, flow| {
        *flow = ControlFlow::WaitUntil(next_poll);
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *flow = ControlFlow::Exit,
            Event::UserEvent(LoginEvent::Navigate(url)) => {
                let _ = webview.load_url(&url);
            }
            Event::UserEvent(LoginEvent::Capture(capture)) => {
                pending = false;
                let Ok(url) = webview.url() else {
                    return;
                };
                if !spec.is_capture_origin(&url) {
                    return;
                }
                let Ok(cookies) = webview.cookies_for_url(spec.url) else {
                    return;
                };
                let cookie_header = cookies
                    .iter()
                    .map(|cookie| format!("{}={}", cookie.name(), cookie.value()))
                    .collect::<Vec<_>>()
                    .join("; ");
                if let Some(session) =
                    login::captured_session(provider, &url, &capture, cookie_header)
                {
                    exit_code = if accounts.save(&session).is_ok() {
                        0
                    } else {
                        4
                    };
                    *flow = ControlFlow::Exit;
                }
            }
            Event::MainEventsCleared if Instant::now() >= next_poll => {
                next_poll = Instant::now() + Duration::from_secs(1);
                if pending && evaluated_at.elapsed() > Duration::from_secs(5) {
                    pending = false;
                }
                if !pending && webview.url().is_ok_and(|url| spec.is_capture_origin(&url)) {
                    let proxy = proxy.clone();
                    evaluated_at = Instant::now();
                    pending = webview
                        .evaluate_script_with_callback(spec.capture_script, move |capture| {
                            let _ = proxy.send_event(LoginEvent::Capture(capture));
                        })
                        .is_ok();
                }
            }
            _ => {}
        }
    });
    // Explicitly close the ephemeral browser before reporting login success.
    drop(webview);
    drop(window);
    Ok(exit_code)
}
