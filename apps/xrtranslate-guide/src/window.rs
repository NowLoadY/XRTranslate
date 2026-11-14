use crate::{
    labels::{local, text},
    remote::Error,
    worker::{Event, Task, Worker},
};
use eframe::egui::{self, Align, Color32, Layout, RichText};
use std::path::PathBuf;
use ui_director::egui::{self as director, Control, ElementKind, ElementValue};
use xrt_guide::{Narrator, Planner, guide::Context};

pub struct Window {
    planner: Planner,
    narrator: Narrator,
    context: Option<Context>,
    worker: Worker,
    tools: director::tools::UiTools,
    language: String,
    client: Option<PathBuf>,
    busy: bool,
    error: Option<Error>,
    shown_page: Option<String>,
}

impl Window {
    pub fn new(port: u16, client: Option<PathBuf>, ctx: egui::Context, language: &str) -> Self {
        let client = client.or_else(sibling_client);
        let worker = Worker::new(port, ctx);
        worker.submit(Task::Connect(client.clone()));
        Self {
            planner: Planner::default(),
            narrator: Narrator::default(),
            context: None,
            worker,
            tools: Default::default(),
            language: language.into(),
            client,
            busy: true,
            error: None,
            shown_page: None,
        }
    }
    fn present(&mut self) {
        self.narrator.transition();
        self.error = None;
        if let Some((chapter, step)) = self
            .planner
            .state
            .session
            .as_ref()
            .and_then(|s| s.current())
        {
            self.worker
                .submit(Task::Present(chapter.page.clone(), step.clone()));
            self.busy = true;
        } else {
            self.narrator.stop();
            self.worker.cancel();
            self.busy = false;
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        for event in self.worker.poll() {
            if !matches!(&event, Ok(Event::Selected(_))) {
                self.busy = false;
            }
            match event {
                Ok(Event::Connected(context)) => {
                    self.language = context.language.clone();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                        text("User Guide", &self.language).into(),
                    ));
                    self.planner.update(context.clone());
                    self.context = Some(context);
                    if self.planner.start(None).is_ok() {
                        self.present();
                    } else {
                        self.error = Some(Error::Incompatible);
                    }
                }
                Ok(Event::Selected(mut fresh)) => {
                    fresh.capabilities = self
                        .context
                        .as_ref()
                        .map(|context| context.capabilities.clone())
                        .unwrap_or_default();
                    self.language = fresh.language.clone();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(
                        text("User Guide", &self.language).into(),
                    ));
                    if let Some((chapter, _)) = self
                        .planner
                        .state
                        .session
                        .as_ref()
                        .and_then(|s| s.current())
                    {
                        fresh.page = chapter.page.clone();
                    }
                    self.planner.update(fresh.clone());
                    self.context = Some(fresh);
                    self.narrator.sync(&self.planner.state);
                }
                Ok(Event::Presented(editor)) => {
                    if let Some(context) = &mut self.context {
                        if let Some((chapter, _)) = self
                            .planner
                            .state
                            .session
                            .as_ref()
                            .and_then(|s| s.current())
                        {
                            context.page = chapter.page.clone();
                        }
                        if let Some(allowed) = editor {
                            context.capabilities = if allowed {
                                vec!["prompt_editor".into()]
                            } else {
                                vec![]
                            };
                        }
                        self.planner.update(context.clone());
                    }
                    self.planner.presented(self.planner.state.revision);
                }
                Err(error) => {
                    self.narrator.stop();
                    self.error = Some(error);
                    if matches!(error, Error::Disconnected | Error::Incompatible) {
                        self.context = None;
                        self.planner.stop();
                    } else if self
                        .planner
                        .state
                        .session
                        .as_ref()
                        .and_then(|s| s.current())
                        .is_some_and(|(_, step)| step.id == "edit-prompts")
                    {
                        if let Some(context) = &mut self.context {
                            context.capabilities.clear();
                            self.planner.update(context.clone());
                        }
                    }
                }
            }
        }
    }
    fn reconnect(&mut self) {
        self.narrator.stop();
        self.planner.stop();
        self.error = None;
        self.busy = true;
        self.worker.submit(Task::Connect(self.client.clone()));
    }
    fn panel(&mut self, ui: &mut egui::Ui) {
        let lang = self.language.clone();
        if let Some(session) = self.planner.state.session.clone() {
            let chapter = &session.chapters[session.chapter];
            let step = &chapter.steps[session.step];
            let follow_selection = self.shown_page.as_deref() != Some(&chapter.page);
            let mut selection_visible = false;
            let mut selected_page = None;
            egui::ScrollArea::horizontal()
                .id_salt("guide_pages")
                .auto_shrink([false, true])
                .animated(false)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        for (index, page) in session.chapters.iter().enumerate() {
                            let selected = index == session.chapter;
                            let accent = Color32::from_rgb(46, 98, 220);
                            let response = ui.add(
                                egui::Button::new(RichText::new(&page.label).size(14.0).color(
                                    if selected {
                                        accent
                                    } else {
                                        Color32::from_rgb(76, 80, 88)
                                    },
                                ))
                                .corner_radius(18)
                                .min_size(egui::vec2(0.0, 36.0))
                                .fill(if selected {
                                    Color32::from_rgb(230, 238, 255)
                                } else {
                                    Color32::from_rgb(241, 242, 245)
                                })
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if selected {
                                        Color32::from_rgb(173, 195, 245)
                                    } else {
                                        Color32::from_rgb(229, 231, 235)
                                    },
                                )),
                            );
                            let response = Control::button(&page.label).response(ui, response);
                            if selected {
                                selection_visible = ui.clip_rect().contains_rect(response.rect);
                                if follow_selection && !selection_visible {
                                    response.scroll_to_me(Some(Align::Center));
                                }
                            }
                            if response.clicked() && !selected {
                                selected_page = Some(page.page.clone());
                            }
                        }
                    });
                });
            if selection_visible && !ui.ctx().will_discard() {
                self.shown_page = Some(chapter.page.clone());
            }
            if let Some(page) = selected_page {
                let _ = self.planner.select(&page, None);
                self.present();
            }
            ui.add_space(16.0);
            let body_height = (ui.available_height() - 58.0).max(60.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), body_height),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_min_height(body_height);
                    egui::ScrollArea::vertical()
                        .max_height(body_height)
                        .show(ui, |ui| {
                            let response = ui.add(
                                egui::Label::new(
                                    RichText::new(&step.text)
                                        .size(21.0)
                                        .color(Color32::from_rgb(45, 50, 59)),
                                )
                                .wrap(),
                            );
                            Control::new(
                                "Guide narration",
                                ElementKind::Other,
                                ElementValue::Text(step.text.clone()),
                            )
                            .record(
                                ui,
                                response.id,
                                true,
                                response.rect,
                            );
                        });
                },
            );
            ui.horizontal(|ui| {
                let previous = ui.add_enabled(
                    session.chapter > 0 || session.step > 0,
                    egui::Button::new(RichText::new("‹").size(25.0))
                        .min_size(egui::vec2(42.0, 38.0)),
                );
                let previous = Control::button("Previous feature")
                    .response(ui, previous)
                    .on_hover_text(text("Previous feature", &lang));
                if previous.clicked() {
                    let _ = self.planner.advance(false);
                    self.present();
                }
                ui.label(
                    RichText::new(format!("{} / {}", session.step + 1, chapter.steps.len()))
                        .size(13.0)
                        .color(Color32::from_rgb(125, 131, 141)),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let next = ui.add(
                        egui::Button::new(
                            RichText::new(format!("{}  ›", text("Next feature", &lang)))
                                .color(Color32::WHITE),
                        )
                        .fill(Color32::from_rgb(46, 98, 220))
                        .min_size(egui::vec2(124.0, 38.0)),
                    );
                    if Control::button("Next feature").response(ui, next).clicked() {
                        let _ = self.planner.advance(true);
                        self.present();
                    }
                    if self.busy {
                        spinner(ui);
                    }
                });
            });
            if let Some(error) = self.error {
                show_error(ui, error, &lang);
            }
        } else {
            ui.vertical_centered(|ui| {
                ui.add_space(36.0);
                if self.busy {
                    spinner(ui);
                } else if let Some(error) = self.error {
                    show_error(ui, error, &lang);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if button(
                            ui,
                            local(
                                &lang,
                                "Open client",
                                "打开客户端",
                                "クライアントを開く",
                                "클라이언트 열기",
                                "Открыть клиент",
                            ),
                            "Open client",
                        ) {
                            if let Some(path) = rfd::FileDialog::new()
                                .add_filter("XRTranslate", &["exe"])
                                .pick_file()
                            {
                                self.client = Some(path);
                                self.reconnect();
                            }
                        }
                        if button(
                            ui,
                            local(&lang, "Retry", "重试", "再試行", "다시 시도", "Повторить"),
                            "Retry",
                        ) {
                            self.reconnect();
                        }
                    });
                } else if button(
                    ui,
                    local(
                        &lang,
                        "Start again",
                        "重新开始",
                        "最初から",
                        "다시 시작",
                        "Начать заново",
                    ),
                    "Start again",
                ) {
                    let _ = self.planner.start(None);
                    self.present();
                }
            });
        }
    }
}

