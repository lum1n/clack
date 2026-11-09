# Clack

Clack is an embeddable speech-to-text core. A Rust, Swift, or web app can add it
without taking the GPUI client. The GPUI app is one client: a small dark settings
popover, a listening HUD, and keystrokes into the focused field of another app.

```text
crates/clack-core   library. Model, microphone, transcript. No window, no hotkey, no typing.
crates/clack-gpui   menu-bar client. `cargo run -p clack-gpui`.
crates/clack-ffi    cdylib + include/clack.h for Swift.
crates/clack-wasm   the same core for the browser. Whisper does not run there.
examples/print-partials
swift/              wrapper and a sample. Not compiled here (no Swift toolchain).
web/                one page.
```

## Build

```sh
cargo build --workspace
cargo test -p clack-core
cargo run -p clack-gpui
```

The desktop binary is `target/debug/clack`. Config is `~/.config/clack/config.toml`
(mode `0600` on Unix). The Hugging Face token is stored there and is not printed.

Default model: `openai/whisper-tiny`. It is the smallest official multilingual
`openai/whisper` checkpoint (about 39M parameters, one safetensors file). Auto has
to hear more than English, so an English-only distilled checkpoint is not the default.

`CLACK_PREVIEW_HUD=1` opens the listening HUD immediately, with the wave at rest.
That is only for seeing the HUD on a machine with no microphone. A normal launch
shows the HUD only while listening.

## Dictation

Hold the hold hotkey (default `ctrl+alt+space`) to listen. Release commits the
transcript and stops. A pause while the key is held does not end the utterance.
The toggle hotkey is optional and empty until you set one. Pressing it listens
without being held. Pressing it again commits the same way.

Escape cancels the current utterance: listening stops, and only the text Clack
typed for that utterance is deleted. Escape is not sent to the target. It is
grabbed only while listening.

The client does not type while a Clack text field is focused, and it does not
send Escape. On X11 it remembers the window you were in and gives it focus back
if a Clack window took it.

## What you see

GPUI 0.2 cannot create a system status item on Linux. The status-item type exists
in the macOS sources and is not part of the public API. The settings UI is the
smallest top-anchored dark popover that fits: model link, Hugging Face token,
microphone, output language, hold hotkey, toggle hotkey. It does not take focus
when it opens. A denied microphone permission or a missing input device is written
in that popover.

While listening, a second dark window sits at the top center. On a MacBook it is
offset below the menu bar so it can clear the notch (GPUI 0.2 does not report the
notch inset, so the offset is fixed). On other displays it sits just below the
menu bar. It does not take focus. It draws the soundwave from the core's level
series — the bars move while speech is detected and rest during silence — and the
live transcript. It closes when listening stops.

## Models

Paste a repo id (`openai/whisper-tiny`) or a `huggingface.co` / `hf.co` model URL.
Extra path, query, and a trailing slash are removed in the core, then the repo is
downloaded and cached with `hf-hub`. The same normalization is what Rust, Swift,
and the web package call.

The built-in runtime loads Whisper-family checkpoints: `model_type` `whisper`, or
an architecture whose name contains "whisper", with the usual Whisper config
fields. That covers `openai/whisper-*`, `distil-whisper/*`, and fine-tunes that
publish a Whisper config. Anything else fails before the weights are used, with
an unsupported-model error. Weights must be `model.safetensors` or a safetensors
shard index. PyTorch `.bin` and ggml are not loaded.

A gated repo needs the access token from the popover. The token is sent only as
the Hub credential. Public models work with the field empty. If `HF_TOKEN` is
set in the environment and the field is empty, the Hub client can still use it.

## Language

Auto detects the spoken language and transcribes in it. Any other choice forces
that language on the decoder. It does not translate into that language. English
is the exception Whisper actually supports: if you pick English and the speech is
not English, the decoder uses the translate task into English. If the speech is
already English, it transcribes.

## Adding Clack

Rust depends on `clack-core` only:

```rust
let session = clack_core::Clack::new_with_token("openai/whisper-tiny", "auto", None)?;
session.push_audio(&samples, 16_000)?;
for update in session.poll() {
    println!("{} {}", if update.is_final { "final" } else { "partial" }, update.text);
}
```

`update.cancelled` means the utterance was discarded. `SpeechModel` is the
extension point for another architecture. `revise_partial` counts grapheme
clusters when a client types over a partial. `speech_activity` is the wave.
`input_devices` lists microphones. `ModelDownload` returns before the weights
finish so a window can open first.

Swift calls `crates/clack-ffi`. See `swift/Clack.swift` and `swift/Sample`.
Build the cdylib with the workspace, then:

```sh
swiftc -import-objc-header swift/clack-bridging.h -L target/debug -lclack_ffi \
    swift/Clack.swift swift/Sample/main.swift -o clack-sample
```

That was not run here. `swift` is not installed.

The web package is `crates/clack-wasm`, page `web/index.html`. URL normalization
works. Creating a session returns a clear error: candle Whisper is not a browser
runtime, and this package does not include a second engine. A page that had a
session would resample the microphone with Web Audio and call `pushAudio`.

```sh
rustup target add wasm32-unknown-unknown
cargo build -p clack-wasm --target wasm32-unknown-unknown
# JS glue, when wasm-pack is installed:
wasm-pack build crates/clack-wasm --target web --out-dir ../../web/pkg
```

## Typing, per OS

The GPUI client uses `enigo` (X11 backend on Linux) and `global-hotkey`.

- Linux X11: hold, toggle, and Escape can be grabbed, and text can be typed into
  the focused window. This is the path exercised here. Wayland is not. Global
  hotkeys and synthetic keystrokes are unreliable under Wayland, and the default
  enigo features do not speak the Wayland protocols.
- macOS: typing needs Accessibility permission. The HUD offset aims under the
  notch. There is still no public GPUI status item in 0.2, so the settings
  window is the same popover.
- Windows: enigo and global-hotkey have backends. They were not run here.

Partials revise the unstable suffix. The client backspaces grapheme clusters, not
bytes, then types the new suffix. Commit leaves the text. Cancel removes only
what this utterance inserted.

## This machine

No `/dev/snd`, so the popover says there is no input device and listening cannot
start. The process still opens. The wave rests, because there is no input level
to draw. A short Whisper download was not required for the unit tests.
