//! Deterministic ID generation using SHA256 hashing.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

/// A deterministic entity ID derived from content hash.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityId(String);

impl EntityId {
    /// Create a new EntityId from a hash string.
    pub fn new(hash: String) -> Self {
        Self(hash)
    }

    /// Generate an EntityId from input fields.
    /// Uses SHA256 and takes the first 16 characters for brevity.
    pub fn generate(fields: &[&str]) -> Self {
        let mut hasher = Sha256::new();
        for (i, field) in fields.iter().enumerate() {
            if i > 0 {
                hasher.update(b"|");
            }
            hasher.update(field.as_bytes());
        }
        let result = hasher.finalize();
        let hash = hex::encode(result);
        Self(hash[..16].to_string())
    }

    /// Get the ID as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityId({})", self.0)
    }
}

impl From<String> for EntityId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for EntityId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Type alias for significant event IDs
pub type SignificantEventId = EntityId;

/// Type alias for epoch IDs
pub type EpochId = EntityId;

/// Type alias for event (tournament) IDs
pub type EventId = EntityId;

/// Type alias for placement IDs
pub type PlacementId = EntityId;

/// Type alias for army list IDs
pub type ArmyListId = EntityId;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_id_generation_deterministic() {
        let id1 = EntityId::generate(&[
            "balance_update",
            "2025-06-15",
            "Balance Dataslate June 2025",
        ]);
        let id2 = EntityId::generate(&[
            "balance_update",
            "2025-06-15",
            "Balance Dataslate June 2025",
        ]);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_entity_id_different_inputs() {
        let id1 = EntityId::generate(&[
            "balance_update",
            "2025-06-15",
            "Balance Dataslate June 2025",
        ]);
        let id2 = EntityId::generate(&[
            "balance_update",
            "2025-03-15",
            "Balance Dataslate March 2025",
        ]);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_entity_id_length() {
        let id = EntityId::generate(&["test", "input"]);
        assert_eq!(id.as_str().len(), 16);
    }

    #[test]
    fn test_entity_id_hex_format() {
        let id = EntityId::generate(&["test"]);
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_entity_id_serialization() {
        let id = EntityId::generate(&["test"]);
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: EntityId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_entity_id_display() {
        let id = EntityId::new("abc123def456".to_string());
        assert_eq!(format!("{}", id), "abc123def456");
    }

    #[test]
    fn test_entity_id_from_string() {
        let id = EntityId::from("test-id".to_string());
        assert_eq!(id.as_str(), "test-id");
    }

    #[test]
    fn test_entity_id_from_str() {
        let id = EntityId::from("another-id");
        assert_eq!(id.as_str(), "another-id");
    }

    #[test]
    fn test_entity_id_debug() {
        let id = EntityId::new("debug-test".to_string());
        let debug_str = format!("{:?}", id);
        assert!(debug_str.contains("debug-test"));
    }

    #[test]
    fn test_entity_id_equality() {
        let id1 = EntityId::from("same");
        let id2 = EntityId::from("same");
        let id3 = EntityId::from("different");
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }
}
