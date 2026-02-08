//! Meta epochs - time periods between significant events.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::{EntityId, EpochId, SignificantEvent, SignificantEventId};

/// A pre-tracking epoch ID for events before any recorded significant events.
pub const PRE_TRACKING_EPOCH_ID: &str = "pre-tracking";

/// A meta epoch - a contiguous time window between significant events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaEpoch {
    /// Unique identifier (derived from start_event_id)
    pub id: EpochId,

    /// Human-readable name
    pub name: String,

    /// ID of the significant event that started this epoch
    pub start_event_id: SignificantEventId,

    /// Start date of the epoch
    pub start_date: NaiveDate,

    /// End date of the epoch (None if current)
    pub end_date: Option<NaiveDate>,

    /// ID of the significant event that ended this epoch (None if current)
    pub end_event_id: Option<SignificantEventId>,

    /// Whether this is the current active epoch
    pub is_current: bool,
}

impl MetaEpoch {
    /// Create a new MetaEpoch from a significant event.
    pub fn from_significant_event(event: &SignificantEvent) -> Self {
        let id = EntityId::generate(&[event.id.as_str()]);
        let name = format!("Post {}", event.title);

        Self {
            id,
            name,
            start_event_id: event.id.clone(),
            start_date: event.date,
            end_date: None,
            end_event_id: None,
            is_current: true,
        }
    }

    /// Create a pre-tracking epoch for events before any recorded significant events.
    pub fn pre_tracking() -> Self {
        Self {
            id: EntityId::from(PRE_TRACKING_EPOCH_ID),
            name: "Pre-Tracking".to_string(),
            start_event_id: EntityId::from("genesis"),
            start_date: NaiveDate::from_ymd_opt(2000, 1, 1).unwrap(),
            end_date: None,
            end_event_id: None,
            is_current: false,
        }
    }

    /// Close this epoch with the next significant event.
    pub fn close_with(&mut self, next_event: &SignificantEvent) {
        // End date is the day before the next epoch starts
        self.end_date = next_event.date.pred_opt();
        self.end_event_id = Some(next_event.id.clone());
        self.is_current = false;
    }

    /// Check if a date falls within this epoch.
    pub fn contains_date(&self, date: NaiveDate) -> bool {
        if date < self.start_date {
            return false;
        }
        match self.end_date {
            Some(end) => date <= end,
            None => true, // Current epoch contains all future dates
        }
    }
}

/// Manager for epoch mapping operations.
#[derive(Debug, Default)]
pub struct EpochMapper {
    epochs: Vec<MetaEpoch>,
}

impl EpochMapper {
    /// Create a new EpochMapper.
    pub fn new() -> Self {
        Self { epochs: Vec::new() }
    }

    /// Create an EpochMapper from a list of significant events.
    /// Events should be sorted by date in ascending order.
    pub fn from_significant_events(events: &[SignificantEvent]) -> Self {
        let mut mapper = Self::new();

        if events.is_empty() {
            return mapper;
        }

        // Sort events by date
        let mut sorted_events: Vec<_> = events.iter().collect();
        sorted_events.sort_by_key(|e| e.date);

        // Create epochs from events
        for (i, event) in sorted_events.iter().enumerate() {
            let mut epoch = MetaEpoch::from_significant_event(event);

            // Close previous epochs
            if i > 0 {
                if let Some(prev_epoch) = mapper.epochs.last_mut() {
                    prev_epoch.close_with(event);
                }
            }

            // Only the last epoch is current
            epoch.is_current = i == sorted_events.len() - 1;

            mapper.epochs.push(epoch);
        }

        mapper
    }

    /// Get the epoch for a given date.
    pub fn get_epoch_for_date(&self, date: NaiveDate) -> Option<&MetaEpoch> {
        // Find the most recent epoch that starts on or before the date
        self.epochs
            .iter()
            .filter(|e| e.start_date <= date)
            .max_by_key(|e| e.start_date)
    }

    /// Get the epoch ID for a given date.
    pub fn get_epoch_id_for_date(&self, date: NaiveDate) -> EpochId {
        self.get_epoch_for_date(date)
            .map(|e| e.id.clone())
            .unwrap_or_else(|| EntityId::from(PRE_TRACKING_EPOCH_ID))
    }

    /// Get the current (most recent) epoch.
    pub fn current_epoch(&self) -> Option<&MetaEpoch> {
        self.epochs.iter().find(|e| e.is_current)
    }

    /// Get all epochs.
    pub fn all_epochs(&self) -> &[MetaEpoch] {
        &self.epochs
    }

    /// Get epoch by ID.
    pub fn get_epoch(&self, id: &EpochId) -> Option<&MetaEpoch> {
        self.epochs.iter().find(|e| &e.id == id)
    }

    /// Add a new significant event and update epochs.
    pub fn add_significant_event(&mut self, event: &SignificantEvent) {
        // Close current epoch if any
        if let Some(current) = self.epochs.iter_mut().find(|e| e.is_current) {
            current.close_with(event);
        }

        // Create new epoch
        let epoch = MetaEpoch::from_significant_event(event);
        self.epochs.push(epoch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Confidence, SignificantEventType};

    fn create_test_event(date: NaiveDate, title: &str) -> SignificantEvent {
        SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            date,
            title.to_string(),
            "https://example.com".to_string(),
        )
        .with_confidence(Confidence::High)
    }

