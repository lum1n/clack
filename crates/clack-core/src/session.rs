//! Streaming session. Silence does not end an utterance. The client commits
//! with [`Clack::stop_listening`] or discards with [`Clack::cancel_listening`].

#[cfg(not(target_arch = "wasm32"))]
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
#[cfg(target_arch = "wasm32")]
use std::sync::mpsc::{self, Receiver};
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, Mutex};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Duration;

#[cfg(not(target_arch = "wasm32"))]
use crate::audio::resample_linear;
use crate::error::{ClackError, ClackResult};
use crate::language::LanguageMode;
#[cfg(not(target_arch = "wasm32"))]
use crate::model::Hypothesis;
use crate::model::SpeechModel;

/// Bars in [`SpeechActivity::levels`]. The view draws them. It does not invent motion.
pub const LEVEL_BARS: usize = 32;

#[cfg(not(target_arch = "wasm32"))]
const LEVEL_CHUNK: usize = 800;
#[cfg(not(target_arch = "wasm32"))]
const SPEAK_THRESHOLD: f32 = 0.06;
#[cfg(not(target_arch = "wasm32"))]
const LEVEL_GAIN: f32 = 8.0;
/// First hypothesis, and each later one, waits for this much new audio.
#[cfg(not(target_arch = "wasm32"))]
const MIN_NEW_SAMPLES: usize = 8_000;
/// Force a last decode on commit when at least this much audio is new.
#[cfg(not(target_arch = "wasm32"))]
const FORCE_MIN_SAMPLES: usize = 1_600;
/// Cap a segment so a held key does not grow one decode without bound.
#[cfg(not(target_arch = "wasm32"))]
const MAX_SEGMENT_SAMPLES: usize = 16_000 * 6;

/// Levels for a soundwave. `speaking` is true only while listening and recent
/// audio is above a small RMS. Silence is a resting wave, not the end of the utterance.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechActivity {
    pub speaking: bool,
    pub levels: Vec<f32>,
}

impl Default for SpeechActivity {
    fn default() -> Self {
        Self {
            speaking: false,
            levels: vec![0.0; LEVEL_BARS],
        }
    }
}

/// One transcript snapshot. `text` is the whole utterance so far, not a delta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptUpdate {
    pub id: u64,
    pub text: String,
    pub is_final: bool,
    /// Escape (or another cancel). `text` is the utterance to drop.
    /// A commit is `is_final` with `cancelled == false`.
    pub cancelled: bool,
}

/// A live feed of [`TranscriptUpdate`]s. Lagging readers skip to the latest window.
pub struct Subscription {
    rx: Receiver<TranscriptUpdate>,
}

