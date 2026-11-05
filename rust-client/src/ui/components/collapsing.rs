//! A native disclosure whose state is inspectable and controllable like other shared widgets.
use eframe::egui;

pub struct CollapsingHeader {
    label: String,
    header: egui::CollapsingHeader,
}

impl CollapsingHeader {
    pub fn new(text: impl Into<egui::WidgetText>) -> Self {
        let text = text.into();
        Self {
            label: text.text().into(),
            header: egui::CollapsingHeader::new(text),
        }
    }
    pub fn id_salt(mut self, id: impl egui::AsIdSalt) -> Self {
        self.header = self.header.id_salt(id);
        self
    }
    pub fn default_open(mut self, open: bool) -> Self {
        self.header = self.header.default_open(open);
        self
    }
    pub fn show<R>(
        self,
        ui: &mut egui::Ui,
        body: impl FnOnce(&mut egui::Ui) -> R,
    ) -> egui::CollapsingResponse<R> {
        let mut response = self.header.show(ui, body);
        let header = &mut response.header_response;
        if let Some(mut state) = egui::collapsing_header::CollapsingState::load(ui.ctx(), header.id)
        {
            if let Some(open) = ui_director::egui::record_toggle(
                ui,
                header.id,
                &self.label,
                state.is_open(),
                header.enabled(),
                header.rect,
            ) {
                state.set_open(open);
                state.store(ui.ctx());
                header.mark_changed();
            }
        }
        response
    }
}
