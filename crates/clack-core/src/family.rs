use serde_json::Value;

use crate::error::{ClackError, ClackResult};

const WHISPER_FIELDS: &[&str] = &[
    "num_mel_bins",
    "d_model",
    "encoder_attention_heads",
    "encoder_layers",
    "decoder_attention_heads",
    "decoder_layers",
    "vocab_size",
    "max_source_positions",
    "max_target_positions",
];

/// Decide whether a Hub `config.json` is a Whisper-family checkpoint this runtime can try.
///
/// Accepts `model_type: "whisper"` and architectures whose name contains "Whisper"
/// (openai/whisper-*, distil-whisper/*, and fine-tunes that publish a Whisper config).
/// Anything else is an [`ClackError::UnsupportedModel`]. This does not mean the weights
/// will load; a later tensor mismatch is a model error, not a claim that the architecture ran.
pub fn classify_hub_config(repo_id: &str, config_json: &str) -> ClackResult<()> {
    let value: Value = serde_json::from_str(config_json).map_err(|err| {
        ClackError::Model(format!("{repo_id} config.json is not JSON: {err}"))
    })?;
    let model_type = value
        .get("model_type")
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .to_string();
    let architectures: Vec<String> = value
        .get("architectures")
        .and_then(|item| item.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let looks_like_whisper = model_type == "whisper"
        || architectures
            .iter()
            .any(|name| name.to_ascii_lowercase().contains("whisper"));
    if !looks_like_whisper {
        let shown = if model_type.is_empty() {
            "unknown".to_string()
        } else {
            model_type
        };
        return Err(ClackError::UnsupportedModel {
            repo_id: repo_id.to_string(),
            model_type: shown,
            detail: "Clack's built-in runtime only runs Whisper-family Hugging Face repos (openai/whisper-*, distil-whisper/*, and fine-tunes whose config.json says model_type \"whisper\"). This id is not one of those, and it was not sent to the decoder.".into(),
        });
    }
    for field in WHISPER_FIELDS {
        let present = value.get(*field).map(|item| item.is_number()).unwrap_or(false);
        if !present {
            return Err(ClackError::Model(format!(
                "{repo_id} looks like Whisper but config.json is missing numeric `{field}`"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bert_is_unsupported() {
        let json = r#"{"model_type":"bert","architectures":["BertModel"]}"#;
        let err = classify_hub_config("google-bert/bert-base-uncased", json).unwrap_err();
        assert!(err.is_unsupported_model(), "{err}");
        let text = err.to_string();
        assert!(text.contains("bert-base-uncased"), "{text}");
        assert!(text.contains("bert"), "{text}");
    }

    #[test]
    fn whisper_config_passes() {
        let json = r#"{
            "model_type": "whisper",
            "architectures": ["WhisperForConditionalGeneration"],
            "num_mel_bins": 80,
            "d_model": 384,
            "encoder_attention_heads": 6,
            "encoder_layers": 4,
            "decoder_attention_heads": 6,
            "decoder_layers": 4,
            "vocab_size": 51865,
            "max_source_positions": 1500,
            "max_target_positions": 448
        }"#;
        classify_hub_config("openai/whisper-tiny", json).unwrap();
    }

    #[test]
    fn incomplete_whisper_config_is_a_model_error() {
        let json = r#"{"model_type":"whisper","architectures":["WhisperForConditionalGeneration"]}"#;
        let err = classify_hub_config("org/finetune", json).unwrap_err();
        assert!(!err.is_unsupported_model(), "{err}");
    }
}
