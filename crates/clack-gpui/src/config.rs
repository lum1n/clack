use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use clack_core::DEFAULT_MODEL_ID;

#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    pub model: String,
    #[serde(default)]
    pub token: String,
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub microphone: String,
    #[serde(default = "default_hold")]
    pub hold_hotkey: String,
    #[serde(default)]
    pub toggle_hotkey: String,
}

fn default_language() -> String {
    "auto".into()
}

fn default_hold() -> String {
    "ctrl+alt+space".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            model: DEFAULT_MODEL_ID.into(),
            token: String::new(),
            language: default_language(),
            microphone: String::new(),
            hold_hotkey: default_hold(),
            toggle_hotkey: String::new(),
        }
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("model", &self.model)
            .field("token", &if self.token.is_empty() { "empty" } else { "set" })
            .field("language", &self.language)
            .field("microphone", &self.microphone)
            .field("hold_hotkey", &self.hold_hotkey)
            .field("toggle_hotkey", &self.toggle_hotkey)
            .finish()
    }
}

pub fn path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("clack").join("config.toml");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config").join("clack").join("config.toml");
    }
    PathBuf::from("clack-config.toml")
}

pub fn load() -> Config {
    let path = path();
    let Ok(text) = fs::read_to_string(&path) else {
        return Config::default();
    };
    toml::from_str(&text).unwrap_or_default()
}

pub fn save(config: &Config) {
    let path = path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(text) = toml::to_string_pretty(config) else {
        return;
    };
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(mut file) = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
        {
            let _ = file.write_all(text.as_bytes());
        }
    }
    #[cfg(not(unix))]
    {
        let _ = fs::write(&path, text);
    }
}
