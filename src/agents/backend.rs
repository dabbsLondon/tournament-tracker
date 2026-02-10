//! AI backend abstraction.
//!
//! Supports multiple AI backends:
//! - Local: Ollama (default)
//! - Remote: OpenAI, Anthropic (feature-flagged)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::AgentError;

/// AI backend configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "backend")]
pub enum AiBackendConfig {
    /// Local Ollama instance
    #[serde(rename = "ollama")]
    Ollama {
        base_url: String,
        model: String,
        #[serde(default = "default_timeout")]
        timeout_seconds: u64,
    },

    /// OpenAI API (requires feature flag)
    #[cfg(feature = "remote-ai")]
    #[serde(rename = "openai")]
    OpenAi {
        api_key_env: String,
        model: String,
        #[serde(default = "default_timeout")]
        timeout_seconds: u64,
    },

    /// Anthropic API (requires feature flag)
    #[cfg(feature = "remote-ai")]
    #[serde(rename = "anthropic")]
    Anthropic {
        api_key_env: String,
        model: String,
        #[serde(default = "default_timeout")]
        timeout_seconds: u64,
    },
}

fn default_timeout() -> u64 {
    120
}

impl Default for AiBackendConfig {
    fn default() -> Self {
        AiBackendConfig::Ollama {
            base_url: "http://localhost:11434".to_string(),
            model: "llama3.2".to_string(),
            timeout_seconds: 120,
        }
    }
}

/// A message in a conversation with the AI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
        }
    }

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

/// Request to the AI backend.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub json_mode: bool,
}

impl ChatRequest {
    pub fn new(messages: Vec<ChatMessage>) -> Self {
        Self {
            messages,
            temperature: None,
            max_tokens: None,
            json_mode: false,
        }
    }

    pub fn with_json_mode(mut self) -> Self {
        self.json_mode = true;
        self
    }

    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }
}

/// Response from the AI backend.
#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub model: String,
    pub tokens_used: Option<TokenUsage>,
}

#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Trait for AI backends.
#[async_trait]
pub trait AiBackend: Send + Sync {
    /// Backend name for logging.
    fn name(&self) -> &'static str;

    /// Send a chat completion request.
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AgentError>;

    /// Check if the backend is available.
    async fn health_check(&self) -> Result<bool, AgentError>;
}

/// Ollama backend implementation.
pub struct OllamaBackend {
    client: reqwest::Client,
    base_url: String,
    model: String,
}

impl OllamaBackend {
    pub fn new(base_url: String, model: String, timeout_seconds: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_seconds))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url,
            model,
        }
    }

    pub fn from_config(config: &AiBackendConfig) -> Option<Self> {
        match config {
            AiBackendConfig::Ollama {
                base_url,
                model,
                timeout_seconds,
            } => Some(Self::new(base_url.clone(), model.clone(), *timeout_seconds)),
            #[cfg(feature = "remote-ai")]
            _ => None,
        }
    }
}

/// Ollama API request format.
#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<String>,
    options: OllamaOptions,
}

#[derive(Debug, Serialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize, Default)]
struct OllamaOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    num_predict: Option<u32>,
}

