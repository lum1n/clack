//! Menu-bar client. The speech engine is `clack-core`. This process owns the
//! hotkeys, the settings popover, the listening HUD, and the keystrokes.

mod config;
mod field;
mod inject;
#[cfg(target_os = "macos")]
mod macos;
mod x11;

use std::time::Duration;

use clack_core::{
    input_devices, language_label, normalize_model_ref, output_languages, parse_language, Clack,
    ClackError, InputDevice, LanguageMode, LoadProgress, ModelDownload, SpeechActivity,
};
use field::TextInput;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::prelude::*;
use gpui::{
    actions, canvas, div, fill, point, px, rgb, size, App, Application, Bounds, Entity, Focusable,
    Pixels, Timer, TitlebarOptions, Window, WindowBounds, WindowDecorations, WindowHandle,
    WindowKind, WindowOptions,
};
use inject::Injector;

actions!(clack, [Quit]);

fn main() {
    Application::new().run(|cx: &mut App| {
        field::bind_keys(cx);
        cx.bind_keys([
            gpui::KeyBinding::new("cmd-q", Quit, None),
            gpui::KeyBinding::new("ctrl-q", Quit, None),
        ]);
        cx.on_action(|_: &Quit, cx: &mut App| cx.quit());

        #[cfg(target_os = "macos")]
        {
            // GPUI sets the regular policy in didFinishLaunching, just before
            // this callback. Accessory keeps Clack out of the Dock and stops
            // it becoming the active app when the HUD is shown.
            macos::accessory_policy();
            macos::install_status_item();
            macos::prompt_accessibility();
        }

        let desk = cx.new(|cx| Desk::new(cx));
        // Linux has no status item. The popover is the stand-in and opens now.
        #[cfg(not(target_os = "macos"))]
        open_settings(cx, &desk);

        let ticking = desk.clone();
        cx.spawn(async move |cx| {
            loop {
                Timer::after(Duration::from_millis(50)).await;
                if cx.update(|app| drive(app, &ticking)).is_err() {
                    break;
                }
            }
        })
        .detach();
    });
}

fn drive(cx: &mut App, desk: &Entity<Desk>) {
    #[cfg(target_os = "macos")]
    {
        let listening = desk.read(cx).listening;
        if !listening {
            if let Some(pid) = macos::frontmost_other_pid() {
                desk.update(cx, |desk, _cx| desk.target_pid = Some(pid));
            }
        }
        let missing = !macos::is_trusted(false);
        desk.update(cx, |desk, cx| {
            if desk.accessibility_missing != missing {
                desk.accessibility_missing = missing;
                cx.notify();
            }
        });
    }

    if let Some(handle) = desk.read(cx).settings {
        if handle
            .update(cx, |desk, window, cx| {
                desk.settings_active = window.is_window_active();
                desk.field_focused = desk.any_field_focused(window, cx);
                desk.commit_fields(window, cx);
            })
            .is_err()
        {
            desk.update(cx, |desk, cx| {
                desk.settings = None;
                desk.settings_active = false;
                desk.field_focused = false;
                cx.notify();
            });
        }
    }

    let (hud, settings) = desk.update(cx, |desk, cx| {
        #[cfg(target_os = "macos")]
        if macos::take_status_click() {
            desk.pending_settings_toggle = true;
        }
        desk.tick(cx);
        (desk.take_hud_request(), desk.take_settings_request())
    });
    apply_hud_request(cx, desk, hud);
    apply_settings_request(cx, desk, settings);
}

