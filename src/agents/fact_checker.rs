//! Fact Checker Agent.
//!
//! Verifies extracted data against the original source content.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use super::backend::{AiBackend, ChatMessage, ChatRequest};
use super::{Agent, AgentError, RetryPolicy};
use crate::models::Confidence;

/// Severity of a discrepancy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Minor issues (typos, formatting)
    Minor,
    /// Major issues (wrong values)
    Major,
    /// Critical issues (fabricated data)
    Critical,
}

/// A discrepancy found during fact checking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discrepancy {
    /// Field that has the issue
    pub field: String,

    /// Value that was extracted
    pub extracted_value: String,

    /// Evidence from source (if found)
    pub source_evidence: Option<String>,

    /// Severity of the discrepancy
    pub severity: Severity,

    /// Description of the issue
    pub description: String,
}

/// Suggested correction for a discrepancy.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Correction {
    /// Field to correct
    pub field: String,

    /// Suggested new value
    pub suggested_value: String,

    /// Confidence in the correction
    pub confidence: Confidence,
}

/// Entity type being fact-checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntityType {
    Event,
    Placement,
    ArmyList,
    SignificantEvent,
}

/// Input for the Fact Checker agent.
#[derive(Debug, Clone)]
pub struct FactCheckerInput {
    /// Original source content
    pub source_content: String,

    /// Extracted data as JSON
    pub extracted_data: serde_json::Value,

    /// Type of entity being checked
    pub entity_type: EntityType,
}

/// Output from the Fact Checker agent.
#[derive(Debug, Clone)]
pub struct FactCheckerOutput {
    /// Whether the data was verified
    pub verified: bool,

    /// Discrepancies found
    pub discrepancies: Vec<Discrepancy>,

    /// Suggested corrections
    pub corrections: Vec<Correction>,

    /// Overall confidence in the extracted data
    pub overall_confidence: Confidence,
}

/// AI verification response.
#[derive(Debug, Deserialize)]
struct ExtractedVerification {
    verified: bool,
    discrepancies: Vec<ExtractedDiscrepancy>,
    corrections: Vec<ExtractedCorrection>,
    overall_confidence: String,
}

#[derive(Debug, Deserialize)]
struct ExtractedDiscrepancy {
    field: String,
    extracted_value: String,
    source_evidence: Option<String>,
    severity: String,
    description: String,
}

#[derive(Debug, Deserialize)]
struct ExtractedCorrection {
    field: String,
    suggested_value: String,
    confidence: String,
}

#[derive(Debug, Deserialize)]
struct FactCheckerResponse {
    verification: ExtractedVerification,
}

/// Fact Checker agent implementation.
pub struct FactCheckerAgent {
    backend: Arc<dyn AiBackend>,
}

impl FactCheckerAgent {
    pub fn new(backend: Arc<dyn AiBackend>) -> Self {
        Self { backend }
    }

    fn build_prompt(
        &self,
        source_content: &str,
        extracted_data: &serde_json::Value,
        entity_type: EntityType,
    ) -> Vec<ChatMessage> {
        let entity_name = match entity_type {
            EntityType::Event => "tournament event",
            EntityType::Placement => "player placement",
            EntityType::ArmyList => "army list",
            EntityType::SignificantEvent => "balance update/edition release",
        };

        vec![
            ChatMessage::system(FACT_CHECKER_SYSTEM_PROMPT),
            ChatMessage::user(format!(
                "Entity type: {}\n\nExtracted data:\n{}\n\nSource content:\n{}",
                entity_name,
                serde_json::to_string_pretty(extracted_data).unwrap_or_default(),
                source_content
            )),
        ]
    }

