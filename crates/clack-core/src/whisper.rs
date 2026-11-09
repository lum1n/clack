//! Whisper-family loader. Audio is zero-padded to the 30 second window the
//! encoder was trained on. Trailing silence is what makes the decoder emit
//! end-of-transcript. Only `model.safetensors` (one file or a shard index) is read.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::whisper::{
    self, Config, DTYPE, EOT_TOKEN, HOP_LENGTH, N_FFT, N_FRAMES, N_SAMPLES, NO_SPEECH_TOKENS,
    NO_TIMESTAMPS_TOKEN, SAMPLE_RATE, SOT_TOKEN, TRANSCRIBE_TOKEN, TRANSLATE_TOKEN,
};
use hf_hub::{HFClient, HFClientSync};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use tokenizers::Tokenizer;

use crate::error::{ClackError, ClackResult};
use crate::family::classify_hub_config;
use crate::language::LanguageMode;
use crate::model::{Hypothesis, SpeechModel};

const N_FREQ: usize = N_FFT / 2 + 1;

pub struct WhisperModel {
    model: whisper::model::Whisper,
    tokenizer: Tokenizer,
    filters: Vec<f32>,
    device: Device,
    fft: std::sync::Arc<dyn Fft<f32>>,
}

impl SpeechModel for WhisperModel {
    fn transcribe(&mut self, pcm_16k_mono: &[f32], language: &LanguageMode) -> ClackResult<Hypothesis> {
        if pcm_16k_mono.len() < 1_600 {
            return Ok(Hypothesis {
                text: String::new(),
                language: None,
            });
        }
        // The encoder context is 1500 positions after a stride-2 conv, which is
        // one 30s mel. Cutting at `max_source_positions * hop` would be 15s.
        let pcm = if pcm_16k_mono.len() > N_SAMPLES {
            &pcm_16k_mono[..N_SAMPLES]
        } else {
            pcm_16k_mono
        };
        let mel = log_mel(pcm, self.model.config.num_mel_bins, &self.filters, self.fft.as_ref())?;
        let frames = mel.len() / self.model.config.num_mel_bins.max(1);
        let input = Tensor::from_vec(mel, (1, self.model.config.num_mel_bins, frames), &self.device)
            .map_err(model_err)?;
        self.model.reset_kv_cache();
        let encoded = self
            .model
            .encoder
            .forward(&input, true)
            .map_err(model_err)?;

        let (code, translate) = match language {
            LanguageMode::Auto => (self.detect_language(&encoded)?, false),
            LanguageMode::Code(code) if code == "en" => {
                let detected = self.detect_language(&encoded)?;
                if detected == "en" {
                    ("en".to_string(), false)
                } else {
                    (detected, true)
                }
            }
            LanguageMode::Code(code) => (code.clone(), false),
        };
        let text = self.decode(&encoded, &code, translate)?;
        Ok(Hypothesis {
            text,
            language: Some(code),
        })
    }
}

impl WhisperModel {
    fn detect_language(&mut self, encoded: &Tensor) -> ClackResult<String> {
        let sot = self.token_id(SOT_TOKEN)?;
        self.model.reset_kv_cache();
        let tokens = Tensor::new(&[sot], &self.device)
            .map_err(model_err)?
            .unsqueeze(0)
            .map_err(model_err)?;
        let logits = self.step_logits(&tokens, encoded)?;
        let mut best_code = "en".to_string();
        let mut best = f32::NEG_INFINITY;
        for option in crate::language::output_languages() {
            if option.code == "auto" {
                continue;
            }
            let Some(id) = self.tokenizer.token_to_id(&format!("<|{}|>", option.code)) else {
                continue;
            };
            let score = logits.get(id as usize).copied().unwrap_or(f32::NEG_INFINITY);
            if score > best {
                best = score;
                best_code = option.code.to_string();
            }
        }
        Ok(best_code)
    }

