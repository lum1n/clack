//! Embeddable speech-to-text. No window, no hotkey, no synthetic keystrokes.
//!
//! A desktop client types the stream. Swift uses the C ABI in `clack-ffi`.
//! The web package uses the same crate through WASM. Whisper is the built-in
//! [`SpeechModel`]. Another architecture implements that trait and is passed
//! to [`Clack::with_model`].

mod audio;
mod devices;
mod error;
mod family;
mod hub;
mod language;
mod load;
mod model;
mod revision;
mod session;

#[cfg(feature = "whisper")]
mod whisper;

pub use devices::{input_devices, InputDevice};
pub use error::{ClackError, ClackResult};
pub use family::classify_hub_config;
pub use hub::normalize_model_ref;
pub use language::{language_label, output_languages, parse_language, LanguageMode, LanguageOption};
pub use load::{LoadProgress, ModelDownload};
pub use model::{Hypothesis, SpeechModel};
pub use revision::{revise_partial, TextRevision};
pub use session::{SpeechActivity, Subscription, TranscriptUpdate, LEVEL_BARS};

pub use session::Clack;

/// Smallest official multilingual Whisper checkpoint. English-only distilled
/// models are not the default, because Auto has to hear more than English.
pub const DEFAULT_MODEL_ID: &str = "openai/whisper-tiny";

/// Why [`DEFAULT_MODEL_ID`] is the default. Shown in the README and by clients.
pub const DEFAULT_MODEL_REASON: &str =
    "openai/whisper-tiny is the smallest official multilingual openai/whisper model (~39M parameters, one safetensors file). distil-whisper English-only checkpoints are smaller to run but cannot serve Auto.";