    fn parse_response(&self, response: &str) -> Result<FactCheckerOutput, AgentError> {
        let parsed: FactCheckerResponse = serde_json::from_str(response)
            .map_err(|e| AgentError::ResponseParseError(format!("Invalid JSON: {}", e)))?;

        let verification = parsed.verification;

        let discrepancies: Vec<Discrepancy> = verification
            .discrepancies
            .into_iter()
            .map(|d| Discrepancy {
                field: d.field,
                extracted_value: d.extracted_value,
                source_evidence: d.source_evidence,
                severity: match d.severity.to_lowercase().as_str() {
                    "minor" => Severity::Minor,
                    "major" => Severity::Major,
                    _ => Severity::Critical,
                },
                description: d.description,
            })
            .collect();

        let corrections: Vec<Correction> = verification
            .corrections
            .into_iter()
            .map(|c| Correction {
                field: c.field,
                suggested_value: c.suggested_value,
                confidence: match c.confidence.to_lowercase().as_str() {
                    "high" => Confidence::High,
                    "medium" => Confidence::Medium,
                    _ => Confidence::Low,
                },
            })
            .collect();

        let overall_confidence = match verification.overall_confidence.to_lowercase().as_str() {
            "high" => Confidence::High,
            "medium" => Confidence::Medium,
            _ => Confidence::Low,
        };

        Ok(FactCheckerOutput {
            verified: verification.verified,
            discrepancies,
            corrections,
            overall_confidence,
        })
    }
}

const FACT_CHECKER_SYSTEM_PROMPT: &str = r#"You are fact-checking extracted data against the original source.

Compare the extracted JSON against the source content carefully.
For each field in the extracted data, verify it matches the source.

