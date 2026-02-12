//! Convert agent stubs to model entities.
//!
//! Bridges between the AI agent output types (EventStub, PlacementStub) and
//! the storage model types (Event, Placement).

use chrono::NaiveDate;

use crate::agents::event_scout::EventStub;
use crate::agents::result_harvester::PlacementStub;
use crate::agents::AgentOutput;
use crate::models::{EntityId, Event, EventId, Placement};

/// Convert an EventStub to an Event model entity.
///
/// Falls back to `article_date` when the stub has no date.
/// Uses the provided `epoch_id` (or `"current"` if None).
pub fn event_from_stub(
    stub: &AgentOutput<EventStub>,
    article_url: &str,
    article_date: NaiveDate,
    source_name: &str,
    epoch_id: Option<EntityId>,
) -> Event {
    let date = stub.data.date.unwrap_or(article_date);
    let epoch_id = epoch_id.unwrap_or_else(|| EntityId::from("current"));

    let mut event = Event::new(
        stub.data.name.clone(),
        date,
        article_url.to_string(),
        source_name.to_string(),
        epoch_id,
    )
    .with_confidence(stub.confidence);

    if let Some(ref location) = stub.data.location {
        event = event.with_location(location.clone());
    }
    if let Some(count) = stub.data.player_count {
        event = event.with_player_count(count);
    }
    if let Some(count) = stub.data.round_count {
        event = event.with_round_count(count);
    }

    event
}