struct Desk {
    config: config::Config,
    model_field: Entity<TextInput>,
    token_field: Entity<TextInput>,
    language_field: Entity<TextInput>,
    hold_field: Entity<TextInput>,
    toggle_field: Entity<TextInput>,
    session: Option<Clack>,
    download: Option<ModelDownload>,
    progress: Option<LoadProgress>,
    model_error: Option<String>,
    devices: Vec<InputDevice>,
    mic_error: Option<String>,
    language_error: Option<String>,
    hold_error: Option<String>,
    toggle_error: Option<String>,
    hotkey_error: Option<String>,
    hotkeys: Option<GlobalHotKeyManager>,
    hold_key: Option<HotKey>,
    toggle_key: Option<HotKey>,
    escape_key: Option<HotKey>,
    applied_hold: String,
    applied_toggle: String,
    hold_down: bool,
    hold_owns: bool,
    injector: Injector,
    watch: x11::Watch,
    activity: SpeechActivity,
    live_transcript: String,
    settings_active: bool,
    hud_active: bool,
    field_focused: bool,
    was_ours: bool,
    settings: Option<WindowHandle<Desk>>,
    hud: Option<WindowHandle<Hud>>,
    preview_hud: bool,
    listening: bool,
    #[cfg(target_os = "macos")]
    target_pid: Option<i32>,
    #[cfg(target_os = "macos")]
    accessibility_missing: bool,
    #[cfg(target_os = "macos")]
    pending_settings_toggle: bool,
}

impl Desk {
    fn new(cx: &mut Context<Self>) -> Self {
        let config = config::load();
        let model_field = cx.new(|cx| TextInput::new(cx, "org/model or a Hugging Face URL", &config.model));
        let token_field = cx.new(|cx| TextInput::new(cx, "hf_… for a gated model", &config.token));
        let language_field = cx.new(|cx| {
            TextInput::new(cx, "Auto", &language_label(&config.language))
        });
        let hold_field = cx.new(|cx| TextInput::new(cx, "ctrl+alt+space", &config.hold_hotkey));
        let toggle_field = cx.new(|cx| TextInput::new(cx, "optional", &config.toggle_hotkey));
        let (devices, mic_error) = match input_devices() {
            Ok(devices) => (devices, None),
            Err(err) => (Vec::new(), Some(mic_sentence(&err))),
        };
        let hotkeys = GlobalHotKeyManager::new().ok();
        let hotkey_error = if hotkeys.is_none() {
            Some("Global hotkeys did not start. On Wayland they often cannot.".into())
        } else {
            None
        };
        let preview_hud = std::env::var("CLACK_PREVIEW_HUD").ok().as_deref() == Some("1");
        let mut desk = Self {
            config,
            model_field,
            token_field,
            language_field,
            hold_field,
            toggle_field,
            session: None,
            download: None,
            progress: None,
            model_error: None,
            devices,
            mic_error,
            language_error: None,
            hold_error: None,
            toggle_error: None,
            hotkey_error,
            hotkeys,
            hold_key: None,
            toggle_key: None,
            escape_key: None,
            applied_hold: String::new(),
            applied_toggle: String::new(),
            hold_down: false,
            hold_owns: false,
            injector: Injector::new(),
            watch: x11::Watch::new(),
            activity: SpeechActivity::default(),
            live_transcript: String::new(),
            settings_active: false,
            hud_active: false,
            field_focused: false,
            was_ours: false,
            settings: None,
            hud: None,
            preview_hud,
            listening: false,
            #[cfg(target_os = "macos")]
            target_pid: None,
            #[cfg(target_os = "macos")]
            accessibility_missing: !macos::is_trusted(false),
            #[cfg(target_os = "macos")]
            pending_settings_toggle: false,
        };
        desk.begin_download();
        desk
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        self.poll_hotkeys(cx);
        if let Some(session) = &self.session {
            for update in session.poll() {
                self.injector.note(&update);
            }
            self.live_transcript = session.transcript();
            self.activity = session.speech_activity();
            self.listening = session.is_listening();
        }
        self.watch.tick();
        let ours = self.settings_active || self.hud_active;
        if ours && !self.was_ours && !self.field_focused {
            self.watch.restore();
        }
        self.was_ours = ours;
        self.injector.set_target(self.dictation_target());
        self.injector.flush(!self.field_focused);
        self.sync_escape();
        if let Some(download) = &self.download {
            self.progress = Some(download.progress());
            if let Some(result) = download.try_take() {
                match result {
                    Ok(session) => self.adopt(session),
                    Err(err) => self.model_error = Some(err.to_string()),
                }
                self.download = None;
                self.progress = None;
            }
        }
        cx.notify();
    }

