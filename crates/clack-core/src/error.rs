use std::fmt;

/// Failure from the speech core. The `Display` text is what an embedder can show.
#[derive(Debug, thiserror::Error)]
pub enum ClackError {
    #[error("unsupported model '{repo_id}' (model_type={model_type}): {detail}")]
    UnsupportedModel {
        repo_id: String,
        model_type: String,
        detail: String,
    },
    #[error("Whisper is not available in this build: {detail}")]
    BackendUnavailable { detail: String },
    #[error("invalid language '{language}': {detail}")]
    InvalidLanguage { language: String, detail: String },
    #[error("invalid model repo id '{repo_id}': {detail}")]
    InvalidRepo { repo_id: String, detail: String },
    #[error("{0}")]
    Model(String),
    #[error("{0}")]
    Audio(String),
    #[error("microphone: {0}")]
    Microphone(String),
    #[error("{0}")]
    Busy(String),
}

impl ClackError {
    pub fn is_unsupported_model(&self) -> bool {
        matches!(self, Self::UnsupportedModel { .. })
    }
}

impl From<String> for ClackError {
    fn from(value: String) -> Self {
        Self::Model(value)
    }
}

pub type ClackResult<T> = Result<T, ClackError>;

pub(crate) fn mic_unavailable(detail: impl fmt::Display) -> ClackError {
    ClackError::Microphone(detail.to_string())
}
