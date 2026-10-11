//! Keep ownership in the host process, including after the temporary window closes.
#[derive(Default)]
pub(super) struct Clipboard(Option<arboard::Clipboard>);

impl Clipboard {
    pub fn write(&mut self, text: &str) -> Result<(), String> {
        if self.0.is_none() {
            self.0 = Some(arboard::Clipboard::new().map_err(|error| error.to_string())?);
        }
        self.0
            .as_mut()
            .unwrap()
            .set_text(text)
            .map_err(|error| error.to_string())
    }
}