    fn adopt(&mut self, session: Clack) {
        if !self.config.microphone.is_empty() {
            if let Err(err) = session.set_input_device(Some(&self.config.microphone)) {
                self.mic_error = Some(mic_sentence(&err));
            }
        }
        if let Err(err) = session.set_language(&self.config.language) {
            self.language_error = Some(err.to_string());
        }
        self.session = Some(session);
        self.model_error = None;
    }

    fn begin_download(&mut self) {
        let token = {
            let trimmed = self.config.token.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        };
        match ModelDownload::start(&self.config.model, &self.config.language, token.as_deref()) {
            Ok(download) => {
                self.download = Some(download);
                self.model_error = None;
                self.progress = Some(LoadProgress {
                    fraction: 0.02,
                    message: "Starting".into(),
                });
            }
            Err(err) => {
                self.download = None;
                self.model_error = Some(err.to_string());
            }
        }
    }

    fn poll_hotkeys(&mut self, cx: &mut Context<Self>) {
        while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
            let hold = self.hold_key.map(|key| key.id());
            let toggle = self.toggle_key.map(|key| key.id());
            let escape = self.escape_key.map(|key| key.id());
            if Some(event.id) == hold {
                match event.state {
                    HotKeyState::Pressed => self.hold_pressed(cx),
                    HotKeyState::Released => self.hold_released(),
                }
            } else if Some(event.id) == toggle && event.state == HotKeyState::Pressed {
                self.toggle_pressed(cx);
            } else if Some(event.id) == escape && event.state == HotKeyState::Pressed {
                self.cancel_utterance();
            }
        }
    }

    fn hold_pressed(&mut self, cx: &mut Context<Self>) {
        self.hold_down = true;
        if self.listening {
            self.hold_owns = false;
            return;
        }
        self.hold_owns = true;
        self.start_listening(cx);
    }

    fn hold_released(&mut self) {
        self.hold_down = false;
        if self.hold_owns {
            self.hold_owns = false;
            self.commit_utterance();
        }
    }

    fn toggle_pressed(&mut self, cx: &mut Context<Self>) {
        if self.hold_down {
            return;
        }
        if self.listening {
            self.commit_utterance();
        } else {
            self.hold_owns = false;
            self.start_listening(cx);
        }
    }

    fn start_listening(&mut self, _cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            self.model_error = Some("The model is not loaded yet.".into());
            self.hold_owns = false;
            return;
        };
        match session.start_listening() {
            Ok(()) => {
                self.listening = true;
                self.mic_error = self.mic_error.clone().filter(|_| self.devices.is_empty());
            }
            Err(err) => {
                self.listening = false;
                self.hold_owns = false;
                self.mic_error = Some(mic_sentence(&err));
            }
        }
    }

    fn commit_utterance(&mut self) {
        if let Some(session) = &self.session {
            if session.is_listening() {
                if let Err(err) = session.stop_listening() {
                    self.model_error = Some(err.to_string());
                }
            }
        }
        self.listening = self.session.as_ref().is_some_and(Clack::is_listening);
    }

    fn cancel_utterance(&mut self) {
        if let Some(session) = &self.session {
            if session.is_listening() {
                if let Err(err) = session.cancel_listening() {
                    self.model_error = Some(err.to_string());
                }
            }
        }
        self.hold_owns = false;
        self.listening = self.session.as_ref().is_some_and(Clack::is_listening);
    }

    fn sync_escape(&mut self) {
        let Some(manager) = &self.hotkeys else {
            return;
        };
        if self.listening && self.escape_key.is_none() {
            if let Ok(key) = "escape".parse::<HotKey>() {
                if manager.register(key).is_ok() {
                    self.escape_key = Some(key);
                }
            }
        } else if !self.listening {
            if let Some(key) = self.escape_key.take() {
                let _ = manager.unregister(key);
            }
        }
    }

    /// Decide whether the listening window should open or close.
    ///
    /// Opening it here would render the HUD while this entity is borrowed,
    /// and the HUD reads this same entity. The caller applies the request
    /// after the update returns.
    fn take_hud_request(&mut self) -> HudRequest {
        let show = self.listening || self.preview_hud;
        if show && self.hud.is_none() {
            HudRequest::Open
        } else if !show {
            match self.hud.take() {
                Some(handle) => {
                    self.hud_active = false;
                    HudRequest::Close(handle)
                }
                None => HudRequest::Keep,
            }
        } else {
            HudRequest::Keep
        }
    }

    fn take_settings_request(&mut self) -> SettingsRequest {
        #[cfg(target_os = "macos")]
        if self.pending_settings_toggle {
            self.pending_settings_toggle = false;
            if let Some(handle) = self.settings.take() {
                self.settings_active = false;
                self.field_focused = false;
                return SettingsRequest::Close(handle);
            }
            return SettingsRequest::Open;
        }
        SettingsRequest::Keep
    }

    fn dictation_target(&self) -> Option<i32> {
        #[cfg(target_os = "macos")]
        {
            self.target_pid
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    fn accessibility_line(&self) -> Option<&'static str> {
        #[cfg(target_os = "macos")]
        {
            if self.accessibility_missing {
                Some(macos::ACCESSIBILITY_HINT)
            } else {
                None
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }

    fn commit_fields(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.commit_model(window, cx);
        self.commit_token(window, cx);
        self.commit_language(window, cx);
        self.commit_hotkeys(window, cx);
    }

    fn commit_model(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.focused(&self.model_field, window, cx) {
            return;
        }
        let text = self.model_field.read(cx).text();
        if text == self.config.model
            && (self.session.is_some() || self.download.is_some() || self.model_error.is_some())
        {
            return;
        }
        match normalize_model_ref(&text) {
            Ok(repo) => {
                let loaded = self.session.as_ref().map(Clack::repo_id) == Some(repo.as_str());
                self.config.model = text;
                config::save(&self.config);
                if !loaded && self.download.is_none() {
                    self.begin_download();
                }
            }
            Err(err) => {
                if !text.trim().is_empty() {
                    self.model_error = Some(err.to_string());
                }
            }
        }
    }

    fn commit_token(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.focused(&self.token_field, window, cx) {
            return;
        }
        let text = self.token_field.read(cx).text();
        if text == self.config.token {
            return;
        }
        self.config.token = text;
        config::save(&self.config);
        if self.download.is_none() {
            self.begin_download();
        }
    }

    fn commit_language(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.focused(&self.language_field, window, cx) {
            return;
        }
        let text = self.language_field.read(cx).text();
        let parsed = parse_lang_field(&text);
        match parsed {
            Ok(mode) => {
                let code = mode.as_code().to_string();
                if code == self.config.language {
                    return;
                }
                if let Some(session) = &self.session {
                    if let Err(err) = session.set_language(&code) {
                        self.language_error = Some(err.to_string());
                        return;
                    }
                }
                self.config.language = code;
                self.language_error = None;
                config::save(&self.config);
            }
            Err(err) => {
                if !text.trim().is_empty() {
                    self.language_error = Some(err.to_string());
                }
            }
        }
    }

    fn commit_hotkeys(&mut self, window: &Window, cx: &mut Context<Self>) {
        let hold_focused = self.focused(&self.hold_field, window, cx);
        let toggle_focused = self.focused(&self.toggle_field, window, cx);
        if hold_focused || toggle_focused {
            return;
        }
        let hold = self.hold_field.read(cx).text();
        let toggle = self.toggle_field.read(cx).text();
        if hold == self.applied_hold && toggle == self.applied_toggle {
            return;
        }
        let hold_key = match parse_hotkey_field(&hold) {
            Ok(key) => key,
            Err(err) => {
                self.hold_error = Some(err);
                self.applied_hold = hold;
                self.applied_toggle = toggle;
                return;
            }
        };
        let toggle_key = match parse_hotkey_field(&toggle) {
            Ok(key) => key,
            Err(err) => {
                self.toggle_error = Some(err);
                self.applied_hold = hold;
                self.applied_toggle = toggle;
                return;
            }
        };
        if let (Some(hold_key), Some(toggle_key)) = (hold_key, toggle_key) {
            if hold_key.id() == toggle_key.id() {
                self.toggle_error = Some("The toggle hotkey has to differ from the hold hotkey.".into());
                self.applied_hold = hold;
                self.applied_toggle = toggle;
                return;
            }
        }
        self.hold_error = if hold.trim().is_empty() {
            Some("Hold hotkey is empty, so holding a key will not listen.".into())
        } else {
            None
        };
        self.toggle_error = None;
        if let Some(manager) = &self.hotkeys {
            if let Some(key) = self.hold_key.take() {
                let _ = manager.unregister(key);
            }
            if let Some(key) = self.toggle_key.take() {
                let _ = manager.unregister(key);
            }
            if let Some(key) = hold_key {
                if let Err(err) = manager.register(key) {
                    self.hold_error = Some(err.to_string());
                } else {
                    self.hold_key = Some(key);
                }
            }
            if let Some(key) = toggle_key {
                if let Err(err) = manager.register(key) {
                    self.toggle_error = Some(err.to_string());
                } else {
                    self.toggle_key = Some(key);
                }
            }
        }
        self.config.hold_hotkey = hold.clone();
        self.config.toggle_hotkey = toggle.clone();
        self.applied_hold = hold;
        self.applied_toggle = toggle;
        config::save(&self.config);
    }

    fn pick_mic(&mut self, id: String, cx: &mut Context<Self>) {
        self.config.microphone = if id == "default" { String::new() } else { id };
        if let Some(session) = &self.session {
            let device = if self.config.microphone.is_empty() {
                None
            } else {
                Some(self.config.microphone.as_str())
            };
            if let Err(err) = session.set_input_device(device) {
                self.mic_error = Some(mic_sentence(&err));
            }
        }
        config::save(&self.config);
        cx.notify();
    }

    fn pick_language(&mut self, code: String, cx: &mut Context<Self>) {
        self.language_field.update(cx, |field, cx| {
            field.set_text(&language_label(&code), cx);
        });
        if let Some(session) = &self.session {
            if let Err(err) = session.set_language(&code) {
                self.language_error = Some(err.to_string());
                return;
            }
        }
        self.config.language = code;
        self.language_error = None;
        config::save(&self.config);
        cx.notify();
    }

    fn any_field_focused(&self, window: &Window, cx: &App) -> bool {
        self.focused(&self.model_field, window, cx)
            || self.focused(&self.token_field, window, cx)
            || self.focused(&self.language_field, window, cx)
            || self.focused(&self.hold_field, window, cx)
            || self.focused(&self.toggle_field, window, cx)
    }

    fn focused(&self, field: &Entity<TextInput>, window: &Window, cx: &App) -> bool {
        field.read(cx).focus_handle(cx).is_focused(window)
    }
}

impl Render for Desk {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let language_focused = self.focused(&self.language_field, window, cx);
        let language_text = self.language_field.read(cx).text();
        let matches = if language_focused {
            language_matches(&language_text, &language_label(&self.config.language))
        } else {
            Vec::new()
        };
        let selected_mic = self.config.microphone.clone();
        let mic_error = self.mic_error.clone();
        let model_line = if let Some(progress) = &self.progress {
            format!("{}  {:.0}%", progress.message, progress.fraction.clamp(0.0, 1.0) * 100.0)
        } else {
            self.model_error.clone().unwrap_or_default()
        };
        let status = if self.listening {
            "Listening".to_string()
        } else if self.session.is_none() && self.download.is_some() {
            "Loading the model".to_string()
        } else {
            "Idle".to_string()
        };

        let accessibility = self.accessibility_line();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x141414))
            .text_color(rgb(0xe6e6e6))
            .text_sm()
            .p_3()
            .gap_2()
            .child(div().text_color(rgb(0x9a9a9a)).child(status))
            .when(accessibility.is_some(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xe08080))
                        .child(accessibility.unwrap_or("")),
                )
            })
            .child(label("Model"))
            .child(self.model_field.clone())
            .when(!model_line.is_empty(), |row| {
                row.child(div().text_xs().text_color(rgb(0xe08080)).child(model_line))
            })
            .child(label("Hugging Face token"))
            .child(self.token_field.clone())
            .child(label("Microphone"))
            .child(self.mic_rows(selected_mic, cx))
            .when(mic_error.is_some(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xe08080))
                        .child(mic_error.unwrap_or_default()),
                )
            })
            .child(label("Output language"))
            .child(self.language_field.clone())
            .children(matches.into_iter().map(|option| {
                let code = option.code.to_string();
                div()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .hover(|style| style.bg(rgb(0x2c2c2c)).cursor_pointer())
                    .child(format!("{} ({})", option.name, option.code))
                    .on_mouse_up(gpui::MouseButton::Left, {
                        let code = code.clone();
                        cx.listener(move |this, _, _, cx| this.pick_language(code.clone(), cx))
                    })
            }))
            .when(self.language_error.is_some(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xe08080))
                        .child(self.language_error.clone().unwrap_or_default()),
                )
            })
            .child(label("Hold hotkey"))
            .child(self.hold_field.clone())
            .when(self.hold_error.is_some(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xe08080))
                        .child(self.hold_error.clone().unwrap_or_default()),
                )
            })
            .child(label("Toggle hotkey"))
            .child(self.toggle_field.clone())
            .when(self.toggle_error.is_some(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xe08080))
                        .child(self.toggle_error.clone().unwrap_or_default()),
                )
            })
            .when(self.hotkey_error.is_some(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xe08080))
                        .child(self.hotkey_error.clone().unwrap_or_default()),
                )
            })
            .when(self.injector.error.is_some(), |row| {
                row.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xe08080))
                        .child(self.injector.error.clone().unwrap_or_default()),
                )
            })
            .when(cfg!(target_os = "macos"), |row| {
                row.child(
                    div()
                        .mt_2()
                        .px_2()
                        .py_1()
                        .rounded_md()
                        .bg(rgb(0x1c1c1c))
                        .hover(|style| style.bg(rgb(0x333333)).cursor_pointer())
                        .child("Quit")
                        .on_mouse_up(gpui::MouseButton::Left, |_event, _window, cx| cx.quit()),
                )
            })
    }
}