/// Ollama API response format.
#[derive(Debug, Deserialize)]
struct OllamaResponse {
    message: OllamaResponseMessage,
    model: String,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OllamaResponseMessage {
    content: String,
}

#[async_trait]
impl AiBackend for OllamaBackend {
    fn name(&self) -> &'static str {
        "ollama"
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AgentError> {
        let url = format!("{}/api/chat", self.base_url);

        let messages: Vec<OllamaMessage> = request
            .messages
            .into_iter()
            .map(|m| OllamaMessage {
                role: match m.role {
                    MessageRole::System => "system".to_string(),
                    MessageRole::User => "user".to_string(),
                    MessageRole::Assistant => "assistant".to_string(),
                },
                content: m.content,
            })
            .collect();

        let ollama_request = OllamaRequest {
            model: self.model.clone(),
            messages,
            stream: false,
            format: if request.json_mode {
                Some("json".to_string())
            } else {
                None
            },
            options: OllamaOptions {
                temperature: request.temperature,
                num_predict: request.max_tokens,
            },
        };

        debug!("Sending request to Ollama: {}", url);

        let response = self
            .client
            .post(&url)
            .json(&ollama_request)
            .send()
            .await
            .map_err(|e| AgentError::BackendUnavailable(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AgentError::BackendUnavailable(format!(
                "Ollama returned {}: {}",
                status, body
            )));
        }

        let ollama_response: OllamaResponse = response
            .json()
            .await
            .map_err(|e| AgentError::ResponseParseError(e.to_string()))?;

        let tokens_used = match (
            ollama_response.prompt_eval_count,
            ollama_response.eval_count,
        ) {
            (Some(prompt), Some(completion)) => Some(TokenUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt + completion,
            }),
            _ => None,
        };

        Ok(ChatResponse {
            content: ollama_response.message.content,
            model: ollama_response.model,
            tokens_used,
        })
    }

    async fn health_check(&self) -> Result<bool, AgentError> {
        let url = format!("{}/api/tags", self.base_url);

        match self.client.get(&url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(e) => {
                warn!("Ollama health check failed: {}", e);
                Ok(false)
            }
        }
    }
}

// --- Anthropic backend ---

#[cfg(feature = "remote-ai")]
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[cfg(feature = "remote-ai")]
#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[cfg(feature = "remote-ai")]
#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicContent>,
    model: String,
    #[serde(default)]
    usage: Option<AnthropicUsage>,
}

#[cfg(feature = "remote-ai")]
#[derive(Debug, Deserialize)]
struct AnthropicContent {
    text: String,
}

#[cfg(feature = "remote-ai")]
#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

/// Anthropic API backend implementation.
#[cfg(feature = "remote-ai")]
pub struct AnthropicBackend {
    client: reqwest::Client,
    model: String,
    api_key: String,
}

#[cfg(feature = "remote-ai")]
impl AnthropicBackend {
    pub fn new(api_key: String, model: String, timeout_seconds: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_seconds))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            model,
            api_key,
        }
    }

    pub fn from_env(model: String) -> Result<Self, AgentError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY").map_err(|_| {
            AgentError::BackendUnavailable("ANTHROPIC_API_KEY env var not set".to_string())
        })?;
        Ok(Self::new(api_key, model, 120))
    }
}

#[cfg(feature = "remote-ai")]
#[async_trait]
impl AiBackend for AnthropicBackend {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, AgentError> {
        let url = "https://api.anthropic.com/v1/messages";

        // Extract system messages into top-level system field
        let mut system_parts: Vec<String> = Vec::new();
        let mut messages: Vec<AnthropicMessage> = Vec::new();

        for msg in request.messages {
            match msg.role {
                MessageRole::System => {
                    system_parts.push(msg.content);
                }
                MessageRole::User => {
                    messages.push(AnthropicMessage {
                        role: "user".to_string(),
                        content: msg.content,
                    });
                }
                MessageRole::Assistant => {
                    messages.push(AnthropicMessage {
                        role: "assistant".to_string(),
                        content: msg.content,
                    });
                }
            }
        }

        // For json_mode, append instruction since Anthropic has no native JSON mode flag
        if request.json_mode {
            system_parts.push(
                "IMPORTANT: You must respond with valid JSON only. No other text.".to_string(),
            );
        }

        let system = if system_parts.is_empty() {
            None
        } else {
            Some(system_parts.join("\n\n"))
        };

        let max_tokens = request.max_tokens.unwrap_or(8192);

        let anthropic_request = AnthropicRequest {
            model: self.model.clone(),
            max_tokens,
            messages,
            system,
            temperature: request.temperature,
        };

        debug!("Sending request to Anthropic API");

        // Retry loop for rate limiting (429) with exponential backoff
        let max_retries = 5;
        let mut anthropic_response: Option<AnthropicResponse> = None;