    fn decode(&mut self, encoded: &Tensor, language: &str, translate: bool) -> ClackResult<String> {
        let mut ids = vec![
            self.token_id(SOT_TOKEN)?,
            self.token_id(&format!("<|{language}|>"))?,
            self.token_id(if translate { TRANSLATE_TOKEN } else { TRANSCRIBE_TOKEN })?,
            self.token_id(NO_TIMESTAMPS_TOKEN)?,
        ];
        let eot = self.token_id(EOT_TOKEN)?;
        let no_speech: Vec<u32> = NO_SPEECH_TOKENS
            .iter()
            .filter_map(|token| self.tokenizer.token_to_id(token))
            .collect();
        let suppress = self.model.config.suppress_tokens.clone();
        // Whisper's blank is a single space. `suppress_blank` keeps it from
        // filling a finished utterance. It is not in `suppress_tokens`.
        let blank = self
            .tokenizer
            .encode(" ", false)
            .ok()
            .and_then(|encoding| encoding.get_ids().first().copied());
        let prompt_len = ids.len();
        self.model.reset_kv_cache();
        for _ in 0..96 {
            let tokens = Tensor::new(ids.as_slice(), &self.device)
                .map_err(model_err)?
                .unsqueeze(0)
                .map_err(model_err)?;
            let mut logits = self.step_logits(&tokens, encoded)?;
            for id in &suppress {
                if let Some(slot) = logits.get_mut(*id as usize) {
                    *slot = f32::NEG_INFINITY;
                }
            }
            if let Some(id) = blank {
                if let Some(slot) = logits.get_mut(id as usize) {
                    *slot = f32::NEG_INFINITY;
                }
            }
            for (index, score) in logits.iter_mut().enumerate() {
                if let Some(token) = self.tokenizer.id_to_token(index as u32) {
                    // Timestamp tokens stay. A pause in the window is a timestamp,
                    // and masking those makes greedy decode emit end-of-transcript
                    // instead of the words that follow the pause. Other task tokens
                    // stay masked. End-of-transcript stays available after the first step.
                    let special = token.starts_with("<|") && token.ends_with("|>");
                    if special && index as u32 != eot && !is_timestamp(&token) {
                        *score = f32::NEG_INFINITY;
                    }
                }
            }
            let spoken = &ids[prompt_len..];
            // The first content token is not end-of-transcript. Later, EOT is
            // how a finished utterance stops. No-speech as that first token
            // means the window was silence.
            if spoken.is_empty() {
                if let Some(slot) = logits.get_mut(eot as usize) {
                    *slot = f32::NEG_INFINITY;
                }
            }
            // Two copies of a token are enough. A third is the repetition loop.
            mask_repeated_tokens(&mut logits, spoken);
            let next = argmax(&logits);
            if !logits.get(next).copied().unwrap_or(f32::NEG_INFINITY).is_finite() {
                break;
            }
            if spoken.is_empty() && no_speech.contains(&(next as u32)) {
                return Ok(String::new());
            }
            if next as u32 == eot {
                break;
            }
            ids.push(next as u32);
        }
        let spoken: Vec<u32> = ids[prompt_len..]
            .iter()
            .copied()
            .filter(|id| {
                self.tokenizer
                    .id_to_token(*id)
                    .map(|token| !is_timestamp(&token))
                    .unwrap_or(true)
            })
            .collect();
        if spoken.is_empty() {
            return Ok(String::new());
        }
        let text = self
            .tokenizer
            .decode(&spoken, true)
            .map_err(|err| ClackError::Model(format!("tokenizer decode failed: {err}")))?;
        Ok(normalize_text(&text))
    }

    fn step_logits(&mut self, tokens: &Tensor, encoded: &Tensor) -> ClackResult<Vec<f32>> {
        let decoded = self
            .model
            .decoder
            .forward(tokens, encoded, true)
            .map_err(model_err)?;
        let logits = self
            .model
            .decoder
            .final_linear(&decoded)
            .map_err(model_err)?;
        let last = tokens.dim(1).map_err(model_err)?.saturating_sub(1);
        logits
            .i((0, last))
            .map_err(model_err)?
            .to_vec1::<f32>()
            .map_err(model_err)
    }

    fn token_id(&self, token: &str) -> ClackResult<u32> {
        self.tokenizer.token_to_id(token).ok_or_else(|| {
            ClackError::Model(format!("tokenizer is missing {token}"))
        })
    }
}

