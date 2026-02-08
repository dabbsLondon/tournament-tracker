//! Significant events that mark epoch boundaries.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use super::{Confidence, EntityId, SignificantEventId};

/// Type of significant event that marks an epoch boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignificantEventType {
    /// Balance update (points changes, errata, datasheet updates)
    BalanceUpdate,
    /// Edition release (new edition launch)
    EditionRelease,
}

impl std::fmt::Display for SignificantEventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignificantEventType::BalanceUpdate => write!(f, "balance_update"),
            SignificantEventType::EditionRelease => write!(f, "edition_release"),
        }
    }
}

/// A significant event that marks an epoch boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignificantEvent {
    /// Unique identifier (derived from type + date + title)
    pub id: SignificantEventId,

    /// Type of significant event
    pub event_type: SignificantEventType,

    /// Date of the event
    pub date: NaiveDate,

    /// Title of the event
    pub title: String,

    /// Source URL where the event was announced
    pub source_url: String,

    /// URL to the PDF (for balance updates)
    pub pdf_url: Option<String>,

    /// AI-extracted summary of key changes
    pub summary: Option<String>,

    /// When this record was created
    pub created_at: DateTime<Utc>,

    /// Confidence level of the extraction
    pub extraction_confidence: Confidence,

    /// Whether this needs manual review
    pub needs_review: bool,

    /// Path to the raw source file
    pub raw_source_path: Option<PathBuf>,
}

impl SignificantEvent {
    /// Create a new SignificantEvent with auto-generated ID.
    pub fn new(
        event_type: SignificantEventType,
        date: NaiveDate,
        title: String,
        source_url: String,
    ) -> Self {
        let id = EntityId::generate(&[&event_type.to_string(), &date.to_string(), &title]);

        Self {
            id,
            event_type,
            date,
            title,
            source_url,
            pdf_url: None,
            summary: None,
            created_at: Utc::now(),
            extraction_confidence: Confidence::default(),
            needs_review: false,
            raw_source_path: None,
        }
    }

    /// Builder method to set PDF URL.
    pub fn with_pdf_url(mut self, url: String) -> Self {
        self.pdf_url = Some(url);
        self
    }

    /// Builder method to set summary.
    pub fn with_summary(mut self, summary: String) -> Self {
        self.summary = Some(summary);
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
    fn test_significant_event_creation() {
        let event = SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
            "Balance Dataslate June 2025".to_string(),
            "https://www.warhammer-community.com/...".to_string(),
        );

        assert_eq!(event.event_type, SignificantEventType::BalanceUpdate);
        assert_eq!(event.date, NaiveDate::from_ymd_opt(2025, 6, 15).unwrap());
        assert_eq!(event.title, "Balance Dataslate June 2025");
        assert!(!event.id.as_str().is_empty());
    }

    #[test]
    fn test_significant_event_id_deterministic() {
        let event1 = SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
            "Balance Dataslate June 2025".to_string(),
            "https://example.com".to_string(),
        );

        let event2 = SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
            "Balance Dataslate June 2025".to_string(),
            "https://different-url.com".to_string(),
        );

        // IDs should be the same because they're based on type + date + title
        assert_eq!(event1.id, event2.id);
    }

    #[test]
    fn test_significant_event_builder() {
        let event = SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
            "Test".to_string(),
            "https://example.com".to_string(),
        )
        .with_pdf_url("https://example.com/file.pdf".to_string())
        .with_summary("Major changes to Aeldari".to_string())
        .with_confidence(Confidence::High);

        assert_eq!(
            event.pdf_url,
            Some("https://example.com/file.pdf".to_string())
        );
        assert_eq!(event.summary, Some("Major changes to Aeldari".to_string()));
        assert_eq!(event.extraction_confidence, Confidence::High);
        assert!(!event.needs_review);
    }

    #[test]
    fn test_significant_event_type_display() {
        assert_eq!(
            format!("{}", SignificantEventType::BalanceUpdate),
            "balance_update"
        );
        assert_eq!(
            format!("{}", SignificantEventType::EditionRelease),
            "edition_release"
        );
    }

    #[test]
    fn test_significant_event_serialization() {
        let event = SignificantEvent::new(
            SignificantEventType::EditionRelease,
            NaiveDate::from_ymd_opt(2023, 6, 1).unwrap(),
            "10th Edition".to_string(),
            "https://example.com".to_string(),
        );

        let json = serde_json::to_string(&event).unwrap();
        let deserialized: SignificantEvent = serde_json::from_str(&json).unwrap();

        assert_eq!(event.id, deserialized.id);
        assert_eq!(event.event_type, deserialized.event_type);
        assert_eq!(event.title, deserialized.title);
    }
}
