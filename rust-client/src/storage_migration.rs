//! A small startup window; no backend or plugin opens storage during a move.
use crate::i18n::{UiLanguage, tr};
use eframe::egui;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub fn startup_request(args: &[String]) -> eframe::Result<Option<PathBuf>> {
    let Some(index) = args.iter().position(|arg| arg == "--move-user-data") else {
        return Ok(None);
    };
    let target = args
        .get(index + 1)
        .ok_or_else(|| error("Missing data folder."))?;
    if let Some(pid) = args
        .iter()
        .position(|arg| arg == "--previous-pid")
        .and_then(|index| args.get(index + 1))
        .and_then(|pid| pid.parse::<u32>().ok())
    {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::{
                Foundation::{CloseHandle, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0},
                System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
            };
            match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
                Ok(process) => {
                    let result = WaitForSingleObject(process, 60_000);
                    let _ = CloseHandle(process);
                    if result != WAIT_OBJECT_0 {
                        return Err(error("XRTranslate is still running."));
                    }
                }
                Err(failure) if failure.code() == ERROR_INVALID_PARAMETER.to_hresult() => {}
                Err(failure) => return Err(error(failure)),
            }
        }
        #[cfg(target_os = "linux")]
        {
            let process = Path::new("/proc").join(pid.to_string());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
            while process.exists() {
                if std::time::Instant::now() >= deadline {
                    return Err(error("XRTranslate is still running."));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
    Ok(Some(PathBuf::from(target)))
}

#[derive(Default)]
struct Progress {
    fraction: f32,
    result: Option<Result<(), String>>,
}

struct MoveWindow {
    target: PathBuf,
    language: UiLanguage,
    progress: Arc<Mutex<Progress>>,
}

impl eframe::App for MoveWindow {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let progress = self.progress.lock().unwrap();
        if progress.result.is_none() && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        egui::CentralPanel::default().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(20.0);
                ui.vertical_centered(|ui| {
                    ui.heading("XRTranslate");
                    ui.add_space(18.0);
                    if progress.result.is_none() {
                        ui.add(egui::Spinner::new().size(24.0));
                    }
                    ui.label(tr(
                        self.language,
                        if matches!(progress.result, Some(Err(_))) {
                            "Could not move user data."
                        } else {
                            "Moving user data…"
                        },
                    ));
                    ui.add(egui::Label::new(self.target.display().to_string()).wrap());
                    ui.add_space(18.0);
                    ui.add(egui::ProgressBar::new(progress.fraction).show_percentage());
                    if let Some(Err(error)) = &progress.result {
                        ui.add_space(12.0);
                        ui.add(egui::Label::new(error).wrap());
                        if crate::ui::components::primary_button(
                            ui,
                            tr(self.language, "Open XRTranslate"),
                        )
                        .clicked()
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }
                });
            });
        });
        if matches!(progress.result, Some(Ok(()))) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

pub fn run(root: &Path, target: &Path) -> eframe::Result<()> {
    let root = root.to_path_buf();
    let target = target.to_path_buf();
    let language = crate::client_settings::ClientSettings::load(&root).ui_language;
    let icon = eframe::icon_data::from_png_bytes(include_bytes!(
        "../resources/branding/xrtranslate-logo.png"
    ))
    .unwrap();
    eframe::run_native(
        "XRTranslate",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([560.0, 300.0])
                .with_min_inner_size([480.0, 240.0])
                .with_icon(icon),
            ..Default::default()
        },
        Box::new(move |cc| {
            crate::ui::fonts::configure_multilingual_fonts(&cc.egui_ctx);
            crate::ui::theme::apply_theme(&cc.egui_ctx);
            let progress = Arc::new(Mutex::new(Progress::default()));
            let state = progress.clone();
            let destination = target.clone();
            let context = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let result =
                    xrtranslate_config::storage_migration::DataMove::new(&root, &destination)
                        .and_then(|plan| {
                            plan.execute(|fraction| {
                                state.lock().unwrap().fraction = fraction;
                                context.request_repaint();
                            })
                        })
                        .map_err(|error| error.to_string());
                state.lock().unwrap().result = Some(result);
                context.request_repaint();
            });
            Ok(Box::new(MoveWindow {
                target,
                language,
                progress,
            }))
        }),
    )
}

fn error(value: impl ToString) -> eframe::Error {
    eframe::Error::AppCreation(value.to_string().into())
}
