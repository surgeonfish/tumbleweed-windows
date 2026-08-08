//! Tiny INI-like settings store at `%LOCALAPPDATA%\tumbleweed\settings.ini`.
//! Preserves multiple keys (last folder, theme, ...) across writes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use windows_reactor::RequestedTheme;

fn settings_file() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    Path::new(&base).join("tumbleweed").join("settings.ini")
}

fn read_map() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    if let Ok(text) = std::fs::read_to_string(settings_file()) {
        for line in text.lines() {
            if let Some((k, v)) = line.split_once('=') {
                map.insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    map
}

/// Set one key, preserving the other keys in the file.
pub(crate) fn set(key: &str, value: &str) {
    let file = settings_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut map = read_map();
    map.insert(key.to_string(), value.to_string());
    let mut text = String::new();
    for (k, v) in &map {
        text.push_str(&format!("{k}={v}\n"));
    }
    let _ = std::fs::write(&file, text);
}

/// Read one key's value, if present.
pub(crate) fn get(key: &str) -> Option<String> {
    read_map().remove(key)
}

/// Persist the requested theme ("system" / "light" / "dark").
pub(crate) fn save_theme(theme: RequestedTheme) {
    let value = match theme {
        RequestedTheme::Light => "light",
        RequestedTheme::Dark => "dark",
        _ => "system",
    };
    set("theme", value);
}

/// Load the saved theme, defaulting to follow-system.
pub(crate) fn load_theme() -> RequestedTheme {
    match get("theme").as_deref() {
        Some("light") => RequestedTheme::Light,
        Some("dark") => RequestedTheme::Dark,
        _ => RequestedTheme::Default,
    }
}

/// Persist the mDNS discovery toggle ("1" on / "0" off).
pub(crate) fn save_mdns_enabled(enabled: bool) {
    set("mdns_enabled", if enabled { "1" } else { "0" });
}

/// Load the mDNS toggle, defaulting to enabled.
pub(crate) fn load_mdns_enabled() -> bool {
    get("mdns_enabled").as_deref() != Some("0")
}
