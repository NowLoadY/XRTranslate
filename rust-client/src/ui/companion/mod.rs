//! One persistent companion for the whole application.
//! Activity and attention enter the shared feedback policy; Motion and Speech
//! turn that decision into one Presentation. Desktop/VR surfaces own placement
//! and sensing, never a second conversation, reaction policy, or character clock.
mod attention;
mod dialogue;
mod feedback;
mod inbox;
mod parking;
mod placement;
mod reader;

pub(crate) use feedback::TranslationWork;
pub(crate) use inbox::Inbox;
pub(crate) use reader::{Command, Reader};

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
            pending: (!intro && scene.automatic).then_some((scene.cue, 0.0)),
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
            last_spoken: -3.0,
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
            if changed_page && matches!(self.line, Message::Localized(_)) {
                self.speech.dismiss(self.clock);
            }
            self.cue = scene.cue;
            self.mentioned &= !Attention::Next.bit();
            self.pending = (scene.automatic
                && (self.announced & scene.cue.bit() == 0
                    || (!changed_page && scene.cue.repeat_after_change())))
            .then_some((scene.cue, self.clock));
        }
        if !scene.automatic {
            self.pending = None;
        } else if self.stage == Stage::Ready
            && self.pending.is_none()
            && self.announced & scene.cue.bit() == 0
        {
            self.pending = Some((scene.cue, self.clock));
        }
    }

    fn announce(&mut self, cue: Cue) {
        self.announced |= cue.bit();
        self.say(cue.text());
    }

    fn announce_pending(&mut self) {
        if let Some((cue, since)) = self.pending
            && self.clock - since >= 1.2
            && self.speech.finished(self.clock)
            && self.clock - self.last_spoken >= 3.0
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
        if let Some(message) = inbox.read_acknowledgement() {
            self.pending = None;
            self.say(message);
        } else if self.stage == Stage::Ready
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
            let line = scene.reply(topic);
            if self.line != Message::Localized(line) {
                self.say(line);
            }
        }
    }
}

fn state_id() -> Id {
    Id::new("application_companion")
}

pub(crate) fn process_commands(app: &mut crate::XRTranslateApp) {
    let commands = app
        .shared_session_state
        .lock()
        .map(|mut state| state.companion_reader.take_commands())
        .unwrap_or_default();
    for command in commands {
        execute_command(app, command);
    }
}

/// Voice and UI actions share the same application-level controls and replies.
pub(crate) fn execute_command(app: &mut crate::XRTranslateApp, command: Command) {
    match command {
        Command::HideSubtitles | Command::ShowSubtitles => {
            app.vr_overlay_plugin.draft_mut().captions_enabled = command == Command::ShowSubtitles;
            app.vr_overlay_plugin.sync_settings();
            app.save_settings();
        }
        Command::ComeHere | Command::ReturnHome => {
            let manager = app.vr_overlay_plugin.manager();
            if !manager.status().avatar_present {
                return;
            }
            if command == Command::ComeHere {
                manager.visit_avatar();
            } else {
                manager.return_avatar();
            }
        }
    }
    app.companion_inbox.acknowledge(command);
}

pub(crate) fn show(ctx: &egui::Context, app: &mut crate::XRTranslateApp, layout: Layout) {
    let vr = app.vr_overlay_plugin.manager().status();
    if desktop_active(app) || vr.avatar_present {
        tick_background(ctx, app);
        return;
    }
    // Navigation can change while the old page is still being drawn. Wait for
    // the matching layout before consuming its companion entry transition.
    if app.first_run != matches!(&layout, Layout::Onboarding(_)) {
        ctx.request_repaint();
        return;
    }
    let (layout, page) = match layout {
        Layout::Onboarding(layout) => (Some(layout), None),
        Layout::Page { bounds, layer } => (None, Some((bounds, layer))),
    };
    let scene = dialogue::Context::read(app, layout.as_ref());
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
        } else if layout.is_some() {
            bounds.center().y
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
    let vr_active = vr.avatar_available;
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
    let reaction = state
        .feedback
        .update(app, state.clock, attentive, response.clicked());
    let expression = if matches!(state.stage, Stage::Peek) || response.dragged() {
        Expression::Curious
    } else if state.stage == Stage::Greet {
        Expression::Happy
    } else {
        reaction.unwrap_or(Expression::Calm)
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
    // The desktop remains until VR has actually presented a frame. Its pointer
    // and authored entrance are local placement, not the spatial avatar's pose.
    pose.gaze = Gaze::default();
    pose.roll = 0.0;
    app.vr_overlay_plugin
        .manager()
        .present_companion(Presentation {
            appearance: app.avatar_appearance.clone(),
            pose,
            speech: state.speech.clone(),
            clock: state.clock,
            visible: vr_active,
            motion: Some(state.motion.on_surface(pose)),
        });
    if vr_active {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
}

/// eframe's hidden-window logic hook runs without a paint pass. Keep the same
/// Guide/mailbox alive using a monotonic clock rather than frozen egui input time.
pub(crate) fn tick_background(ctx: &egui::Context, app: &mut crate::XRTranslateApp) {
    let desktop = desktop_active(app);
    let vr = app.vr_overlay_plugin.manager().status();
    let scene = dialogue::Context::read(app, None);
    let mut state = ctx
        .data(|data| data.get_temp::<Guide>(state_id()))
        .unwrap_or_else(|| Guide::new(0.0, app.ui_language, Pos2::ZERO, &scene, false));
    if !desktop && !vr.avatar_available {
        state.background_tick = None;
        app.vr_overlay_plugin
            .manager()
            .present_companion(Presentation::default());
        ctx.data_mut(|data| data.insert_temp(state_id(), state));
        return;
    }
    state.advance_background(std::time::Instant::now());
    if vr.avatar_available {
        // Desktop navigation and modal focus must not interrupt spatial speech.
        state.pending = None;
    } else {
        state.follow_scene(&scene);
    }
    if state.stage != Stage::Ready {
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
        .update(app, state.clock, vr.avatar_attention, false)
        .unwrap_or(Expression::Calm);
    let pose = state.pose(ctx, expression, Gaze::default());
    // Background logic skips Pose's foreground repaint request. Keep gestures,
    // blinks and speech smooth without polling a resting face at frame rate.
    let repaint_after =
        state
            .motion
            .repaint_after(state.clock)
            .min(if state.speech.finished(state.clock) {
                Duration::from_millis(33)
            } else {
                Duration::from_millis(16)
            });
    let presentation = Presentation {
        appearance: app.avatar_appearance.clone(),
        pose,
        speech: state.speech.clone(),
        clock: state.clock,
        visible: true,
        motion: Some(state.motion.clone()),
    };
    if desktop && let Ok(mut overlay) = app.overlay_manager.lock() {
        overlay.present_companion(if vr.avatar_present {
            // A constant hidden snapshot lets the existing IPC writer sleep
            // instead of waking the desktop window for every VR face update.
            Presentation::default()
        } else {
            presentation.clone()
        });
    }
    app.vr_overlay_plugin
        .manager()
        .present_companion(presentation);
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
    ctx.request_repaint_after(repaint_after);
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