impl Desk {
    fn mic_rows(&self, selected: String, cx: &mut Context<Self>) -> gpui::Div {
        let mut column = div().flex().flex_col().gap_1();
        let default_selected = selected.is_empty();
        column = column.child(mic_row(
            "System default",
            default_selected,
            cx.listener(|this, _, _, cx| this.pick_mic("default".into(), cx)),
        ));
        for device in &self.devices {
            let id = device.id.clone();
            let name = if device.is_default {
                format!("{}  ·  default", device.name)
            } else {
                device.name.clone()
            };
            let chosen = selected == device.id;
            column = column.child(mic_row(
                name,
                chosen,
                cx.listener(move |this, _, _, cx| this.pick_mic(id.clone(), cx)),
            ));
        }
        column
    }
}

fn mic_row(
    name: impl Into<String>,
    selected: bool,
    handler: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut App) + 'static,
) -> gpui::Div {
    let bg = if selected { rgb(0x2e2e2e) } else { rgb(0x1c1c1c) };
    div()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(bg)
        .hover(|style| style.bg(rgb(0x333333)).cursor_pointer())
        .child(name.into())
        .on_mouse_up(gpui::MouseButton::Left, handler)
}

fn label(text: &'static str) -> gpui::Div {
    div().text_xs().text_color(rgb(0x8d8d8d)).child(text)
}

