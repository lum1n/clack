#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

/// Linear resample. Good enough to feed Whisper's 16 kHz front end.
pub fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if input.is_empty() || from_rate == 0 || to_rate == 0 {
        return Vec::new();
    }
    if from_rate == to_rate {
        return input.to_vec();
    }
    let out_len = ((input.len() as u64) * (to_rate as u64) / (from_rate as u64)) as usize;
    if out_len == 0 {
        return Vec::new();
    }
    if input.len() == 1 || out_len == 1 {
        return vec![input[0]; out_len];
    }
    let ratio = from_rate as f32 / to_rate as f32;
    let last = input.len() - 1;
    let mut out = Vec::with_capacity(out_len);
    for index in 0..out_len {
        let position = index as f32 * ratio;
        let left = (position.floor() as usize).min(last);
        let right = (left + 1).min(last);
        let mix = position - left as f32;
        out.push(input[left] * (1.0 - mix) + input[right] * mix);
    }
    out
}

/// Downmix interleaved frames to mono f32, then resample to 16 kHz.
pub fn interleaved_to_16k(frames: &[f32], channels: usize, sample_rate: u32) -> Vec<f32> {
    let channels = channels.max(1);
    let frame_count = frames.len() / channels;
    let mut mono = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let start = frame * channels;
        let sum: f32 = frames[start..start + channels].iter().sum();
        mono.push(sum / channels as f32);
    }
    resample_linear(&mono, sample_rate, 16_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_is_copied() {
        let input = vec![0.0, 0.5, -0.5];
        assert_eq!(resample_linear(&input, 16_000, 16_000), input);
    }

    #[test]
    fn forty_eight_k_down_to_sixteen_k_is_about_a_third() {
        let input = vec![0.25; 48_000];
        let out = resample_linear(&input, 48_000, 16_000);
        assert!((out.len() as i32 - 16_000).abs() < 8, "len {}", out.len());
        assert!(out.iter().all(|sample| sample.is_finite()));
    }
}
