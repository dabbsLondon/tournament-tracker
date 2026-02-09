//! List Normalizer Agent.
//!
//! Converts raw army list text to canonical structured format.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::backend::{AiBackend, ChatMessage, ChatRequest};
use super::{Agent, AgentError, AgentOutput, RetryPolicy};
use crate::models::{Confidence, Unit};

/// Input for the List Normalizer agent.
#[derive(Debug, Clone)]
pub struct ListNormalizerInput {
    /// Raw army list text
    pub raw_text: String,

    /// Faction hint from placement (helps accuracy)
    pub faction_hint: Option<String>,

    /// Player name (for tracking)
    pub player_name: String,
}

/// Normalized army list (intermediate before full ArmyList).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NormalizedArmyList {
    pub faction: String,
    pub subfaction: Option<String>,
    pub detachment: Option<String>,
    pub total_points: u32,
    pub units: Vec<Unit>,
    pub raw_text: String,
}

/// Output from the List Normalizer agent.
#[derive(Debug, Clone)]
pub struct ListNormalizerOutput {
    /// Normalized army list
    pub list: AgentOutput<NormalizedArmyList>,
}

/// AI-extracted unit data.
#[derive(Debug, Deserialize)]
struct ExtractedUnit {
    name: String,
    model_count: Option<u32>,
    points: Option<u32>,
    wargear: Vec<String>,
    keywords: Vec<String>,
}

/// AI-extracted army list data.
#[derive(Debug, Deserialize)]
struct ExtractedList {
    faction: String,
    subfaction: Option<String>,
    detachment: Option<String>,
    total_points: Option<u32>,
    units: Vec<ExtractedUnit>,
    confidence: String,
    notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ListNormalizerResponse {
    list: ExtractedList,
}

/// List Normalizer agent implementation.
pub struct ListNormalizerAgent {
    backend: Arc<dyn AiBackend>,
}

impl ListNormalizerAgent {
    pub fn new(backend: Arc<dyn AiBackend>) -> Self {
        Self { backend }
    }

    fn build_prompt(&self, raw_text: &str, faction_hint: Option<&str>) -> Vec<ChatMessage> {
        let hint_text = faction_hint
            .map(|f| format!("\nFaction hint: {}", f))
            .unwrap_or_default();

        vec![
            ChatMessage::system(LIST_NORMALIZER_SYSTEM_PROMPT),
            ChatMessage::user(format!("{}Raw army list:\n\n{}", hint_text, raw_text)),
        ]
    }

    fn parse_response(
        &self,
        response: &str,
        raw_text: &str,
    ) -> Result<AgentOutput<NormalizedArmyList>, AgentError> {
        let parsed: ListNormalizerResponse = serde_json::from_str(response)
            .map_err(|e| AgentError::ResponseParseError(format!("Invalid JSON: {}", e)))?;

        let extracted = parsed.list;

        let units: Vec<Unit> = extracted
            .units
            .into_iter()
            .map(|u| {
                Unit::new(u.name, u.model_count.unwrap_or(1))
                    .with_points(u.points.unwrap_or(0))
                    .with_wargear(u.wargear)
                    .with_keywords(u.keywords)
            })
            .collect();

        let total_points = extracted
            .total_points
            .unwrap_or_else(|| units.iter().filter_map(|u| u.points).sum());

        let army_list = NormalizedArmyList {
            faction: extracted.faction,
            subfaction: extracted.subfaction,
            detachment: extracted.detachment,
            total_points,
            units,
            raw_text: raw_text.to_string(),
        };

        let confidence = match extracted.confidence.to_lowercase().as_str() {
            "high" => Confidence::High,
            "medium" => Confidence::Medium,
            _ => Confidence::Low,
        };

        Ok(AgentOutput::new(army_list, confidence).with_notes(extracted.notes))
    }
}

const LIST_NORMALIZER_SYSTEM_PROMPT: &str = r#"You are normalizing a Warhammer 40,000 army list into a structured format.

Given raw list text, extract:
- faction: Main faction (canonical GW name)
- subfaction: Subfaction if applicable (Chapter, Craftworld, etc.)
- detachment: Detachment name
- total_points: Total army points
- units: Array of units with:
  - name: Unit name (canonical GW name)
  - model_count: Number of models (default 1)
  - points: Points cost
  - wargear: Array of selected wargear/upgrades
  - keywords: Array of relevant keywords
- confidence: "high", "medium", or "low"
- notes: Array of any issues or uncertainties

Handle various list formats:
- Battlescribe exports
- New Recruit exports
- Official app exports
- Plain text lists
- Abbreviated/shorthand notation

Return JSON in this exact format:
{
  "list": {
    "faction": "Aeldari",
    "subfaction": "Craftworld Ulthwe",
    "detachment": "Battle Host",
    "total_points": 2000,
    "units": [
      {
        "name": "Avatar of Khaine",
        "model_count": 1,
        "points": 335,
        "wargear": ["Wailing Doom"],
        "keywords": ["Epic Hero", "Monster"]
      }
    ],
    "confidence": "high",
    "notes": []
  }
}

IMPORTANT:
- Use canonical Games Workshop unit names
- If a unit name is unclear, include as-is with confidence "low"
- Do NOT add units not mentioned in the source text
- Include all wargear/upgrades mentioned
- Sum points if total not explicitly stated
- Note any parsing issues in the notes array"#;

#[async_trait]
impl Agent for ListNormalizerAgent {
    type Input = ListNormalizerInput;
    type Output = ListNormalizerOutput;

