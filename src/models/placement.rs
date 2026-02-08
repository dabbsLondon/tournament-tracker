//! Tournament placement model.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{ArmyListId, Confidence, EntityId, EpochId, EventId, PlacementId};

/// Win/loss/draw record.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WinLossRecord {
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
}

impl WinLossRecord {
    /// Create a new record.
    pub fn new(wins: u32, losses: u32, draws: u32) -> Self {
        Self {
            wins,
            losses,
            draws,
        }
    }

    /// Total games played.
    pub fn total_games(&self) -> u32 {
        self.wins + self.losses + self.draws
    }

    /// Win rate as a fraction (0.0 to 1.0).
    pub fn win_rate(&self) -> f64 {
        let total = self.total_games();
        if total == 0 {
            0.0
        } else {
            self.wins as f64 / total as f64
        }
    }
}

/// A player's placement in a tournament.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Placement {
    /// Unique identifier (derived from event_id + rank + player_name)
    pub id: PlacementId,

    /// Event this placement belongs to
    pub event_id: EventId,

    /// Epoch this placement belongs to
    pub epoch_id: EpochId,

    /// Placement rank (1 = winner)
    pub rank: u32,

    /// Player name
    pub player_name: String,

    /// Faction (e.g., "Aeldari", "Space Marines")
    pub faction: String,

    /// Subfaction (e.g., "Ynnari", "Black Templars")
    pub subfaction: Option<String>,

    /// Detachment name
    pub detachment: Option<String>,

    /// Win/loss/draw record
    pub record: Option<WinLossRecord>,

    /// Battle points (if available)
    pub battle_points: Option<u32>,

    /// Link to army list
    pub list_id: Option<ArmyListId>,

    /// When this record was created
    pub created_at: DateTime<Utc>,

    /// Confidence level of the extraction
    pub extraction_confidence: Confidence,

    /// Whether this needs manual review
    pub needs_review: bool,
}

impl Placement {
    /// Create a new Placement with auto-generated ID.
    pub fn new(
        event_id: EventId,
        epoch_id: EpochId,
        rank: u32,
        player_name: String,
        faction: String,
    ) -> Self {
        let id = EntityId::generate(&[event_id.as_str(), &rank.to_string(), &player_name]);

        Self {
            id,
            event_id,
            epoch_id,
            rank,
            player_name,
            faction,
            subfaction: None,
            detachment: None,
            record: None,
            battle_points: None,
            list_id: None,
            created_at: Utc::now(),
            extraction_confidence: Confidence::default(),
            needs_review: false,
        }
    }

    /// Builder method to set subfaction.
    pub fn with_subfaction(mut self, subfaction: String) -> Self {
        self.subfaction = Some(subfaction);
        self
    }

    /// Builder method to set detachment.
    pub fn with_detachment(mut self, detachment: String) -> Self {
        self.detachment = Some(detachment);
        self
    }

    /// Builder method to set record.
    pub fn with_record(mut self, wins: u32, losses: u32, draws: u32) -> Self {
        self.record = Some(WinLossRecord::new(wins, losses, draws));
        self
    }

    /// Builder method to set battle points.
    pub fn with_battle_points(mut self, points: u32) -> Self {
        self.battle_points = Some(points);
        self
    }

    /// Builder method to set list ID.
    pub fn with_list_id(mut self, list_id: ArmyListId) -> Self {
        self.list_id = Some(list_id);
        self
    }

    /// Builder method to set confidence.
    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.extraction_confidence = confidence;
        self.needs_review = confidence.needs_review();
        self
    }

    /// Check if this is a podium finish (top 4).
    pub fn is_podium(&self) -> bool {
        self.rank <= 4
    }

    /// Check if this is a win (1st place).
    pub fn is_winner(&self) -> bool {
        self.rank == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_win_loss_record() {
        let record = WinLossRecord::new(5, 1, 0);
        assert_eq!(record.total_games(), 6);
        assert!((record.win_rate() - 0.833).abs() < 0.01);
    }

    #[test]
    fn test_win_loss_record_zero_games() {
        let record = WinLossRecord::default();
        assert_eq!(record.total_games(), 0);
        assert_eq!(record.win_rate(), 0.0);
    }

    #[test]
    fn test_placement_creation() {
        let placement = Placement::new(
            EntityId::from("event-123"),
            EntityId::from("epoch-456"),
            1,
            "John Smith".to_string(),
            "Aeldari".to_string(),
        );

        assert_eq!(placement.rank, 1);
        assert_eq!(placement.player_name, "John Smith");
        assert_eq!(placement.faction, "Aeldari");
        assert!(placement.is_winner());
        assert!(placement.is_podium());
    }

    #[test]
    fn test_placement_builder() {
        let placement = Placement::new(
            EntityId::from("event-123"),
            EntityId::from("epoch-456"),
            2,
            "Jane Doe".to_string(),
            "Space Marines".to_string(),
        )
        .with_subfaction("Black Templars".to_string())
        .with_detachment("Righteous Crusaders".to_string())
        .with_record(5, 1, 0)
        .with_battle_points(450)
        .with_confidence(Confidence::High);

        assert_eq!(placement.subfaction, Some("Black Templars".to_string()));
        assert_eq!(
            placement.detachment,
            Some("Righteous Crusaders".to_string())
        );
        assert!(placement.record.is_some());
        assert_eq!(placement.battle_points, Some(450));
        assert!(!placement.is_winner());
        assert!(placement.is_podium());
    }

    #[test]
    fn test_placement_not_podium() {
        let placement = Placement::new(
            EntityId::from("event-123"),
            EntityId::from("epoch-456"),
            5,
            "Player".to_string(),
            "Faction".to_string(),
        );

        assert!(!placement.is_winner());
        assert!(!placement.is_podium());
    }

    #[test]
    fn test_placement_id_deterministic() {
        let placement1 = Placement::new(
            EntityId::from("event-123"),
            EntityId::from("epoch-456"),
            1,
            "John Smith".to_string(),
            "Aeldari".to_string(),
        );

        let placement2 = Placement::new(
            EntityId::from("event-123"),
            EntityId::from("epoch-456"),
            1,
            "John Smith".to_string(),
            "Different Faction".to_string(), // Faction not used in ID
        );

        assert_eq!(placement1.id, placement2.id);
    }

    #[test]
    fn test_placement_serialization() {
        let placement = Placement::new(
            EntityId::from("event-123"),
            EntityId::from("epoch-456"),
            1,
            "John Smith".to_string(),
            "Aeldari".to_string(),
        )
        .with_record(5, 1, 0);

        let json = serde_json::to_string(&placement).unwrap();
        let deserialized: Placement = serde_json::from_str(&json).unwrap();

        assert_eq!(placement.id, deserialized.id);
        assert_eq!(placement.rank, deserialized.rank);
        assert!(deserialized.record.is_some());
    }
}