Report:
- Fields that match exactly (don't list these)
- Discrepancies with their severity:
  - "minor": Typos, formatting differences, abbreviations
  - "major": Wrong values, misattributed data
  - "critical": Fabricated data not in source at all
- Suggested corrections when possible

Return JSON in this exact format:
{
  "verification": {
    "verified": true,
    "discrepancies": [
      {
        "field": "player_name",
        "extracted_value": "John Smyth",
        "source_evidence": "John Smith placed first...",
        "severity": "minor",
        "description": "Name spelling differs from source"
      }
    ],
    "corrections": [
      {
        "field": "player_name",
        "suggested_value": "John Smith",
        "confidence": "high"
      }
    ],
    "overall_confidence": "high"
  }
}

Set verified=true if:
- No critical discrepancies
- No more than 2 major discrepancies
- Overall data is accurate

Set verified=false if:
- Any critical discrepancies (fabricated data)
- More than 2 major discrepancies
- Core identifying fields are wrong

IMPORTANT:
- Be strict: if you can't find evidence for a claim, flag it as critical
- Consider variations in formatting/abbreviations as minor
- Wrong faction/player names are major or critical
- Include the source evidence when possible"#;

#[async_trait]
impl Agent for FactCheckerAgent {
    type Input = FactCheckerInput;
    type Output = FactCheckerOutput;

    fn name(&self) -> &'static str {
        "fact_checker"
    }

    async fn execute(&self, input: Self::Input) -> Result<Self::Output, AgentError> {
        info!("Running Fact Checker for {:?}", input.entity_type);

        let messages = self.build_prompt(
            &input.source_content,
            &input.extracted_data,
            input.entity_type,
        );
        let request = ChatRequest::new(messages).with_json_mode();

        let response = self.backend.chat(request).await?;
        debug!("AI response: {}", response.content);

        let output = self.parse_response(&response.content)?;

        if output.verified {
            info!(
                "Fact check passed with {:?} confidence",
                output.overall_confidence
            );
        } else {
            warn!(
                "Fact check failed: {} discrepancies found",
                output.discrepancies.len()
            );
        }

        Ok(output)
    }

    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy {
            max_retries: 2,
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::backend::MockBackend;

    fn mock_verified_response() -> &'static str {
        r#"{
            "verification": {
                "verified": true,
                "discrepancies": [],
                "corrections": [],
                "overall_confidence": "high"
            }
        }"#
    }

    fn mock_failed_response() -> &'static str {
        r#"{
            "verification": {
                "verified": false,
                "discrepancies": [
                    {
                        "field": "faction",
                        "extracted_value": "Eldar",
                        "source_evidence": "The Aeldari player...",
                        "severity": "major",
                        "description": "Old faction name used instead of Aeldari"
                    },
                    {
                        "field": "player_count",
                        "extracted_value": "100",
                        "source_evidence": null,
                        "severity": "critical",
                        "description": "Player count not mentioned in source"
                    }
                ],
                "corrections": [
                    {
                        "field": "faction",
                        "suggested_value": "Aeldari",
                        "confidence": "high"
                    }
                ],
                "overall_confidence": "low"
            }
        }"#
    }

    #[tokio::test]
    async fn test_fact_checker_verified() {
        let backend = Arc::new(MockBackend::new(mock_verified_response()));
        let agent = FactCheckerAgent::new(backend);

        let input = FactCheckerInput {
            source_content: "John Smith won the event with Aeldari".to_string(),
            extracted_data: serde_json::json!({
                "player_name": "John Smith",
                "faction": "Aeldari"
            }),
            entity_type: EntityType::Placement,
        };

        let output = agent.execute(input).await.unwrap();

        assert!(output.verified);
        assert!(output.discrepancies.is_empty());
        assert_eq!(output.overall_confidence, Confidence::High);
    }

    #[tokio::test]
    async fn test_fact_checker_failed() {
        let backend = Arc::new(MockBackend::new(mock_failed_response()));
        let agent = FactCheckerAgent::new(backend);

        let input = FactCheckerInput {
            source_content: "The Aeldari player...".to_string(),
            extracted_data: serde_json::json!({
                "faction": "Eldar",
                "player_count": 100
            }),
            entity_type: EntityType::Event,
        };

        let output = agent.execute(input).await.unwrap();

        assert!(!output.verified);
        assert_eq!(output.discrepancies.len(), 2);
        assert_eq!(output.corrections.len(), 1);
        assert_eq!(output.overall_confidence, Confidence::Low);

        let critical = output
            .discrepancies
            .iter()
            .find(|d| d.severity == Severity::Critical);
        assert!(critical.is_some());
    }

    #[test]
    fn test_severity_serialization() {
        let severity = Severity::Major;
        let json = serde_json::to_string(&severity).unwrap();
        assert_eq!(json, "\"major\"");
    }

    #[test]
    fn test_entity_type_serialization() {
        let et = EntityType::Placement;
        let json = serde_json::to_string(&et).unwrap();
        assert_eq!(json, "\"placement\"");
    }

    #[test]
    fn test_agent_name() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = FactCheckerAgent::new(backend);
        assert_eq!(agent.name(), "fact_checker");
    }

    #[test]
    fn test_fact_checker_parse_verified_response() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = FactCheckerAgent::new(backend);

        let output = agent.parse_response(mock_verified_response()).unwrap();
        assert!(output.verified);
        assert!(output.discrepancies.is_empty());
        assert_eq!(output.overall_confidence, Confidence::High);
    }

    #[test]
    fn test_fact_checker_parse_failed_response() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = FactCheckerAgent::new(backend);

        let output = agent.parse_response(mock_failed_response()).unwrap();
        assert!(!output.verified);
        assert_eq!(output.discrepancies.len(), 2);
        assert_eq!(output.corrections.len(), 1);
    }

    #[test]
    fn test_fact_checker_retry_policy() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = FactCheckerAgent::new(backend);
        let policy = agent.retry_policy();
        assert_eq!(policy.max_retries, 2);
    }

    #[test]
    fn test_discrepancy_serialization() {
        let disc = Discrepancy {
            field: "faction".to_string(),
            extracted_value: "Eldar".to_string(),
            source_evidence: Some("Aeldari player".to_string()),
            severity: Severity::Major,
            description: "Wrong name".to_string(),
        };

        let json = serde_json::to_string(&disc).unwrap();
        let parsed: Discrepancy = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.field, "faction");
        assert_eq!(parsed.severity, Severity::Major);
    }
}