        for attempt in 0..=max_retries {
            let response = self
                .client
                .post(url)
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&anthropic_request)
                .send()
                .await
                .map_err(|e| AgentError::BackendUnavailable(e.to_string()))?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                if attempt == max_retries {
                    let body = response.text().await.unwrap_or_default();
                    return Err(AgentError::BackendUnavailable(format!(
                        "Anthropic API rate limit after {} retries: {}",
                        max_retries, body
                    )));
                }

                // Parse retry-after header, default to exponential backoff
                let wait_secs = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(30 * (1 << attempt)); // 30s, 60s, 120s, 240s...

                warn!(
                    "Rate limited (attempt {}/{}), waiting {}s before retry",
                    attempt + 1,
                    max_retries,
                    wait_secs
                );
                tokio::time::sleep(std::time::Duration::from_secs(wait_secs)).await;
                continue;
            }

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                return Err(AgentError::BackendUnavailable(format!(
                    "Anthropic API returned {}: {}",
                    status, body
                )));
            }

            let body_text = response
                .text()
                .await
                .map_err(|e| AgentError::ResponseParseError(e.to_string()))?;

            match serde_json::from_str::<AnthropicResponse>(&body_text) {
                Ok(parsed) => {
                    anthropic_response = Some(parsed);
                    break;
                }
                Err(e) => {
                    warn!("Failed to parse Anthropic response: {}. Body: {}",
                        e, &body_text[..body_text.len().min(500)]);
                    return Err(AgentError::ResponseParseError(format!(
                        "Invalid JSON from Anthropic: {}", e
                    )));
                }
            }
        }

        let anthropic_response = anthropic_response
            .ok_or_else(|| AgentError::BackendUnavailable("No response after retries".to_string()))?;

        let content = anthropic_response
            .content
            .into_iter()
            .map(|c| c.text)
            .collect::<Vec<_>>()
            .join("");

        let tokens_used = anthropic_response.usage.map(|u| TokenUsage {
            prompt_tokens: u.input_tokens,
            completion_tokens: u.output_tokens,
            total_tokens: u.input_tokens + u.output_tokens,
        });

        Ok(ChatResponse {
            content,
            model: anthropic_response.model,
            tokens_used,
        })
    }

    async fn health_check(&self) -> Result<bool, AgentError> {
        // Anthropic has no health endpoint; assume available if key is set
        Ok(true)
    }
}

/// Create an AI backend from configuration.
pub fn create_backend(config: &AiBackendConfig) -> Box<dyn AiBackend> {
    match config {
        AiBackendConfig::Ollama {
            base_url,
            model,
            timeout_seconds,
        } => Box::new(OllamaBackend::new(
            base_url.clone(),
            model.clone(),
            *timeout_seconds,
        )),
        #[cfg(feature = "remote-ai")]
        AiBackendConfig::OpenAi { .. } => {
            unimplemented!("OpenAI backend not yet implemented")
        }
        #[cfg(feature = "remote-ai")]
        AiBackendConfig::Anthropic {
            api_key_env,
            model,
            timeout_seconds,
        } => {
            let api_key = std::env::var(api_key_env).unwrap_or_else(|_| {
                panic!("Environment variable {} not set", api_key_env);
            });
            Box::new(AnthropicBackend::new(
                api_key,
                model.clone(),
                *timeout_seconds,
            ))
        }
    }
}

/// Mock backend for testing.
#[cfg(test)]
pub struct MockBackend {
    response: String,
}

#[cfg(test)]
impl MockBackend {
    pub fn new(response: impl Into<String>) -> Self {
        Self {
            response: response.into(),
        }
    }
}