enum HudRequest {
    Keep,
    Open,
    Close(WindowHandle<Hud>),
}

#[allow(dead_code)]
enum SettingsRequest {
    Keep,
    Open,
    Close(WindowHandle<Desk>),
}

fn apply_hud_request(cx: &mut App, desk: &Entity<Desk>, request: HudRequest) {
    match request {
        HudRequest::Keep => {}
        HudRequest::Close(handle) => {
            handle
                .update(cx, |_hud, window, _cx| window.remove_window())
                .ok();
        }
        HudRequest::Open => {
            let bounds = hud_bounds(cx);
            let desk_for_view = desk.clone();
            let Ok(handle) = cx.open_window(hud_options(bounds), move |_window, cx| {
                cx.new(|_| Hud { desk: desk_for_view })
            }) else {
                return;
            };
            #[cfg(target_os = "macos")]
            {
                macos::configure_hud_window();
                macos::accessory_policy();
                if let Some(pid) = desk.read(cx).target_pid {
                    macos::focus_target(pid);
                }
            }
            let desk_for_focus = desk.clone();
            handle
                .update(cx, |_hud, window, cx| {
                    window
                        .spawn(cx, async move |cx| {
                            loop {
                                Timer::after(Duration::from_millis(80)).await;
                                let alive = cx.update(|window, app| {
                                    let active = window.is_window_active();
                                    desk_for_focus.update(app, |desk, cx| {
                                        desk.hud_active = active;
                                        cx.notify();
                                    });
                                });
                                if alive.is_err() {
                                    break;
                                }
                            }
                        })
                        .detach();
                })
                .ok();
            desk.update(cx, |state, _| {
                if state.hud.is_none() {
                    state.hud = Some(handle);
                }
            });
        }
    }
}

