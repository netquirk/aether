use async_openai::config::{Config, OpenAIConfig};
use reqwest::header::{AUTHORIZATION, HeaderMap};
use secrecy::SecretString;

use crate::ProviderAuthMode;

#[derive(Clone, Debug)]
pub struct AetherOpenAiConfig {
    inner: OpenAIConfig,
    auth_mode: ProviderAuthMode,
    extra_headers: Vec<(String, String)>,
}

impl AetherOpenAiConfig {
    pub fn new(inner: OpenAIConfig, auth_mode: ProviderAuthMode) -> Self {
        Self { inner, auth_mode, extra_headers: Vec::new() }
    }

    /// Add headers a provider needs on every request in addition to the API
    /// key — opencode-go's routing session id, for one. Applied AFTER the
    /// credential so a provider can never shadow the Authorization header.
    #[must_use]
    pub fn with_extra_headers(mut self, extra_headers: Vec<(String, String)>) -> Self {
        self.extra_headers = extra_headers;
        self
    }
}

impl Config for AetherOpenAiConfig {
    fn headers(&self) -> HeaderMap {
        let mut headers = self.inner.headers();
        if self.auth_mode == ProviderAuthMode::None {
            headers.remove(AUTHORIZATION);
        }
        for (name, value) in &self.extra_headers {
            // A malformed name/value is skipped rather than panicking: it is
            // provider configuration, and a bad one must not take down the
            // whole run with an unrelated panic.
            if let (Ok(name), Ok(value)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_str(value),
            ) {
                headers.insert(name, value);
            }
        }
        headers
    }

    fn url(&self, path: &str) -> String {
        self.inner.url(path)
    }

    fn query(&self) -> Vec<(&str, &str)> {
        self.inner.query()
    }

    fn api_base(&self) -> &str {
        self.inner.api_base()
    }

    fn api_key(&self) -> &SecretString {
        self.inner.api_key()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_auth_keeps_authorization_header() {
        let config = AetherOpenAiConfig::new(OpenAIConfig::new().with_api_key("token"), ProviderAuthMode::Default);
        assert!(config.headers().contains_key(AUTHORIZATION));
    }

    #[test]
    fn none_auth_removes_authorization_header() {
        let config = AetherOpenAiConfig::new(OpenAIConfig::new().with_api_key("token"), ProviderAuthMode::None);
        assert!(!config.headers().contains_key(AUTHORIZATION));
    }
}