    #[test]
    fn test_epoch_from_significant_event() {
        let event = create_test_event(
            NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
            "Balance Dataslate June 2025",
        );

        let epoch = MetaEpoch::from_significant_event(&event);

        assert_eq!(
            epoch.start_date,
            NaiveDate::from_ymd_opt(2025, 6, 15).unwrap()
        );
        assert_eq!(epoch.start_event_id, event.id);
        assert!(epoch.is_current);
        assert!(epoch.end_date.is_none());
    }

    #[test]
    fn test_epoch_contains_date() {
        let event = create_test_event(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(), "Test");

        let mut epoch = MetaEpoch::from_significant_event(&event);

        // Before start date
        assert!(!epoch.contains_date(NaiveDate::from_ymd_opt(2025, 6, 14).unwrap()));

        // On start date
        assert!(epoch.contains_date(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap()));

        // After start date (current epoch, no end)
        assert!(epoch.contains_date(NaiveDate::from_ymd_opt(2025, 12, 31).unwrap()));

        // Close the epoch
        let next_event = create_test_event(NaiveDate::from_ymd_opt(2025, 9, 15).unwrap(), "Next");
        epoch.close_with(&next_event);

        // After end date
        assert!(!epoch.contains_date(NaiveDate::from_ymd_opt(2025, 9, 15).unwrap()));

        // On end date
        assert!(epoch.contains_date(NaiveDate::from_ymd_opt(2025, 9, 14).unwrap()));
    }

    #[test]
    fn test_epoch_mapper_empty() {
        let mapper = EpochMapper::from_significant_events(&[]);
        assert!(mapper.current_epoch().is_none());
        assert!(mapper.all_epochs().is_empty());
    }

    #[test]
    fn test_epoch_mapper_single_event() {
        let event = create_test_event(
            NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
            "Balance Dataslate June 2025",
        );

        let mapper = EpochMapper::from_significant_events(&[event]);

        assert_eq!(mapper.all_epochs().len(), 1);
        assert!(mapper.current_epoch().is_some());
        assert!(mapper.current_epoch().unwrap().is_current);
    }

    #[test]
    fn test_epoch_mapper_multiple_events() {
        let events = vec![
            create_test_event(
                NaiveDate::from_ymd_opt(2025, 3, 15).unwrap(),
                "Balance Dataslate March 2025",
            ),
            create_test_event(
                NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(),
                "Balance Dataslate June 2025",
            ),
            create_test_event(
                NaiveDate::from_ymd_opt(2025, 9, 15).unwrap(),
                "Balance Dataslate September 2025",
            ),
        ];

        let mapper = EpochMapper::from_significant_events(&events);

        assert_eq!(mapper.all_epochs().len(), 3);

        // Only the last should be current
        let current = mapper.current_epoch().unwrap();
        assert!(current.is_current);
        assert!(current.name.contains("September"));

        // Previous epochs should be closed
        let epochs = mapper.all_epochs();
        assert!(!epochs[0].is_current);
        assert!(epochs[0].end_date.is_some());
        assert!(!epochs[1].is_current);
        assert!(epochs[1].end_date.is_some());
    }

    #[test]
    fn test_epoch_mapper_date_lookup() {
        let events = vec![
            create_test_event(NaiveDate::from_ymd_opt(2025, 3, 15).unwrap(), "March"),
            create_test_event(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(), "June"),
        ];

        let mapper = EpochMapper::from_significant_events(&events);

        // Date before any events
        let pre_epoch_id =
            mapper.get_epoch_id_for_date(NaiveDate::from_ymd_opt(2025, 1, 1).unwrap());
        assert_eq!(pre_epoch_id.as_str(), PRE_TRACKING_EPOCH_ID);

        // Date in first epoch
        let epoch = mapper.get_epoch_for_date(NaiveDate::from_ymd_opt(2025, 4, 1).unwrap());
        assert!(epoch.is_some());
        assert!(epoch.unwrap().name.contains("March"));

        // Date in second epoch
        let epoch = mapper.get_epoch_for_date(NaiveDate::from_ymd_opt(2025, 7, 1).unwrap());
        assert!(epoch.is_some());
        assert!(epoch.unwrap().name.contains("June"));
    }

    #[test]
    fn test_epoch_mapper_add_event() {
        let event1 = create_test_event(NaiveDate::from_ymd_opt(2025, 3, 15).unwrap(), "March");

        let mut mapper = EpochMapper::from_significant_events(&[event1]);
        assert_eq!(mapper.all_epochs().len(), 1);
        assert!(mapper.current_epoch().unwrap().is_current);

        // Add new event
        let event2 = create_test_event(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(), "June");
        mapper.add_significant_event(&event2);

        assert_eq!(mapper.all_epochs().len(), 2);

        // First epoch should now be closed
        assert!(!mapper.all_epochs()[0].is_current);
        assert!(mapper.all_epochs()[0].end_date.is_some());

        // Second epoch should be current
        assert!(mapper.all_epochs()[1].is_current);
    }

    #[test]
    fn test_pre_tracking_epoch() {
        let epoch = MetaEpoch::pre_tracking();
        assert_eq!(epoch.id.as_str(), PRE_TRACKING_EPOCH_ID);
        assert_eq!(epoch.name, "Pre-Tracking");
        assert!(!epoch.is_current);
    }

    #[test]
    fn test_epoch_serialization() {
        let event = create_test_event(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap(), "Test");
        let epoch = MetaEpoch::from_significant_event(&event);

        let json = serde_json::to_string(&epoch).unwrap();
        let deserialized: MetaEpoch = serde_json::from_str(&json).unwrap();

        assert_eq!(epoch.id, deserialized.id);
        assert_eq!(epoch.start_date, deserialized.start_date);
        assert_eq!(epoch.is_current, deserialized.is_current);
    }
}
