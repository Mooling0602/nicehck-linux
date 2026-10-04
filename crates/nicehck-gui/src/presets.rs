//! Local custom-preset persistence.
//!
//! Custom presets live in a JSON file under the user's config directory, not on
//! the device: the headset's own preset slots are factory-defined and read-only
//! through the recovered protocol, so host-side storage is the only place a
//! user's own tuning can live.

use std::path::PathBuf;

use nicehck_protocol::command::Band;
use serde::{Deserialize, Serialize};

/// A user-saved EQ preset: the bands, the offset, and a display name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomPreset {
    pub name: String,
    pub offset: f32,
    pub bands: Vec<Band>,
}

/// Where the preset file lives.
///
/// Follows XDG_CONFIG_HOME if set, otherwise `~/.config`. The directory is
/// created on demand so a first-run save does not need setup.
fn preset_file() -> Option<PathBuf> {
    let dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            PathBuf::from(xdg)
        } else {
            home_config()?
        }
    } else {
        home_config()?
    };
    Some(dir.join("nicehck").join("presets.json"))
}

fn home_config() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    if home.is_empty() {
        return None;
    }
    Some(PathBuf::from(home).join(".config"))
}

/// Load all saved presets, or an empty list if none exist yet.
pub fn load_presets() -> Vec<CustomPreset> {
    let Some(path) = preset_file() else {
        return Vec::new();
    };
    let Ok(data) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str(&data).unwrap_or_default()
}

/// Persist the preset list. Returns the path written on success.
pub fn save_presets(presets: &[CustomPreset]) -> Result<PathBuf, String> {
    let path = preset_file().ok_or_else(|| "无法确定配置目录".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let json = serde_json::to_string_pretty(presets).map_err(|e| format!("序列化失败: {e}"))?;
    std::fs::write(&path, json).map_err(|e| format!("写入失败: {e}"))?;
    Ok(path)
}

/// Generate the next available default name: "自定义-1", "自定义-2", …
///
/// The number is based on the *current* list length, not on any gap, so the
/// sequence is predictable and the user always sees a fresh suggestion.
pub fn next_default_name(presets: &[CustomPreset]) -> String {
    let n = presets.len() + 1;
    format!("自定义-{n}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_default_name_increments() {
        let empty: Vec<CustomPreset> = vec![];
        assert_eq!(next_default_name(&empty), "自定义-1");

        let one = vec![CustomPreset {
            name: "自定义-1".into(),
            offset: 0.0,
            bands: vec![],
        }];
        assert_eq!(next_default_name(&one), "自定义-2");
    }

    #[test]
    fn custom_preset_serialises_and_roundtrips() {
        let preset = CustomPreset {
            name: "自定义-3".into(),
            offset: -2.0,
            bands: vec![Band {
                index: 1,
                frequency: 1000,
                gain: 3.5,
                q: 1.2,
            }],
        };
        let json = serde_json::to_string(&preset).unwrap();
        let back: CustomPreset = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "自定义-3");
        assert_eq!(back.offset, -2.0);
        assert_eq!(back.bands.len(), 1);
        assert_eq!(back.bands[0].frequency, 1000);
    }

    #[test]
    fn load_presets_returns_empty_when_no_file() {
        // Use a temp HOME so the test never touches the real config.
        let tmp = std::env::temp_dir().join("nicehck-test-no-presets");
        std::env::set_var("XDG_CONFIG_HOME", &tmp);
        let presets = load_presets();
        assert!(presets.is_empty());
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