struct Hud {
    desk: Entity<Desk>,
}

impl Render for Hud {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let desk = self.desk.read(cx);
        let levels = desk.activity.levels.clone();
        let speaking = desk.activity.speaking;
        let transcript = tail(&desk.live_transcript);
        let transcript_color = if transcript.is_empty() {
            rgb(0x6e6e6e)
        } else {
            rgb(0xf2f2f2)
        };
        let transcript_text = if transcript.is_empty() {
            "Live transcript".to_string()
        } else {
            transcript
        };
        div()
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .bg(rgb(0x101010))
            .text_color(rgb(0xf2f2f2))
            .child(wave(levels, speaking))
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(transcript_color)
                    .child(transcript_text),
            )
    }
}

fn wave(levels: Vec<f32>, speaking: bool) -> impl IntoElement {
    canvas(
        |bounds, _, _| bounds,
        move |bounds, _, window, _| {
            let count = levels.len().max(1);
            let width = bounds.size.width;
            let slot = width / count as f32;
            let bar_w = (slot - px(2.)).max(px(1.));
            for (index, level) in levels.iter().enumerate() {
                let height = (bounds.size.height * *level).max(px(3.));
                let x = bounds.left() + slot * index as f32;
                let y = bounds.top() + (bounds.size.height - height) / 2.0;
                let color = if speaking { rgb(0xf0f0f0) } else { rgb(0x5c5c5c) };
                window.paint_quad(fill(
                    Bounds::new(point(x, y), size(bar_w, height)),
                    color,
                ));
            }
        },
    )
    .h(px(18.))
    .w(px(88.))
}

