use crate::{i18n::t, secrets::SecretRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProtocol {
    #[default]
    Responses,
    ChatCompletions,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Suggest,
    #[default]
    Approve,
}

/// 密钥单独保存在系统钥匙串，配置文件只保存模型参数。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct AssistantSettings {
    pub endpoint: String,
    pub protocol: ApiProtocol,
    pub model: String,
    pub timeout_seconds: u64,
    pub max_output_tokens: u32,
    pub context_tokens: u32,
    pub temperature: Option<f64>,
    pub instructions: String,
    pub reasoning_effort: String,
    pub permission: PermissionMode,
}

impl Default for AssistantSettings {
    fn default() -> Self {
        Self {
            endpoint: "https://api.openai.com/v1".into(),
            protocol: ApiProtocol::Responses,
            model: String::new(),
            timeout_seconds: 60,
            max_output_tokens: 4096,
            context_tokens: 16384,
            temperature: None,
            instructions: String::new(),
            reasoning_effort: String::new(),
            permission: PermissionMode::Approve,
        }
    }
}

impl AssistantSettings {
    pub fn validated(mut self) -> Result<Self, String> {
        self.endpoint = self.endpoint.trim().trim_end_matches('/').to_owned();
        self.model = self.model.trim().to_owned();
        let url = url::Url::parse(&self.endpoint)
            .map_err(|_| t!("assistant.invalid_endpoint").to_string())?;
        let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
        if !(url.scheme() == "https" || url.scheme() == "http" && local)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(t!("assistant.invalid_endpoint").to_string());
        }
        self.endpoint = url.to_string().trim_end_matches('/').to_owned();
        if self.model.is_empty() || self.model.len() > 256 {
            return Err(t!("assistant.invalid_model").to_string());
        }
        if !(5..=300).contains(&self.timeout_seconds)
            || !(256..=32768).contains(&self.max_output_tokens)
            || !(4096..=262144).contains(&self.context_tokens)
            || self.max_output_tokens >= self.context_tokens
            || self
                .temperature
                .is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v))
            || self.instructions.len() > 16000
            || !matches!(
                self.reasoning_effort.as_str(),
                "" | "none" | "minimal" | "low" | "medium" | "high" | "xhigh"
            )
        {
            return Err(t!("assistant.invalid_parameters").to_string());
        }
        // 完整地址的协议必须匹配，避免把密钥发送到误拼接的路径。
        if self.endpoint.ends_with("/responses") && self.protocol != ApiProtocol::Responses
            || self.endpoint.ends_with("/chat/completions")
                && self.protocol != ApiProtocol::ChatCompletions
        {
            return Err(t!("assistant.invalid_endpoint").to_string());
        }
        Ok(self)
    }

    pub fn request_url(&self) -> String {
        let suffix = match self.protocol {
            ApiProtocol::Responses => "/responses",
            ApiProtocol::ChatCompletions => "/chat/completions",
        };
        if self.endpoint.ends_with(suffix) {
            self.endpoint.clone()
        } else {
            format!("{}{suffix}", self.endpoint)
        }
    }

    pub fn secret(&self) -> SecretRef {
        SecretRef::Assistant {
            endpoint: self.endpoint.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiProtocol, AssistantSettings, PermissionMode};

    #[test]
    fn validates_endpoints_parameters_and_keeps_secrets_out_of_settings() {
        let mut config = AssistantSettings {
            model: "custom-model".into(),
            ..Default::default()
        };
        assert_eq!(config.permission, PermissionMode::Approve);
        for endpoint in [
            "http://example.org/v1",
            "https://user:key@example.org",
            "https://example.org?key=x",
            "file:///tmp/model",
        ] {
            config.endpoint = endpoint.into();
            assert!(config.clone().validated().is_err());
        }
        config.endpoint = " http://127.0.0.1:1234/v1/ ".into();
        config.protocol = ApiProtocol::ChatCompletions;
        let config = config.validated().unwrap();
        assert_eq!(
            config.request_url(),
            "http://127.0.0.1:1234/v1/chat/completions"
        );
        assert!(!serde_json::to_string(&config).unwrap().contains("api_key"));
        let other = AssistantSettings {
            endpoint: "https://other.example/v1".into(),
            ..config.clone()
        };
        assert_ne!(config.secret(), other.secret());
        assert_eq!(
            serde_json::from_str::<AssistantSettings>("{}").unwrap(),
            AssistantSettings::default()
        );
    }
}
