//! AI-powered extraction agents.
//!
//! Agents extract structured data from unstructured content (HTML, PDFs)
//! using AI models. All agents implement the `Agent` trait.

use async_trait::async_trait;
use thiserror::Error;

use crate::models::Confidence;

/// Errors that can occur during agent execution.
#[derive(Debug, Error)]
pub enum AgentError {
    #[error("AI backend unavailable: {0}")]
    BackendUnavailable(String),

    #[error("AI response unparseable: {0}")]
    ResponseParseError(String),

    #[error("AI refused to extract (content unclear): {0}")]
    ExtractionRefused(String),

    #[error("Timeout after {0} seconds")]
    Timeout(u64),

    #[error("Rate limited, retry after {0} seconds")]
    RateLimited(u64),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Retry policy for agents.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub initial_delay_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
        }
    }
}

/// Output wrapper with confidence and metadata.
#[derive(Debug, Clone)]
pub struct AgentOutput<T> {
    pub data: T,
    pub confidence: Confidence,
    pub extraction_notes: Vec<String>,
}

impl<T> AgentOutput<T> {
    pub fn new(data: T, confidence: Confidence) -> Self {
        Self {
            data,
            confidence,
            extraction_notes: Vec::new(),
        }
    }

    pub fn with_notes(mut self, notes: Vec<String>) -> Self {
        self.extraction_notes = notes;
        self
    }
}

/// Core trait for all AI agents.
#[async_trait]
pub trait Agent {
    type Input;
    type Output;

    /// Agent identifier for logging and metrics.
    fn name(&self) -> &'static str;

    /// Execute the agent's task.
    async fn execute(&self, input: Self::Input) -> Result<Self::Output, AgentError>;

    /// Retry policy for this agent.
    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_policy_default() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.initial_delay_ms, 1000);
    }

    #[test]
    fn test_agent_output() {
        let output =
            AgentOutput::new("test data", Confidence::High).with_notes(vec!["note 1".to_string()]);

        assert_eq!(output.data, "test data");
        assert_eq!(output.confidence, Confidence::High);
        assert_eq!(output.extraction_notes.len(), 1);
    }
}
