//! Result Harvester Agent.
//!
//! Extracts placement results and army lists from event coverage.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use super::backend::{AiBackend, ChatMessage, ChatRequest};
use super::event_scout::EventStub;
use super::{Agent, AgentError, AgentOutput, RetryPolicy};
use crate::models::Confidence;

/// Win/Loss/Draw record.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WinLossRecord {
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
}

/// Stub for a placement (before normalization).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementStub {
    /// Final rank (1 = winner)
    pub rank: u32,

    /// Player name as shown
    pub player_name: String,

    /// Faction name
    pub faction: String,

    /// Subfaction if mentioned
    pub subfaction: Option<String>,

    /// Detachment name if mentioned
    pub detachment: Option<String>,

    /// Win/Loss/Draw record
    pub record: Option<WinLossRecord>,

    /// Battle points if shown
    pub battle_points: Option<u32>,
}

/// Raw army list text for normalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawListText {
    /// Rank of the player this list belongs to
    pub placement_rank: u32,

    /// Player name (for matching)
    pub player_name: String,

    /// Raw list text as extracted
    pub text: String,
}

/// Input for the Result Harvester agent.
#[derive(Debug, Clone)]
pub struct ResultHarvesterInput {
    /// HTML content (section about this event)
    pub article_html: String,

    /// Event stub from Event Scout
    pub event_stub: EventStub,
}

/// Output from the Result Harvester agent.
#[derive(Debug, Clone)]
pub struct ResultHarvesterOutput {
    /// Extracted placements
    pub placements: Vec<AgentOutput<PlacementStub>>,

    /// Raw army list texts
    pub raw_lists: Vec<RawListText>,
}

/// AI-extracted placement data.
#[derive(Debug, Deserialize)]
struct ExtractedPlacement {
    rank: u32,
    player_name: String,
    faction: String,
    subfaction: Option<String>,
    detachment: Option<String>,
    wins: Option<u32>,
    losses: Option<u32>,
    draws: Option<u32>,
    battle_points: Option<u32>,
    army_list: Option<String>,
    confidence: String,
}

#[derive(Debug, Deserialize)]
struct ResultHarvesterResponse {
    placements: Vec<ExtractedPlacement>,
}

/// Result Harvester agent implementation.
pub struct ResultHarvesterAgent {
    backend: Arc<dyn AiBackend>,
}

impl ResultHarvesterAgent {
    pub fn new(backend: Arc<dyn AiBackend>) -> Self {
        Self { backend }
    }

    fn build_prompt(&self, html_content: &str, event: &EventStub) -> Vec<ChatMessage> {
        vec![
            ChatMessage::system(RESULT_HARVESTER_SYSTEM_PROMPT),
            ChatMessage::user(format!(
                "Event: {} ({})\nPlayer count: {:?}\n\nContent:\n\n{}",
                event.name,
                event.location.as_deref().unwrap_or("Unknown location"),
                event.player_count,
                html_content
            )),
        ]
    }

    fn parse_response(&self, response: &str) -> Result<ResultHarvesterOutput, AgentError> {
        let json = super::extract_json(response);
        let parsed: ResultHarvesterResponse = serde_json::from_str(json)
            .map_err(|e| {
                tracing::warn!("Result Harvester JSON parse error. Response start: {}", &response[..response.len().min(200)]);
                AgentError::ResponseParseError(format!("Invalid JSON: {}", e))
            })?;

        let mut placements = Vec::new();
        let mut raw_lists = Vec::new();

        for placement in parsed.placements {
            let record = match (placement.wins, placement.losses, placement.draws) {
                (Some(w), Some(l), d) => Some(WinLossRecord {
                    wins: w,
                    losses: l,
                    draws: d.unwrap_or(0),
                }),
                _ => None,
            };

            let stub = PlacementStub {
                rank: placement.rank,
                player_name: placement.player_name.clone(),
                faction: placement.faction,
                subfaction: placement.subfaction,
                detachment: placement.detachment,
                record,
                battle_points: placement.battle_points,
            };

            let confidence = match placement.confidence.to_lowercase().as_str() {
                "high" => Confidence::High,
                "medium" => Confidence::Medium,
                _ => Confidence::Low,
            };

            let mut notes = Vec::new();
            if stub.record.is_none() {
                notes.push("Win/loss record not found".to_string());
            }
            if stub.detachment.is_none() {
                notes.push("Detachment not specified".to_string());
            }

            placements.push(AgentOutput::new(stub, confidence).with_notes(notes));

            // Extract raw list if present
            if let Some(list_text) = placement.army_list {
                if !list_text.is_empty() {
                    raw_lists.push(RawListText {
                        placement_rank: placement.rank,
                        player_name: placement.player_name,
                        text: list_text,
                    });
                }
            }
        }

        Ok(ResultHarvesterOutput {
            placements,
            raw_lists,
        })
    }
}

