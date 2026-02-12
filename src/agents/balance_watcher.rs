//! Balance Watcher Agent.
//!
//! Monitors Warhammer Community for balance updates and edition releases.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::backend::{AiBackend, ChatMessage, ChatRequest};
use super::{Agent, AgentError, AgentOutput, RetryPolicy};
use crate::models::{Confidence, SignificantEvent, SignificantEventId, SignificantEventType};

/// Input for the Balance Watcher agent.
#[derive(Debug, Clone)]
pub struct BalanceWatcherInput {
    /// HTML content from Warhammer Community
    pub html_content: String,

    /// URL that was fetched
    pub source_url: String,

    /// Known event IDs to skip (already tracked)
    pub known_event_ids: Vec<SignificantEventId>,
}

/// Output from the Balance Watcher agent.
#[derive(Debug, Clone)]
pub struct BalanceWatcherOutput {
    /// Newly discovered balance updates / edition releases
    pub events: Vec<AgentOutput<SignificantEvent>>,

    /// PDF URLs found (to be downloaded separately)
    pub pdf_urls: Vec<String>,
}

/// AI-extracted balance update data.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExtractedBalanceUpdate {
    title: String,
    date: Option<String>,
    event_type: String,
    pdf_url: Option<String>,
    summary: Option<String>,
    confidence: String,
}

#[derive(Debug, Deserialize)]
struct BalanceWatcherResponse {
    updates: Vec<ExtractedBalanceUpdate>,
}

/// Balance Watcher agent implementation.
pub struct BalanceWatcherAgent {
    backend: Arc<dyn AiBackend>,
}

impl BalanceWatcherAgent {
    pub fn new(backend: Arc<dyn AiBackend>) -> Self {
        Self { backend }
    }

    fn build_prompt(&self, html_content: &str) -> Vec<ChatMessage> {
        vec![
            ChatMessage::system(BALANCE_WATCHER_SYSTEM_PROMPT),
            ChatMessage::user(format!(
                "Analyze this Warhammer Community page content for balance updates:\n\n{}",
                html_content
            )),
        ]
    }

    fn parse_response(
        &self,
        response: &str,
        source_url: &str,
    ) -> Result<Vec<AgentOutput<SignificantEvent>>, AgentError> {
        let parsed: BalanceWatcherResponse = serde_json::from_str(response)
            .map_err(|e| AgentError::ResponseParseError(format!("Invalid JSON: {}", e)))?;

        let mut results = Vec::new();

        for update in parsed.updates {
            let event_type = match update.event_type.to_lowercase().as_str() {
                "balance_update" | "dataslate" | "balance" => SignificantEventType::BalanceUpdate,
                "edition_release" | "edition" | "new_edition" => {
                    SignificantEventType::EditionRelease
                }
                _ => SignificantEventType::BalanceUpdate,
            };

            let date = update
                .date
                .as_ref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .unwrap_or_else(|| chrono::Utc::now().date_naive());

            let mut event = SignificantEvent::new(
                event_type,
                date,
                update.title.clone(),
                source_url.to_string(),
            );

            if let Some(pdf_url) = update.pdf_url.clone() {
                event = event.with_pdf_url(pdf_url);
            }
            if let Some(summary) = update.summary.clone() {
                event = event.with_summary(summary);
            }

            let confidence = match update.confidence.to_lowercase().as_str() {
                "high" => Confidence::High,
                "medium" => Confidence::Medium,
                _ => Confidence::Low,
            };

            let mut notes = Vec::new();
            if update.date.is_none() {
                notes.push("Date not found in source, using current date".to_string());
            }
            if update.pdf_url.is_none() {
                notes.push("No PDF URL found".to_string());
            }

            results.push(AgentOutput::new(event, confidence).with_notes(notes));
        }

        Ok(results)
    }
}

const BALANCE_WATCHER_SYSTEM_PROMPT: &str = r#"You are analyzing a Warhammer Community webpage for balance updates and edition releases.

Look for:
1. "Balance Dataslate" announcements with PDF links
2. Edition release announcements (e.g., "10th Edition", "Index Update")
3. Major FAQ updates that affect competitive play

For each found, extract:
- title: Exact title as shown on page
- date: Publication date in YYYY-MM-DD format (null if not found)
- event_type: "balance_update" or "edition_release"
- pdf_url: Full URL to PDF download (null if not available)
- summary: Brief summary of key changes (null if unclear)
- confidence: "high", "medium", or "low" based on how clearly the info was stated

Return JSON in this exact format:
{
  "updates": [
    {
      "title": "Balance Dataslate Spring 2025",
      "date": "2025-03-15",
      "event_type": "balance_update",
      "pdf_url": "https://...",
      "summary": "Major changes to...",
      "confidence": "high"
    }
  ]
}

If no updates found, return: {"updates": []}

IMPORTANT:
- Only extract information clearly present on the page
- Do NOT invent or guess information
- Set confidence to "low" for any uncertain fields
- Include null for missing optional fields"#;