pub fn load_whisper(
    repo_id: &str,
    token: Option<&str>,
    report: &dyn Fn(f32, &str),
) -> ClackResult<WhisperModel> {
    let (owner, name) = split_repo(repo_id)?;
    let client = hub_client(token)?;
    let repo = client.model(owner, name);
    report(0.08, "Downloading config");
    let config_path = repo
        .download_file()
        .filename("config.json")
        .send()
        .map_err(|err| hub_err(err))?;
    let config_json = std::fs::read_to_string(&config_path)
        .map_err(|err| ClackError::Model(format!("reading config.json: {err}")))?;
    classify_hub_config(repo_id, &config_json)?;
    let config: Config = serde_json::from_str(&config_json)
        .map_err(|err| ClackError::Model(format!("{repo_id} config.json is not a Whisper config: {err}")))?;

    report(0.16, "Downloading tokenizer");
    let tokenizer_path = repo
        .download_file()
        .filename("tokenizer.json")
        .send()
        .map_err(|err| hub_err(err))?;
    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|err| ClackError::Model(format!("tokenizer.json: {err}")))?;

    report(0.22, "Downloading weights");
    let tensors = download_weights(&repo, report)?;
    let tensors = prefix_model_keys(tensors);
    report(0.94, "Loading weights");
    let device = Device::Cpu;
    let vb = VarBuilder::from_tensors(tensors, DTYPE, &device);
    let model = whisper::model::Whisper::load(&vb, config)
        .map_err(|err| ClackError::Model(format!("{repo_id} weights did not load as Whisper: {err}")))?;
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N_FFT);
    let filters = mel_filters(model.config.num_mel_bins, SAMPLE_RATE, N_FFT);
    let _ = DType::F32;
    Ok(WhisperModel {
        model,
        tokenizer,
        filters,
        device,
        fft,
    })
}

fn download_weights(
    repo: &hf_hub::HFRepositorySync<hf_hub::RepoTypeModel>,
    report: &dyn Fn(f32, &str),
) -> ClackResult<HashMap<String, Tensor>> {
    let device = Device::Cpu;
    match repo.download_file().filename("model.safetensors").send() {
        Ok(path) => return load_safetensors(&path, &device),
        Err(err) => {
            let text = err.to_string().to_lowercase();
            let missing = text.contains("404") || text.contains("not found") || text.contains("entry not found");
            if !missing {
                return Err(hub_err(err));
            }
        }
    }
    let index_path = match repo
        .download_file()
        .filename("model.safetensors.index.json")
        .send()
    {
        Ok(path) => path,
        Err(err) => {
            return Err(ClackError::Model(format!(
                "no model.safetensors (single file or shard index). pytorch .bin and ggml are not loaded. {err}"
            )));
        }
    };
    let index_text = std::fs::read_to_string(&index_path)
        .map_err(|err| ClackError::Model(format!("reading the safetensors index: {err}")))?;
    let index: serde_json::Value = serde_json::from_str(&index_text)
        .map_err(|err| ClackError::Model(format!("safetensors index is not JSON: {err}")))?;
    let map = index
        .get("weight_map")
        .and_then(|value| value.as_object())
        .ok_or_else(|| ClackError::Model("safetensors index has no weight_map".into()))?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for file in map.values().filter_map(|value| value.as_str()) {
        if seen.insert(file.to_string()) {
            files.push(file.to_string());
        }
    }
    if files.is_empty() {
        return Err(ClackError::Model("safetensors index lists no files".into()));
    }
    let mut tensors = HashMap::new();
    for (index, file) in files.iter().enumerate() {
        let fraction = 0.25 + 0.65 * (index as f32 / files.len() as f32);
        report(fraction, &format!("Downloading {file}"));
        let path = repo
            .download_file()
            .filename(file)
            .send()
            .map_err(|err| hub_err(err))?;
        tensors.extend(load_safetensors(&path, &device)?);
    }
    Ok(tensors)
}

fn load_safetensors(path: &Path, device: &Device) -> ClackResult<HashMap<String, Tensor>> {
    candle_core::safetensors::load(path, device).map_err(|err| {
        ClackError::Model(format!(
            "reading {}: {err}",
            path.file_name().and_then(|name| name.to_str()).unwrap_or("safetensors")
        ))
    })
}

fn prefix_model_keys(tensors: HashMap<String, Tensor>) -> HashMap<String, Tensor> {
    let needs_prefix = tensors.keys().any(|key| {
        key.starts_with("encoder.") || key.starts_with("decoder.")
    }) && !tensors.keys().any(|key| key.starts_with("model."));
    if !needs_prefix {
        return tensors;
    }
    tensors
        .into_iter()
        .map(|(key, value)| (format!("model.{key}"), value))
        .collect()
}

fn hub_client(token: Option<&str>) -> ClackResult<HFClientSync> {
    let mut builder = HFClient::builder();
    if let Some(token) = token.map(str::trim).filter(|token| !token.is_empty()) {
        builder = builder.token(token);
    }
    let client = builder
        .build()
        .map_err(|err| ClackError::Model(format!("Hugging Face client: {err}")))?;
    HFClientSync::from_inner(client)
        .map_err(|err| ClackError::Model(format!("Hugging Face client: {err}")))
}

