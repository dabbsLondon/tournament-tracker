//! Duplicate Detector Agent.
//!
//! Identifies potential duplicate entries before storage.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::backend::{AiBackend, ChatMessage, ChatRequest};
use super::{Agent, AgentError, RetryPolicy};
use crate::models::EntityId;

/// Summary of an existing entity for comparison.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntitySummary {
    /// Entity ID
    pub id: EntityId,

    /// Entity type (event, placement, etc.)
    pub entity_type: String,

    /// Key identifying fields as JSON
    pub key_fields: serde_json::Value,
}

/// Input for the Duplicate Detector agent.
#[derive(Debug, Clone)]
pub struct DuplicateDetectorInput {
    /// Candidate entity to check
    pub candidate: serde_json::Value,

    /// Existing entities to compare against
    pub existing_entities: Vec<EntitySummary>,
}

/// Output from the Duplicate Detector agent.
#[derive(Debug, Clone)]
pub struct DuplicateDetectorOutput {
    /// Whether this is likely a duplicate
    pub is_duplicate: bool,

    /// ID of matching entity (if duplicate)
    pub matching_entity_id: Option<EntityId>,

    /// Similarity score (0.0 to 1.0)
    pub similarity_score: f32,

    /// Reasons for the match determination
    pub match_reasons: Vec<String>,
}

/// AI duplicate detection response.
#[derive(Debug, Deserialize)]
struct ExtractedDuplicateCheck {
    is_duplicate: bool,
    matching_index: Option<usize>,
    similarity_score: f32,
    match_reasons: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct DuplicateDetectorResponse {
    check: ExtractedDuplicateCheck,
}

/// Duplicate Detector agent implementation.
pub struct DuplicateDetectorAgent {
    backend: Arc<dyn AiBackend>,
}

impl DuplicateDetectorAgent {
    pub fn new(backend: Arc<dyn AiBackend>) -> Self {
        Self { backend }
    }

    fn build_prompt(
        &self,
        candidate: &serde_json::Value,
        existing: &[EntitySummary],
    ) -> Vec<ChatMessage> {
        let existing_json: Vec<serde_json::Value> = existing
            .iter()
            .enumerate()
            .map(|(i, e)| {
                serde_json::json!({
                    "index": i,
                    "id": e.id.as_str(),
                    "type": e.entity_type,
                    "fields": e.key_fields
                })
            })
            .collect();

        vec![
            ChatMessage::system(DUPLICATE_DETECTOR_SYSTEM_PROMPT),
            ChatMessage::user(format!(
                "Candidate entity:\n{}\n\nExisting entities:\n{}",
                serde_json::to_string_pretty(candidate).unwrap_or_default(),
                serde_json::to_string_pretty(&existing_json).unwrap_or_default()
            )),
        ]
    }

