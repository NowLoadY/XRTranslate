//! One persistent companion for the whole application.
mod attention;
mod dialogue;
mod feedback;
mod inbox;
mod parking;
mod placement;

pub(crate) use inbox::Inbox;

use crate::{
    i18n::UiLanguage,
    ui::components::avatar::{Expression, Gaze, Motion, Pose, Presentation, Speech},
};
use attention::{Attention, Target};
use dialogue::{Cue, Route};
use eframe::egui::{self, Id, Pos2, Rect, Vec2};
use inbox::Message;
use std::{
    f32::consts::{PI, TAU},
    time::Duration,
};

pub(crate) struct OnboardingLayout {
    pub features: Option<[Rect; 3]>,
    pub header: Rect,
    pub content: Option<(Rect, egui::LayerId)>,
    pub next: Rect,
    pub footer: Rect,
    pub requirement: Option<&'static str>,
}

pub(crate) enum Layout {
    Onboarding(OnboardingLayout),
    Page { bounds: Rect, layer: egui::LayerId },
}

impl Default for OnboardingLayout {
    fn default() -> Self {
        Self {
            features: None,
            header: Rect::NOTHING,
            content: None,
            next: Rect::NOTHING,
            footer: Rect::NOTHING,
            requirement: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Peek,
    Greet,
    TurnAway,
    WalkAway,
    TurnBack,
    Ready,
}

struct Entrance {
    start: Pos2,
    greeting: Pos2,
    home: Pos2,
    radius: f32,
    resting_radius: f32,
}

struct EntranceFrame {
    center: Pos2,
    radius: f32,
    yaw: f32,
    roll: f32,
}

#[derive(Clone)]
struct Guide {
    stage: Stage,
    stage_started: f64,
    clock: f64,
    last_wall: f64,
    paused: bool,
    background_tick: Option<std::time::Instant>,
    language: UiLanguage,
    route: Route,
    cue: Cue,
    pending: Option<(Cue, f64)>,
    announced: u32,
    center: Pos2,
    velocity: Vec2,
    opacity: f32,
    relocating: bool,
    placement: placement::Placement,
    parking: parking::Parking,
    candidate: Option<Attention>,
    candidate_since: f64,
    focus: Option<Target>,
    resume_at: f64,
    last_activity: f64,
    last_spoken: f64,
    mentioned: u8,
    line: Message,
    speech: Speech,
    motion: Motion,
    feedback: feedback::Feedback,
}

impl Guide {
    fn new(
        now: f64,
        language: UiLanguage,
        start: Pos2,
        scene: &dialogue::Context,
        intro: bool,
    ) -> Self {
        Self {
            stage: if intro { Stage::Peek } else { Stage::Ready },
            stage_started: 0.0,
            clock: 0.0,
            last_wall: now,
            paused: true,
            background_tick: None,
            language,
            route: scene.route,
            cue: scene.cue,
            pending: (!intro && matches!(scene.route, Route::Onboarding(_)))
                .then_some((scene.cue, 0.0)),
            announced: 0,
            center: start,
            velocity: Vec2::ZERO,
            opacity: if intro { 1.0 } else { 0.0 },
            relocating: false,
            placement: placement::Placement::default(),
            parking: parking::Parking::default(),
            candidate: None,
            candidate_since: 0.0,
            focus: None,
            resume_at: 0.0,
            last_activity: 0.0,
            last_spoken: 0.0,
            mentioned: 0,
            line: scene.cue.text().into(),
            speech: Speech::default(),
            motion: Motion::default(),
            feedback: feedback::Feedback::default(),
        }
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.stage_started = self.clock;
    }

    fn welcome_frame(&mut self, entrance: &Entrance, playing: bool) -> Option<EntranceFrame> {
        let elapsed = (self.clock - self.stage_started) as f32;
        let mut frame = EntranceFrame {
            center: entrance.greeting,
            radius: entrance.radius,
            yaw: 0.0,
            roll: 0.0,
        };
        match self.stage {
            Stage::Peek => {
                let t = smooth((elapsed - 0.25) / 1.15);
                frame.center = entrance.start.lerp(entrance.greeting, t);
                frame.roll = -0.60 * (1.0 - smooth((t - 0.4) / 0.6));
                if playing && elapsed >= 1.4 {
                    self.enter(Stage::Greet);
                    self.announce(Cue::Hello);
                }
            }
            Stage::Greet => {
                if playing && self.speech.finished(self.clock) {
                    self.enter(Stage::TurnAway);
                }
            }
            Stage::TurnAway => {
                frame.yaw = -PI * smooth(elapsed / 0.7);
                if playing && elapsed >= 0.7 {
                    self.enter(Stage::WalkAway);
                }
            }
            Stage::WalkAway => {
                let t = (elapsed / 1.65).clamp(0.0, 1.0);
                let envelope = (PI * t).sin();
                frame.center = entrance.greeting.lerp(entrance.home, smooth(t));
                frame.center.y -= (t * PI * 3.0).sin().abs() * 3.0 * envelope;
                frame.radius = egui::lerp(entrance.radius..=entrance.resting_radius, smooth(t));
                frame.yaw = -PI;
                frame.roll = (t * TAU * 3.0).sin() * 0.045 * envelope;
                if playing && t >= 1.0 {
                    self.enter(Stage::TurnBack);
                }
            }
            Stage::TurnBack => {
                frame.center = entrance.home;
                frame.radius = entrance.resting_radius;
                frame.yaw = -PI - PI * smooth(elapsed / 0.75);
                if playing && elapsed >= 0.75 {
                    self.enter(Stage::Ready);
                    self.last_activity = self.clock;
                    frame.yaw = 0.0;
                }
            }
            Stage::Ready => return None,
        }
        Some(frame)
    }

    fn move_to(&mut self, target: Option<Pos2>, radius: f32, opacity: f32, dt: f32) {
        let dt = dt.clamp(0.0, 0.05);
        if dt == 0.0 {
            return;
        }
        let Some(target) = target else {
            // An occupied page is a reason to quietly disappear, not to keep
            // walking between temporary gaps or sit on top of a control.
            self.velocity = Vec2::ZERO;
            self.relocating = false;
            self.fade_to(0.0, dt);
            return;
        };
        let distance = self.center.distance(target);
        if self.opacity == 0.0 {
            self.center = target;
            self.velocity = Vec2::ZERO;
            self.relocating = false;
        } else if distance > radius * 0.75 {
            self.relocating = true;
        } else if distance <= 1.0 {
            self.relocating = false;
        }
        if self.relocating {
            self.velocity = Vec2::ZERO;
            self.fade_to(0.0, dt);
            if self.opacity == 0.0 {
                self.center = target;
                self.relocating = false;
                self.speech = self.speech.on_surface(&Speech::default());
            }
        } else {
            self.fade_to(opacity, dt);
            if self.center.distance(target) > 1.0 {
                placement::approach(&mut self.center, &mut self.velocity, target, dt);
            } else {
                self.velocity = Vec2::ZERO;
            }
        }
    }

    fn fade_to(&mut self, target: f32, dt: f32) {
        let duration = if target > self.opacity { 0.9 } else { 0.65 };
        self.opacity += (target - self.opacity).clamp(-dt / duration, dt / duration);
    }

    fn say(&mut self, line: impl Into<Message>) {
        self.line = line.into();
        self.last_spoken = self.clock;
        self.speech.say(self.line.text(self.language), self.clock);
    }

    fn pose(&mut self, ctx: &egui::Context, expression: Expression, gaze: Gaze) -> Pose {
        let expression =
            if !self.speech.finished(self.clock) && matches!(self.line, Message::Error(_)) {
                Expression::Concerned
            } else {
                expression
            };
        let mouth = self.speech.advance(self.clock);
        let pose = self.motion.advance(self.clock, expression, gaze, mouth);
        if !self.paused {
            ctx.request_repaint_after(self.motion.repaint_after(self.clock));
        }
        pose
    }

    fn follow_scene(&mut self, scene: &dialogue::Context) {
        let changed_page = self.route != scene.route;
        if changed_page {
            self.route = scene.route;
            self.velocity = Vec2::ZERO;
            self.parking = parking::Parking::default();
            self.enter(Stage::Ready);
            self.focus = None;
            self.candidate = None;
            self.candidate_since = self.clock;
            self.mentioned = 0;
        }
        if changed_page || self.cue != scene.cue {
            self.cue = scene.cue;
            self.speech.dismiss(self.clock);
            self.pending = (matches!(scene.route, Route::Onboarding(_))
                && self.announced & scene.cue.bit() == 0)
                .then_some((scene.cue, self.clock));
        }
    }

    fn announce(&mut self, cue: Cue) {
        self.announced |= cue.bit();
        self.say(cue.text());
    }

    fn announce_pending(&mut self) {
        if let Some((cue, since)) = self.pending
            && self.clock - since >= 1.2
        {
            self.pending = None;
            self.announce(cue);
        }
    }

    fn ready_to_speak(&self) -> bool {
        self.pending.is_none()
            && self.speech.finished(self.clock)
            && self.clock - self.last_spoken >= 3.0
    }

    fn advance_background(&mut self, now: std::time::Instant) -> f32 {
        let dt = self
            .background_tick
            .replace(now)
            .map_or(0.0, |last| now.duration_since(last).as_secs_f64().min(0.5));
        self.clock += dt;
        // A resumed paint pass must not count the hidden time again.
        self.paused = true;
        dt as f32
    }

    fn read_mail(&mut self, inbox: &mut Inbox) {
        if self.stage == Stage::Ready
            && self.ready_to_speak()
            && let Some(message) = inbox.read()
        {
            self.say(message);
        }
    }

    fn attend(&mut self, target: Option<Target>, clicked: bool, scene: &dialogue::Context) {
        let hovered = target.map(|target| target.attention);
        if hovered != self.candidate {
            self.candidate = hovered;
            self.candidate_since = self.clock;
        }
        let dwell = self.clock - self.candidate_since;
        if dwell >= 0.7 {
            self.focus = target;
        }
        if clicked && hovered == Some(Attention::Avatar) {
            self.focus = target;
        }
        if !self.ready_to_speak() {
            return;
        }
        if clicked && hovered == Some(Attention::Avatar) {
            self.say(scene.reply(Attention::Avatar));
        } else if let Some(topic) = hovered
            && dwell >= 2.0
            && topic.bit() != 0
            && self.mentioned & topic.bit() == 0
        {
            self.mentioned |= topic.bit();
            self.say(scene.reply(topic));
        }
    }
}

fn state_id() -> Id {
    Id::new("application_companion")
}

pub(crate) fn show(ctx: &egui::Context, app: &mut crate::XRTranslateApp, layout: Layout) {
    if desktop_active(app) {
        tick_background(ctx, app);
        return;
    }
    // Navigation can change while the old page is still being drawn. Wait for
    // the matching layout before consuming its companion entry transition.
    if app.first_run != matches!(&layout, Layout::Onboarding(_)) {
        ctx.request_repaint();
        return;
    }
    let (layout, mut page) = match layout {
        Layout::Onboarding(layout) => (Some(layout), None),
        Layout::Page { bounds, layer } => (None, Some((bounds, layer))),
    };
    let scene = dialogue::Context::read(app, layout.as_ref().and_then(|layout| layout.requirement));
    let language = app.ui_language;
    let screen = ctx.viewport_rect();
    // The first welcome keeps its lively entrance; daily use stays parked.
    let welcome = layout
        .as_ref()
        .and_then(|layout| {
            layout.features.map(|cards| {
                Rect::from_min_max(
                    egui::pos2(
                        screen.left() + 20.0,
                        cards.iter().map(Rect::bottom).fold(screen.top(), f32::max) + 14.0,
                    ),
                    egui::pos2(screen.right() - 20.0, layout.footer.top() - 10.0),
                )
            })
        })
        .filter(|rect| rect.height() >= 180.0);
    let bounds = welcome
        .or_else(|| {
            layout
                .as_ref()
                .map(|layout| layout.header)
                .filter(|rect| rect.is_finite() && rect.is_positive())
        })
        .unwrap_or(screen.shrink(12.0));
    if welcome.is_none() {
        page = page.or_else(|| layout.as_ref().and_then(|layout| layout.content));
    }
    let (wall, pointer, focused, pressed) = ctx.input(|input| {
        (
            input.time,
            input.pointer.hover_pos(),
            input.viewport().focused.unwrap_or(true),
            input.pointer.any_down(),
        )
    });
    let small = resting_radius(screen);
    let radius = (screen.width().min(screen.height()) * 0.067).clamp(28.0, 48.0);
    let greeting = egui::pos2(
        bounds.right() - radius * 2.6,
        bounds.bottom() - radius * 1.6,
    );
    let home = egui::pos2(
        bounds.right() - small * 1.6,
        if welcome.is_some() {
            bounds.top() + small * 1.8
        } else {
            bounds.bottom() - small * 2.0
        },
    );
    let entrance = Entrance {
        start: egui::pos2(screen.right() + radius * 1.2, greeting.y + 12.0),
        greeting,
        home,
        radius,
        resting_radius: small,
    };
    let mut state = ctx
        .data(|data| data.get_temp::<Guide>(state_id()))
        .unwrap_or_else(|| {
            Guide::new(
                wall,
                language,
                if welcome.is_some() {
                    entrance.start
                } else {
                    home
                },
                &scene,
                welcome.is_some(),
            )
        });
    let hidden = app.modal_dialog.open
        || ctx.memory(|memory| memory.top_modal_layer().is_some())
        || screen.height() < small * 4.0;
    let vr_active = app.vr_overlay_plugin.manager().companion_active();
    let interactive = !hidden && (focused || vr_active) && !ctx.any_popup_open();
    let paused = !interactive || pressed;
    let visual_dt = (wall - state.last_wall).clamp(0.0, 0.05) as f32;
    let elapsed_wall = if paused || state.paused {
        0.0
    } else {
        (wall - state.last_wall).clamp(0.0, 1.0)
    };
    state.last_wall = wall;
    state.background_tick = None;
    state.paused = paused;
    state.clock += elapsed_wall;
    state.follow_scene(&scene);
    if welcome.is_none() && state.stage != Stage::Ready {
        state.enter(Stage::Ready);
    }
    if state.language != language {
        state.language = language;
        if !state.speech.finished(state.clock) {
            state.say(state.line.clone());
        }
    }
    let dock = page.and_then(|(bounds, layer)| {
        state.parking.locate(
            ctx,
            layer,
            bounds,
            small,
            state.clock,
            app.background_image.texture_id(),
        )
    });
    let small = dock.map_or(small, |spot| spot.radius);
    let available = !hidden && (page.is_none() || dock.is_some() || state.clock < state.resume_at);
    if (available || (vr_active && !hidden)) && !paused && !ctx.text_edit_focused() {
        state.announce_pending();
        state.read_mail(&mut app.companion_inbox);
    }
    let previous_center = state.center;
    let mut entrance_frame = state.welcome_frame(&entrance, available && !paused);
    // Hovering controls may explain them, but never sends the avatar across the
    // page. Explicit placement survives the welcome conversation too.
    let target = if !available {
        None
    } else if paused || state.clock < state.resume_at {
        Some(state.center)
    } else if let Some(spot) = dock {
        Some(spot.center)
    } else if state.placement.was_dragged() {
        Some(state.center)
    } else {
        Some(home)
    };
    let size = state.placement.radius(
        entrance_frame.as_ref().map_or(small, |frame| frame.radius),
        small,
        visual_dt,
    );
    let target = if let Some(frame) = &entrance_frame {
        Some(frame.center)
    } else {
        target.map(|target| placement::constrain(target, size, screen))
    };
    let opacity = if entrance_frame.is_some()
        || !state.speech.finished(state.clock)
        || state.clock - state.last_activity < 3.0
    {
        1.0
    } else {
        0.6
    };
    if let Some(frame) = &entrance_frame {
        // Authored welcome movement stays visible; automatic daily relocation
        // continues to use the quiet fade-out/move/fade-in path below.
        state.center = frame.center;
        state.velocity = Vec2::ZERO;
        state.relocating = false;
        if !available {
            state.fade_to(0.0, visual_dt);
        } else if !paused {
            state.fade_to(1.0, visual_dt);
        }
    } else {
        // Pausing a relocation preserves its opacity until release.
        state.move_to(
            target,
            size,
            opacity,
            if paused && available { 0.0 } else { visual_dt },
        );
    }
    let mut anchor = state.center;
    let response = state.placement.interact(
        ctx,
        state_id(),
        &mut anchor,
        size,
        interactive && available && !state.relocating && state.opacity >= 0.25,
    );
    if state.stage == Stage::Ready {
        state.center = anchor;
    }
    if response.dragged_by(egui::PointerButton::Primary)
        || response.drag_stopped_by(egui::PointerButton::Primary)
    {
        if state.stage != Stage::Ready {
            state.enter(Stage::Ready);
        }
        state.center = anchor;
        state.velocity = Vec2::ZERO;
        state.relocating = false;
        state.fade_to(1.0, visual_dt);
        state.last_activity = state.clock;
        state.resume_at = state.clock + 1.2;
        state.focus = None;
        state.candidate = None;
        state.candidate_since = state.clock;
        if page.is_some() && response.drag_stopped_by(egui::PointerButton::Primary) {
            state.parking.dropped(anchor, size);
        }
        entrance_frame = None;
    }
    let focus = Target::read(ctx, layout.as_ref(), &response);
    let hovered = focus.map(|target| target.attention);
    if state.stage == Stage::Ready
        && !paused
        && !ctx.text_edit_focused()
        && state.clock >= state.resume_at
    {
        state.attend(focus, response.clicked(), &scene);
    } else {
        state.candidate_since = state.clock;
    }
    let attentive = available && interactive && response.hovered();
    if attentive || response.clicked() {
        state.last_activity = state.clock;
    }
    let gaze = if available && !paused {
        pointer.map_or(Gaze::default(), |point| {
            Gaze::toward(point - state.center, 420.0)
        })
    } else {
        Gaze::default()
    };
    let reaction = state.feedback.update(app, state.clock);
    let interaction = state
        .motion
        .interact(state.clock, attentive, response.clicked());
    let expression = if matches!(state.stage, Stage::Peek) || response.dragged() {
        Expression::Curious
    } else if state.stage == Stage::Greet {
        Expression::Happy
    } else {
        interaction.or(reaction).unwrap_or(Expression::Calm)
    };
    let mut pose = state.pose(ctx, expression, gaze);
    if let Some(frame) = entrance_frame {
        pose.gaze.yaw = frame.yaw;
        pose.roll = frame.roll;
    }

    let layer = egui::LayerId::new(egui::Order::Foreground, state_id());
    let speed = if visual_dt > 0.0 {
        anchor.distance(previous_center) / visual_dt
    } else {
        0.0
    };
    let mut painter = ctx.layer_painter(layer).with_clip_rect(screen);
    painter.multiply_opacity(smooth(state.opacity));
    if state.opacity > 0.0 {
        app.avatar_appearance
            .model()
            .paint(&painter, state_id(), anchor, size, pose);
    }
    let talking = state.speech.paint_avoiding(
        &painter,
        if state.stage == Stage::Ready {
            screen.shrink(12.0)
        } else {
            bounds
        },
        anchor,
        size,
        state.clock,
        visual_dt,
        state.parking.occupied(),
        speed < 12.0
            && target.is_none_or(|target| anchor.distance(target) < 2.0)
            && !response.dragged(),
    );
    if interactive
        && (response.dragged() || (state.placement.was_dragged() && (size - small).abs() > 0.1))
    {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    if !paused || state.opacity > 0.0 {
        // Sleep once settled; pointer events and Pose's scheduled blink wake us.
        let moving = state.stage != Stage::Ready
            || target.is_some_and(|target| state.center.distance(target) > 1.0);
        let dwelling = state.pending.is_some()
            || state.candidate != state.focus.map(|target| target.attention)
            || hovered.is_some_and(|topic| topic.bit() != 0 && state.mentioned & topic.bit() == 0);
        let fading = state.relocating
            || (state.opacity - if available { opacity } else { 0.0 }).abs() > 0.001;
        ctx.request_repaint_after(if moving || (talking && available) || fading {
            Duration::from_millis(16)
        } else if dwelling {
            Duration::from_millis(80)
        } else {
            Duration::from_secs(1)
        });
    }
    app.vr_overlay_plugin
        .manager()
        .present_companion(Presentation {
            appearance: app.avatar_appearance.clone(),
            pose,
            speech: state.speech.clone(),
            clock: state.clock,
            visible: !hidden,
        });
    if vr_active {
        ctx.request_repaint_after(Duration::from_millis(33));
    }
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
}

/// eframe's hidden-window logic hook runs without a paint pass. Keep the same
/// Guide/mailbox alive using a monotonic clock rather than frozen egui input time.
pub(crate) fn tick_background(ctx: &egui::Context, app: &mut crate::XRTranslateApp) {
    let desktop = desktop_active(app);
    let scene = dialogue::Context::read(app, None);
    let mut state = ctx
        .data(|data| data.get_temp::<Guide>(state_id()))
        .unwrap_or_else(|| Guide::new(0.0, app.ui_language, Pos2::ZERO, &scene, false));
    if !desktop && (!app.vr_overlay_plugin.manager().companion_active() || app.modal_dialog.open) {
        state.background_tick = None;
        app.vr_overlay_plugin
            .manager()
            .present_companion(Presentation::default());
        ctx.data_mut(|data| data.insert_temp(state_id(), state));
        return;
    }
    state.advance_background(std::time::Instant::now());
    state.follow_scene(&scene);
    if desktop && state.stage != Stage::Ready {
        state.enter(Stage::Ready);
    }
    if state.language != app.ui_language {
        state.language = app.ui_language;
        if !state.speech.finished(state.clock) {
            state.say(state.line.clone());
        }
    }
    state.announce_pending();
    state.read_mail(&mut app.companion_inbox);
    let expression = state
        .feedback
        .update(app, state.clock)
        .unwrap_or(Expression::Calm);
    let pose = state.pose(ctx, expression, Gaze::default());
    let presentation = Presentation {
        appearance: app.avatar_appearance.clone(),
        pose,
        speech: state.speech.clone(),
        clock: state.clock,
        visible: true,
    };
    if let Ok(mut overlay) = app.overlay_manager.lock() {
        overlay.present_companion(presentation.clone());
    }
    app.vr_overlay_plugin
        .manager()
        .present_companion(presentation);
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
    ctx.request_repaint_after(Duration::from_millis(33));
}

fn desktop_active(app: &crate::XRTranslateApp) -> bool {
    app.overlay_manager
        .lock()
        .is_ok_and(|mut overlay| overlay.companion_active())
}

fn resting_radius(screen: Rect) -> f32 {
    let edge = screen.width().min(screen.height());
    if cfg!(target_os = "android") {
        (edge * 0.067).clamp(28.0, 48.0) * 0.58
    } else {
        (edge * 0.052).clamp(24.0, 38.0) * 0.58
    }
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn welcome_guide() -> (Guide, Entrance) {
        let entrance = Entrance {
            start: egui::pos2(860.0, 420.0),
            greeting: egui::pos2(640.0, 400.0),
            home: egui::pos2(740.0, 280.0),
            radius: 48.0,
            resting_radius: 22.0,
        };
        let guide = Guide::new(
            0.0,
            UiLanguage::English,
            entrance.start,
            &dialogue::Context {
                route: Route::Onboarding(0),
                cue: Cue::Hello,
                blocked: None,
            },
            true,
        );
        (guide, entrance)
    }

    #[test]
    fn welcome_keeps_the_lively_sequence_and_settles_at_the_smaller_size() {
        let (mut guide, entrance) = welcome_guide();
        let first = guide.welcome_frame(&entrance, true).unwrap();
        assert_eq!(first.center, entrance.start);
        assert_eq!(first.radius, entrance.radius);
        assert!(first.roll < -0.5);
        assert_eq!(guide.opacity, 1.0);
        let mut stages = vec![guide.stage];
        let mut last = first;
        for tick in 1..=1800 {
            guide.clock = tick as f64 / 60.0;
            if let Some(frame) = guide.welcome_frame(&entrance, true) {
                last = frame;
            }
            guide.speech.advance(guide.clock);
            if stages.last() != Some(&guide.stage) {
                stages.push(guide.stage);
            }
        }
        assert_eq!(
            stages,
            vec![
                Stage::Peek,
                Stage::Greet,
                Stage::TurnAway,
                Stage::WalkAway,
                Stage::TurnBack,
                Stage::Ready
            ]
        );
        assert_eq!(last.center, entrance.home);
        assert_eq!(last.radius, entrance.resting_radius);
        assert_eq!(last.yaw, 0.0);
        assert_eq!(last.roll, 0.0);
        assert!(guide.welcome_frame(&entrance, true).is_none());
        assert_eq!(guide.announced & Cue::Hello.bit(), Cue::Hello.bit());
    }

    #[test]
    fn paused_welcome_does_not_announce_or_advance_the_sequence() {
        let (mut guide, entrance) = welcome_guide();
        guide.clock = 1.4;
        guide.welcome_frame(&entrance, false);
        assert_eq!(guide.stage, Stage::Peek);
        assert_eq!(guide.announced, 0);
        guide.welcome_frame(&entrance, true);
        assert_eq!(guide.stage, Stage::Greet);
        guide.speech.dismiss(guide.clock);
        guide.welcome_frame(&entrance, false);
        assert_eq!(guide.stage, Stage::Greet);
        guide.welcome_frame(&entrance, true);
        assert_eq!(guide.stage, Stage::TurnAway);
    }

    #[test]
    fn leaving_welcome_cancels_unfinished_entrance_motion() {
        let (mut guide, entrance) = welcome_guide();
        guide.enter(Stage::WalkAway);
        guide.follow_scene(&dialogue::Context {
            route: Route::Page(crate::ui::Page::Translation),
            cue: Cue::Translation,
            blocked: None,
        });
        assert_eq!(guide.stage, Stage::Ready);
        assert!(guide.welcome_frame(&entrance, true).is_none());
    }

    fn quiet_guide() -> Guide {
        Guide::new(
            0.0,
            UiLanguage::English,
            egui::pos2(80.0, 80.0),
            &dialogue::Context {
                route: Route::Page(crate::ui::Page::Translation),
                cue: Cue::Translation,
                blocked: None,
            },
            false,
        )
    }

    #[test]
    fn relocation_never_travels_while_visible() {
        let mut guide = quiet_guide();
        let origin = guide.center;
        let destination = egui::pos2(480.0, 320.0);
        guide.opacity = 1.0;
        for _ in 0..12 {
            guide.move_to(Some(destination), 20.0, 0.6, 0.05);
            assert_eq!(guide.center, origin);
            assert!(guide.opacity > 0.0);
        }
        for _ in 0..4 {
            guide.move_to(Some(destination), 20.0, 0.6, 0.05);
            if guide.center != origin {
                assert_eq!(guide.center, destination);
                assert_eq!(guide.opacity, 0.0);
                break;
            }
        }
        assert_eq!(guide.center, destination);
        guide.move_to(Some(destination), 20.0, 0.6, 0.05);
        assert!(guide.opacity > 0.0 && guide.opacity < 0.1);
        for _ in 0..60 {
            guide.move_to(Some(destination), 20.0, 0.6, 0.05);
        }
        assert_eq!(guide.opacity, 0.6);
        assert!(!guide.relocating);
    }

    #[test]
    fn unavailable_space_fades_out_and_returns_without_a_flash() {
        let mut guide = quiet_guide();
        guide.opacity = 0.6;
        let origin = guide.center;
        guide.move_to(None, 20.0, 1.0, 0.05);
        assert!(guide.opacity > 0.5 && guide.opacity < 0.6);
        assert_eq!(guide.center, origin);
        for _ in 0..20 {
            guide.move_to(None, 20.0, 1.0, 0.05);
        }
        assert_eq!(guide.opacity, 0.0);
        let destination = egui::pos2(400.0, 300.0);
        guide.move_to(Some(destination), 20.0, 0.6, 0.05);
        assert_eq!(guide.center, destination);
        assert!(guide.opacity < 0.1);
    }

    #[test]
    fn paused_relocation_resumes_without_an_opacity_pulse() {
        let mut guide = quiet_guide();
        guide.opacity = 0.6;
        let destination = egui::pos2(480.0, 320.0);
        guide.move_to(Some(destination), 20.0, 0.6, 0.05);
        let opacity = guide.opacity;
        let origin = guide.center;
        for _ in 0..60 {
            guide.move_to(Some(origin), 20.0, 0.6, 0.0);
        }
        assert_eq!(guide.opacity, opacity);
        assert_eq!(guide.center, origin);
        assert!(guide.relocating);
        guide.move_to(Some(destination), 20.0, 0.6, 0.05);
        assert!(guide.opacity < opacity);
        assert_eq!(guide.center, origin);
    }

    #[test]
    fn small_layout_jitter_does_not_move_a_resting_avatar() {
        let mut guide = quiet_guide();
        guide.opacity = 0.6;
        let origin = guide.center;
        for frame in 0..100 {
            let dx = if frame % 2 == 0 { 0.5 } else { -0.5 };
            guide.move_to(Some(origin + egui::vec2(dx, 0.0)), 20.0, 0.6, 0.05);
        }
        assert_eq!(guide.center, origin);
        assert_eq!(guide.opacity, 0.6);
        assert_eq!(guide.velocity, Vec2::ZERO);
    }

    #[test]
    fn returning_to_welcome_does_not_restart_the_entrance() {
        let mut guide = quiet_guide();
        guide.announced = Cue::Hello.bit();
        guide.opacity = 0.6;
        let origin = guide.center;
        guide.follow_scene(&dialogue::Context {
            route: Route::Onboarding(0),
            cue: Cue::Hello,
            blocked: None,
        });
        assert!(guide.stage == Stage::Ready);
        assert_eq!(guide.center, origin);
        assert_eq!(guide.opacity, 0.6);
        assert!(guide.pending.is_none());
    }

    #[test]
    fn mail_waits_for_current_speech_and_cooldown() {
        let scene = dialogue::Context {
            route: Route::Page(crate::ui::Page::Translation),
            cue: Cue::Translation,
            blocked: None,
        };
        let mut guide = Guide::new(0.0, UiLanguage::English, Pos2::ZERO, &scene, false);
        let mut inbox = Inbox::default();
        guide.clock = 2.9;
        assert!(!guide.ready_to_speak()); // Even an empty bubble respects the cooldown.
        guide.clock = 3.0;
        assert!(guide.ready_to_speak());
        guide.say("Select audio and start.");
        inbox.post("Translation session is not active. Please start translation first.");
        inbox.post("Translation session is not active. Please start translation first.");
        guide.clock = 6.0;
        assert!(!guide.ready_to_speak()); // The current sentence is still being spoken.
        guide.speech.advance(guide.clock);
        guide.clock = 20.0;
        assert!(guide.ready_to_speak());
        guide.read_mail(&mut inbox);
        assert_eq!(
            guide.line,
            Message::from("Translation session is not active. Please start translation first.")
        );
        assert_eq!(inbox.read(), None);
        assert!(!guide.ready_to_speak());
    }

    #[test]
    fn background_clock_reads_mail_without_a_paint_pass() {
        let scene = dialogue::Context {
            route: Route::Page(crate::ui::Page::Translation),
            cue: Cue::Translation,
            blocked: None,
        };
        let mut guide = Guide::new(123.0, UiLanguage::English, Pos2::ZERO, &scene, false);
        guide.say("Select audio and start.");
        let mut inbox = Inbox::default();
        let message = "Translation session is not active. Please start translation first.";
        inbox.post(message);
        inbox.post(message);
        let now = std::time::Instant::now();
        for tick in 0..=100 {
            guide.advance_background(now + Duration::from_millis(tick * 100));
            guide.read_mail(&mut inbox);
            guide.speech.advance(guide.clock);
        }
        assert_eq!(guide.line, Message::from(message));
        assert_eq!(inbox.read(), None);
        assert!((guide.clock - 10.0).abs() < 0.001);
        assert_eq!(guide.last_wall, 123.0); // No egui wall clock or paint needed.
        assert!(guide.paused); // Resuming painting skips its first elapsed interval.
        let before = guide.clock;
        guide.advance_background(now + Duration::from_secs(200));
        assert!((guide.clock - before - 0.5).abs() < 0.001);
    }

    #[test]
    fn background_errors_reach_speech_once_without_a_paint_pass() {
        let mut guide = quiet_guide();
        let mut inbox = Inbox::default();
        let detail = format!("Translation provider returned HTTP {}", 503);
        let now = std::time::Instant::now();
        for tick in 0..=600 {
            inbox.observe_error("session", Some(&detail));
            guide.advance_background(now + Duration::from_millis(tick * 100));
            guide.read_mail(&mut inbox);
            guide.speech.advance(guide.clock);
        }
        assert_eq!(guide.line, Message::Error(detail.clone()));
        assert!((guide.last_spoken - 3.0).abs() < 0.001);
        let mut expected = Speech::default();
        expected.say(format!("Something went wrong: {detail}"), guide.last_spoken);
        expected.advance(guide.clock);
        assert_eq!(guide.speech, expected);
        assert!(guide.speech.finished(guide.clock));
        assert_eq!(inbox.read(), None);

        // Re-localizing the prefix must retain the owned runtime reason.
        guide.language = UiLanguage::Chinese;
        guide.say(guide.line.clone());
        let mut localized = Speech::default();
        localized.say(
            format!(
                "{}: {detail}",
                crate::i18n::tr(UiLanguage::Chinese, "Something went wrong")
            ),
            guide.clock,
        );
        assert_eq!(guide.speech, localized);
        assert_eq!(guide.line, Message::Error(detail));
    }

    #[test]
    fn mail_waits_for_entrance_and_pending_dialogue() {
        let scene = dialogue::Context {
            route: Route::Page(crate::ui::Page::Translation),
            cue: Cue::Translation,
            blocked: None,
        };
        let mut guide = Guide::new(0.0, UiLanguage::English, Pos2::ZERO, &scene, true);
        let mut inbox = Inbox::default();
        inbox.post("Translation session is not active. Please start translation first.");
        guide.clock = 20.0;
        guide.read_mail(&mut inbox);
        assert_eq!(guide.line, Message::from(scene.cue.text()));
        guide.enter(Stage::Ready);
        guide.pending = Some((Cue::Hello, guide.clock));
        guide.read_mail(&mut inbox);
        assert_eq!(guide.line, Message::from(scene.cue.text()));
        guide.pending = None;
        guide.read_mail(&mut inbox);
        assert_eq!(
            guide.line,
            Message::from("Translation session is not active. Please start translation first.")
        );
        assert_eq!(inbox.read(), None);
    }
}
