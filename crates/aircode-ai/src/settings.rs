use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::{AiError, ProviderConfig};

pub const MAX_AGENT_PROFILES: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub alias: String,
    pub provider: ProviderConfig,
}

impl AgentProfile {
    pub fn new(alias: impl Into<String>, provider: ProviderConfig) -> Result<Self, String> {
        let alias = normalize_alias(&alias.into())?;
        Ok(Self { alias, provider })
    }
}

pub fn normalize_alias(value: &str) -> Result<String, String> {
    let alias = value.trim().trim_start_matches('@').to_ascii_lowercase();
    let valid = !alias.is_empty()
        && alias.len() <= 32
        && alias
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase())
        && alias.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '-')
        });
    if valid {
        Ok(alias)
    } else {
        Err("Alias must start with a letter and contain only lowercase letters, digits, '_' or '-' (32 characters maximum)".into())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub provider: ProviderConfig,
    pub include_document: bool,
    pub inline_completion: bool,
    pub language: String,
    pub locale_offer_dismissed: bool,
    pub left_width: f32,
    pub right_width: f32,
    pub permission_mode: String,
    pub agent_profiles: Vec<AgentProfile>,
    /// Canonical project root string -> attached profile aliases.
    pub project_agents: BTreeMap<String, Vec<String>>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            provider: ProviderConfig::default(),
            include_document: true,
            inline_completion: false,
            language: "en".into(),
            locale_offer_dismissed: false,
            left_width: 260.0,
            right_width: 360.0,
            permission_mode: "ask_important".into(),
            agent_profiles: Vec::new(),
            project_agents: BTreeMap::new(),
        }
    }
}

impl AppSettings {
    pub fn attached_profiles(&self, project: &str) -> Vec<AgentProfile> {
        self.project_agents
            .get(project)
            .into_iter()
            .flatten()
            .filter_map(|alias| {
                self.agent_profiles
                    .iter()
                    .find(|profile| profile.alias.eq_ignore_ascii_case(alias))
            })
            .cloned()
            .collect()
    }
}

pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new() -> Result<Self, AiError> {
        if let Some(directory) = std::env::var_os("AIRCODE_DATA_DIR") {
            let directory = PathBuf::from(directory);
            if !directory.is_absolute() {
                return Err(AiError::Credentials(
                    "AIRCODE_DATA_DIR must be an absolute path".into(),
                ));
            }
            return Ok(Self::at(directory.join("settings.toml")));
        }
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

    pub fn config_dir(&self) -> Option<&Path> {
        self.path.parent()
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
    fn existing_settings_do_not_enable_inline_completion() {
        let settings: AppSettings = toml::from_str("language = 'ru'").unwrap();
        assert!(!settings.inline_completion);
    }

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
            inline_completion: true,
            ..AppSettings::default()
        };

        store.save(&settings).unwrap();
        let loaded = store.load().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();

        assert_eq!(loaded.provider.model, "k3.1-pro");
        assert_eq!(loaded.provider.effort, ReasoningEffort::High);
        assert!(!loaded.include_document);
        assert!(loaded.inline_completion);
        assert!(!raw.contains("api_key"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn agent_profiles_keep_project_attachments_without_credentials() {
        let path = std::env::temp_dir().join(format!(
            "aircode-team-settings-test-{}/settings.toml",
            std::process::id()
        ));
        let store = SettingsStore::at(&path);
        let profile =
            AgentProfile::new("@DeepSeek", ProviderConfig::new(ProviderKind::DeepSeek)).unwrap();
        let mut settings = AppSettings::default();
        settings.agent_profiles.push(profile);
        settings
            .project_agents
            .insert("D:/project".into(), vec!["deepseek".into()]);

        store.save(&settings).unwrap();
        let loaded = store.load().unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();

        assert_eq!(loaded.attached_profiles("D:/project")[0].alias, "deepseek");
        assert!(!raw.contains("api_key"));
        assert!(!raw.contains("credential"));
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
