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
    pub allegiance: Option<String>,
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
    #[serde(default)]
    allegiance: Option<String>,
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
        let json_str = super::extract_json(response);
        let parsed: ListNormalizerResponse = serde_json::from_str(json_str)
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
            allegiance: extracted.allegiance,
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
- faction: Main faction — MUST be one of the canonical faction names listed below
- subfaction: Subfaction if applicable (Chapter, Craftworld, etc.)
- allegiance: Allegiance group — "Imperium", "Chaos", or "Xenos"
- detachment: Detachment name
- total_points: Total army points
- units: Array of units with:
  - name: Unit name (canonical GW name)
  - model_count: Number of models (default 1)
  - points: Points cost
  - wargear: Array of selected wargear/upgrades
  - keywords: Array of keywords — MUST include the unit's battlefield role
- confidence: "high", "medium", or "low"
- notes: Array of any issues or uncertainties

CANONICAL FACTION NAMES (use EXACTLY one of these for the "faction" field):
  Space Marines (Imperium)        Blood Angels (Imperium)
  Dark Angels (Imperium)          Space Wolves (Imperium)
  Black Templars (Imperium)       Deathwatch (Imperium)
  Grey Knights (Imperium)         Adepta Sororitas (Imperium)
  Adeptus Custodes (Imperium)     Adeptus Mechanicus (Imperium)
  Astra Militarum (Imperium)      Imperial Knights (Imperium)
  Agents of the Imperium (Imperium)
  Chaos Space Marines (Chaos)     Death Guard (Chaos)
  Thousand Sons (Chaos)           World Eaters (Chaos)
  Emperor's Children (Chaos)      Chaos Daemons (Chaos)
  Chaos Knights (Chaos)
  Aeldari (Xenos)                 Drukhari (Xenos)
  Tyranids (Xenos)                Genestealer Cults (Xenos)
  Leagues of Votann (Xenos)       Necrons (Xenos)
  Orks (Xenos)                    T'au Empire (Xenos)

SPACE MARINE CHAPTER IDENTIFICATION:
You MUST identify the specific Space Marine chapter from the list contents.
Use the chapter name as the "faction" field. If you cannot determine the chapter,
use "Space Marines" as faction and leave subfaction null.

Identify chapters by these signature units, detachments, and keywords:

  Blood Angels → faction: "Blood Angels"
    Units: Sanguinary Guard, Death Company Marines, Death Company with Jump Packs,
    Death Company Intercessors, Lemartes, Astorath, Dante, The Sanguinor, Mephiston,
    Baal Predator, Sanguinary Priest, Furioso Dreadnought
    Detachments: Sons of Sanguinius, Blade of Sanguinius

  Dark Angels → faction: "Dark Angels"
    Units: Deathwing Knights, Deathwing Terminators, Deathwing Command Squad,
    Ravenwing Black Knights, Ravenwing Dark Talon, Lazarus, Azrael, Sammael,
    Belial, Lion El'Jonson, Inner Circle Companions
    Detachments: Inner Circle Task Force, Unforgiven Task Force

  Space Wolves → faction: "Space Wolves"
    Units: Thunderwolf Cavalry, Blood Claws, Wulfen, Fenrisian Wolves,
    Wolf Guard, Wolf Guard Terminators, Hounds of Morkai, Ragnar Blackmane,
    Bjorn the Fell-Handed, Canis Wolfborn, Logan Grimnar, Long Fangs
    Detachments: Champions of Russ, Stormlance Task Force

  Black Templars → faction: "Black Templars"
    Units: Sword Brethren, Primaris Crusader Squad, Castellan, Marshal,
    Emperor's Champion, High Marshal Helbrecht, Grimaldus
    Detachments: Righteous Crusaders

  Deathwatch → faction: "Deathwatch"
    Units: Deathwatch Veterans, Kill Team, Watch Master, Watch Captain,
    Corvus Blackstar, Deathwatch Terminator Squad, Beacon Angelis
    Detachments: Black Spear Task Force

  Grey Knights → faction: "Grey Knights"
    Units: Grey Knight Strike Squad, Grey Knight Terminator Squad, Paladin Squad,
    Nemesis Dreadknight, Grand Master in Nemesis Dreadknight, Kaldor Draigo,
    Brother-Captain, Brotherhood Librarian, Purifier Squad, Interceptor Squad
    Detachments: Teleport Strike Force

  Ultramarines → faction: "Space Marines", subfaction: "Ultramarines"
    Units: Marneus Calgar, Roboute Guilliman, Victrix Honour Guard, Tyrannic War Veterans,
    Chief Librarian Tigurius, Captain Sicarius
    Detachments: Blade of Ultramar, Gladius Task Force

  Iron Hands → faction: "Space Marines", subfaction: "Iron Hands"
    Units: Iron Father Feirros, Techmarine

  Salamanders → faction: "Space Marines", subfaction: "Salamanders"
    Units: Adrax Agatone, Vulkan He'stan

  Raven Guard → faction: "Space Marines", subfaction: "Raven Guard"
    Units: Kayvaan Shrike

  White Scars → faction: "Space Marines", subfaction: "White Scars"
    Units: Kor'sarro Khan

  Imperial Fists → faction: "Space Marines", subfaction: "Imperial Fists"
    Units: Tor Garadon

  Also check the list header, FACTION KEYWORD line, or detachment name for chapter clues.
  Battlescribe lists often show "Faction: Imperium - Blood Angels" or similar.

