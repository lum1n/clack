use crate::error::ClackResult;
use crate::language::LanguageMode;

/// One decode of a chunk of 16 kHz mono audio.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hypothesis {
    pub text: String,
    /// Language the decoder used, when it knows. Whisper codes such as "en".
    pub language: Option<String>,
}

/// A speech backend. Whisper is the built-in one. Another architecture implements this
/// and is passed to [`crate::Clack::with_model`].
pub trait SpeechModel: Send {
    fn transcribe(
        &mut self,
        pcm_16k_mono: &[f32],
        language: &LanguageMode,
    ) -> ClackResult<Hypothesis>;
}
