//! Persistent inference preference with a live, non-blocking service update.
use crate::{i18n::tr, ui::components};
use crossbeam_channel::{TryRecvError, bounded};
use eframe::egui;
use std::time::Duration;

impl crate::XRTranslateApp {
    pub(crate) fn render_inference_mode_button(&mut self, ui: &mut egui::Ui) {
        let parallel = self.service_config.parallel_inference();
        let available = crate::backend::is_local_server(&self.server_url)
            && self.service_config.local_parallel_inference_available();
        let label = tr(
            self.ui_language,
            if parallel {
                "Parallel recognition and translation. Click to run one at a time."
            } else {
                "Recognition and translation run one at a time. Click to allow parallel inference."
            },
        );
        let response = ui
            .add_enabled_ui(
                available
                    && self.inference_mode_request.is_none()
                    && self.backend_start_deadline.is_none(),
                |ui| {
                    components::round_icon_button(
                        ui,
                        label,
                        xrtranslate_client_core::ui::icon_source(if parallel {
                            "rabbit.svg"
                        } else {
                            "turtle.svg"
                        }),
                        32.0,
                    )
                },
            )
            .inner;
        if !available {
            response.on_disabled_hover_text(tr(
                self.ui_language,
                "Requires local speech recognition and translation models.",
            ));
        } else if response.clicked() {
            if let Err(error) = self.set_inference_parallelism(!parallel, ui.ctx().clone()) {
                self.last_error = Some(error);
            }
        }
    }

    fn set_inference_parallelism(
        &mut self,
        enabled: bool,
        ctx: egui::Context,
    ) -> Result<(), String> {
        self.service_config.set_parallel_inference(enabled)?;
        let server = self.server_url.clone();
        let (sender, receiver) = bounded(1);
        std::thread::Builder::new()
            .name("inference-mode".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())
                    .and_then(|runtime| runtime.block_on(apply(&server, enabled)));
                let _ = sender.send(result);
                ctx.request_repaint();
            })
            .map_err(|error| error.to_string())?;
        self.inference_mode_request = Some(receiver);
        Ok(())
    }

    pub(crate) fn poll_inference_mode(&mut self) {
        let Some(receiver) = &self.inference_mode_request else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("Inference mode update stopped unexpectedly".into())
            }
        };
        self.inference_mode_request = None;
        if let Err(error) = result {
            self.last_error = Some(format!(
                "Inference preference was saved but could not be applied to the running service: {error}"
            ));
        }
    }
}

async fn apply(server: &str, enabled: bool) -> Result<(), String> {
    let mut url = reqwest::Url::parse(server).map_err(|error| error.to_string())?;
    let scheme = if matches!(url.scheme(), "wss" | "https") {
        "https"
    } else {
        "http"
    };
    url.set_scheme(scheme)
        .map_err(|_| "Invalid service address")?;
    url.set_path("/inference/parallel");
    url.set_query(None);
    let response = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|error| error.to_string())?
        .put(url)
        .json(&enabled)
        .send()
        .await;
    match response {
        Ok(response) => response
            .error_for_status()
            .map(|_| ())
            .map_err(|error| error.to_string()),
        Err(error) => {
            // A stopped service reads the saved preference when it next starts.
            let mut cause = std::error::Error::source(&error);
            while let Some(source) = cause {
                if source
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused)
                {
                    return Ok(());
                }
                cause = source.source();
            }
            Err(error.to_string())
        }
    }
}