UNIT KEYWORDS — Every unit MUST have at least one role keyword from this list:
  "Character"          — HQ / leader models (Captains, Farseers, Warbosses, etc.)
  "Epic Hero"          — Named unique characters (Calgar, Ghazghkull, etc.). Also add "Character".
  "Battleline"         — Core troops (Intercessors, Guardians, Warriors, etc.)
  "Dedicated Transport"— Transport vehicles (Rhinos, Wave Serpents, Trukks, etc.)
  "Vehicle"            — Tanks, walkers, artillery (Leman Russ, War Walker, etc.)
  "Monster"            — Large creatures (Carnifex, Avatar of Khaine, Wraithlord, etc.)
  "Infantry"           — Foot soldiers that aren't Battleline (Terminators, Aspect Warriors, etc.)
  "Mounted"            — Cavalry / bike units (Windriders, Thunderwolf Cavalry, etc.)
  "Fortification"      — Terrain / buildings (Aegis Defence Line, etc.)

Additional keywords to include when applicable:
  "Warlord"            — The army's warlord
  "Psyker"             — Units with psychic abilities
  "Fly"                — Units that can fly
  "Swarm"              — Swarm units
  "Beast"              — Beast units

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
    "allegiance": "Xenos",
    "detachment": "Battle Host",
    "total_points": 2000,
    "units": [
      {
        "name": "Avatar of Khaine",
        "model_count": 1,
        "points": 335,
        "wargear": ["Wailing Doom"],
        "keywords": ["Epic Hero", "Character", "Monster"]
      },
      {
        "name": "Guardians",
        "model_count": 10,
        "points": 110,
        "wargear": ["Shuriken Catapults", "Heavy Weapon Platform"],
        "keywords": ["Battleline", "Infantry"]
      },
      {
        "name": "Wave Serpent",
        "model_count": 1,
        "points": 120,
        "wargear": ["Twin Shuriken Cannon"],
        "keywords": ["Dedicated Transport", "Vehicle", "Fly"]
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
- Every unit MUST have at least one role keyword (Character/Battleline/Vehicle/etc.)
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

    #[test]
    fn test_normalized_list_serialization() {
        let list = NormalizedArmyList {
            faction: "Necrons".to_string(),
            subfaction: None,
            allegiance: Some("Xenos".to_string()),
            detachment: Some("Canoptek Court".to_string()),
            total_points: 2000,
            units: vec![],
            raw_text: "raw list text".to_string(),
        };

        let json = serde_json::to_string(&list).unwrap();
        let parsed: NormalizedArmyList = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.faction, "Necrons");
        assert_eq!(parsed.total_points, 2000);
    }

    #[test]
    fn test_list_normalizer_parse_empty_units() {
        let response = r#"{
            "list": {
                "faction": "Orks",
                "subfaction": null,
                "detachment": "Waaagh! Tribe",
                "total_points": 0,
                "units": [],
                "confidence": "low",
                "notes": ["Could not parse any units"]
            }
        }"#;

        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = ListNormalizerAgent::new(backend);
        let result = agent.parse_response(response, "raw text").unwrap();

        assert_eq!(result.data.faction, "Orks");
        assert_eq!(result.data.units.len(), 0);
        assert_eq!(result.confidence, Confidence::Low);
    }

    #[test]
    fn test_list_normalizer_retry_policy() {
        let backend: Arc<dyn AiBackend> = Arc::new(MockBackend::new("{}"));
        let agent = ListNormalizerAgent::new(backend);
        let policy = agent.retry_policy();
        assert_eq!(policy.max_retries, 2);
    }
}
