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
        AiBackendConfig::Anthropic { .. } => {
            unimplemented!("Anthropic backend not yet implemented")
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
}
