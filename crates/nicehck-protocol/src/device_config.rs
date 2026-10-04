//! Device catalogue: the vendor's `assets/device-config/*.json` blobs, parsed.
//!
//! These files ship inside the Android app and describe every supported model
//! (VID/PID, protocol variant, feature list and full EQ capability limits). We
//! embed them verbatim so the native tool has the same knowledge as the app.

use serde::{Deserialize, Serialize};

/// Raw shape of a `device-*.json` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfigFile {
    pub revision: u32,
    pub device: DeviceEntry,
    #[serde(default)]
    pub features: Vec<Feature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    pub id: u32,
    pub name: String,
    #[serde(rename = "customName", default)]
    pub custom_name: Option<String>,
    #[serde(default)]
    pub tag: String,
    #[serde(rename = "imageUrl", default)]
    pub image_url: Vec<String>,
    pub runtime: Runtime,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Runtime {
    pub transport: String,
    #[serde(rename = "controllerKind")]
    pub controller_kind: String,
    #[serde(rename = "protocolVariant")]
    pub protocol_variant: String,
    pub identity: Identity,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Identity {
    #[serde(rename = "bluetoothNames", default)]
    pub bluetooth_names: Vec<String>,
    #[serde(rename = "vendorId", default)]
    pub vendor_id: Option<u16>,
    #[serde(rename = "productId", default)]
    pub product_id: Option<u16>,
    #[serde(rename = "serviceUuid", default)]
    pub service_uuid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    pub key: String,
    #[serde(rename = "behaviorVariant", default)]
    pub behavior_variant: Option<String>,
    #[serde(default)]
    pub priority: Option<u32>,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

impl DeviceConfigFile {
    /// USB vendor/product ID pair, when this model is a USB device.
    pub fn usb_id(&self) -> Option<(u16, u16)> {
        if self.device.runtime.transport != "usb" {
            return None;
        }
        Some((
            self.device.runtime.identity.vendor_id?,
            self.device.runtime.identity.product_id?,
        ))
    }

    /// Locate a feature by its wire key, e.g. `usb.equalizer`.
    pub fn feature(&self, key: &str) -> Option<&Feature> {
        self.features.iter().find(|f| f.key == key)
    }

    /// Parsed equaliser capability, when present.
    pub fn equalizer(&self) -> Option<EqualizerCapability> {
        let f = self.feature("usb.equalizer")?;
        serde_json::from_value(f.parameters.clone()).ok()
    }
}

/// Equaliser limits and factory presets (`usb.equalizer` parameters).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EqualizerCapability {
    #[serde(rename = "minFrequency")]
    pub min_frequency: u32,
    #[serde(rename = "maxFrequency")]
    pub max_frequency: u32,
    #[serde(rename = "frequencyNumber")]
    pub frequency_number: u8,
    #[serde(rename = "minGain")]
    pub min_gain: f32,
    #[serde(rename = "maxGain")]
    pub max_gain: f32,
    #[serde(rename = "minQValue")]
    pub min_q_value: f32,
    #[serde(rename = "maxQValue")]
    pub max_q_value: f32,
    #[serde(rename = "minOffset")]
    pub min_offset: f32,
    #[serde(rename = "maxOffset")]
    pub max_offset: f32,
    #[serde(rename = "sampleRate")]
    pub sample_rate: f64,
    #[serde(default)]
    pub presets: Vec<Preset>,
}

/// A factory EQ preset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Preset {
    #[serde(rename = "presetKey")]
    pub preset_key: String,
    #[serde(rename = "presetIndex")]
    pub preset_index: u8,
    #[serde(default)]
    pub offset: f32,
    #[serde(rename = "nameI18n", default)]
    pub name_i18n: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub bands: Vec<PresetBand>,
}

impl Preset {
    /// Localised display name, preferring Chinese then English.
    pub fn display_name(&self) -> String {
        for lang in ["zh", "en"] {
            if let Some(n) = self.name_i18n.get(lang) {
                if !n.is_empty() {
                    return n.clone();
                }
            }
        }
        self.preset_key.clone()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PresetBand {
    #[serde(rename = "bandIndex")]
    pub band_index: u8,
    pub frequency: u16,
    pub gain: f32,
    #[serde(rename = "qValue")]
    pub q_value: f32,
}

/// Every device config embedded from the APK.
const CONFIG_JSON: &[&str] = &[
    include_str!("../data/device-config/device-2.json"),
    include_str!("../data/device-config/device-2184.json"),
    include_str!("../data/device-config/device-4753.json"),
    include_str!("../data/device-config/device-4766.json"),
    include_str!("../data/device-config/device-5525.json"),
    include_str!("../data/device-config/device-13101.json"),
    include_str!("../data/device-config/device-13127.json"),
    include_str!("../data/device-config/device-14787.json"),
    include_str!("../data/device-config/device-17186.json"),
    include_str!("../data/device-config/device-49664.json"),
];

/// Parse every embedded device config.
///
/// Panics only if a vendored asset is malformed; that is a build-time error.
pub fn all_devices() -> Vec<DeviceConfigFile> {
    CONFIG_JSON
        .iter()
        .map(|s| serde_json::from_str(s).expect("embedded device-config JSON must parse"))
        .collect()
}

/// Look up a device by USB VID/PID.
pub fn find_usb_device(vendor_id: u16, product_id: u16) -> Option<DeviceConfigFile> {
    all_devices()
        .into_iter()
        .find(|d| d.usb_id() == Some((vendor_id, product_id)))
}

/// USB vendor ID shared by all wired NICEHCK/YUANDAO DSP earphones.
pub const VENDOR_ID: u16 = 0x3302;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_embedded_config_parses() {
        let all = all_devices();
        assert_eq!(all.len(), 10, "expected 10 bundled device configs");
    }

    #[test]
    fn anniversary_device_is_known() {
        let d = find_usb_device(0x3302, 0xC200).expect("10th Anniversary Edition");
        assert_eq!(d.device.id, 49664);
        assert_eq!(d.device.name, "10th Anniversary Edition");
        assert_eq!(d.device.runtime.protocol_variant, "ttgk_v1");
    }

    #[test]
    fn anniversary_eq_capability_matches_payload_format() {
        let d = find_usb_device(0x3302, 0xC200).unwrap();
        let eq = d.equalizer().expect("usb.equalizer feature");
        assert_eq!(eq.frequency_number, 8);
        assert_eq!(eq.min_gain, -12.0);
        assert_eq!(eq.max_gain, 12.0);
        assert_eq!(eq.sample_rate, 96_000.0);
        assert_eq!(eq.presets.len(), 7);
        assert_eq!(eq.presets[0].preset_key, "default");
        assert_eq!(eq.presets[0].bands.len(), 8);
    }

    #[test]
    fn octave_has_ten_bands() {
        let d = find_usb_device(0x3302, 0x39C3).expect("NICEHCK Octave");
        let eq = d.equalizer().unwrap();
        assert_eq!(eq.frequency_number, 10);
        assert_eq!(eq.presets[0].bands.len(), 10);
    }

    #[test]
    fn bluetooth_devices_have_no_usb_id() {
        let d = find_usb_device(0, 0);
        assert!(d.is_none());
        let orig = all_devices()
            .into_iter()
            .find(|d| d.device.id == 2)
            .unwrap();
        assert!(orig.usb_id().is_none());
    }

    #[test]
    fn preset_names_localise_to_chinese() {
        let d = find_usb_device(0x3302, 0xC200).unwrap();
        let eq = d.equalizer().unwrap();
        assert_eq!(eq.presets[1].display_name(), "悔恨之泪");
    }
}