    fn parse_response(
        &self,
        response: &str,
        existing: &[EntitySummary],
    ) -> Result<DuplicateDetectorOutput, AgentError> {
        let parsed: DuplicateDetectorResponse = serde_json::from_str(response)
            .map_err(|e| AgentError::ResponseParseError(format!("Invalid JSON: {}", e)))?;

        let check = parsed.check;

        let matching_entity_id = check
            .matching_index
            .and_then(|idx| existing.get(idx))
            .map(|e| e.id.clone());

        Ok(DuplicateDetectorOutput {
            is_duplicate: check.is_duplicate,
            matching_entity_id,
            similarity_score: check.similarity_score.clamp(0.0, 1.0),
            match_reasons: check.match_reasons,
        })
    }
}

const DUPLICATE_DETECTOR_SYSTEM_PROMPT: &str = r#"You are checking if a new entity is a duplicate of existing entries.

Compare the candidate entity against each existing entity.
Consider these factors for similarity:

For Events:
- Name similarity (exact match, typos, abbreviations like "GT" vs "Grand Tournament")
- Date match (same day or within 3 days)
- Location match (same city, country)
- Player count similarity (within 10%)

For Placements:
- Same event
- Same player name (with typo tolerance)
- Same faction

For Army Lists:
- Same player
- Same faction
- Same total points (within 5%)

Return JSON in this exact format:
{
  "check": {
    "is_duplicate": true,
    "matching_index": 2,
    "similarity_score": 0.95,
    "match_reasons": [
      "Event name matches (London GT 2025)",
      "Same date",
      "Same location"
    ]
  }
}

If no match, return:
{
  "check": {
    "is_duplicate": false,
    "matching_index": null,
    "similarity_score": 0.0,
    "match_reasons": []
  }
}

Scoring guide:
- 0.9+ : Almost certainly a duplicate
- 0.7-0.9 : Likely duplicate, flag for review
- 0.5-0.7 : Possible duplicate, investigate
- 0.0-0.5 : Probably not a duplicate

IMPORTANT:
- Err on the side of flagging potential duplicates
- Name variations (typos, abbreviations) should still match
- Different year = not a duplicate (London GT 2024 != London GT 2025)
- Include clear reasons for the match determination"#;

#[async_trait]
impl Agent for DuplicateDetectorAgent {
    type Input = DuplicateDetectorInput;
    type Output = DuplicateDetectorOutput;

    fn name(&self) -> &'static str {
        "duplicate_detector"
    }

    async fn execute(&self, input: Self::Input) -> Result<Self::Output, AgentError> {
        if input.existing_entities.is_empty() {
            info!("No existing entities to compare against");
            return Ok(DuplicateDetectorOutput {
                is_duplicate: false,
                matching_entity_id: None,
                similarity_score: 0.0,
                match_reasons: vec![],
            });
        }

        info!(
            "Running Duplicate Detector against {} existing entities",
            input.existing_entities.len()
        );

        let messages = self.build_prompt(&input.candidate, &input.existing_entities);
        let request = ChatRequest::new(messages).with_json_mode();

        let response = self.backend.chat(request).await?;
        debug!("AI response: {}", response.content);

        let output = self.parse_response(&response.content, &input.existing_entities)?;

        if output.is_duplicate {
            info!(
                "Duplicate detected with score {:.2}: {:?}",
                output.similarity_score, output.matching_entity_id
            );
        } else {
            info!("No duplicate found");
        }

        Ok(output)
    }

    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_retries: 2,
            initial_delay_ms: 500,
            backoff_multiplier: 2.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::backend::MockBackend;