fn split_repo(repo_id: &str) -> ClackResult<(&str, &str)> {
    let (owner, name) = repo_id
        .split_once('/')
        .ok_or_else(|| ClackError::InvalidRepo {
            repo_id: repo_id.into(),
            detail: "expected org/model".into(),
        })?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return Err(ClackError::InvalidRepo {
            repo_id: repo_id.into(),
            detail: "expected org/model".into(),
        });
    }
    Ok((owner, name))
}

fn hub_err(err: hf_hub::HFError) -> ClackError {
    let text = err.to_string();
    let lower = text.to_lowercase();
    if lower.contains("401")
        || lower.contains("403")
        || lower.contains("gated")
        || lower.contains("unauthorized")
        || lower.contains("forbidden")
    {
        ClackError::Model(format!(
            "could not download this model ({text}). If it is gated, set a Hugging Face access token."
        ))
    } else {
        ClackError::Model(text)
    }
}

fn model_err(err: candle_core::Error) -> ClackError {
    ClackError::Model(err.to_string())
}

/// A token that already occurs twice in this utterance cannot be chosen again.
fn mask_repeated_tokens(logits: &mut [f32], spoken: &[u32]) {
    let mut counts: HashMap<u32, u8> = HashMap::new();
    for id in spoken {
        let count = counts.entry(*id).or_insert(0);
        *count = count.saturating_add(1);
        if *count >= 2 {
            if let Some(slot) = logits.get_mut(*id as usize) {
                *slot = f32::NEG_INFINITY;
            }
        }
    }
}

fn argmax(values: &[f32]) -> usize {
    let mut best_at = 0;
    let mut best = f32::NEG_INFINITY;
    for (index, value) in values.iter().enumerate() {
        if *value > best {
            best = *value;
            best_at = index;
        }
    }
    best_at
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn log_mel(
    samples: &[f32],
    n_mels: usize,
    filters: &[f32],
    fft: &dyn Fft<f32>,
) -> ClackResult<Vec<f32>> {
    if n_mels == 0 || filters.len() != n_mels * N_FREQ {
        return Err(ClackError::Model(format!(
            "mel filters are {} values, expected {} by {N_FREQ}",
            filters.len(),
            n_mels
        )));
    }
    // Pad or trim to Whisper's fixed 30s chunk. A short mel has no trailing
    // silence, so greedy decode never selects end-of-transcript and repeats.
    let mut pcm = samples.to_vec();
    if pcm.len() < N_SAMPLES {
        pcm.resize(N_SAMPLES, 0.0);
    } else if pcm.len() > N_SAMPLES {
        pcm.truncate(N_SAMPLES);
    }
    // torch.stft(center=True) reflect-pads N_FFT/2 on each side, then Whisper
    // drops the last frame, leaving N_FRAMES for a 30s chunk.
    let frames = N_FRAMES;
    let padded = reflect_pad(&pcm, N_FFT / 2);
    let mut hann = vec![0.0_f32; N_FFT];
    for (index, window) in hann.iter_mut().enumerate() {
        *window = 0.5 * (1.0 - (std::f32::consts::TAU * index as f32 / N_FFT as f32).cos());
    }
    let mut mel = vec![0.0_f32; n_mels * frames];
    let mut buffer = vec![Complex::new(0.0, 0.0); N_FFT];
    for frame in 0..frames {
        let offset = frame * HOP_LENGTH;
        for bin in buffer.iter_mut() {
            *bin = Complex::new(0.0, 0.0);
        }
        for index in 0..N_FFT {
            buffer[index] = Complex::new(padded[offset + index] * hann[index], 0.0);
        }
        fft.process(&mut buffer);
        let mut power = vec![0.0_f32; N_FREQ];
        for (index, bin) in power.iter_mut().enumerate() {
            *bin = buffer[index].norm_sqr();
        }
        for mel_index in 0..n_mels {
            let mut sum = 0.0_f32;
            let row = mel_index * N_FREQ;
            for freq in 0..N_FREQ {
                sum += power[freq] * filters[row + freq];
            }
            mel[mel_index * frames + frame] = sum.max(1e-10).log10();
        }
    }
    let max = mel.iter().copied().fold(f32::MIN, f32::max);
    let floor = max - 8.0;
    for value in &mut mel {
        *value = value.max(floor) / 4.0 + 1.0;
    }
    Ok(mel)
}

fn is_timestamp(token: &str) -> bool {
    let Some(inner) = token.strip_prefix("<|").and_then(|token| token.strip_suffix("|>")) else {
        return false;
    };
    let Some((seconds, fraction)) = inner.split_once('.') else {
        return false;
    };
    !seconds.is_empty()
        && seconds.chars().all(|c| c.is_ascii_digit())
        && fraction.len() == 2
        && fraction.chars().all(|c| c.is_ascii_digit())
}

/// Reflect pad, matching NumPy/PyTorch `reflect`: the edge sample is not repeated.
fn reflect_pad(samples: &[f32], pad: usize) -> Vec<f32> {
    let n = samples.len();
    if n <= pad {
        return samples.to_vec();
    }
    let mut out = Vec::with_capacity(n + pad * 2);
    for index in (1..=pad).rev() {
        out.push(samples[index]);
    }
    out.extend_from_slice(samples);
    for index in 1..=pad {
        out.push(samples[n - 1 - index]);
    }
    out
}

/// Slaney mel filterbank, the scale librosa used for Whisper's filters.
fn mel_filters(n_mels: usize, sample_rate: usize, n_fft: usize) -> Vec<f32> {
    let n_freq = n_fft / 2 + 1;
    let mut freqs = vec![0.0_f32; n_freq];
    for (index, freq) in freqs.iter_mut().enumerate() {
        *freq = index as f32 * sample_rate as f32 / n_fft as f32;
    }
    let f_max = sample_rate as f32 / 2.0;
    let mel_min = hz_to_mel(0.0);
    let mel_max = hz_to_mel(f_max);
    let mut points = vec![0.0_f32; n_mels + 2];
    for (index, point) in points.iter_mut().enumerate() {
        let mel = mel_min + (mel_max - mel_min) * index as f32 / (n_mels + 1) as f32;
        *point = mel_to_hz(mel);
    }
    let mut weights = vec![0.0_f32; n_mels * n_freq];
    for mel_index in 0..n_mels {
        let lower = points[mel_index];
        let center = points[mel_index + 1];
        let upper = points[mel_index + 2];
        let enorm = 2.0 / (upper - lower).max(1e-6);
        for (freq_index, freq) in freqs.iter().enumerate() {
            let weight = if *freq >= lower && *freq <= center && center > lower {
                (*freq - lower) / (center - lower)
            } else if *freq > center && *freq <= upper && upper > center {
                (upper - *freq) / (upper - center)
            } else {
                0.0
            };
            weights[mel_index * n_freq + freq_index] = weight * enorm;
        }
    }
    weights
}

fn hz_to_mel(hz: f32) -> f32 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4_f32.ln() / 27.0;
    if hz < min_log_hz {
        hz / f_sp
    } else {
        min_log_mel + (hz / min_log_hz).ln() / logstep
    }
}

