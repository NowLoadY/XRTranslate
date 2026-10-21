use crate::{
    i18n::{self, UiLanguage},
    ui::theme,
};
use eframe::egui;
use xrtranslate_protocol::AudioPreprocessing;

#[derive(Clone, Debug, Default)]
pub enum AudioPreprocessingModelState {
    #[default]
    Missing,
    Checking,
    Queued,
    Downloading {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Paused {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    Ready,
    Failed(String),
}

impl AudioPreprocessingModelState {
    fn progress(&self) -> Option<f32> {
        match self {
            Self::Downloading {
                downloaded_bytes,
                total_bytes,
            }
            | Self::Paused {
                downloaded_bytes,
                total_bytes,
            } => Some(if *total_bytes == 0 {
                0.0
            } else {
                (*downloaded_bytes as f64 / *total_bytes as f64).clamp(0.0, 1.0) as f32
            }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AudioPreprocessingModels {
    pub gtcrn: AudioPreprocessingModelState,
    pub vocal: AudioPreprocessingModelState,
    pub can_delete: bool,
    pub use_mirror: bool,
}

impl AudioPreprocessingModels {
    pub fn state(&self, mode: AudioPreprocessing) -> Option<&AudioPreprocessingModelState> {
        match mode {
            AudioPreprocessing::None => None,
            AudioPreprocessing::Gtcrn => Some(&self.gtcrn),
            AudioPreprocessing::VocalSeparation => Some(&self.vocal),
        }
    }

    pub fn is_ready(&self, mode: AudioPreprocessing) -> bool {
        self.state(mode)
            .is_none_or(|state| matches!(state, AudioPreprocessingModelState::Ready))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioPreprocessingAction {
    Download(AudioPreprocessing),
    Pause(AudioPreprocessing),
    Resume(AudioPreprocessing),
    Delete(AudioPreprocessing),
    SetMirror(bool),
}

pub struct AudioPreprocessingResponse {
    pub response: egui::Response,
    pub action: Option<AudioPreprocessingAction>,
}

/// Keep the normal button label legible above its download progress.
pub(crate) fn download_progress_button(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    enabled: bool,
    progress: Option<f32>,
) -> egui::Response {
    download_progress_button_with_loading(ui, label, selected, enabled, progress, false)
}

fn download_progress_button_with_loading(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    enabled: bool,
    progress: Option<f32>,
    loading: bool,
) -> egui::Response {
    let background = ui.painter().add(egui::Shape::Noop);
    let fill = ui.painter().add(egui::Shape::Noop);
    let spinner_id = ui.make_persistent_id("download_spinner");
    let mut atoms = egui::Atoms::new(label);
    if loading {
        atoms.push_left(egui::Atom::custom(spinner_id, egui::vec2(14.0, 14.0)));
    }
    let button = ui
        .add_enabled_ui(enabled, |ui| {
            egui::Button::new(atoms)
                .selected(selected)
                .fill(egui::Color32::TRANSPARENT)
                .gap(6.0)
                .min_size(egui::vec2(ui.available_width(), 30.0))
                .atom_ui(ui)
        })
        .inner;
    if let Some(rect) = button.rect(spinner_id) {
        egui::Spinner::new()
            .color(theme::text_normal())
            .paint_at(ui, rect);
    }
    let response = button.response;
    let visuals = ui.style().interact_selectable(&response, selected);
    ui.painter().set(
        background,
        egui::Shape::rect_filled(response.rect, visuals.corner_radius, visuals.bg_fill),
    );
    if let Some(progress) = progress {
        let mut rect = response.rect;
        rect.max.x = rect.min.x + rect.width() * progress.clamp(0.0, 1.0);
        ui.painter().set(
            fill,
            egui::Shape::rect_filled(
                rect,
                visuals.corner_radius,
                theme::primary().gamma_multiply(0.18),
            ),
        );
    }
    crate::ui::automation::button_response(
        ui,
        response.id,
        label,
        enabled && ui.is_enabled(),
        response,
    )
}

/// Per-input selection and resource actions; the host owns download lifecycle.
pub(crate) fn audio_preprocessing_selector(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash + std::fmt::Debug,
    selection: &mut Option<AudioPreprocessing>,
    default: AudioPreprocessing,
    allow_vocal_separation: bool,
    models: &AudioPreprocessingModels,
    language: UiLanguage,
) -> AudioPreprocessingResponse {
    let labels = match language {
        UiLanguage::English => [
            "Audio processing",
            "None",
            "Speech denoising",
            "Vocal separation",
            "Available for media files",
        ],
        UiLanguage::Chinese => ["音频处理", "无", "语音降噪", "人声分离", "适用于媒体文件"],
        UiLanguage::Japanese => [
            "音声処理",
            "なし",
            "音声ノイズ除去",
            "ボーカル分離",
            "メディアファイルで利用できます",
        ],
        UiLanguage::Korean => [
            "오디오 처리",
            "없음",
            "음성 잡음 제거",
            "보컬 분리",
            "미디어 파일에서 사용할 수 있습니다",
        ],
        UiLanguage::Russian => [
            "Обработка звука",
            "Нет",
            "Подавление шума речи",
            "Выделение вокала",
            "Доступно для медиафайлов",
        ],
    };
    let choices = [
        (AudioPreprocessing::None, labels[1]),
        (AudioPreprocessing::Gtcrn, labels[2]),
        (AudioPreprocessing::VocalSeparation, labels[3]),
    ];
    let selected = selection.unwrap_or(default);
    let selected_label = choices
        .iter()
        .find_map(|(value, label)| (*value == selected).then_some(*label))
        .unwrap_or(labels[1]);
    let before = *selection;
    let mut action = None;
    let mut response = ui
        .horizontal_wrapped(|ui| {
            ui.label(labels[0]);
            egui::ComboBox::from_id_salt(id_salt)
                .selected_text(selected_label)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show_ui(ui, |ui| {
                    ui.set_min_width(190.0);
                    for (index, (value, label)) in choices.into_iter().enumerate() {
                        let available =
                            allow_vocal_separation || value != AudioPreprocessing::VocalSeparation;
                        let state = models
                            .state(value)
                            .unwrap_or(&AudioPreprocessingModelState::Ready);
                        ui.push_id(index, |ui| {
                            ui.horizontal(|ui| {
                                let deletable = value != AudioPreprocessing::None
                                    && matches!(state, AudioPreprocessingModelState::Ready);
                                let width = ui.available_width()
                                    - if deletable {
                                        32.0 + ui.spacing().item_spacing.x
                                    } else {
                                        0.0
                                    };
                                let response = ui
                                    .allocate_ui_with_layout(
                                        egui::vec2(width, 32.0),
                                        egui::Layout::left_to_right(egui::Align::Center),
                                        |ui| {
                                            download_progress_button_with_loading(
                                                ui,
                                                label,
                                                selected == value,
                                                available,
                                                state.progress(),
                                                available
                                                    && matches!(
                                                        state,
                                                        AudioPreprocessingModelState::Checking
                                                            | AudioPreprocessingModelState::Queued
                                                            | AudioPreprocessingModelState::Downloading { .. }
                                                    ),
                                            )
                                        },
                                    )
                                    .inner;
                                if !available {
                                    response.on_disabled_hover_text(labels[4]);
                                } else {
                                    let hint = match state {
                                        AudioPreprocessingModelState::Downloading { .. }
                                        | AudioPreprocessingModelState::Queued => "Pause",
                                        AudioPreprocessingModelState::Paused { .. } => "Resume",
                                        AudioPreprocessingModelState::Failed(_) => "Retry",
                                        AudioPreprocessingModelState::Missing => "Download",
                                        AudioPreprocessingModelState::Checking => "Preparing…",
                                        AudioPreprocessingModelState::Ready => "Installed",
                                    };
                                    let response = if value == AudioPreprocessing::None {
                                        response
                                    } else if let AudioPreprocessingModelState::Failed(error) =
                                        state
                                    {
                                        response.on_hover_text(format!(
                                            "{}\n{error}",
                                            i18n::tr(language, hint)
                                        ))
                                    } else {
                                        response.on_hover_text(i18n::tr(language, hint))
                                    };
                                    if response.clicked() {
                                        *selection = Some(value);
                                        action = match state {
                                            AudioPreprocessingModelState::Downloading {
                                                ..
                                            }
                                            | AudioPreprocessingModelState::Queued => {
                                                Some(AudioPreprocessingAction::Pause(value))
                                            }
                                            AudioPreprocessingModelState::Paused { .. } => {
                                                Some(AudioPreprocessingAction::Resume(value))
                                            }
                                            AudioPreprocessingModelState::Missing
                                            | AudioPreprocessingModelState::Checking
                                            | AudioPreprocessingModelState::Failed(_) => {
                                                Some(AudioPreprocessingAction::Download(value))
                                            }
                                            AudioPreprocessingModelState::Ready => None,
                                        };
                                        if action.is_none() {
                                            ui.close();
                                        }
                                    }
                                }
                                if deletable
                                    && ui
                                        .add_enabled_ui(models.can_delete, |ui| {
                                            super::resource_delete_button(
                                                ui,
                                                "preprocessing_model",
                                                language,
                                            )
                                        })
                                        .inner
                                        .clicked()
                                {
                                    action = Some(AudioPreprocessingAction::Delete(value));
                                }
                            });
                        });
                    }
                    ui.separator();
                    let mut use_mirror = models.use_mirror;
                    super::download_mirror_toggle(ui, language, &mut use_mirror);
                    if use_mirror != models.use_mirror {
                        action = Some(AudioPreprocessingAction::SetMirror(use_mirror));
                    }
                })
                .response
        })
        .inner;
    if *selection != before {
        response.mark_changed();
    }
    if action.is_some() {
        ui.ctx().request_repaint();
    }
    AudioPreprocessingResponse { response, action }
}