impl eframe::App for Window {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        director::raw_input(ctx, input);
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        director::begin_frame(ui.ctx(), "Guide", vec![]);
        for request in director::driver().take_host_requests() {
            if let director::driver::Command::Call(call) = request.command {
                let response = match ui_director::dispatch(&mut [&mut self.tools], &call) {
                    Ok(value) => ui_director::Response::ok("Completed", Some(value)),
                    Err(error) => ui_director::Response::err(error),
                };
                let _ = request.responder.send(response);
            }
        }
        self.poll(ui.ctx());
        if !self.busy && self.error.is_none() {
            self.narrator.sync(&self.planner.state);
        }
        // The page strip follows lesson selection, not inspector reads of its controls.
        director::feedback::dismiss_reveal(ui.ctx());
        if ui.ctx().input(|input| {
            input.key_pressed(egui::Key::Escape) || input.viewport().close_requested()
        }) {
            self.worker.cancel();
            self.narrator.stop();
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        egui::Frame::central_panel(ui.style())
            .inner_margin(24)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.panel(ui);
            });
        director::finish_frame("Guide");
    }
}

fn button(ui: &mut egui::Ui, label: &str, key: &str) -> bool {
    let response = ui.button(label);
    Control::button(key).response(ui, response).clicked()
}

fn spinner(ui: &mut egui::Ui) {
    let response = ui.spinner();
    Control::new("Preparing", ElementKind::Other, ElementValue::Bool(true)).record(
        ui,
        response.id,
        true,
        response.rect,
    );
}