fn tail(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 48 {
        text.to_string()
    } else {
        chars[chars.len() - 48..].iter().collect()
    }
}

fn settings_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            // The X11 stand-in finds this window by name.
            title: Some("Clack".into()),
            appears_transparent: cfg!(target_os = "macos"),
            traffic_light_position: None,
        }),
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("clack".into()),
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    }
}

fn hud_options(bounds: Bounds<Pixels>) -> WindowOptions {
    // macOS opens with no titlebar at all, so the first frame has no traffic
    // lights. Linux keeps the name so the X11 stand-in can place the window.
    let titlebar = if cfg!(target_os = "macos") {
        None
    } else {
        Some(TitlebarOptions {
            title: Some("Clack HUD".into()),
            appears_transparent: true,
            traffic_light_position: None,
        })
    };
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        app_id: Some("clack".into()),
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    }
}

fn open_settings(cx: &mut App, desk: &Entity<Desk>) {
    if desk.read(cx).settings.is_some() {
        return;
    }
    let bounds = settings_bounds(cx);
    let root = desk.clone();
    let Ok(handle) = cx.open_window(settings_options(bounds), move |_window, _cx| root) else {
        return;
    };
    #[cfg(target_os = "macos")]
    macos::accessory_policy();
    desk.update(cx, |desk, cx| {
        desk.settings = Some(handle);
        cx.notify();
    });
}

