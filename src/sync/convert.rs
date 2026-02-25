//! Convert agent stubs to model entities.
//!
//! Bridges between the AI agent output types (EventStub, PlacementStub) and
//! the storage model types (Event, Placement).

use chrono::NaiveDate;

use crate::agents::event_scout::EventStub;
use crate::agents::result_harvester::PlacementStub;
use crate::agents::AgentOutput;
use crate::models::{
    ArmyList, ArmyListId, Confidence, EntityId, Event, EventId, Pairing, Placement,
};
use crate::sync::bcp::{BcpArmyList, BcpEvent, BcpPairing, BcpStanding};

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

/// Convert a BcpEvent to an Event model entity.
pub fn event_from_bcp(bcp_event: &BcpEvent, epoch_id: Option<EntityId>) -> Event {
    let date = bcp_event
        .parsed_start_date()
        .unwrap_or_else(|| chrono::Utc::now().date_naive());

    let epoch_id = epoch_id.unwrap_or_else(|| EntityId::from("current"));

    let mut event = Event::new(
        bcp_event.name.clone(),
        date,
        bcp_event.event_url(),
        "bcp".to_string(),
        epoch_id,
    )
    .with_confidence(Confidence::High);

    if let Some(location) = bcp_event.location_string() {
        event = event.with_location(location);
    }
    if let Some(count) = bcp_event.player_count {
        event = event.with_player_count(count);
    }
    if let Some(count) = bcp_event.round_count {
        event = event.with_round_count(count);
    }

    event
}

/// Convert a BcpStanding to a Placement model entity.
pub fn placement_from_bcp(
    standing: &BcpStanding,
    event_id: EventId,
    epoch_id: Option<EntityId>,
    list_id: Option<ArmyListId>,
) -> Placement {
    let epoch_id = epoch_id.unwrap_or_else(|| EntityId::from("current"));
    let rank = standing.placing.unwrap_or(0);
    let player_name = standing
        .player_name
        .clone()
        .unwrap_or_else(|| "Unknown".to_string());
    let faction = standing
        .faction
        .clone()
        .unwrap_or_else(|| "Unknown".to_string());

    let mut placement = Placement::new(event_id, epoch_id, rank, player_name, faction)
        .with_confidence(Confidence::High);

    if let (Some(w), Some(l), Some(d)) = (standing.wins, standing.losses, standing.draws) {
        placement = placement.with_record(w, l, d);
    }
    if let Some(bp) = standing.total_battle_points {
        placement = placement.with_battle_points(bp);
    }
    if let Some(lid) = list_id {
        placement = placement.with_list_id(lid);
    }

    placement
}

/// Convert a BcpArmyList to an ArmyList model entity.
///
/// BCP lists are raw text that still needs AI normalization; this creates a
/// minimal ArmyList stub that the normalizer can refine later.
pub fn army_list_from_bcp(
    bcp_list: &BcpArmyList,
    event_id: EventId,
    event_date: NaiveDate,
    source_url: &str,
    player_name: Option<&str>,
) -> ArmyList {
    let raw_text = bcp_list.army_list.clone().unwrap_or_default();
    let faction = bcp_list
        .faction
        .clone()
        .unwrap_or_else(|| "Unknown".to_string());

    let mut list = ArmyList::new(faction, 0, Vec::new(), raw_text)
        .with_event_date(event_date)
        .with_event_id(event_id)
        .with_source_url(source_url.to_string())
        .with_confidence(Confidence::Medium);

    if let Some(name) = player_name {
        list = list.with_player_name(name.to_string());
    }

    list
}