#[async_trait]
impl Agent for BalanceWatcherAgent {
    type Input = BalanceWatcherInput;
    type Output = BalanceWatcherOutput;

    fn name(&self) -> &'static str {
        "balance_watcher"
    }

    async fn execute(&self, input: Self::Input) -> Result<Self::Output, AgentError> {
        info!("Running Balance Watcher on {}", input.source_url);

        let messages = self.build_prompt(&input.html_content);
        let request = ChatRequest::new(messages).with_json_mode();

        let response = self.backend.chat(request).await?;
        debug!("AI response: {}", response.content);

        let events = self.parse_response(&response.content, &input.source_url)?;

        // Filter out known events
        let new_events: Vec<_> = events
            .into_iter()
            .filter(|e| !input.known_event_ids.contains(&e.data.id))
            .collect();

        // Extract PDF URLs for download
        let pdf_urls: Vec<String> = new_events
            .iter()
            .filter_map(|e| e.data.pdf_url.clone())
            .collect();

        info!(
            "Balance Watcher found {} new events, {} PDFs",
            new_events.len(),
            pdf_urls.len()
        );

        Ok(BalanceWatcherOutput {
            events: new_events,
            pdf_urls,
        })
    }

    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            initial_delay_ms: 2000,
            backoff_multiplier: 2.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::backend::MockBackend;

    fn mock_response() -> &'static str {
        r#"{
            "updates": [
                {
                    "title": "Balance Dataslate Spring 2025",
                    "date": "2025-03-15",
                    "event_type": "balance_update",
                    "pdf_url": "https://example.com/dataslate.pdf",
                    "summary": "Major nerfs to Aeldari",
                    "confidence": "high"
                },
                {
                    "title": "10th Edition FAQ",
                    "date": null,
                    "event_type": "balance_update",
                    "pdf_url": null,
                    "summary": null,
                    "confidence": "medium"
                }
            ]
        }"#
    }

    #[tokio::test]
    async fn test_balance_watcher_extraction() {
        let backend = Arc::new(MockBackend::new(mock_response()));
        let agent = BalanceWatcherAgent::new(backend);

        let input = BalanceWatcherInput {
            html_content: "<html>...</html>".to_string(),
            source_url: "https://warhammer-community.com/updates".to_string(),
            known_event_ids: vec![],
        };

        let output = agent.execute(input).await.unwrap();

        assert_eq!(output.events.len(), 2);
        assert_eq!(output.pdf_urls.len(), 1);

        let first_event = &output.events[0];
        assert_eq!(first_event.data.title, "Balance Dataslate Spring 2025");
        assert_eq!(first_event.confidence, Confidence::High);
    }

    #[tokio::test]
    async fn test_balance_watcher_filters_known() {
        let backend = Arc::new(MockBackend::new(mock_response()));
        let agent = BalanceWatcherAgent::new(backend);

        // Get the ID of the first event by running once
        let first_input = BalanceWatcherInput {
            html_content: "<html>...</html>".to_string(),
            source_url: "https://warhammer-community.com/updates".to_string(),
            known_event_ids: vec![],
        };
        let first_output = agent.execute(first_input).await.unwrap();
        let known_id = first_output.events[0].data.id.clone();

        // Create new agent with fresh mock (since mock doesn't reset)
        let backend2 = Arc::new(MockBackend::new(mock_response()));
        let agent2 = BalanceWatcherAgent::new(backend2);

        // Run again with known ID
        let input = BalanceWatcherInput {
            html_content: "<html>...</html>".to_string(),
            source_url: "https://warhammer-community.com/updates".to_string(),
            known_event_ids: vec![known_id],
        };

        let output = agent2.execute(input).await.unwrap();
        assert_eq!(output.events.len(), 1); // Only the FAQ, not the dataslate
    }

    #[tokio::test]
    async fn test_balance_watcher_empty_response() {
        let backend = Arc::new(MockBackend::new(r#"{"updates": []}"#));
        let agent = BalanceWatcherAgent::new(backend);

        let input = BalanceWatcherInput {
            html_content: "<html>No updates here</html>".to_string(),
            source_url: "https://warhammer-community.com".to_string(),
            known_event_ids: vec![],
        };

        let output = agent.execute(input).await.unwrap();
        assert!(output.events.is_empty());
        assert!(output.pdf_urls.is_empty());
    }

    #[test]
    fn test_agent_name() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = BalanceWatcherAgent::new(backend);
        assert_eq!(agent.name(), "balance_watcher");
    }

    #[test]
    fn test_balance_watcher_parse_response() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = BalanceWatcherAgent::new(backend);

        let events = agent
            .parse_response(mock_response(), "https://example.com")
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data.title, "Balance Dataslate Spring 2025");
        assert_eq!(events[0].confidence, Confidence::High);
    }

    #[test]
    fn test_balance_watcher_retry_policy() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = BalanceWatcherAgent::new(backend);
        let policy = agent.retry_policy();
        assert_eq!(policy.max_retries, 3);
        assert_eq!(policy.initial_delay_ms, 2000);
    }
}