fn mel_to_hz(mel: f32) -> f32 {
    let f_sp = 200.0 / 3.0;
    let min_log_hz = 1000.0;
    let min_log_mel = min_log_hz / f_sp;
    let logstep = 6.4_f32.ln() / 27.0;
    if mel < min_log_mel {
        mel * f_sp
    } else {
        min_log_hz * (logstep * (mel - min_log_mel)).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_of_one_second_is_padded_to_thirty_seconds() {
        let filters = mel_filters(80, SAMPLE_RATE, N_FFT);
        assert_eq!(filters.len(), 80 * N_FREQ);
        assert!(filters.iter().any(|value| *value > 0.0));
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(N_FFT);
        let pcm = vec![0.0_f32; SAMPLE_RATE];
        let mel = log_mel(&pcm, 80, &filters, fft.as_ref()).unwrap();
        assert_eq!(mel.len(), 80 * N_FRAMES);
        assert!(mel.iter().all(|value| value.is_finite()));
    }

    #[test]
    fn reflect_pad_matches_numpy() {
        let samples = [0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0];
        let padded = reflect_pad(&samples, 3);
        assert_eq!(padded, vec![3.0, 2.0, 1.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 8.0, 7.0, 6.0]);
        assert!(is_timestamp("<|2.00|>"));
        assert!(!is_timestamp("<|en|>"));
        assert!(!is_timestamp("<|endoftext|>"));
    }

    #[test]
    fn a_token_cannot_be_chosen_a_third_time() {
        let mut logits = vec![1.0, 5.0, 4.0, 3.0];
        mask_repeated_tokens(&mut logits, &[1, 2, 1]);
        assert!(!logits[1].is_finite());
        assert!(logits[2].is_finite());
        assert_eq!(argmax(&logits), 2);
    }
}
