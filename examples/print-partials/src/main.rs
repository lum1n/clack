//! A Rust embedder. It prints partial transcripts from `clack-core`.
//! It does not open a window or type keystrokes.
//!
//! ```text
//! cargo run -p print-partials -- --model https://huggingface.co/openai/whisper-tiny
//! cargo run -p print-partials -- --run --wav sample.wav
//! cargo run -p print-partials -- --run --mic
//! ```
//!
//! Without `--run`, this only normalizes the model link and exits. Loading
//! Whisper downloads the checkpoint.

use std::env;
use std::fs::File;
use std::io::Read;
use std::process::ExitCode;
use std::time::Duration;

use clack_core::{normalize_model_ref, Clack, DEFAULT_MODEL_ID, DEFAULT_MODEL_REASON};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let model = flag(&args, "--model").unwrap_or(DEFAULT_MODEL_ID);
    let language = flag(&args, "--language").unwrap_or("auto");
    let token = flag(&args, "--token");
    let wav = flag(&args, "--wav");
    let run = args.iter().any(|arg| arg == "--run");
    let mic = args.iter().any(|arg| arg == "--mic");

    match normalize_model_ref(model) {
        Ok(repo) => println!("model {repo}"),
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(2);
        }
    }
    println!("{DEFAULT_MODEL_REASON}");

    if !run {
        println!("pass --run to load the model and print partials. Add --wav file.wav or --mic.");
        return ExitCode::SUCCESS;
    }

    let session = match Clack::new_with_token(model, language, token) {
        Ok(session) => session,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
    };

    if let Some(path) = wav {
        let pcm = match read_wav_mono_16k(path) {
            Ok(pcm) => pcm,
            Err(err) => {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
        };
        // Feed the whole clip. Previews may print. The only string that would
        // be typed is the one final transcript of the whole utterance.
        let chunk = 16_000 * 2;
        let mut offset = 0;
        while offset < pcm.len() {
            let end = (offset + chunk).min(pcm.len());
            if let Err(err) = session.push_audio(&pcm[offset..end], 16_000) {
                eprintln!("{err}");
                return ExitCode::from(1);
            }
            offset = end;
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        let mut saw_partial = false;
        while !saw_partial && std::time::Instant::now() < deadline {
            for update in session.poll() {
                if update.cancelled || update.text.is_empty() {
                    continue;
                }
                if update.is_final {
                    eprintln!("a final arrived before release: {}", update.text);
                    return ExitCode::from(1);
                }
                println!("partial {}", update.text);
                saw_partial = true;
            }
            if !saw_partial {
                std::thread::sleep(Duration::from_millis(40));
            }
        }
        if !saw_partial {
            eprintln!("no preview before release");
            return ExitCode::from(1);
        }
        if let Err(err) = session.stop_listening() {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
        let mut injected = Vec::new();
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        while injected.is_empty() && std::time::Instant::now() < deadline {
            for update in session.poll() {
                if update.cancelled || update.text.is_empty() {
                    continue;
                }
                if update.is_final {
                    injected.push(update.text);
                } else {
                    println!("partial {}", update.text);
                }
            }
            if injected.is_empty() {
                std::thread::sleep(Duration::from_millis(40));
            }
        }
        if injected.len() != 1 {
            eprintln!("expected one injected transcript, got {injected:?}");
            return ExitCode::from(1);
        }
        println!("injected {}", injected[0]);
        return ExitCode::SUCCESS;
    }

    if mic {
        if let Err(err) = session.start_listening() {
            eprintln!("{err}");
            return ExitCode::from(1);
        }
        println!("listening. ctrl-c stops the process; partials print as they arrive.");
        let mut last = String::new();
        loop {
            for update in session.poll() {
                println!(
                    "{} {}{}",
                    if update.is_final { "final" } else { "partial" },
                    if update.cancelled { "(cancelled) " } else { "" },
                    update.text
                );
                last = update.text;
            }
            if session.transcript() != last && !session.transcript().is_empty() {
                last = session.transcript();
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    eprintln!("--run needs --wav file.wav or --mic");
    ExitCode::from(2)
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|arg| arg == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn read_wav_mono_16k(path: &str) -> Result<Vec<f32>, String> {
    let mut file = File::open(path).map_err(|err| format!("{path}: {err}"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("{path} is not a PCM wav"));
    }
    let mut offset = 12;
    let mut format = None;
    let mut data = None;
    while offset + 8 <= bytes.len() {
        let id = &bytes[offset..offset + 4];
        let size = u32::from_le_bytes(bytes[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start + size;
        if end > bytes.len() {
            break;
        }
        if id == b"fmt " && size >= 16 {
            format = Some(start);
        } else if id == b"data" {
            data = Some((start, end));
        }
        offset = end + (size % 2);
    }
    let format = format.ok_or("wav is missing fmt")?;
    let (data_start, data_end) = data.ok_or("wav is missing data")?;
    let audio_format = u16::from_le_bytes(bytes[format..format + 2].try_into().unwrap());
    let channels = u16::from_le_bytes(bytes[format + 2..format + 4].try_into().unwrap());
    let rate = u32::from_le_bytes(bytes[format + 4..format + 8].try_into().unwrap());
    let bits = u16::from_le_bytes(bytes[format + 14..format + 16].try_into().unwrap());
    if audio_format != 1 || channels != 1 || rate != 16_000 || bits != 16 {
        return Err(format!(
            "need mono 16-bit PCM at 16000 Hz, got format {audio_format} channels {channels} rate {rate} bits {bits}"
        ));
    }
    let samples = bytes[data_start..data_end]
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
        .collect();
    Ok(samples)
}
