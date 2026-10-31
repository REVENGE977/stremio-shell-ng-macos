use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

const WINDOW_SETTINGS_FILE: &str = "window-state.json";

#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub struct WindowSettings {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

impl WindowSettings {
    pub fn load() -> Option<Self> {
        fs::read_to_string(settings_path())
            .ok()
            .and_then(|settings| serde_json::from_str(&settings).ok())
    }

    pub fn save(&self) -> io::Result<()> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        fs::write(path, json)
    }
}

fn settings_path() -> PathBuf {
    if let Some(dir) = dirs::config_dir() {
        dir.join("stremio-enhanced").join(WINDOW_SETTINGS_FILE)
    } else {
        PathBuf::from(WINDOW_SETTINGS_FILE)
    }
}
