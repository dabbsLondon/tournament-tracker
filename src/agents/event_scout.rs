//! Event Scout Agent.
//!
//! Discovers tournament events from Goonhammer Competitive Innovations articles.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::backend::{AiBackend, ChatMessage, ChatRequest};
use super::{Agent, AgentError, AgentOutput, RetryPolicy};
use crate::models::Confidence;

/// Stub for a discovered event (before full extraction).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventStub {
    /// Event name as written in article
    pub name: String,

    /// Event date if mentioned
    pub date: Option<NaiveDate>,

    /// Location (city, country)
    pub location: Option<String>,

    /// Number of players
    pub player_count: Option<u32>,

    /// Number of rounds
    pub round_count: Option<u32>,

    /// Event type (GT, Major, RTT, etc.)
    pub event_type: Option<String>,

    /// Section of article where this event was found
    pub article_section: Option<String>,
}

/// Input for the Event Scout agent.
#[derive(Debug, Clone)]
pub struct EventScoutInput {
    /// HTML content from Goonhammer article
    pub article_html: String,

    /// URL of the article
    pub article_url: String,

    /// Publication date of the article
    pub article_date: NaiveDate,
}

/// Output from the Event Scout agent.
#[derive(Debug, Clone)]
pub struct EventScoutOutput {
    /// Discovered event stubs
    pub events: Vec<AgentOutput<EventStub>>,
}

/// AI-extracted event data.
#[derive(Debug, Deserialize)]
struct ExtractedEvent {
    name: String,
    date: Option<String>,
    location: Option<String>,
    player_count: Option<u32>,
    round_count: Option<u32>,
    event_type: Option<String>,
    article_section: Option<String>,
    confidence: String,
}

#[derive(Debug, Deserialize)]
struct EventScoutResponse {
    events: Vec<ExtractedEvent>,
}

/// Event Scout agent implementation.
pub struct EventScoutAgent {
    backend: Arc<dyn AiBackend>,
}

impl EventScoutAgent {
    pub fn new(backend: Arc<dyn AiBackend>) -> Self {
        Self { backend }
    }

    fn build_prompt(&self, html_content: &str, article_date: NaiveDate) -> Vec<ChatMessage> {
        vec![
            ChatMessage::system(EVENT_SCOUT_SYSTEM_PROMPT),
            ChatMessage::user(format!(
                "Article date: {}\n\nArticle content:\n\n{}",
                article_date, html_content
            )),
        ]
    }

    fn parse_response(&self, response: &str) -> Result<Vec<AgentOutput<EventStub>>, AgentError> {
        let json = super::extract_json(response);
        let parsed: EventScoutResponse = serde_json::from_str(json).map_err(|e| {
            tracing::warn!(
                "Event Scout JSON parse error. Response start: {}",
                &response[..response.len().min(200)]
            );
            AgentError::ResponseParseError(format!("Invalid JSON: {}", e))
        })?;

        let mut results = Vec::new();

        for event in parsed.events {
            let date = event
                .date
                .as_ref()
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());

            let stub = EventStub {
                name: event.name,
                date,
                location: event.location,
                player_count: event.player_count,
                round_count: event.round_count,
                event_type: event.event_type,
                article_section: event.article_section,
            };

            let confidence = match event.confidence.to_lowercase().as_str() {
                "high" => Confidence::High,
                "medium" => Confidence::Medium,
                _ => Confidence::Low,
            };

            let mut notes = Vec::new();
            if stub.date.is_none() {
                notes.push("Event date not specified".to_string());
            }
            if stub.player_count.is_none() {
                notes.push("Player count not found".to_string());
            }

            results.push(AgentOutput::new(stub, confidence).with_notes(notes));
        }

        Ok(results)
    }
}

const EVENT_SCOUT_SYSTEM_PROMPT: &str = r#"You are extracting tournament information from a Goonhammer Competitive Innovations article.

For each tournament mentioned, extract:
- name: Exact event name as written
- date: Event date in YYYY-MM-DD format (null if not found, NOT the article date)
- location: City, country if available (e.g., "London, UK")
- player_count: Number of players as integer
- round_count: Number of rounds as integer
- event_type: "GT", "Major", "RTT", "Open", etc. (null if unclear)
- article_section: Which section of article covers this event (for tracking)
- confidence: "high", "medium", or "low"