impl Subscription {
    pub fn try_recv(&self) -> Option<TranscriptUpdate> {
        self.rx.try_recv().ok()
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct PublicState {
    listening: bool,
    transcript: String,
    last_error: Option<String>,
    activity: SpeechActivity,
    device_id: Option<String>,
    language: LanguageMode,
}

#[cfg(not(target_arch = "wasm32"))]
struct Shared {
    events: Vec<TranscriptUpdate>,
    next_id: u64,
    state: PublicState,
    subscribers: Vec<Sender<TranscriptUpdate>>,
}

#[cfg(not(target_arch = "wasm32"))]
enum Job {
    StartMic,
    Stop,
    Cancel,
    Push(Vec<f32>),
    SetLanguage(LanguageMode),
    SetDevice(Option<String>),
}

#[cfg(not(target_arch = "wasm32"))]
struct WorkerMsg {
    job: Job,
    reply: Option<Sender<ClackResult<()>>>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Engine {
    model: Box<dyn SpeechModel>,
    language: LanguageMode,
    device_id: Option<String>,
    samples: Vec<f32>,
    segment_start: usize,
    last_decode_len: usize,
    committed: String,
    live: String,
    listening: bool,
    external: bool,
    level_carry: Vec<f32>,
    levels: Vec<f32>,
    activity: SpeechActivity,
    last_emit_key: Option<(String, bool, bool)>,
    pending_emit: Option<TranscriptUpdate>,
    last_error: Option<String>,
    #[cfg(feature = "microphone")]
    mic: Option<cpal::Stream>,
    #[cfg(feature = "microphone")]
    inbox: Option<Arc<Mutex<Vec<f32>>>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Engine {
    fn new(model: Box<dyn SpeechModel>, language: LanguageMode) -> Self {
        Self {
            model,
            language,
            device_id: None,
            samples: Vec::new(),
            segment_start: 0,
            last_decode_len: 0,
            committed: String::new(),
            live: String::new(),
            listening: false,
            external: false,
            level_carry: Vec::new(),
            levels: Vec::new(),
            activity: SpeechActivity::default(),
            last_emit_key: None,
            pending_emit: None,
            last_error: None,
            #[cfg(feature = "microphone")]
            mic: None,
            #[cfg(feature = "microphone")]
            inbox: None,
        }
    }

    fn full_text(&self) -> String {
        join_text(&self.committed, &self.live)
    }

    fn apply(&mut self, job: Job) -> ClackResult<()> {
        let result = match job {
            Job::StartMic => self.start_mic(),
            Job::Stop => self.stop_commit(),
            Job::Cancel => self.cancel(),
            Job::Push(pcm) => self.push_pcm(&pcm),
            Job::SetLanguage(language) => {
                self.language = language;
                Ok(())
            }
            Job::SetDevice(id) => self.set_device(id),
        };
        if let Err(err) = &result {
            self.last_error = Some(err.to_string());
        }
        result
    }

    fn start_mic(&mut self) -> ClackResult<()> {
        #[cfg(not(feature = "microphone"))]
        {
            return Err(ClackError::Microphone(
                "this build has no microphone capture. Push audio instead.".into(),
            ));
        }
        #[cfg(feature = "microphone")]
        {
            if self.external {
                return Err(ClackError::Busy(
                    "audio is being pushed, so the microphone stays closed".into(),
                ));
            }
            if self.listening && self.mic.is_some() {
                return Ok(());
            }
            if !self.listening {
                self.begin();
            }
            self.open_mic()?;
            self.listening = true;
            self.last_error = None;
            Ok(())
        }
    }

    #[cfg(feature = "microphone")]
    fn open_mic(&mut self) -> ClackResult<()> {
        self.mic = None;
        let inbox = Arc::new(Mutex::new(Vec::new()));
        let stream = crate::devices::open_input_stream(self.device_id.as_deref(), Arc::clone(&inbox))?;
        self.inbox = Some(inbox);
        self.mic = Some(stream);
        Ok(())
    }

    #[cfg(feature = "microphone")]
    fn close_mic(&mut self) {
        self.mic = None;
        self.inbox = None;
    }

    #[cfg(not(feature = "microphone"))]
    fn close_mic(&mut self) {}

    fn set_device(&mut self, id: Option<String>) -> ClackResult<()> {
        self.device_id = id;
        #[cfg(feature = "microphone")]
        if self.mic.is_some() {
            self.open_mic()?;
        }
        Ok(())
    }

    fn stop_commit(&mut self) -> ClackResult<()> {
        if !self.listening && self.samples.is_empty() && self.live.is_empty() && self.committed.is_empty()
        {
            self.close_mic();
            self.rest();
            return Ok(());
        }
        self.drain_inbox();
        self.finish_audio(true)?;
        self.close_mic();
        self.listening = false;
        self.external = false;
        self.rest();
        Ok(())
    }

    fn cancel(&mut self) -> ClackResult<()> {
        if !self.listening && self.full_text().is_empty() {
            self.close_mic();
            self.rest();
            return Ok(());
        }
        let discarded = self.full_text();
        self.close_mic();
        self.listening = false;
        self.external = false;
        self.samples.clear();
        self.segment_start = 0;
        self.last_decode_len = 0;
        self.committed.clear();
        self.live.clear();
        self.rest();
        self.emit(discarded, true, true);
        Ok(())
    }

    fn push_pcm(&mut self, pcm: &[f32]) -> ClackResult<()> {
        #[cfg(feature = "microphone")]
        if self.mic.is_some() {
            return Err(ClackError::Busy(
                "the microphone is open. Stop it before pushing audio.".into(),
            ));
        }
        if pcm.is_empty() {
            return Ok(());
        }
        if !self.listening {
            self.begin();
            self.listening = true;
            self.external = true;
        }
        self.ingest(pcm);
        self.finish_audio(false)?;
        Ok(())
    }

    fn begin(&mut self) {
        self.samples.clear();
        self.segment_start = 0;
        self.last_decode_len = 0;
        self.committed.clear();
        self.live.clear();
        self.level_carry.clear();
        self.levels.clear();
        self.activity = SpeechActivity::default();
        self.last_emit_key = None;
        self.pending_emit = None;
        self.last_error = None;
    }

    fn tick(&mut self) {
        self.drain_inbox();
        if self.listening {
            let _ = self.finish_audio(false);
        }
    }

    fn drain_inbox(&mut self) {
        #[cfg(feature = "microphone")]
        if let Some(inbox) = &self.inbox {
            let mut guard = inbox.lock().unwrap_or_else(|err| err.into_inner());
            if !guard.is_empty() {
                let pcm = std::mem::take(&mut *guard);
                drop(guard);
                self.ingest(&pcm);
            }
        }
    }

    fn ingest(&mut self, pcm: &[f32]) {
        self.samples.extend_from_slice(pcm);
        self.push_levels(pcm);
    }

    fn push_levels(&mut self, pcm: &[f32]) {
        self.level_carry.extend_from_slice(pcm);
        while self.level_carry.len() >= LEVEL_CHUNK {
            let chunk: Vec<f32> = self.level_carry.drain(..LEVEL_CHUNK).collect();
            let mean = chunk.iter().map(|sample| sample * sample).sum::<f32>() / LEVEL_CHUNK as f32;
            let level = (mean.sqrt() * LEVEL_GAIN).clamp(0.0, 1.0);
            self.levels.push(level);
            if self.levels.len() > LEVEL_BARS {
                let extra = self.levels.len() - LEVEL_BARS;
                self.levels.drain(..extra);
            }
        }
        self.activity = self.activity_now();
    }

    fn activity_now(&self) -> SpeechActivity {
        let mut levels = vec![0.0; LEVEL_BARS];
        let start = LEVEL_BARS.saturating_sub(self.levels.len());
        for (index, level) in self.levels.iter().enumerate() {
            levels[start + index] = *level;
        }
        let speaking = self.listening
            && self
                .levels
                .iter()
                .rev()
                .take(8)
                .any(|level| *level > SPEAK_THRESHOLD);
        SpeechActivity { speaking, levels }
    }

    fn rest(&mut self) {
        self.level_carry.clear();
        self.levels.clear();
        self.activity = SpeechActivity::default();
    }

    fn finish_audio(&mut self, force: bool) -> ClackResult<()> {
        if !self.listening && !force {
            return Ok(());
        }
        loop {
            let segment_len = self.samples.len().saturating_sub(self.segment_start);
            let new_len = self.samples.len().saturating_sub(self.last_decode_len);
            let capped = segment_len >= MAX_SEGMENT_SAMPLES;
            let ready = new_len >= MIN_NEW_SAMPLES || (force && new_len >= FORCE_MIN_SAMPLES) || capped;
            if !ready {
                break;
            }
            let end = if capped {
                self.segment_start + MAX_SEGMENT_SAMPLES
            } else {
                self.samples.len()
            };
            let hypothesis = self.decode_range(self.segment_start, end)?;
            self.last_decode_len = end;
            if hypothesis.text.is_empty() && !force && !capped {
                break;
            }
            if capped {
                self.committed = join_text(&self.committed, &hypothesis.text);
                self.live.clear();
                self.samples.drain(..end);
                self.segment_start = 0;
                self.last_decode_len = 0;
                self.emit(self.full_text(), false, false);
                if force && self.samples.is_empty() {
                    break;
                }
                continue;
            }
            self.live = hypothesis.text;
            break;
        }
        if force {
            self.emit(self.full_text(), true, false);
            self.samples.clear();
            self.segment_start = 0;
            self.last_decode_len = 0;
            self.committed.clear();
            self.live.clear();
        } else if self.listening {
            let text = self.full_text();
            if !text.is_empty() {
                self.emit(text, false, false);
            }
        }
        Ok(())
    }

    fn decode_range(&mut self, start: usize, end: usize) -> ClackResult<Hypothesis> {
        let pcm = &self.samples[start..end];
        self.model.transcribe(pcm, &self.language)
    }

    fn emit(&mut self, text: String, is_final: bool, cancelled: bool) {
        let key = (text.clone(), is_final, cancelled);
        if self.last_emit_key.as_ref() == Some(&key) {
            return;
        }
        self.last_emit_key = Some(key);
        self.pending_emit = Some(TranscriptUpdate {
            id: 0,
            text,
            is_final,
            cancelled,
        });
    }

    fn take_emit(&mut self) -> Option<TranscriptUpdate> {
        self.pending_emit.take()
    }
}

// The emit slot is declared separately so the struct above stays readable.
// It is a real field; the impl above assigns it. Add it to the struct.
#[cfg(not(target_arch = "wasm32"))]
impl Engine {
    fn snapshot(&self) -> PublicState {
        PublicState {
            listening: self.listening,
            transcript: if self.listening {
                self.full_text()
            } else if self.last_emit_key.as_ref().is_some_and(|(_, _, cancelled)| *cancelled) {
                String::new()
            } else {
                self.last_emit_key
                    .as_ref()
                    .filter(|(_, is_final, cancelled)| *is_final && !cancelled)
                    .map(|(text, _, _)| text.clone())
                    .unwrap_or_default()
            },
            last_error: self.last_error.clone(),
            activity: self.activity.clone(),
            device_id: self.device_id.clone(),
            language: self.language.clone(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn join_text(committed: &str, live: &str) -> String {
    let committed = normalize_ws(committed);
    let live = normalize_ws(live);
    match (committed.is_empty(), live.is_empty()) {
        (true, true) => String::new(),
        (false, true) => committed,
        (true, false) => live,
        (false, false) => format!("{committed} {live}"),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn normalize_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The embeddable session. On wasm32 the constructor reports that Whisper
/// is not a browser runtime. Native builds run the worker thread.
#[cfg(not(target_arch = "wasm32"))]
pub struct Clack {
    repo_id: String,
    cmd: Option<Sender<WorkerMsg>>,
    shared: Arc<Mutex<Shared>>,
    cursor: Mutex<u64>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(target_arch = "wasm32")]
pub struct Clack {
    repo_id: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl Clack {
    pub fn with_model(
        repo_id: impl Into<String>,
        language: LanguageMode,
        model: Box<dyn SpeechModel>,
    ) -> ClackResult<Self> {
        let repo_id = repo_id.into();
        let shared = Arc::new(Mutex::new(Shared {
            events: Vec::new(),
            next_id: 1,
            state: PublicState {
                listening: false,
                transcript: String::new(),
                last_error: None,
                activity: SpeechActivity::default(),
                device_id: None,
                language: language.clone(),
            },
            subscribers: Vec::new(),
        }));
        let (cmd_tx, cmd_rx) = mpsc::channel::<WorkerMsg>();
        let shared_worker = Arc::clone(&shared);
        let worker = std::thread::Builder::new()
            .name("clack-session".into())
            .spawn(move || worker_loop(model, language, cmd_rx, shared_worker))
            .map_err(|err| ClackError::Model(format!("could not start the session: {err}")))?;
        Ok(Self {
            repo_id,
            cmd: Some(cmd_tx),
            shared,
            cursor: Mutex::new(0),
            worker: Some(worker),
        })
    }

    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    pub fn language(&self) -> String {
        self.lock_shared().state.language.as_code().to_string()
    }

    pub fn set_language(&self, language: &str) -> ClackResult<()> {
        let language = crate::language::parse_language(language)?;
        self.call(Job::SetLanguage(language))
    }

    pub fn set_input_device(&self, id: Option<&str>) -> ClackResult<()> {
        self.call(Job::SetDevice(id.map(str::to_string)))
    }

    pub fn input_device(&self) -> Option<String> {
        self.lock_shared().state.device_id.clone()
    }

    /// Open the microphone. A pause does not end the utterance.
    pub fn start_listening(&self) -> ClackResult<()> {
        self.call(Job::StartMic)
    }

    /// Commit the current transcript and stop.
    pub fn stop_listening(&self) -> ClackResult<()> {
        self.call(Job::Stop)
    }

    /// Stop and discard the utterance. The update is `cancelled`.
    pub fn cancel_listening(&self) -> ClackResult<()> {
        self.call(Job::Cancel)
    }

    /// Push mono PCM instead of owning the microphone. 16 kHz is ideal.
    /// Other rates are resampled. This does not stop on silence either.
    pub fn push_audio(&self, samples: &[f32], sample_rate: u32) -> ClackResult<()> {
        if sample_rate == 0 {
            return Err(ClackError::Audio("sample rate is 0".into()));
        }
        let pcm = if sample_rate == 16_000 {
            samples.to_vec()
        } else {
            resample_linear(samples, sample_rate, 16_000)
        };
        self.call(Job::Push(pcm))
    }

    pub fn subscribe(&self) -> Subscription {
        let (tx, rx) = mpsc::channel();
        self.lock_shared().subscribers.push(tx);
        Subscription { rx }
    }

    pub fn poll(&self) -> Vec<TranscriptUpdate> {
        let mut cursor = self.cursor.lock().unwrap_or_else(|err| err.into_inner());
        let shared = self.lock_shared();
        if shared.events.is_empty() {
            return Vec::new();
        }
        let out: Vec<_> = shared
            .events
            .iter()
            .filter(|event| event.id > *cursor)
            .cloned()
            .collect();
        if let Some(last) = out.last() {
            *cursor = last.id;
        }
        out
    }

    pub fn is_listening(&self) -> bool {
        self.lock_shared().state.listening
    }

    pub fn transcript(&self) -> String {
        self.lock_shared().state.transcript.clone()
    }

    pub fn last_error(&self) -> Option<String> {
        self.lock_shared().state.last_error.clone()
    }

    pub fn speech_activity(&self) -> SpeechActivity {
        self.lock_shared().state.activity.clone()
    }

    fn call(&self, job: Job) -> ClackResult<()> {
        let Some(cmd) = &self.cmd else {
            return Err(ClackError::Model("session closed".into()));
        };
        let (tx, rx) = mpsc::channel();
        cmd.send(WorkerMsg {
            job,
            reply: Some(tx),
        })
        .map_err(|_| ClackError::Model("session closed".into()))?;
        rx.recv()
            .unwrap_or_else(|_| Err(ClackError::Model("session closed".into())))
    }

    fn lock_shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|err| err.into_inner())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Clack {
    fn drop(&mut self) {
        self.cmd.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(target_arch = "wasm32")]
impl Clack {
    pub fn with_model(
        repo_id: impl Into<String>,
        language: LanguageMode,
        model: Box<dyn SpeechModel>,
    ) -> ClackResult<Self> {
        let _ = (language, model);
        let _ = repo_id;
        Err(ClackError::BackendUnavailable {
            detail: "candle Whisper is not a browser runtime, and this build has no other engine. SpeechModel is the extension point. Clack does not invent a second recognizer.".into(),
        })
    }

    pub fn repo_id(&self) -> &str {
        &self.repo_id
    }

    pub fn language(&self) -> String {
        "auto".into()
    }

    pub fn set_language(&self, _: &str) -> ClackResult<()> {
        Err(self.missing())
    }

    pub fn set_input_device(&self, _: Option<&str>) -> ClackResult<()> {
        Err(self.missing())
    }

    pub fn input_device(&self) -> Option<String> {
        None
    }

    pub fn start_listening(&self) -> ClackResult<()> {
        Err(self.missing())
    }

    pub fn stop_listening(&self) -> ClackResult<()> {
        Err(self.missing())
    }

    pub fn cancel_listening(&self) -> ClackResult<()> {
        Err(self.missing())
    }

    pub fn push_audio(&self, _: &[f32], _: u32) -> ClackResult<()> {
        Err(self.missing())
    }

    pub fn subscribe(&self) -> Subscription {
        let (_tx, rx) = mpsc::channel();
        Subscription { rx }
    }

    pub fn poll(&self) -> Vec<TranscriptUpdate> {
        Vec::new()
    }

    pub fn is_listening(&self) -> bool {
        false
    }

    pub fn transcript(&self) -> String {
        String::new()
    }

    pub fn last_error(&self) -> Option<String> {
        Some(self.missing().to_string())
    }

    pub fn speech_activity(&self) -> SpeechActivity {
        SpeechActivity::default()
    }

    fn missing(&self) -> ClackError {
        ClackError::BackendUnavailable {
            detail: "no speech session in this build".into(),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn worker_loop(
    model: Box<dyn SpeechModel>,
    language: LanguageMode,
    cmd_rx: Receiver<WorkerMsg>,
    shared: Arc<Mutex<Shared>>,
) {
    let mut engine = Engine::new(model, language);
    loop {
        let incoming = cmd_rx.recv_timeout(Duration::from_millis(40));
        match incoming {
            Ok(msg) => {
                let result = engine.apply(msg.job);
                publish_mut(&mut engine, &shared);
                if let Some(reply) = msg.reply {
                    let _ = reply.send(result);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                engine.tick();
                publish_mut(&mut engine, &shared);
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn publish_mut(engine: &mut Engine, shared: &Mutex<Shared>) {
    let emit = engine.take_emit();
    let mut guard = shared.lock().unwrap_or_else(|err| err.into_inner());
    if let Some(mut update) = emit {
        update.id = guard.next_id;
        guard.next_id += 1;
        guard.events.push(update.clone());
        if guard.events.len() > 128 {
            let extra = guard.events.len() - 128;
            guard.events.drain(..extra);
        }
        guard.subscribers.retain(|subscriber| subscriber.send(update.clone()).is_ok());
    }
    guard.state = engine.snapshot();
}

#[cfg(test)]
mod tests {
    use super::*;

    struct LenModel;

    impl SpeechModel for LenModel {
        fn transcribe(
            &mut self,
            pcm_16k_mono: &[f32],
            _language: &LanguageMode,
        ) -> ClackResult<Hypothesis> {
            Ok(Hypothesis {
                text: format!("n{}", pcm_16k_mono.len()),
                language: Some("en".into()),
            })
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn session() -> Clack {
        Clack::with_model("test/len", LanguageMode::Auto, Box::new(LenModel)).unwrap()
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn partial_arrives_before_stop_and_silence_does_not_commit() {
        let clack = session();
        let quiet = vec![0.0_f32; 4_000];
        clack.push_audio(&quiet, 16_000).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        assert!(clack.poll().is_empty(), "4000 samples is under the first window");
        assert!(clack.is_listening(), "silence must not end a pushed utterance");
        clack.push_audio(&vec![0.0_f32; 8_000], 16_000).unwrap();
        std::thread::sleep(Duration::from_millis(120));
        let updates = clack.poll();
        assert!(
            updates.iter().any(|update| update.text == "n12000" && !update.is_final),
            "{updates:?}"
        );
        assert!(clack.is_listening());
        clack.stop_listening().unwrap();
        let updates = clack.poll();
        assert!(
            updates.iter().any(|update| update.text == "n12000" && update.is_final && !update.cancelled),
            "{updates:?}"
        );
        assert!(!clack.is_listening());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn cancel_marks_the_utterance_and_clears_it() {
        let clack = session();
        clack.push_audio(&vec![0.1_f32; 8_000], 16_000).unwrap();
        std::thread::sleep(Duration::from_millis(120));
        let _ = clack.poll();
        clack.cancel_listening().unwrap();
        let updates = clack.poll();
        assert!(
            updates.iter().any(|update| update.cancelled && update.is_final && update.text == "n8000"),
            "{updates:?}"
        );
        assert!(!clack.is_listening());
        assert!(clack.transcript().is_empty());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn push_resamples_to_16k() {
        let clack = session();
        clack.push_audio(&vec![0.2_f32; 48_000], 48_000).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        clack.stop_listening().unwrap();
        let updates = clack.poll();
        let final_text = updates
            .iter()
            .rev()
            .find(|update| update.is_final && !update.cancelled)
            .map(|update| update.text.clone())
            .unwrap_or_default();
        let count: usize = final_text.trim_start_matches('n').parse().unwrap_or(0);
        assert!((15_992..=16_008).contains(&count), "{final_text}");
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn loud_audio_is_speaking_and_silence_rests() {
        let clack = session();
        clack.push_audio(&vec![0.0_f32; 8_000], 16_000).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        let quiet = clack.speech_activity();
        assert!(!quiet.speaking, "silence rests");
        assert_eq!(quiet.levels.len(), LEVEL_BARS);
        clack.push_audio(&vec![0.5_f32; 8_000], 16_000).unwrap();
        std::thread::sleep(Duration::from_millis(80));
        let loud = clack.speech_activity();
        assert!(loud.speaking, "a loud chunk is speech");
        assert!(loud.levels.iter().any(|level| *level > SPEAK_THRESHOLD));
        clack.stop_listening().unwrap();
        assert!(!clack.speech_activity().speaking);
    }
}