fn show_error(ui: &mut egui::Ui, error: Error, language: &str) {
    let message = error_text(error, language);
    let response = ui.colored_label(Color32::from_rgb(170, 65, 65), message);
    Control::new(
        "Guide error",
        ElementKind::Other,
        ElementValue::Text(message.into()),
    )
    .record(ui, response.id, true, response.rect);
}

fn error_text(error: Error, lang: &str) -> &str {
    match error {
        Error::Incompatible => local(
            lang,
            "This client is not supported.",
            "此客户端不兼容。",
            "このクライアントには対応していません。",
            "지원하지 않는 클라이언트입니다.",
            "Этот клиент не поддерживается.",
        ),
        Error::Unavailable => local(
            lang,
            "This feature is unavailable.",
            "此功能当前不可用。",
            "この機能は現在使えません。",
            "현재 이 기능을 사용할 수 없습니다.",
            "Эта функция сейчас недоступна.",
        ),
        Error::AlreadyRunning => local(
            lang,
            "Close the client and retry.",
            "请先退出客户端，再重试。",
            "クライアントを閉じて再試行してください。",
            "클라이언트를 종료한 후 다시 시도하세요.",
            "Закройте клиент и повторите.",
        ),
        Error::Launch => local(
            lang,
            "Could not open the client.",
            "无法打开客户端。",
            "クライアントを開けません。",
            "클라이언트를 열 수 없습니다.",
            "Не удалось открыть клиент.",
        ),
        _ => local(
            lang,
            "Client not connected.",
            "未连接客户端。",
            "クライアントに接続していません。",
            "클라이언트에 연결되지 않았습니다.",
            "Клиент не подключён.",
        ),
    }
}

fn sibling_client() -> Option<PathBuf> {
    let directory = std::env::current_exe().ok()?.parent()?.to_path_buf();
    if let Ok(bytes) = std::fs::read(directory.join("release-manifest.json")) {
        let manifest: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let path = directory.join(manifest["entrypoints"]["client"].as_str()?);
        if path.is_file() {
            return Some(path);
        }
    }
    ["XRTranslate.exe", "rust-client.exe"]
        .into_iter()
        .map(|name| directory.join(name))
        .find(|path| path.is_file())
}