Tournaments are typically introduced with phrases like:
- "X-player, Y-round Major/GT in [Location]"
- "The [Event Name] was held..."
- "[Event Name] Results"

Return JSON in this exact format:
{
  "events": [
    {
      "name": "London GT 2025",
      "date": "2025-06-15",
      "location": "London, UK",
      "player_count": 96,
      "round_count": 5,
      "event_type": "GT",
      "article_section": "London GT Results",
      "confidence": "high"
    }
  ]
}

If no events found, return: {"events": []}

IMPORTANT:
- Do NOT confuse article publication date with event date
- Only extract events clearly mentioned with results
- Use null for any field not explicitly stated
- Do NOT invent player counts or locations
- Set confidence to "low" for uncertain extractions"#;

#[async_trait]
impl Agent for EventScoutAgent {
    type Input = EventScoutInput;
    type Output = EventScoutOutput;

    fn name(&self) -> &'static str {
        "event_scout"
    }

    async fn execute(&self, input: Self::Input) -> Result<Self::Output, AgentError> {
        info!("Running Event Scout on {}", input.article_url);

        let messages = self.build_prompt(&input.article_html, input.article_date);
        let request = ChatRequest::new(messages).with_json_mode();

        let response = self.backend.chat(request).await?;
        debug!("AI response: {}", response.content);

        let events = self.parse_response(&response.content)?;

        info!("Event Scout found {} events", events.len());

        Ok(EventScoutOutput { events })
    }

    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_retries: 3,
            initial_delay_ms: 1000,
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
            "events": [
                {
                    "name": "London GT 2025",
                    "date": "2025-06-15",
                    "location": "London, UK",
                    "player_count": 96,
                    "round_count": 5,
                    "event_type": "GT",
                    "article_section": "London GT Results",
                    "confidence": "high"
                },
                {
                    "name": "Birmingham Open",
                    "date": null,
                    "location": "Birmingham, UK",
                    "player_count": 48,
                    "round_count": null,
                    "event_type": "Open",
                    "article_section": null,
                    "confidence": "medium"
                }
            ]
        }"#
    }

    #[tokio::test]
    async fn test_event_scout_extraction() {
        let backend = Arc::new(MockBackend::new(mock_response()));
        let agent = EventScoutAgent::new(backend);

        let input = EventScoutInput {
            article_html: "<html>Tournament results...</html>".to_string(),
            article_url: "https://goonhammer.com/competitive-innovations".to_string(),
            article_date: NaiveDate::from_ymd_opt(2025, 6, 20).unwrap(),
        };

        let output = agent.execute(input).await.unwrap();

        assert_eq!(output.events.len(), 2);

        let london_gt = &output.events[0];
        assert_eq!(london_gt.data.name, "London GT 2025");
        assert_eq!(london_gt.data.player_count, Some(96));
        assert_eq!(london_gt.confidence, Confidence::High);

        let birmingham = &output.events[1];
        assert!(birmingham.data.date.is_none());
        assert_eq!(birmingham.confidence, Confidence::Medium);
    }

    #[tokio::test]
    async fn test_event_scout_empty() {
        let backend = Arc::new(MockBackend::new(r#"{"events": []}"#));
        let agent = EventScoutAgent::new(backend);

        let input = EventScoutInput {
            article_html: "<html>No tournaments here</html>".to_string(),
            article_url: "https://goonhammer.com/article".to_string(),
            article_date: NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
        };

        let output = agent.execute(input).await.unwrap();
        assert!(output.events.is_empty());
    }

    #[test]
    fn test_event_stub_serialization() {
        let stub = EventStub {
            name: "Test GT".to_string(),
            date: Some(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap()),
            location: Some("London".to_string()),
            player_count: Some(100),
            round_count: Some(5),
            event_type: Some("GT".to_string()),
            article_section: None,
        };

        let json = serde_json::to_string(&stub).unwrap();
        assert!(json.contains("Test GT"));

        let parsed: EventStub = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "Test GT");
    }

    #[test]
    fn test_agent_name() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = EventScoutAgent::new(backend);
        assert_eq!(agent.name(), "event_scout");
    }
}
