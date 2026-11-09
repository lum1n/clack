use std::sync::{Arc, Mutex};

use crate::error::{ClackError, ClackResult};
use crate::hub::normalize_model_ref;
use crate::language::parse_language;
use crate::session::Clack;
#[cfg(all(feature = "whisper", not(target_arch = "wasm32")))]
use crate::LanguageMode;

/// Download / load progress a client can poll. `fraction` is 0 to 1.
#[derive(Debug, Clone)]
pub struct LoadProgress {
    pub fraction: f32,
    pub message: String,
}

impl LoadProgress {
    fn new(fraction: f32, message: impl Into<String>) -> Self {
        Self {
            fraction,
            message: message.into(),
        }
    }
}

/// A model load that returns immediately. Poll [`progress`](Self::progress)
/// and [`try_take`](Self::try_take). The window can open before the weights arrive.
pub struct ModelDownload {
    progress: Arc<Mutex<LoadProgress>>,
    slot: Arc<Mutex<Option<ClackResult<Clack>>>>,
}

impl std::fmt::Debug for ModelDownload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelDownload").finish_non_exhaustive()
    }
}

impl ModelDownload {
    /// `model` is a repo id or a huggingface.co / hf.co URL.
    /// `token` is an optional Hugging Face access token for gated repos.
    /// An empty token is treated as absent so the Hub client can still use `HF_TOKEN`.
    pub fn start(model: &str, language: &str, token: Option<&str>) -> ClackResult<Self> {
        let repo_id = normalize_model_ref(model)?;
        let language = parse_language(language)?;
        let token = clean_token(token);
        let progress = Arc::new(Mutex::new(LoadProgress::new(0.02, "Starting")));
        let slot = Arc::new(Mutex::new(None));

        #[cfg(all(feature = "whisper", not(target_arch = "wasm32")))]
        {
            let progress_bg = Arc::clone(&progress);
            let slot_bg = Arc::clone(&slot);
            std::thread::Builder::new()
                .name("clack-model".into())
                .spawn(move || {
                    let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        load_whisper_session(&repo_id, language, token.as_deref(), &progress_bg)
                    }));
                    let result = match loaded {
                        Ok(result) => result,
                        Err(_) => Err(ClackError::Model(
                            "the model loader panicked before it finished".into(),
                        )),
                    };
                    *slot_bg.lock().unwrap_or_else(|err| err.into_inner()) = Some(result);
                })
                .map_err(|err| ClackError::Model(format!("could not start the loader: {err}")))?;
        }

        #[cfg(not(all(feature = "whisper", not(target_arch = "wasm32"))))]
        {
            let _ = (repo_id, language, token);
            *slot.lock().unwrap_or_else(|err| err.into_inner()) = Some(Err(unavailable()));
        }

        Ok(Self { progress, slot })
    }

    pub fn progress(&self) -> LoadProgress {
        self.progress
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    /// `Some` once, when the load has finished. `None` while it is still running.
    pub fn try_take(&self) -> Option<ClackResult<Clack>> {
        self.slot.lock().unwrap_or_else(|err| err.into_inner()).take()
    }
}

pub(crate) fn clean_token(token: Option<&str>) -> Option<String> {
    token
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_string)
}

#[cfg(not(all(feature = "whisper", not(target_arch = "wasm32"))))]
fn unavailable() -> ClackError {
    ClackError::BackendUnavailable {
        detail: "candle Whisper is not a browser runtime, and this build has no other engine. SpeechModel is the extension point. Clack does not invent a second recognizer.".into(),
    }
}

#[cfg(all(feature = "whisper", not(target_arch = "wasm32")))]
fn load_whisper_session(
    repo_id: &str,
    language: LanguageMode,
    token: Option<&str>,
    progress: &Mutex<LoadProgress>,
) -> ClackResult<Clack> {
    let report = |fraction: f32, message: &str| {
        *progress.lock().unwrap_or_else(|err| err.into_inner()) =
            LoadProgress::new(fraction, message);
    };
    report(0.05, "Downloading config");
    let model = crate::whisper::load_whisper(repo_id, token, &report)?;
    report(0.98, "Starting the session");
    let session = Clack::with_model(repo_id, language, Box::new(model))?;
    report(1.0, "Ready");
    Ok(session)
}

#[cfg(all(feature = "whisper", not(target_arch = "wasm32")))]
pub(crate) fn load_whisper_blocking(
    repo_id: &str,
    language: LanguageMode,
    token: Option<&str>,
) -> ClackResult<Clack> {
    let progress = Mutex::new(LoadProgress::new(0.0, "Starting"));
    load_whisper_session(repo_id, language, token, &progress)
}

impl Clack {
    /// Download, cache, and load. Blocks. Prefer [`ModelDownload`] in a UI.
    pub fn new(model: &str, language: &str) -> ClackResult<Self> {
        Self::new_with_token(model, language, None)
    }

    /// Same as [`new`](Self::new), with an optional Hub token for gated models.
    /// The token is sent only to Hugging Face. It is not logged.
    pub fn new_with_token(model: &str, language: &str, token: Option<&str>) -> ClackResult<Self> {
        let repo_id = normalize_model_ref(model)?;
        let language = parse_language(language)?;
        let token = clean_token(token);
        #[cfg(all(feature = "whisper", not(target_arch = "wasm32")))]
        {
            return load_whisper_blocking(&repo_id, language, token.as_deref());
        }
        #[cfg(not(all(feature = "whisper", not(target_arch = "wasm32"))))]
        {
            let _ = (repo_id, language, token);
            Err(unavailable())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bad_url_fails_before_any_download() {
        let err = ModelDownload::start("https://example.com/org/model", "en", None).unwrap_err();
        assert!(matches!(err, ClackError::InvalidRepo { .. }), "{err}");
    }

    #[test]
    fn a_bad_language_fails_before_any_download() {
        let err = ModelDownload::start("openai/whisper-tiny", "not-a-language", None).unwrap_err();
        assert!(matches!(err, ClackError::InvalidLanguage { .. }), "{err}");
    }
}