const RESULT_HARVESTER_SYSTEM_PROMPT: &str = r#"You are extracting tournament results from a Goonhammer article section.

For each placing player, extract:
- rank: Final position (1 = winner, 2 = second, etc.)
- player_name: Player name as shown
- faction: Main faction — MUST be one of the canonical faction names listed below
- subfaction: Subfaction if mentioned (e.g., "Ynnari", "Ultramarines")
- detachment: Detachment name if shown
- wins: Number of wins (integer, null if not shown)
- losses: Number of losses (integer, null if not shown)
- draws: Number of draws (integer, null if not shown)
- battle_points: Total battle points if shown
- army_list: Full army list text if present (preserve formatting)
- confidence: "high", "medium", or "low"

CANONICAL FACTION NAMES (use EXACTLY one of these for the "faction" field):
  Space Marines               Blood Angels
  Dark Angels                 Space Wolves
  Black Templars              Deathwatch
  Grey Knights                Adepta Sororitas
  Adeptus Custodes            Adeptus Mechanicus
  Astra Militarum             Imperial Knights
  Agents of the Imperium
  Chaos Space Marines         Death Guard
  Thousand Sons               World Eaters
  Emperor's Children          Chaos Daemons
  Chaos Knights
  Aeldari                     Drukhari
  Tyranids                    Genestealer Cults
  Leagues of Votann           Necrons
  Orks                        T'au Empire

SPACE MARINE CHAPTER IDENTIFICATION:
You MUST identify the specific Space Marine chapter when the article mentions one.
Use the chapter name as the "faction" field for codex-supplement chapters.

  These are DISTINCT factions (use chapter name as "faction"):
    Blood Angels, Dark Angels, Space Wolves, Black Templars, Deathwatch, Grey Knights

  These are subfactions (use "Space Marines" as faction, chapter name as "subfaction"):
    Ultramarines, Iron Hands, Salamanders, Raven Guard, White Scars, Imperial Fists,
    Crimson Fists, Flesh Tearers, Black Dragons

  Common abbreviations in articles:
    BA = Blood Angels, DA = Dark Angels, SW = Space Wolves,
    BT = Black Templars, DW = Deathwatch, GK = Grey Knights,
    UM = Ultramarines, IF = Imperial Fists, IH = Iron Hands,
    RG = Raven Guard, SM = Space Marines

  Look for chapter names in parentheses, e.g. "Space Marines (Dark Angels)" → faction: "Dark Angels"

Results are typically shown as:
- "1st - PlayerName (Faction) - 5-0"
- "PlayerName won with Faction (Detachment)"
- Tables with placement data

Return JSON in this exact format:
{
  "placements": [
    {
      "rank": 1,
      "player_name": "John Smith",
      "faction": "Aeldari",
      "subfaction": "Ynnari",
      "detachment": "Soulrender",
      "wins": 5,
      "losses": 0,
      "draws": 0,
      "battle_points": 94,
      "army_list": "++ Battalion Detachment...",
      "confidence": "high"
    }
  ]
}

If no placements found, return: {"placements": []}