    fn mock_duplicate_response() -> &'static str {
        r#"{
            "check": {
                "is_duplicate": true,
                "matching_index": 0,
                "similarity_score": 0.95,
                "match_reasons": [
                    "Event name matches",
                    "Same date",
                    "Same location"
                ]
            }
        }"#
    }

    fn mock_no_duplicate_response() -> &'static str {
        r#"{
            "check": {
                "is_duplicate": false,
                "matching_index": null,
                "similarity_score": 0.2,
                "match_reasons": []
            }
        }"#
    }

    #[tokio::test]
    async fn test_duplicate_detector_finds_duplicate() {
        let backend = Arc::new(MockBackend::new(mock_duplicate_response()));
        let agent = DuplicateDetectorAgent::new(backend);

        let existing = vec![EntitySummary {
            id: EntityId::from("existing-123"),
            entity_type: "event".to_string(),
            key_fields: serde_json::json!({
                "name": "London GT 2025",
                "date": "2025-06-15"
            }),
        }];

        let input = DuplicateDetectorInput {
            candidate: serde_json::json!({
                "name": "London Grand Tournament 2025",
                "date": "2025-06-15"
            }),
            existing_entities: existing,
        };

        let output = agent.execute(input).await.unwrap();

        assert!(output.is_duplicate);
        assert!(output.matching_entity_id.is_some());
        assert!(output.similarity_score > 0.9);
        assert!(!output.match_reasons.is_empty());
    }

    #[tokio::test]
    async fn test_duplicate_detector_no_duplicate() {
        let backend = Arc::new(MockBackend::new(mock_no_duplicate_response()));
        let agent = DuplicateDetectorAgent::new(backend);

        let existing = vec![EntitySummary {
            id: EntityId::from("existing-123"),
            entity_type: "event".to_string(),
            key_fields: serde_json::json!({
                "name": "London GT 2024",
                "date": "2024-06-15"
            }),
        }];

        let input = DuplicateDetectorInput {
            candidate: serde_json::json!({
                "name": "Birmingham Open 2025",
                "date": "2025-07-20"
            }),
            existing_entities: existing,
        };

        let output = agent.execute(input).await.unwrap();

        assert!(!output.is_duplicate);
        assert!(output.matching_entity_id.is_none());
        assert!(output.similarity_score < 0.5);
    }

    #[tokio::test]
    async fn test_duplicate_detector_empty_existing() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = DuplicateDetectorAgent::new(backend);

        let input = DuplicateDetectorInput {
            candidate: serde_json::json!({"name": "Test Event"}),
            existing_entities: vec![],
        };

        let output = agent.execute(input).await.unwrap();

        assert!(!output.is_duplicate);
        assert_eq!(output.similarity_score, 0.0);
    }

    #[test]
    fn test_entity_summary_serialization() {
        let summary = EntitySummary {
            id: EntityId::from("test-123"),
            entity_type: "event".to_string(),
            key_fields: serde_json::json!({"name": "Test"}),
        };

        let json = serde_json::to_string(&summary).unwrap();
        assert!(json.contains("test-123"));

        let parsed: EntitySummary = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.entity_type, "event");
    }

    #[test]
    fn test_agent_name() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = DuplicateDetectorAgent::new(backend);
        assert_eq!(agent.name(), "duplicate_detector");
    }

    #[test]
    fn test_duplicate_detector_parse_response_duplicate() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = DuplicateDetectorAgent::new(backend);

        let existing = vec![EntitySummary {
            id: EntityId::from("existing-123"),
            entity_type: "event".to_string(),
            key_fields: serde_json::json!({"name": "London GT 2025"}),
        }];

        let output = agent
            .parse_response(mock_duplicate_response(), &existing)
            .unwrap();
        assert!(output.is_duplicate);
        assert_eq!(
            output.matching_entity_id,
            Some(EntityId::from("existing-123"))
        );
        assert!(output.similarity_score > 0.9);
    }

    #[test]
    fn test_duplicate_detector_parse_response_no_duplicate() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = DuplicateDetectorAgent::new(backend);

        let existing = vec![EntitySummary {
            id: EntityId::from("existing-123"),
            entity_type: "event".to_string(),
            key_fields: serde_json::json!({"name": "London GT 2024"}),
        }];

        let output = agent
            .parse_response(mock_no_duplicate_response(), &existing)
            .unwrap();
        assert!(!output.is_duplicate);
        assert!(output.matching_entity_id.is_none());
    }

    #[test]
    fn test_duplicate_detector_retry_policy() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = DuplicateDetectorAgent::new(backend);
        let policy = agent.retry_policy();
        assert_eq!(policy.max_retries, 2);
        assert_eq!(policy.initial_delay_ms, 500);
    }

    #[test]
    fn test_duplicate_detector_similarity_clamped() {
        let json = r#"{
            "check": {
                "is_duplicate": true,
                "matching_index": 0,
                "similarity_score": 1.5,
                "match_reasons": ["test"]
            }
        }"#;

        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = DuplicateDetectorAgent::new(backend);

        let existing = vec![EntitySummary {
            id: EntityId::from("test"),
            entity_type: "event".to_string(),
            key_fields: serde_json::json!({}),
        }];

        let output = agent.parse_response(json, &existing).unwrap();
        assert_eq!(output.similarity_score, 1.0); // Clamped to 1.0
    }
}