    fn name(&self) -> &'static str {
        "list_normalizer"
    }

    async fn execute(&self, input: Self::Input) -> Result<Self::Output, AgentError> {
        info!("Running List Normalizer for {}", input.player_name);

        let messages = self.build_prompt(&input.raw_text, input.faction_hint.as_deref());
        let request = ChatRequest::new(messages).with_json_mode();

        let response = self.backend.chat(request).await?;
        debug!("AI response: {}", response.content);

        let list = self.parse_response(&response.content, &input.raw_text)?;

        info!(
            "Normalized list: {} ({} units, {} pts)",
            list.data.faction,
            list.data.units.len(),
            list.data.total_points
        );

        Ok(ListNormalizerOutput { list })
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

    fn mock_response() -> &'static str {
        r#"{
            "list": {
                "faction": "Aeldari",
                "subfaction": "Craftworld Ulthwe",
                "detachment": "Battle Host",
                "total_points": 2000,
                "units": [
                    {
                        "name": "Avatar of Khaine",
                        "model_count": 1,
                        "points": 335,
                        "wargear": ["Wailing Doom"],
                        "keywords": ["Epic Hero", "Monster"]
                    },
                    {
                        "name": "Guardians",
                        "model_count": 10,
                        "points": 110,
                        "wargear": ["Shuriken Catapults", "Heavy Weapon Platform"],
                        "keywords": ["Infantry", "Battleline"]
                    }
                ],
                "confidence": "high",
                "notes": []
            }
        }"#
    }

    #[tokio::test]
    async fn test_list_normalizer_extraction() {
        let backend = Arc::new(MockBackend::new(mock_response()));
        let agent = ListNormalizerAgent::new(backend);

        let input = ListNormalizerInput {
            raw_text: "++ Battalion ++\nAvatar of Khaine...\n10x Guardians...".to_string(),
            faction_hint: Some("Aeldari".to_string()),
            player_name: "Test Player".to_string(),
        };

        let output = agent.execute(input).await.unwrap();

        assert_eq!(output.list.data.faction, "Aeldari");
        assert_eq!(output.list.data.total_points, 2000);
        assert_eq!(output.list.data.units.len(), 2);
        assert_eq!(output.list.confidence, Confidence::High);

        let avatar = &output.list.data.units[0];
        assert_eq!(avatar.name, "Avatar of Khaine");
        assert_eq!(avatar.points, Some(335));
    }

    #[tokio::test]
    async fn test_list_normalizer_with_notes() {
        let response_with_notes = r#"{
            "list": {
                "faction": "Space Marines",
                "subfaction": null,
                "detachment": "Gladius Task Force",
                "total_points": 1000,
                "units": [
                    {
                        "name": "Captain",
                        "model_count": 1,
                        "points": 100,
                        "wargear": [],
                        "keywords": []
                    }
                ],
                "confidence": "low",
                "notes": ["Unit names abbreviated", "Points not clearly shown"]
            }
        }"#;

        let backend = Arc::new(MockBackend::new(response_with_notes));
        let agent = ListNormalizerAgent::new(backend);

        let input = ListNormalizerInput {
            raw_text: "Cap w/ sword - 100pts".to_string(),
            faction_hint: Some("Space Marines".to_string()),
            player_name: "Test".to_string(),
        };

        let output = agent.execute(input).await.unwrap();

        assert_eq!(output.list.confidence, Confidence::Low);
        assert_eq!(output.list.extraction_notes.len(), 2);
    }

    #[test]
    fn test_agent_name() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = ListNormalizerAgent::new(backend);
        assert_eq!(agent.name(), "list_normalizer");
    }
}