/// Convert a PlacementStub to a Placement model entity.
pub fn placement_from_stub(
    stub: &AgentOutput<PlacementStub>,
    event_id: EventId,
    epoch_id: Option<EntityId>,
) -> Placement {
    let epoch_id = epoch_id.unwrap_or_else(|| EntityId::from("current"));

    let mut placement = Placement::new(
        event_id,
        epoch_id,
        stub.data.rank,
        stub.data.player_name.clone(),
        stub.data.faction.clone(),
    )
    .with_confidence(stub.confidence);

    if let Some(ref subfaction) = stub.data.subfaction {
        placement = placement.with_subfaction(subfaction.clone());
    }
    if let Some(ref detachment) = stub.data.detachment {
        placement = placement.with_detachment(detachment.clone());
    }
    if let Some(ref record) = stub.data.record {
        placement = placement.with_record(record.wins, record.losses, record.draws);
    }
    if let Some(bp) = stub.data.battle_points {
        placement = placement.with_battle_points(bp);
    }

    placement
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::result_harvester::WinLossRecord;
    use crate::models::Confidence;

    fn make_event_stub(
        name: &str,
        date: Option<NaiveDate>,
        location: Option<&str>,
        player_count: Option<u32>,
    ) -> AgentOutput<EventStub> {
        AgentOutput::new(
            EventStub {
                name: name.to_string(),
                date,
                location: location.map(|s| s.to_string()),
                player_count,
                round_count: Some(5),
                event_type: Some("GT".to_string()),
                article_section: None,
            },
            Confidence::High,
        )
    }

    #[test]
    fn test_event_from_stub_full_data() {
        let article_date = NaiveDate::from_ymd_opt(2025, 6, 20).unwrap();
        let event_date = NaiveDate::from_ymd_opt(2025, 6, 15).unwrap();

        let stub = make_event_stub(
            "London GT 2025",
            Some(event_date),
            Some("London, UK"),
            Some(96),
        );

        let event = event_from_stub(
            &stub,
            "https://goonhammer.com/article",
            article_date,
            "goonhammer",
            None,
        );

        assert_eq!(event.name, "London GT 2025");
        assert_eq!(event.date, event_date); // Uses stub date, not article date
        assert_eq!(event.location, Some("London, UK".to_string()));
        assert_eq!(event.player_count, Some(96));
        assert_eq!(event.round_count, Some(5));
        assert_eq!(event.source_name, "goonhammer");
        assert_eq!(event.extraction_confidence, Confidence::High);
    }

    #[test]
    fn test_event_from_stub_falls_back_to_article_date() {
        let article_date = NaiveDate::from_ymd_opt(2025, 6, 20).unwrap();
        let stub = make_event_stub("Unknown Event", None, None, None);

        let event = event_from_stub(
            &stub,
            "https://goonhammer.com/article",
            article_date,
            "goonhammer",
            None,
        );

        assert_eq!(event.date, article_date); // Falls back to article date
        assert!(event.location.is_none());
        assert!(event.player_count.is_none());
    }

    #[test]
    fn test_placement_from_stub_full_data() {
        let event_id = EntityId::from("event-123");

        let stub = AgentOutput::new(
            PlacementStub {
                rank: 1,
                player_name: "John Smith".to_string(),
                faction: "Aeldari".to_string(),
                subfaction: Some("Ynnari".to_string()),
                detachment: Some("Soulrender".to_string()),
                record: Some(WinLossRecord {
                    wins: 5,
                    losses: 0,
                    draws: 0,
                }),
                battle_points: Some(94),
            },
            Confidence::High,
        );

        let placement = placement_from_stub(&stub, event_id.clone(), None);

        assert_eq!(placement.rank, 1);
        assert_eq!(placement.player_name, "John Smith");
        assert_eq!(placement.faction, "Aeldari");
        assert_eq!(placement.subfaction, Some("Ynnari".to_string()));
        assert_eq!(placement.detachment, Some("Soulrender".to_string()));
        assert!(placement.record.is_some());
        assert_eq!(placement.record.as_ref().unwrap().wins, 5);
        assert_eq!(placement.battle_points, Some(94));
        assert_eq!(placement.event_id, event_id);
    }

    #[test]
    fn test_placement_from_stub_partial_data() {
        let event_id = EntityId::from("event-456");

        let stub = AgentOutput::new(
            PlacementStub {
                rank: 3,
                player_name: "Bob Wilson".to_string(),
                faction: "Death Guard".to_string(),
                subfaction: None,
                detachment: None,
                record: None,
                battle_points: None,
            },
            Confidence::Medium,
        );

        let placement = placement_from_stub(&stub, event_id, None);

        assert_eq!(placement.rank, 3);
        assert_eq!(placement.faction, "Death Guard");
        assert!(placement.subfaction.is_none());
        assert!(placement.detachment.is_none());
        assert!(placement.record.is_none());
        assert!(placement.battle_points.is_none());
        assert_eq!(placement.extraction_confidence, Confidence::Medium);
    }

    #[test]
    fn test_event_from_stub_with_epoch() {
        let article_date = NaiveDate::from_ymd_opt(2025, 6, 20).unwrap();
        let stub = make_event_stub("Epoch Test", Some(article_date), None, None);
        let epoch_id = EntityId::from("epoch-001");

        let event = event_from_stub(
            &stub,
            "https://goonhammer.com/article",
            article_date,
            "goonhammer",
            Some(epoch_id.clone()),
        );

        assert_eq!(event.epoch_id, epoch_id);
    }

    #[test]
    fn test_event_from_stub_no_date_uses_article_date() {
        let article_date = NaiveDate::from_ymd_opt(2025, 8, 1).unwrap();
        let stub = make_event_stub("No Date Event", None, Some("Somewhere"), Some(50));

        let event = event_from_stub(
            &stub,
            "https://goonhammer.com/article",
            article_date,
            "goonhammer",
            None,
        );

        assert_eq!(event.date, article_date);
        assert_eq!(event.epoch_id, EntityId::from("current"));
    }

    #[test]
    fn test_placement_from_stub_with_record() {
        let event_id = EntityId::from("event-789");
        let epoch_id = EntityId::from("epoch-002");

        let stub = AgentOutput::new(
            PlacementStub {
                rank: 2,
                player_name: "Alice".to_string(),
                faction: "Necrons".to_string(),
                subfaction: None,
                detachment: Some("Canoptek Court".to_string()),
                record: Some(WinLossRecord {
                    wins: 4,
                    losses: 1,
                    draws: 0,
                }),
                battle_points: None,
            },
            Confidence::Medium,
        );

        let placement = placement_from_stub(&stub, event_id, Some(epoch_id.clone()));

        assert_eq!(placement.epoch_id, epoch_id);
        assert_eq!(placement.detachment, Some("Canoptek Court".to_string()));
        assert!(placement.record.is_some());
    }

    #[test]
    fn test_placement_from_stub_minimal() {
        let event_id = EntityId::from("event-min");

        let stub = AgentOutput::new(
            PlacementStub {
                rank: 10,
                player_name: "Bob".to_string(),
                faction: "Orks".to_string(),
                subfaction: None,
                detachment: None,
                record: None,
                battle_points: None,
            },
            Confidence::Low,
        );

        let placement = placement_from_stub(&stub, event_id, None);

        assert_eq!(placement.rank, 10);
        assert_eq!(placement.epoch_id, EntityId::from("current"));
        assert!(placement.subfaction.is_none());
        assert!(placement.detachment.is_none());
        assert!(placement.record.is_none());
    }
}
