use clack_core::{revise_partial, TranscriptUpdate};
use unicode_segmentation::UnicodeSegmentation;

#[cfg(not(target_os = "macos"))]
use enigo::{Direction, Enigo, Key, Keyboard, Settings};

#[cfg(target_os = "macos")]
use crate::macos;

/// Types into another app. Escape is never a key this sends.
///
/// On macOS the events are posted to the process captured when the hotkey
/// went down. A granted microphone permission is not treated as success.
pub struct Injector {
    #[cfg(not(target_os = "macos"))]
    enigo: Option<Enigo>,
    #[cfg(target_os = "macos")]
    target_pid: Option<i32>,
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
            #[cfg(not(target_os = "macos"))]
            enigo: None,
            #[cfg(target_os = "macos")]
            target_pid: None,
            injected: String::new(),
            needs_space: false,
            pending_text: None,
            pending_final: false,
            pending_cancel: false,
            error: None,
        }
    }

    pub fn set_target(&mut self, pid: Option<i32>) {
        #[cfg(target_os = "macos")]
        {
            self.target_pid = pid;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = pid;
        }
    }

    pub fn note(&mut self, update: &TranscriptUpdate) {
        if update.cancelled {
            self.pending_text = None;
            self.pending_final = false;
            // Previews are never typed, so a cancel has nothing to backspace
            // unless a finished utterance is still sitting in the field.
            self.pending_cancel = !self.injected.is_empty();
            return;
        }
        // Partials update the HUD only. The field gets one insert, on release.
        if !update.is_final || update.text.trim().is_empty() {
            return;
        }
        self.pending_cancel = false;
        self.pending_text = Some(update.text.clone());
        self.pending_final = true;
    }

    /// `allow` is false while a Clack text field is focused. The pending edit waits.
    pub fn flush(&mut self, allow: bool) {
        if !allow {
            return;
        }
        if self.pending_cancel {
            if self.undo() {
                self.pending_cancel = false;
            }
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
        if self.revise_to(&shown) && self.pending_final {
            self.needs_space = !text.trim().is_empty();
            self.injected.clear();
            self.pending_text = None;
            self.pending_final = false;
        }
    }

    fn undo(&mut self) -> bool {
        let revision = revise_partial(&self.injected, "");
        match self.backspace(revision.backspace_graphemes) {
            Ok(()) => {
                self.injected.clear();
                true
            }
            Err(sent) => {
                self.injected = drop_graphemes(&self.injected, sent);
                false
            }
        }
    }

    fn revise_to(&mut self, next: &str) -> bool {
        let revision = revise_partial(&self.injected, next);
        let sent = match self.backspace(revision.backspace_graphemes) {
            Ok(()) => revision.backspace_graphemes,
            Err(sent) => {
                self.injected = drop_graphemes(&self.injected, sent);
                return false;
            }
        };
        if self.type_text(&revision.suffix).is_err() {
            self.injected = drop_graphemes(&self.injected, sent);
            return false;
        }
        self.injected = next.to_string();
        true
    }

    /// `Err(sent)` is how many backspaces landed before the failure.
    fn backspace(&mut self, count: usize) -> Result<(), usize> {
        if count == 0 {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            let Some(pid) = self.prepare_target() else {
                return Err(0);
            };
            if let Err(err) = macos::backspace(pid, count) {
                self.error = Some(err);
                // The batch stops at the first failed event, so count the ones
                // before it as sent only when the whole batch fails at the start.
                return Err(0);
            }
            self.error = None;
            return Ok(());
        }
        #[cfg(not(target_os = "macos"))]
        {
            let Some(enigo) = self.keyboard() else {
                return Err(0);
            };
            for sent in 0..count {
                if enigo.key(Key::Backspace, Direction::Click).is_err() {
                    self.error = Some("could not delete the dictated text".into());
                    return Err(sent);
                }
            }
            Ok(())
        }
    }

    fn type_text(&mut self, text: &str) -> Result<(), ()> {
        if text.is_empty() {
            return Ok(());
        }
        #[cfg(target_os = "macos")]
        {
            let Some(pid) = self.prepare_target() else {
                return Err(());
            };
            match macos::type_text(pid, text) {
                Ok(()) => {
                    self.error = None;
                    Ok(())
                }
                Err(err) => {
                    self.error = Some(err);
                    Err(())
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let Some(enigo) = self.keyboard() else {
                return Err(());
            };
            match enigo.text(text) {
                Ok(()) => Ok(()),
                Err(err) => {
                    self.error = Some(err.to_string());
                    Err(())
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn prepare_target(&mut self) -> Option<i32> {
        if !macos::is_trusted(false) {
            // The settings popover explains this. Do not report it as a microphone success.
            self.error = None;
            return None;
        }
        let Some(pid) = self.target_pid else {
            self.error = Some(
                "No app was focused when the hotkey went down, so the text was not inserted."
                    .into(),
            );
            return None;
        };
        macos::focus_target(pid);
        Some(pid)
    }

    #[cfg(not(target_os = "macos"))]
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

fn drop_graphemes(text: &str, count: usize) -> String {
    if count == 0 {
        return text.to_string();
    }
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let keep = graphemes.len().saturating_sub(count);
    graphemes[..keep].concat()
}

#[cfg(test)]
mod tests {
    use clack_core::TranscriptUpdate;

    use super::*;

    fn update(text: &str, is_final: bool) -> TranscriptUpdate {
        TranscriptUpdate {
            id: 1,
            text: text.into(),
            is_final,
            cancelled: false,
        }
    }

    #[test]
    fn previews_are_not_typed_and_the_final_is_one_insert() {
        let mut injector = Injector::new();
        injector.note(&update("Og så", false));
        injector.note(&update("Og så amerikansk", false));
        assert!(injector.pending_text.is_none());
        assert!(injector.injected.is_empty());
        injector.note(&update("And so my fellow Americans", true));
        let next = injector.pending_text.clone().unwrap();
        let revision = revise_partial(&injector.injected, &next);
        assert_eq!(revision.backspace_graphemes, 0);
        assert_eq!(revision.suffix, "And so my fellow Americans");
    }
}
