//! Same core, compiled for the browser. Whisper's candle runtime does not
//! target wasm. `SpeechModel` is the extension point. This package does not
//! invent a second engine. `normalize_model_ref` still runs here.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(js_name = normalizeModelRef)]
pub fn normalize_model_ref(input: &str) -> Result<String, JsError> {
    clack_core::normalize_model_ref(input).map_err(|err| JsError::new(&err.to_string()))
}

#[wasm_bindgen]
pub struct Clack {
    inner: clack_core::Clack,
}

#[wasm_bindgen]
impl Clack {
    #[wasm_bindgen(constructor)]
    pub fn new(model: &str, language: &str, token: Option<String>) -> Result<Clack, JsError> {
        clack_core::Clack::new_with_token(model, language, token.as_deref())
            .map(|inner| Clack { inner })
            .map_err(|err| JsError::new(&err.to_string()))
    }

    #[wasm_bindgen(js_name = repoId)]
    pub fn repo_id(&self) -> String {
        self.inner.repo_id().to_string()
    }

    #[wasm_bindgen(js_name = setLanguage)]
    pub fn set_language(&self, language: &str) -> Result<(), JsError> {
        self.inner
            .set_language(language)
            .map_err(|err| JsError::new(&err.to_string()))
    }

    #[wasm_bindgen(js_name = startListening)]
    pub fn start_listening(&self) -> Result<(), JsError> {
        self.inner
            .start_listening()
            .map_err(|err| JsError::new(&err.to_string()))
    }

    #[wasm_bindgen(js_name = stopListening)]
    pub fn stop_listening(&self) -> Result<(), JsError> {
        self.inner
            .stop_listening()
            .map_err(|err| JsError::new(&err.to_string()))
    }

    #[wasm_bindgen(js_name = cancelListening)]
    pub fn cancel_listening(&self) -> Result<(), JsError> {
        self.inner
            .cancel_listening()
            .map_err(|err| JsError::new(&err.to_string()))
    }

    /// Mono PCM. The page's Web Audio graph would resample to 16 kHz and call this.
    /// There is no session to push into until a wasm `SpeechModel` exists.
    #[wasm_bindgen(js_name = pushAudio)]
    pub fn push_audio(&self, samples: &[f32], sample_rate: u32) -> Result<(), JsError> {
        self.inner
            .push_audio(samples, sample_rate)
            .map_err(|err| JsError::new(&err.to_string()))
    }

    pub fn transcript(&self) -> String {
        self.inner.transcript()
    }

    #[wasm_bindgen(js_name = isListening)]
    pub fn is_listening(&self) -> bool {
        self.inner.is_listening()
    }
}