#[cfg(test)]
#[async_trait]
impl AiBackend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, AgentError> {
        Ok(ChatResponse {
            content: self.response.clone(),
            model: "mock".to_string(),
            tokens_used: None,
        })
    }

    async fn health_check(&self) -> Result<bool, AgentError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ai_backend_config_default() {
        let config = AiBackendConfig::default();
        match config {
            AiBackendConfig::Ollama {
                base_url, model, ..
            } => {
                assert_eq!(base_url, "http://localhost:11434");
                assert_eq!(model, "llama3.2");
            }
            #[cfg(feature = "remote-ai")]
            _ => panic!("Expected Ollama default"),
        }
    }

    #[test]
    fn test_chat_message_constructors() {
        let system = ChatMessage::system("You are helpful");
        assert_eq!(system.role, MessageRole::System);

        let user = ChatMessage::user("Hello");
        assert_eq!(user.role, MessageRole::User);

        let assistant = ChatMessage::assistant("Hi there");
        assert_eq!(assistant.role, MessageRole::Assistant);
    }

    #[test]
    fn test_chat_request_builder() {
        let request = ChatRequest::new(vec![ChatMessage::user("Test")])
            .with_json_mode()
            .with_temperature(0.7);

        assert!(request.json_mode);
        assert_eq!(request.temperature, Some(0.7));
    }

    #[tokio::test]
    async fn test_mock_backend() {
        let backend = MockBackend::new(r#"{"result": "test"}"#);

        let request = ChatRequest::new(vec![ChatMessage::user("Test")]);
        let response = backend.chat(request).await.unwrap();

        assert_eq!(response.content, r#"{"result": "test"}"#);
        assert!(backend.health_check().await.unwrap());
    }

    #[test]
    fn test_config_serialization() {
        let config = AiBackendConfig::Ollama {
            base_url: "http://localhost:11434".to_string(),
            model: "llama3.2".to_string(),
            timeout_seconds: 60,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("ollama"));

        let parsed: AiBackendConfig = serde_json::from_str(&json).unwrap();
        match parsed {
            AiBackendConfig::Ollama { model, .. } => assert_eq!(model, "llama3.2"),
            #[cfg(feature = "remote-ai")]
            _ => panic!("Expected Ollama"),
        }
    }

    #[cfg(feature = "remote-ai")]
    #[test]
    fn test_anthropic_request_serialization() {
        let request = AnthropicRequest {
            model: "claude-sonnet-4-20250514".to_string(),
            max_tokens: 4096,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
            system: Some("You are helpful".to_string()),
            temperature: Some(0.5),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("claude-sonnet-4-20250514"));
        assert!(json.contains("You are helpful"));
        assert!(json.contains("4096"));
    }

    #[cfg(feature = "remote-ai")]
    #[test]
    fn test_anthropic_response_deserialization() {
        let json = r#"{
            "content": [{"type": "text", "text": "{\"events\": []}"}],
            "model": "claude-sonnet-4-20250514",
            "usage": {"input_tokens": 100, "output_tokens": 50}
        }"#;

        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.content.len(), 1);
        assert_eq!(response.content[0].text, "{\"events\": []}");
        assert_eq!(response.model, "claude-sonnet-4-20250514");
        let usage = response.usage.unwrap();
        assert_eq!(usage.input_tokens, 100);
        assert_eq!(usage.output_tokens, 50);
    }

    #[cfg(feature = "remote-ai")]
    #[test]
    fn test_anthropic_response_without_usage() {
        let json = r#"{
            "content": [{"type": "text", "text": "hello"}],
            "model": "claude-sonnet-4-20250514"
        }"#;

        let response: AnthropicResponse = serde_json::from_str(json).unwrap();
        assert!(response.usage.is_none());
    }

    #[cfg(feature = "remote-ai")]
    #[test]
    fn test_anthropic_config_serialization() {
        let config = AiBackendConfig::Anthropic {
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            timeout_seconds: 120,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains("anthropic"));
        assert!(json.contains("ANTHROPIC_API_KEY"));

        let parsed: AiBackendConfig = serde_json::from_str(&json).unwrap();
        match parsed {
            AiBackendConfig::Anthropic { model, .. } => {
                assert_eq!(model, "claude-sonnet-4-20250514")
            }
            _ => panic!("Expected Anthropic"),
        }
    }
}