IMPORTANT:
- Extract placements in order (1st, 2nd, 3rd...)
- Do NOT invent player names or factions
- Use canonical faction names from the list above
- Include full army list text if available
- Set confidence to "low" for uncertain entries"#;

#[async_trait]
impl Agent for ResultHarvesterAgent {
    type Input = ResultHarvesterInput;
    type Output = ResultHarvesterOutput;

    fn name(&self) -> &'static str {
        "result_harvester"
    }

    async fn execute(&self, input: Self::Input) -> Result<Self::Output, AgentError> {
        info!("Running Result Harvester for {}", input.event_stub.name);

        let messages = self.build_prompt(&input.article_html, &input.event_stub);
        let request = ChatRequest::new(messages).with_json_mode();

        let response = self.backend.chat(request).await?;
        debug!("AI response: {}", response.content);

        let output = self.parse_response(&response.content)?;

        info!(
            "Result Harvester found {} placements, {} lists",
            output.placements.len(),
            output.raw_lists.len()
        );

        Ok(output)
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
            "placements": [
                {
                    "rank": 1,
                    "player_name": "John Smith",
                    "faction": "Aeldari",
                    "subfaction": "Ynnari",
                    "detachment": "Soulrender",
                    "wins": 5,
                    "losses": 0,
                    "draws": 0,
                    "battle_points": 94,
                    "army_list": "++ Army List ++\nHQ\n- Avatar of Khaine",
                    "confidence": "high"
                },
                {
                    "rank": 2,
                    "player_name": "Jane Doe",
                    "faction": "Space Marines",
                    "subfaction": "Ultramarines",
                    "detachment": null,
                    "wins": 4,
                    "losses": 1,
                    "draws": 0,
                    "battle_points": 85,
                    "army_list": null,
                    "confidence": "high"
                }
            ]
        }"#
    }

    fn test_event_stub() -> EventStub {
        EventStub {
            name: "London GT 2025".to_string(),
            date: None,
            location: Some("London, UK".to_string()),
            player_count: Some(96),
            round_count: Some(5),
            event_type: Some("GT".to_string()),
            article_section: None,
        }
    }

    #[tokio::test]
    async fn test_result_harvester_extraction() {
        let backend = Arc::new(MockBackend::new(mock_response()));
        let agent = ResultHarvesterAgent::new(backend);

        let input = ResultHarvesterInput {
            article_html: "<html>Results...</html>".to_string(),
            event_stub: test_event_stub(),
        };

        let output = agent.execute(input).await.unwrap();

        assert_eq!(output.placements.len(), 2);
        assert_eq!(output.raw_lists.len(), 1); // Only first has list

        let winner = &output.placements[0];
        assert_eq!(winner.data.rank, 1);
        assert_eq!(winner.data.player_name, "John Smith");
        assert_eq!(winner.data.faction, "Aeldari");
        assert!(winner.data.record.is_some());
        assert_eq!(winner.data.record.as_ref().unwrap().wins, 5);

        let second = &output.placements[1];
        assert_eq!(second.data.rank, 2);
        assert!(second.data.detachment.is_none());
    }

    #[tokio::test]
    async fn test_result_harvester_empty() {
        let backend = Arc::new(MockBackend::new(r#"{"placements": []}"#));
        let agent = ResultHarvesterAgent::new(backend);

        let input = ResultHarvesterInput {
            article_html: "<html>No results</html>".to_string(),
            event_stub: test_event_stub(),
        };

        let output = agent.execute(input).await.unwrap();
        assert!(output.placements.is_empty());
        assert!(output.raw_lists.is_empty());
    }

    #[test]
    fn test_win_loss_record_default() {
        let record = WinLossRecord::default();
        assert_eq!(record.wins, 0);
        assert_eq!(record.losses, 0);
        assert_eq!(record.draws, 0);
    }

    #[test]
    fn test_agent_name() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = ResultHarvesterAgent::new(backend);
        assert_eq!(agent.name(), "result_harvester");
    }
}
