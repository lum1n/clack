use clack_core::{revise_partial, TranscriptUpdate};
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

/// Types into the focused field of another app. Escape is never a key this sends.
pub struct Injector {
    enigo: Option<Enigo>,
    /// Text currently in the target from this utterance, including a leading space.
    injected: String,
    needs_space: bool,
    pending_text: Option<String>,
    pending_final: bool,
    pending_cancel: bool,
    pub error: Option<String>,
}

impl Injector {
    pub fn new() -> Self {
        Self {
            enigo: None,
            injected: String::new(),
            needs_space: false,
            pending_text: None,
            pending_final: false,
            pending_cancel: false,
            error: None,
        }
    }

    pub fn note(&mut self, update: &TranscriptUpdate) {
        if update.cancelled {
            self.pending_cancel = true;
            self.pending_text = None;
            self.pending_final = false;
            return;
        }
        self.pending_cancel = false;
        self.pending_text = Some(update.text.clone());
        self.pending_final = update.is_final;
    }

    /// `allow` is false while a Clack text field is focused. The pending edit waits.
    pub fn flush(&mut self, allow: bool) {
        if !allow {
            return;
        }
        if self.pending_cancel {
            self.undo();
            self.pending_cancel = false;
            return;
        }
        let Some(text) = self.pending_text.clone() else {
            return;
        };
        let shown = if self.needs_space && !text.is_empty() {
            format!(" {text}")
        } else {
            text.clone()
        };
        self.revise_to(&shown);
        if self.pending_final {
            self.needs_space = !text.trim().is_empty();
            self.injected.clear();
            self.pending_text = None;
            self.pending_final = false;
        }
    }

    fn undo(&mut self) {
        let revision = revise_partial(&self.injected, "");
        self.backspace(revision.backspace_graphemes);
        self.injected.clear();
    }

    fn revise_to(&mut self, next: &str) {
        let revision = revise_partial(&self.injected, next);
        self.backspace(revision.backspace_graphemes);
        self.type_text(&revision.suffix);
        self.injected = next.to_string();
    }

    fn backspace(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let Some(enigo) = self.keyboard() else {
            return;
        };
        for _ in 0..count {
            if enigo.key(Key::Backspace, Direction::Click).is_err() {
                self.error = Some("could not delete the dictated text".into());
                break;
            }
        }
    }

    fn type_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let Some(enigo) = self.keyboard() else {
            return;
        };
        if let Err(err) = enigo.text(text) {
            self.error = Some(err.to_string());
        }
    }

    fn keyboard(&mut self) -> Option<&mut Enigo> {
        if self.enigo.is_none() {
            match Enigo::new(&Settings {
                linux_delay: 1,
                ..Settings::default()
            }) {
                Ok(enigo) => self.enigo = Some(enigo),
                Err(err) => {
                    self.error = Some(format!("keyboard typing is unavailable: {err}"));
                    return None;
                }
            }
        }
        self.enigo.as_mut()
    }
}
