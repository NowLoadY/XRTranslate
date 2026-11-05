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
            } if *total_bytes > 0 => {
                Some((*downloaded_bytes as f64 / *total_bytes as f64).clamp(0.0, 1.0) as f32)
            }
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

/// A bounded option with a subtle progress fill and an always-visible percentage.
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
    let progress = progress
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 1.0));
    ui.add_enabled_ui(enabled, |ui| {
        let background = ui.painter().add(egui::Shape::Noop);
        let fill = ui.painter().add(egui::Shape::Noop);
        let spinner_id = egui::IdSalt::new("download_spinner");
        let mut atoms = egui::Atoms::new(label);
        if loading && progress.is_none() {
            atoms.push_left(egui::Atom::custom(spinner_id, egui::vec2(14.0, 14.0)));
        }
        let mut button = egui::Button::new(atoms)
            .truncate()
            .selected(selected)
            .fill(egui::Color32::TRANSPARENT)
            .gap(6.0)
            .min_size(egui::vec2(ui.available_width(), 32.0));
        if let Some(progress) = progress {
            button = button.right_text(egui::RichText::new(progress_percent(progress)).monospace());
        }
        let button = button.atom_ui(ui);
        if let Some(rect) = button.rect(spinner_id) {
            egui::Spinner::new().paint_at(ui, rect);
        }
        let response = button.response;
        let visuals = ui.style().interact_selectable(&response, selected);
        ui.painter().set(
            background,
            egui::Shape::rect_filled(response.rect, visuals.corner_radius, visuals.bg_fill),
        );
        if let Some(progress) = progress {
            // Clip a full-size rounded shape, so even a tiny amount of progress
            // follows the button's corners instead of forming a separate pill.
            let inner = response.rect.shrink(1.0);
            let mut clip = inner;
            clip.max.x = inner.min.x + inner.width() * progress;
            ui.painter().with_clip_rect(clip).set(
                fill,
                egui::Shape::rect_filled(
                    inner,
                    visuals.corner_radius - 1,
                    theme::primary().gamma_multiply(0.12),
                ),
            );
        }
        ui_director::egui::button_response(ui, response.id, label, ui.is_enabled(), response)
    })
    .inner
}

fn progress_percent(progress: f32) -> String {
    format!("{:.0}%", (progress * 100.0).floor())
}

/// Per-input selection and resource actions; the host owns download lifecycle.
pub(crate) fn audio_preprocessing_selector(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
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
    let selected_label = models
        .state(selected)
        .and_then(|state| state.progress())
        .map_or_else(
            || selected_label.to_owned(),
            |progress| format!("{selected_label} · {}", progress_percent(progress)),
        );
    let before = *selection;
    let mut action = None;
    let mut response = ui
        .horizontal_wrapped(|ui| {
            ui.label(labels[0]);
            egui::ComboBox::from_id_salt(id_salt)
                .width(160.0_f32.min(ui.available_width()))
                .selected_text(selected_label)
                .truncate()
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
                                let deletable = value == AudioPreprocessing::VocalSeparation
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
