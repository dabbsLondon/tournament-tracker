//! Tournament event model.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{Confidence, EntityId, EpochId, EventId};

/// A tournament event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Unique identifier (derived from name + date + location)
    pub id: EventId,

    /// Tournament name
    pub name: String,

    /// Date of the tournament
    pub date: NaiveDate,

    /// Location (city, country)
    pub location: Option<String>,

    /// Number of players
    pub player_count: Option<u32>,

    /// Number of rounds
    pub round_count: Option<u32>,

    /// Source URL where results were found
    pub source_url: String,

    /// Name of the source (e.g., "goonhammer")
    pub source_name: String,

    /// Epoch this event belongs to
    pub epoch_id: EpochId,

    /// When this record was created
    pub created_at: DateTime<Utc>,

    /// Confidence level of the extraction
    pub extraction_confidence: Confidence,

    /// Whether this needs manual review
    pub needs_review: bool,

    /// Path to the raw source file
    pub raw_source_path: Option<PathBuf>,
}

impl Event {
    /// Create a new Event with auto-generated ID.
    pub fn new(
        name: String,
        date: NaiveDate,
        source_url: String,
        source_name: String,
        epoch_id: EpochId,
    ) -> Self {
        let location_str = "";
        let id = EntityId::generate(&[&name, &date.to_string(), location_str]);

        Self {
            id,
            name,
            date,
            location: None,
            player_count: None,
            round_count: None,
            source_url,
            source_name,
            epoch_id,
            created_at: Utc::now(),
            extraction_confidence: Confidence::default(),
            needs_review: false,
            raw_source_path: None,
        }
    }

    /// Regenerate ID with location included.
    pub fn with_location(mut self, location: String) -> Self {
        self.location = Some(location.clone());
        self.id = EntityId::generate(&[&self.name, &self.date.to_string(), &location]);
        self
    }

    /// Builder method to set player count.
    pub fn with_player_count(mut self, count: u32) -> Self {
        self.player_count = Some(count);
        self
    }

    /// Builder method to set round count.
    pub fn with_round_count(mut self, count: u32) -> Self {
        self.round_count = Some(count);
        self
    }

    /// Builder method to set confidence.
    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.extraction_confidence = confidence;
        self.needs_review = confidence.needs_review();
        self
    }

    /// Builder method to set raw source path.
    pub fn with_raw_source_path(mut self, path: PathBuf) -> Self {
        self.raw_source_path = Some(path);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let event = Event::new(
            "London GT 2025".to_string(),
            NaiveDate::from_ymd_opt(2025, 7, 12).unwrap(),
            "https://example.com".to_string(),
            "goonhammer".to_string(),
            EntityId::from("epoch-123"),
        );

        assert_eq!(event.name, "London GT 2025");
        assert!(!event.id.as_str().is_empty());
        assert!(event.location.is_none());
    }

    #[test]
    fn test_event_with_location() {
        let event = Event::new(
            "London GT 2025".to_string(),
            NaiveDate::from_ymd_opt(2025, 7, 12).unwrap(),
            "https://example.com".to_string(),
            "goonhammer".to_string(),
            EntityId::from("epoch-123"),
        )
        .with_location("London, UK".to_string());

        assert_eq!(event.location, Some("London, UK".to_string()));
    }

    #[test]
    fn test_event_id_includes_location() {
        let event1 = Event::new(
            "GT 2025".to_string(),
            NaiveDate::from_ymd_opt(2025, 7, 12).unwrap(),
            "https://example.com".to_string(),
            "goonhammer".to_string(),
            EntityId::from("epoch-123"),
        )
        .with_location("London, UK".to_string());

        let event2 = Event::new(
            "GT 2025".to_string(),
            NaiveDate::from_ymd_opt(2025, 7, 12).unwrap(),
            "https://example.com".to_string(),
            "goonhammer".to_string(),
            EntityId::from("epoch-123"),
        )
        .with_location("Paris, France".to_string());

        // Different locations should produce different IDs
        assert_ne!(event1.id, event2.id);
    }

    #[test]
    fn test_event_builder() {
        let event = Event::new(
            "Test GT".to_string(),
            NaiveDate::from_ymd_opt(2025, 7, 12).unwrap(),
            "https://example.com".to_string(),
            "goonhammer".to_string(),
            EntityId::from("epoch-123"),
        )
        .with_player_count(120)
        .with_round_count(6)
        .with_confidence(Confidence::High);

        assert_eq!(event.player_count, Some(120));
        assert_eq!(event.round_count, Some(6));
        assert_eq!(event.extraction_confidence, Confidence::High);
    }

    #[test]
    fn test_event_serialization() {
        let event = Event::new(
            "Test GT".to_string(),
            NaiveDate::from_ymd_opt(2025, 7, 12).unwrap(),
            "https://example.com".to_string(),
            "goonhammer".to_string(),
            EntityId::from("epoch-123"),
        );

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();

        assert_eq!(event.id, deserialized.id);
        assert_eq!(event.name, deserialized.name);
    }
}
