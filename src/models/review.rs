//! Review queue items for manual attention.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

use super::EntityId;

/// Reason an item was flagged for review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewReason {
    /// AI extraction had low confidence
    LowConfidence,
    /// Fact checker found discrepancies
    FactCheckFailed,
    /// Possible duplicate entry
    DuplicateSuspected,
    /// Manual flag by user
    ManualFlag,
}

impl std::fmt::Display for ReviewReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReviewReason::LowConfidence => write!(f, "low_confidence"),
            ReviewReason::FactCheckFailed => write!(f, "fact_check_failed"),
            ReviewReason::DuplicateSuspected => write!(f, "duplicate_suspected"),
            ReviewReason::ManualFlag => write!(f, "manual_flag"),
        }
    }
}

/// Type of entity in the review queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Event,
    Placement,
    ArmyList,
    SignificantEvent,
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntityType::Event => write!(f, "event"),
            EntityType::Placement => write!(f, "placement"),
            EntityType::ArmyList => write!(f, "army_list"),
            EntityType::SignificantEvent => write!(f, "significant_event"),
        }
    }
}

/// An item in the review queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewQueueItem {
    /// Unique identifier
    pub id: String,

    /// Type of entity
    pub entity_type: EntityType,

    /// ID of the entity
    pub entity_id: EntityId,

    /// Reason for flagging
    pub reason: ReviewReason,

    /// Detailed explanation
    pub details: String,

    /// Path to the source file
    pub source_path: Option<PathBuf>,

    /// When this was created
    pub created_at: DateTime<Utc>,

    /// Whether this has been resolved
    pub resolved: bool,

    /// When this was resolved
    pub resolved_at: Option<DateTime<Utc>>,

    /// Notes about the resolution
    pub resolution_notes: Option<String>,
}

impl ReviewQueueItem {
    /// Create a new review queue item.
    pub fn new(
        entity_type: EntityType,
        entity_id: EntityId,
        reason: ReviewReason,
        details: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            entity_type,
            entity_id,
            reason,
            details,
            source_path: None,
            created_at: Utc::now(),
            resolved: false,
            resolved_at: None,
            resolution_notes: None,
        }
    }

    /// Builder method to set source path.
    pub fn with_source_path(mut self, path: PathBuf) -> Self {
        self.source_path = Some(path);
        self
    }

    /// Mark as resolved.
    pub fn resolve(&mut self, notes: Option<String>) {
        self.resolved = true;
        self.resolved_at = Some(Utc::now());
        self.resolution_notes = notes;
    }

    /// Check if this is pending (not resolved).
    pub fn is_pending(&self) -> bool {
        !self.resolved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_review_item_creation() {
        let item = ReviewQueueItem::new(
            EntityType::Placement,
            EntityId::from("abc123"),
            ReviewReason::LowConfidence,
            "Faction 'Craftworlds' not recognized".to_string(),
        );

        assert_eq!(item.entity_type, EntityType::Placement);
        assert_eq!(item.reason, ReviewReason::LowConfidence);
        assert!(item.is_pending());
        assert!(!item.resolved);
    }

    #[test]
    fn test_review_item_resolve() {
        let mut item = ReviewQueueItem::new(
            EntityType::Event,
            EntityId::from("abc123"),
            ReviewReason::DuplicateSuspected,
            "Possible duplicate".to_string(),
        );

        assert!(item.is_pending());

        item.resolve(Some("Confirmed duplicate, rejected".to_string()));

        assert!(!item.is_pending());
        assert!(item.resolved);
        assert!(item.resolved_at.is_some());
        assert_eq!(
            item.resolution_notes,
            Some("Confirmed duplicate, rejected".to_string())
        );
    }

    #[test]
    fn test_review_reason_display() {
        assert_eq!(format!("{}", ReviewReason::LowConfidence), "low_confidence");
        assert_eq!(
            format!("{}", ReviewReason::FactCheckFailed),
            "fact_check_failed"
        );
    }

    #[test]
    fn test_entity_type_display() {
        assert_eq!(format!("{}", EntityType::Event), "event");
        assert_eq!(format!("{}", EntityType::ArmyList), "army_list");
    }

    #[test]
    fn test_review_item_with_source_path() {
        let item = ReviewQueueItem::new(
            EntityType::Placement,
            EntityId::from("abc123"),
            ReviewReason::FactCheckFailed,
            "Details".to_string(),
        )
        .with_source_path(PathBuf::from("/data/raw/test.html"));

        assert_eq!(item.source_path, Some(PathBuf::from("/data/raw/test.html")));
    }

    #[test]
    fn test_review_item_serialization() {
        let item = ReviewQueueItem::new(
            EntityType::Placement,
            EntityId::from("abc123"),
            ReviewReason::LowConfidence,
            "Test".to_string(),
        );

        let json = serde_json::to_string(&item).unwrap();
        let deserialized: ReviewQueueItem = serde_json::from_str(&json).unwrap();

        assert_eq!(item.id, deserialized.id);
        assert_eq!(item.entity_type, deserialized.entity_type);
        assert_eq!(item.reason, deserialized.reason);
    }
}
