use std::{fs, path::PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{AiError, ProviderConfig};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub provider: ProviderConfig,
    pub include_document: bool,
    pub language: String,
    pub locale_offer_dismissed: bool,
    pub left_width: f32,
    pub right_width: f32,
    pub permission_mode: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::default(),
            include_document: true,
            language: "en".into(),
            locale_offer_dismissed: false,
            left_width: 260.0,
            right_width: 360.0,
            permission_mode: "ask_important".into(),
        }
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new() -> Result<Self, AiError> {
        let directories = ProjectDirs::from("dev", "AIRcode", "AIRcode").ok_or_else(|| {
            AiError::Credentials("Cannot locate the application settings directory".into())
        })?;
        Ok(Self {
            path: directories.config_dir().join("settings.toml"),
        })
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn load(&self) -> Result<AppSettings, AiError> {
        if !self.path.exists() {
            return Ok(AppSettings::default());
        }
        let text = fs::read_to_string(&self.path)
            .map_err(|error| AiError::Credentials(error.to_string()))?;
        toml::from_str(&text).map_err(|error| AiError::Credentials(error.to_string()))
    }

    pub fn save(&self, settings: &AppSettings) -> Result<(), AiError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|error| AiError::Credentials(error.to_string()))?;
        }
        let text = toml::to_string_pretty(settings)
            .map_err(|error| AiError::Credentials(error.to_string()))?;
        fs::write(&self.path, text).map_err(|error| AiError::Credentials(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ProviderKind, ReasoningEffort};

    #[test]
    fn settings_round_trip_contains_no_api_key_field() {
        let path = std::env::temp_dir().join(format!(
            "aircode-settings-test-{}/settings.toml",
            std::process::id()
        ));
        let store = SettingsStore::at(&path);
        let settings = AppSettings {
            provider: ProviderConfig::new(ProviderKind::KimiCode),
            include_document: false,
            ..AppSettings::default()
        };

        store.save(&settings).unwrap();
        let loaded = store.load().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();

        assert_eq!(loaded.provider.model, "k3.1-pro");
        assert_eq!(loaded.provider.effort, ReasoningEffort::High);
        assert!(!loaded.include_document);
        assert!(!raw.contains("api_key"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
