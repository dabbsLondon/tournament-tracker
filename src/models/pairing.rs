//! Pairing model — individual game results between two players.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{EntityId, EpochId, EventId};

/// Type alias for pairing IDs.
pub type PairingId = EntityId;

/// A single game pairing between two players at a tournament.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pairing {
    /// Unique identifier
    pub id: PairingId,

    /// Event this pairing belongs to
    pub event_id: EventId,

    /// Epoch this pairing belongs to
    pub epoch_id: EpochId,

    /// Round number
    pub round: u32,

    /// Player 1 name
    pub player1_name: String,

    /// Player 1 faction
    pub player1_faction: Option<String>,

    /// Player 2 name
    pub player2_name: String,

    /// Player 2 faction
    pub player2_faction: Option<String>,

    /// Player 1 result: "win", "loss", or "draw"
    pub player1_result: Option<String>,

    /// Player 1 game points
    pub player1_game_points: Option<u32>,

    /// Player 2 game points
    pub player2_game_points: Option<u32>,

    /// When this record was created
    pub created_at: DateTime<Utc>,
}

impl Pairing {
    /// Create a new Pairing with auto-generated ID.
    pub fn new(
        event_id: EventId,
        epoch_id: EpochId,
        round: u32,
        player1_name: String,
        player2_name: String,
    ) -> Self {
        let id = EntityId::generate(&[
            event_id.as_str(),
            &round.to_string(),
            &player1_name,
            &player2_name,
        ]);

        Self {
            id,
            event_id,
            epoch_id,
            round,
            player1_name,
            player2_name,
            player1_faction: None,
            player2_faction: None,
            player1_result: None,
            player1_game_points: None,
            player2_game_points: None,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pairing_creation() {
        let pairing = Pairing::new(
            EntityId::from("event-1"),
            EntityId::from("epoch-1"),
            1,
            "Alice".to_string(),
            "Bob".to_string(),
        );

        assert_eq!(pairing.round, 1);
        assert_eq!(pairing.player1_name, "Alice");
        assert_eq!(pairing.player2_name, "Bob");
        assert!(!pairing.id.as_str().is_empty());
    }

    #[test]
    fn test_pairing_serialization() {
        let pairing = Pairing::new(
            EntityId::from("event-1"),
            EntityId::from("epoch-1"),
            1,
            "Alice".to_string(),
            "Bob".to_string(),
        );

        let json = serde_json::to_string(&pairing).unwrap();
        let deserialized: Pairing = serde_json::from_str(&json).unwrap();
        assert_eq!(pairing.id, deserialized.id);
        assert_eq!(pairing.player1_name, deserialized.player1_name);
    }

    #[test]
    fn test_pairing_id_deterministic() {
        let p1 = Pairing::new(
            EntityId::from("event-1"),
            EntityId::from("epoch-1"),
            1,
            "Alice".to_string(),
            "Bob".to_string(),
        );
        let p2 = Pairing::new(
            EntityId::from("event-1"),
            EntityId::from("epoch-1"),
            1,
            "Alice".to_string(),
            "Bob".to_string(),
        );
        assert_eq!(p1.id, p2.id);
    }
}
