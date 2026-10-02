//! Capture the app's rendered viewport, without recording the user's desktop or audio.
use std::{io::Write, net::TcpStream, sync::Arc, time::Duration};

use crossbeam_channel::{Sender, bounded};
use eframe::egui;

#[derive(Clone)]
struct TimedFrame {
    time: f64,
    sender: Sender<Arc<egui::ColorImage>>,
}

fn clock_id() -> egui::Id {
    egui::Id::new("director_capture_clock")
}

pub(super) fn receive(ctx: &egui::Context, input: &mut egui::RawInput) {
    input.events.retain(|event| {
        if let egui::Event::Screenshot {
            user_data, image, ..
        } = event
            && let Some(sender) = user_data
                .data
                .as_ref()
                .and_then(|data| data.downcast_ref::<Sender<Arc<egui::ColorImage>>>())
        {
            let _ = sender.try_send(image.clone());
            return false;
        }
        true
    });
    let request = ctx.data_mut(|data| {
        let request = data.get_temp::<TimedFrame>(clock_id().with("request"));
        data.remove::<TimedFrame>(clock_id().with("request"));
        request
    });
    let wall_time = input.time.unwrap_or_default();
    let (time, directed) = ctx.data_mut(|data| {
        let offset = data.get_temp::<f64>(clock_id().with("offset"));
        if let Some(request) = &request {
            let origin = data
                .get_temp::<(f64, f64, f64)>(clock_id())
                .map_or(wall_time + offset.unwrap_or_default(), |clock| clock.0);
            data.insert_temp(clock_id(), (origin, request.time, wall_time));
        }
        if let Some((origin, time, _)) = data.get_temp::<(f64, f64, f64)>(clock_id()) {
            data.insert_temp(clock_id(), (origin, time, wall_time));
            (Some(origin + time), true)
        } else {
            (offset.map(|offset| wall_time + offset), false)
        }
    });
    if let Some(time) = time {
        input.predicted_dt = (time - ctx.input(|input| input.time)).max(0.0) as f32;
        input.time = Some(time);
    }
    if directed {
        // Animate the directed viewport in the background without taking OS focus.
        input.focused = true;
        if let Some(viewport) = input.viewports.get_mut(&input.viewport_id) {
            viewport.focused = Some(true);
        }
    }
    if let Some(request) = request {
        // Queue the screenshot only after this input frame has adopted its requested time.
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            request.sender,
        )));
    }
}

/// Persistent Director TCP capture: JSON dimensions followed by exactly width*height*4 RGBA bytes.
/// UI time advances only when the recorder asks for the next frame, regardless of machine speed.
pub(super) fn timed_frame(
    stream: &mut TcpStream,
    ctx: &egui::Context,
    time: f64,
) -> std::io::Result<()> {
    if !time.is_finite() || time < 0.0 {
        return Err(std::io::Error::other("Invalid capture time"));
    }
    if ctx.data(|data| {
        data.get_temp::<(f64, f64, f64)>(clock_id())
            .is_some_and(|clock| time < clock.1)
    }) {
        return Err(std::io::Error::other("Capture time cannot move backwards"));
    }
    let (sender, receiver) = bounded(1);
    ctx.data_mut(|data| data.insert_temp(clock_id().with("request"), TimedFrame { time, sender }));
    ctx.request_repaint();
    let image = receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(std::io::Error::other)?;
    writeln!(
        stream,
        "{{\"width\":{},\"height\":{}}}",
        image.width(),
        image.height()
    )?;
    stream.write_all(image.as_raw())
}

pub(super) fn resume_clock(ctx: &egui::Context) {
    ctx.data_mut(|data| {
        if let Some((origin, time, wall_time)) = data.get_temp::<(f64, f64, f64)>(clock_id()) {
            // Continue from the last directed frame, even when recording ran faster than real time.
            data.insert_temp(clock_id().with("offset"), origin + time - wall_time);
        }
        data.remove::<(f64, f64, f64)>(clock_id());
        data.remove::<TimedFrame>(clock_id().with("request"));
    });
    ctx.request_repaint();
}

pub(super) fn respond(stream: &mut TcpStream, ctx: &egui::Context) {
    let (sender, receiver) = bounded::<Arc<egui::ColorImage>>(1);
    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
        sender,
    )));
    ctx.request_repaint();
    match receiver.recv_timeout(Duration::from_secs(5)) {
        Ok(image) => {
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nX-Width: {}\r\nX-Height: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                image.width(),
                image.height(),
                image.as_raw().len(),
            );
            if stream.write_all(header.as_bytes()).is_ok() {
                let _ = stream.write_all(image.as_raw());
            }
        }
        Err(_) => {
            let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    }
}
