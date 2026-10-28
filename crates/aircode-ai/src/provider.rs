use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const KIMI_CODE_MODELS: &[&str] = &[
    "k3.1-pro",
    "k3.1-pro-highspeed",
    "k3",
    "k3-256k",
    "kimi-for-coding",
    "kimi-for-coding-highspeed",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    KimiCode,
    KimiPlatform,
    DeepSeek,
    RouterAi,
    OpenAiCompatible,
}

impl ProviderKind {
    pub const ALL: [Self; 5] = [
        Self::KimiCode,
        Self::KimiPlatform,
        Self::DeepSeek,
        Self::RouterAi,
        Self::OpenAiCompatible,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::KimiCode => "Kimi Code",
            Self::KimiPlatform => "Kimi Platform",
            Self::DeepSeek => "DeepSeek",
            Self::RouterAi => "RouterAI",
            Self::OpenAiCompatible => "OpenAI compatible",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    China,
    Overseas,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    Low,
    High,
    Max,
}

impl ReasoningEffort {
    pub const ALL: [Self; 3] = [Self::Low, Self::High, Self::Max];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
            Self::Max => "max",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub region: Region,
    pub base_url: String,
    pub model: String,
    pub effort: ReasoningEffort,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::kimi_code(Region::China)
    }
}

impl ProviderConfig {
    pub fn kimi_code(region: Region) -> Self {
        Self {
            kind: ProviderKind::KimiCode,
            region,
            base_url: match region {
                Region::China => "https://api.kimi.com/coding/v1",
                Region::Overseas => "https://api.kimi.ai/coding/v1",
            }
            .into(),
            model: "k3.1-pro".into(),
            effort: ReasoningEffort::High,
        }
    }

    pub fn new(kind: ProviderKind) -> Self {
        match kind {
            ProviderKind::KimiCode => Self::kimi_code(Region::China),
            ProviderKind::KimiPlatform => Self {
                kind,
                region: Region::China,
                base_url: "https://api.moonshot.cn/v1".into(),
                model: String::new(),
                effort: ReasoningEffort::High,
            },
            ProviderKind::DeepSeek => Self {
                kind,
                region: Region::Overseas,
                base_url: "https://api.deepseek.com".into(),
                model: "deepseek-flash".into(),
                effort: ReasoningEffort::High,
            },
            ProviderKind::RouterAi | ProviderKind::OpenAiCompatible => Self {
                kind,
                region: Region::Overseas,
                base_url: if kind == ProviderKind::RouterAi {
                    "https://routerai.ru/api/v1".into()
                } else {
                    String::new()
                },
                model: String::new(),
                effort: ReasoningEffort::High,
            },
        }
    }

    /// Remembers the region on every provider (so cycling through other providers does
    /// not lose it) and picks the official endpoint for the Kimi profiles.
    pub fn with_region(mut self, region: Region) -> Self {
        self.region = region;
        let base_url = match (self.kind, region) {
            (ProviderKind::KimiCode, Region::China) => "https://api.kimi.com/coding/v1",
            (ProviderKind::KimiCode, Region::Overseas) => "https://api.kimi.ai/coding/v1",
            (ProviderKind::KimiPlatform, Region::China) => "https://api.moonshot.cn/v1",
            (ProviderKind::KimiPlatform, Region::Overseas) => "https://api.moonshot.ai/v1",
            _ => return self,
        };
        self.base_url = base_url.into();
        self
    }

    pub fn model_options(&self) -> &'static [&'static str] {
        match self.kind {
            ProviderKind::KimiCode => KIMI_CODE_MODELS,
            _ => &[],
        }
    }

    pub fn supports_effort(&self) -> bool {
        self.kind == ProviderKind::KimiCode && self.model != "kimi-for-coding-highspeed"
    }

    pub fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.into()
        } else {
            format!("{base}/chat/completions")
        }
    }

    pub fn credential_id(&self) -> String {
        let base = self.base_url.trim().trim_end_matches('/');
        let base = base.strip_suffix("/chat/completions").unwrap_or(base);
        format!("{}:{}", self.kind.label(), base.trim_end_matches('/'))
    }

    pub fn session_compatible_with(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.region == other.region
            && self.base_url == other.base_url
            && self.model == other.model
            && self.effort == other.effort
    }

    pub fn request_body(&self, messages: &[ChatMessage]) -> Value {
        let mut body = json!({
            "model": self.model,
            "messages": messages,
            "stream": true,
        });
        if self.kind == ProviderKind::KimiCode {
            body["thinking"] = json!({"type": "enabled"});
            if self.supports_effort() {
                body["reasoning_effort"] = Value::String(self.effort.label().into());
            }
        }
        body
    }

    pub fn request_body_with_tools(&self, messages: &[ChatMessage], tools: Option<Value>) -> Value {
        let mut body = self.request_body(messages);
        if let Some(tools) = tools {
            body["tools"] = tools;
            body["tool_choice"] = Value::String("auto".into());
        }
        body
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kimi_k31_pro_always_enables_thinking() {
        let config = ProviderConfig::default();
        let body = config.request_body(&[ChatMessage::user("Hello")]);

        assert_eq!(body["model"], "k3.1-pro");
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["stream"], true);
    }

    #[test]
    fn kimi_highspeed_does_not_send_adjustable_effort() {
        let config = ProviderConfig {
            model: "kimi-for-coding-highspeed".into(),
            ..ProviderConfig::default()
        };

        let body = config.request_body(&[]);

        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn region_selects_official_kimi_endpoint() {
        let config = ProviderConfig::default().with_region(Region::Overseas);

        assert_eq!(config.base_url, "https://api.kimi.ai/coding/v1");
        assert_eq!(
            config.endpoint(),
            "https://api.kimi.ai/coding/v1/chat/completions"
        );
    }

    #[test]
    fn endpoint_does_not_duplicate_path() {
        let config = ProviderConfig {
            base_url: "https://api.kimi.com/coding/v1/chat/completions/".into(),
            ..ProviderConfig::default()
        };

        assert_eq!(
            config.endpoint(),
            "https://api.kimi.com/coding/v1/chat/completions"
        );
    }

    #[test]
    fn model_or_effort_change_requires_new_session() {
        let original = ProviderConfig::default();
        let mut changed = original.clone();
        changed.model = "k3".into();

        assert!(!original.session_compatible_with(&changed));
        assert!(original.session_compatible_with(&original.clone()));
    }

    #[test]
    fn non_kimi_request_does_not_get_kimi_fields() {
        let mut config = ProviderConfig::new(ProviderKind::DeepSeek);
        let body = config.request_body(&[]);

        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
        config.base_url = "https://routerai.ru/api/v1".into();
        assert_eq!(config.kind, ProviderKind::DeepSeek);
    }
}