/// Compute word-overlap (Jaccard) similarity between two event names.
///
/// Returns a score in `0.0..=1.0`. Names are lowercased and common
/// noise words (year numbers, "gt", "40k", "warhammer") are stripped.
pub fn event_name_similarity(a: &str, b: &str) -> f64 {
    let noise: std::collections::HashSet<&str> =
        ["gt", "40k", "warhammer", "2024", "2025", "2026", "2027"]
            .iter()
            .copied()
            .collect();

    let words = |s: &str| -> std::collections::HashSet<String> {
        s.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .map(|w| w.to_string())
            .filter(|w| !w.is_empty() && !noise.contains(w.as_str()))
            .collect()
    };

    let set_a = words(a);
    let set_b = words(b);

    if set_a.is_empty() && set_b.is_empty() {
        return 1.0;
    }

    let intersection = set_a.intersection(&set_b).count();
    let union = set_a.union(&set_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Check if a new event is a near-duplicate of any existing event.
///
/// Same date is required. Fuzzy name match uses Jaccard > 0.8.
/// Returns the matching existing event's ID if found.
pub fn find_duplicate_event(new_event: &Event, existing_events: &[Event]) -> Option<EventId> {
    for existing in existing_events {
        if existing.date != new_event.date {
            continue;
        }
        // Exact ID match
        if existing.id == new_event.id {
            return Some(existing.id.clone());
        }
        // Fuzzy name match
        if event_name_similarity(&existing.name, &new_event.name) > 0.8 {
            return Some(existing.id.clone());
        }
    }
    None
}

/// Convert BCP pairings into our Pairing model entities.
pub fn pairings_from_bcp(
    bcp_pairings: &[BcpPairing],
    event_id: &EventId,
    epoch_id: Option<EntityId>,
) -> Vec<Pairing> {
    let epoch_id = epoch_id.unwrap_or_else(|| EntityId::from("current"));
    let mut result = Vec::new();

    for bp in bcp_pairings {
        let p1 = match &bp.player1 {
            Some(p) => p,
            None => continue,
        };
        let p2 = match &bp.player2 {
            Some(p) => p,
            None => continue,
        };

        let round = bp.round.unwrap_or(0);
        let p1_name = p1.full_name();
        let p2_name = p2.full_name();

        if p1_name.is_empty() || p2_name.is_empty() {
            continue;
        }

        let mut pairing = Pairing::new(event_id.clone(), epoch_id.clone(), round, p1_name, p2_name);

        pairing.player1_faction = p1.army_name.clone();
        pairing.player2_faction = p2.army_name.clone();

        if let Some(ref meta) = bp.meta_data {
            pairing.player1_result = match meta.p1_game_result {
                Some(2) => Some("win".to_string()),
                Some(0) => Some("loss".to_string()),
                Some(1) => Some("draw".to_string()),
                _ => None,
            };
            pairing.player1_game_points = meta.p1_game_points.map(|p| p as u32);
            pairing.player2_game_points = meta.p2_game_points.map(|p| p as u32);
        }

        result.push(pairing);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::result_harvester::WinLossRecord;

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

    #[test]
    fn test_event_from_bcp() {
        let bcp_event = BcpEvent {
            id: "bcp-123".to_string(),
            name: "London GT 2026".to_string(),
            start_date: Some("2026-02-01".to_string()),
            end_date: Some("2026-02-02".to_string()),
            venue: None,
            city: Some("London".to_string()),
            state: None,
            country: Some("UK".to_string()),
            player_count: Some(96),
            round_count: Some(5),
            game_type: Some(1),
            ended: None,
            team_event: None,
            hide_placings: None,
        };

        let event = event_from_bcp(&bcp_event, None);

        assert_eq!(event.name, "London GT 2026");
        assert_eq!(event.date, NaiveDate::from_ymd_opt(2026, 2, 1).unwrap());
        assert_eq!(event.location, Some("London, UK".to_string()));
        assert_eq!(event.player_count, Some(96));
        assert_eq!(event.source_name, "bcp");
        assert_eq!(event.extraction_confidence, Confidence::High);
    }

    #[test]
    fn test_placement_from_bcp() {
        let standing = BcpStanding {
            placing: Some(1),
            player_name: Some("Jane Doe".to_string()),
            faction: Some("Aeldari".to_string()),
            wins: Some(5),
            losses: Some(0),
            draws: Some(0),
            total_battle_points: Some(94),
            player_id: Some("p1".to_string()),
            army_list_object_id: Some("list-1".to_string()),
        };

        let event_id = EntityId::from("event-bcp-1");
        let placement = placement_from_bcp(&standing, event_id.clone(), None, None);

        assert_eq!(placement.rank, 1);
        assert_eq!(placement.player_name, "Jane Doe");
        assert_eq!(placement.faction, "Aeldari");
        assert_eq!(placement.event_id, event_id);
        assert_eq!(placement.record.as_ref().unwrap().wins, 5);
        assert_eq!(placement.battle_points, Some(94));
    }

    #[test]
    fn test_event_name_similarity_exact() {
        assert!((event_name_similarity("London Open", "London Open") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_event_name_similarity_with_noise() {
        // "GT", "2026", "40k" are stripped, so "London" is the only meaningful word
        let score = event_name_similarity("London GT 2026", "London 40k GT");
        assert!(score > 0.8, "score was {}", score);
    }

    #[test]
    fn test_event_name_similarity_different() {
        let score = event_name_similarity("London Open", "Dallas Major");
        assert!(score < 0.3, "score was {}", score);
    }

    #[test]
    fn test_find_duplicate_event_exact() {
        let event = Event::new(
            "Test GT".to_string(),
            NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
            "https://bcp.com/1".to_string(),
            "bcp".to_string(),
            EntityId::from("current"),
        );

        let existing = vec![Event::new(
            "Test GT".to_string(),
            NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
            "https://goonhammer.com/1".to_string(),
            "goonhammer".to_string(),
            EntityId::from("current"),
        )];

        // Same name + date → same ID (since location is None for both)
        let result = find_duplicate_event(&event, &existing);
        assert!(result.is_some());
    }

    #[test]
    fn test_find_duplicate_event_different_date() {
        let event = Event::new(
            "Test GT".to_string(),
            NaiveDate::from_ymd_opt(2026, 2, 1).unwrap(),
            "https://bcp.com/1".to_string(),
            "bcp".to_string(),
            EntityId::from("current"),
        );

        let existing = vec![Event::new(
            "Test GT".to_string(),
            NaiveDate::from_ymd_opt(2026, 3, 1).unwrap(),
            "https://goonhammer.com/1".to_string(),
            "goonhammer".to_string(),
            EntityId::from("current"),
        )];

        let result = find_duplicate_event(&event, &existing);
        assert!(result.is_none());
    }

    #[test]
    fn test_army_list_from_bcp_basic() {
        use crate::sync::bcp::BcpArmyList;

        let bcp_list = BcpArmyList {
            army_list: Some("Necrons\nWarriors x10".to_string()),
            faction: Some("Necrons".to_string()),
            detachment: Some("Awakened Dynasty".to_string()),
            army_faction: None,
        };

        let event_id = EntityId::from("test-event");
        let event_date = NaiveDate::from_ymd_opt(2025, 6, 1).unwrap();

        let list = army_list_from_bcp(
            &bcp_list,
            event_id.clone(),
            event_date,
            "https://example.com",
            Some("Alice"),
        );

        assert_eq!(list.faction, "Necrons");
        assert_eq!(list.player_name, Some("Alice".to_string()));
        assert_eq!(list.source_url, Some("https://example.com".to_string()));
        assert_eq!(list.event_date, Some(event_date));
    }

    #[test]
    fn test_army_list_from_bcp_no_player() {
        use crate::sync::bcp::BcpArmyList;

        let bcp_list = BcpArmyList {
            army_list: Some("Aeldari list".to_string()),
            faction: Some("Aeldari".to_string()),
            detachment: None,
            army_faction: None,
        };

        let list = army_list_from_bcp(
            &bcp_list,
            EntityId::from("evt"),
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            "https://example.com",
            None,
        );

        assert_eq!(list.faction, "Aeldari");
        assert!(list.player_name.is_none());
    }

    #[test]
    fn test_army_list_from_bcp_missing_faction() {
        use crate::sync::bcp::BcpArmyList;

        let bcp_list = BcpArmyList {
            army_list: None,
            faction: None,
            detachment: None,
            army_faction: None,
        };

        let list = army_list_from_bcp(
            &bcp_list,
            EntityId::from("evt"),
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            "https://example.com",
            None,
        );

        assert_eq!(list.faction, "Unknown");
        assert_eq!(list.raw_text, "");
    }

    #[test]
    fn test_pairings_from_bcp_basic() {
        use crate::sync::bcp::{BcpPairing, BcpPairingMeta, BcpPairingPlayer};

        let pairings = vec![BcpPairing {
            player1: Some(BcpPairingPlayer {
                id: None,
                first_name: Some("Alice".to_string()),
                last_name: Some("Smith".to_string()),
                army_name: Some("Necrons".to_string()),
                army_list_object_id: None,
            }),
            player2: Some(BcpPairingPlayer {
                id: None,
                first_name: Some("Bob".to_string()),
                last_name: Some("Jones".to_string()),
                army_name: Some("Aeldari".to_string()),
                army_list_object_id: None,
            }),
            meta_data: Some(BcpPairingMeta {
                p1_game_result: Some(2), // win
                p1_game_points: Some(90.0),
                p2_game_result: Some(0), // loss
                p2_game_points: Some(60.0),
            }),
            round: Some(1),
        }];

        let event_id = EntityId::from("test-event");
        let result = pairings_from_bcp(&pairings, &event_id, None);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].player1_name, "Alice Smith");
        assert_eq!(result[0].player2_name, "Bob Jones");
        assert_eq!(result[0].round, 1);
        assert_eq!(result[0].player1_faction, Some("Necrons".to_string()));
        assert_eq!(result[0].player2_faction, Some("Aeldari".to_string()));
        assert_eq!(result[0].player1_result, Some("win".to_string()));
        assert_eq!(result[0].player1_game_points, Some(90));
        assert_eq!(result[0].player2_game_points, Some(60));
    }

    #[test]
    fn test_pairings_from_bcp_skips_missing_players() {
        use crate::sync::bcp::{BcpPairing, BcpPairingPlayer};

        let pairings = vec![
            // Missing player2
            BcpPairing {
                player1: Some(BcpPairingPlayer {
                    id: None,
                    first_name: Some("Alice".to_string()),
                    last_name: Some("Smith".to_string()),
                    army_name: None,
                    army_list_object_id: None,
                }),
                player2: None,
                meta_data: None,
                round: Some(1),
            },
            // Missing player1
            BcpPairing {
                player1: None,
                player2: Some(BcpPairingPlayer {
                    id: None,
                    first_name: Some("Bob".to_string()),
                    last_name: Some("Jones".to_string()),
                    army_name: None,
                    army_list_object_id: None,
                }),
                meta_data: None,
                round: Some(1),
            },
        ];

        let event_id = EntityId::from("test-event");
        let result = pairings_from_bcp(&pairings, &event_id, None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_pairings_from_bcp_skips_empty_names() {
        use crate::sync::bcp::{BcpPairing, BcpPairingPlayer};

        let pairings = vec![BcpPairing {
            player1: Some(BcpPairingPlayer {
                id: None,
                first_name: None,
                last_name: None,
                army_name: None,
                army_list_object_id: None,
            }),
            player2: Some(BcpPairingPlayer {
                id: None,
                first_name: Some("Bob".to_string()),
                last_name: Some("Jones".to_string()),
                army_name: None,
                army_list_object_id: None,
            }),
            meta_data: None,
            round: Some(1),
        }];

        let event_id = EntityId::from("test-event");
        let result = pairings_from_bcp(&pairings, &event_id, None);
        assert!(result.is_empty());
    }

    #[test]
    fn test_pairings_from_bcp_loss_and_draw() {
        use crate::sync::bcp::{BcpPairing, BcpPairingMeta, BcpPairingPlayer};

        let make_player = |first: &str, last: &str| BcpPairingPlayer {
            id: None,
            first_name: Some(first.to_string()),
            last_name: Some(last.to_string()),
            army_name: None,
            army_list_object_id: None,
        };

        let pairings = vec![
            BcpPairing {
                player1: Some(make_player("A", "B")),
                player2: Some(make_player("C", "D")),
                meta_data: Some(BcpPairingMeta {
                    p1_game_result: Some(0), // loss
                    p1_game_points: None,
                    p2_game_result: Some(2),
                    p2_game_points: None,
                }),
                round: Some(1),
            },
            BcpPairing {
                player1: Some(make_player("E", "F")),
                player2: Some(make_player("G", "H")),
                meta_data: Some(BcpPairingMeta {
                    p1_game_result: Some(1), // draw
                    p1_game_points: None,
                    p2_game_result: Some(1),
                    p2_game_points: None,
                }),
                round: Some(2),
            },
        ];

        let event_id = EntityId::from("test");
        let result = pairings_from_bcp(&pairings, &event_id, None);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].player1_result, Some("loss".to_string()));
        assert_eq!(result[1].player1_result, Some("draw".to_string()));
    }
}