fn apply_settings_request(cx: &mut App, desk: &Entity<Desk>, request: SettingsRequest) {
    match request {
        SettingsRequest::Keep => {}
        SettingsRequest::Close(handle) => {
            handle
                .update(cx, |_desk, window, _cx| window.remove_window())
                .ok();
        }
        SettingsRequest::Open => open_settings(cx, desk),
    }
}

fn settings_bounds(cx: &App) -> Bounds<Pixels> {
    let screen = primary_bounds(cx);
    let width = px(320.);
    let height = px(640.);
    let x = screen.origin.x + screen.size.width - width - px(12.);
    // Same clearance as the HUD. xfwm4 will not place a notification window
    // higher than the panel, and a smaller y makes it keep correcting.
    let y = screen.origin.y + px(28.);
    Bounds::new(point(x, y), size(width, height))
}

fn hud_bounds(cx: &App) -> Bounds<Pixels> {
    let screen = primary_bounds(cx);
    let width = px(400.);
    let height = px(44.);
    // Under a MacBook notch the menu bar is taller. GPUI 0.2 does not expose
    // the notch inset, so macOS uses a fixed clearance and other displays sit
    // just below a menu bar.
    let inset = if cfg!(target_os = "macos") { px(52.) } else { px(28.) };
    let x = screen.origin.x + (screen.size.width - width) / 2.0;
    let y = screen.origin.y + inset;
    Bounds::new(point(x, y), size(width, height))
}

fn primary_bounds(cx: &App) -> Bounds<Pixels> {
    cx.displays()
        .into_iter()
        .next()
        .map(|display| display.bounds())
        .unwrap_or_else(|| Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(800.))))
}

fn language_matches(query: &str, current_label: &str) -> Vec<clack_core::LanguageOption> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() || query == current_label {
        return output_languages().iter().copied().take(6).collect();
    }
    output_languages()
        .iter()
        .copied()
        .filter(|option| {
            option.name.to_ascii_lowercase().contains(&needle) || option.code.contains(&needle)
        })
        .take(6)
        .collect()
}

fn parse_lang_field(text: &str) -> Result<LanguageMode, ClackError> {
    if let (Some(start), Some(end)) = (text.rfind('('), text.rfind(')')) {
        if end > start {
            return parse_language(&text[start + 1..end]);
        }
    }
    parse_language(text)
}

fn parse_hotkey_field(text: &str) -> Result<Option<HotKey>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.eq_ignore_ascii_case("escape") || trimmed.eq_ignore_ascii_case("esc") {
        return Err("Escape cancels the utterance. Pick a different hotkey.".into());
    }
    trimmed.parse::<HotKey>().map(Some).map_err(|err| err.to_string())
}

fn mic_sentence(err: &ClackError) -> String {
    let raw = err.to_string();
    let lower = raw.to_lowercase();
    if lower.contains("permission")
        || lower.contains("denied")
        || lower.contains("not permitted")
        || lower.contains("not authorized")
    {
        format!("Microphone permission is denied. {raw}")
    } else if lower.contains("no input") {
        format!("No input device. {raw}")
    } else {
        raw
    }
}
