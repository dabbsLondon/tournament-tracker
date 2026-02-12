use std::collections::HashMap;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::state::AppState;
use crate::api::{dedup_by_id, resolve_epoch, ApiError};
use crate::models::{ArmyList, Event, Placement};
use crate::storage::{EntityType, JsonlReader};

use super::events::{army_list_to_detail, faction_allegiance, normalize_faction_name, ArmyListDetail};

#[derive(Debug, Deserialize)]
pub struct FactionStatsParams {
    pub min_players: Option<u32>,
    pub epoch: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FactionDetailParams {
    pub epoch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UnitCount {
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Serialize)]
pub struct FactionStat {
    pub faction: String,
    pub allegiance: Option<String>,
    pub allegiance_sub: Option<String>,
    pub count: u32,
    pub meta_share: f64,
    pub first_place_count: u32,
    pub top4_count: u32,
    pub top4_rate: f64,
    pub win_rate: f64,
    pub top_detachments: Vec<DetachmentCount>,
    pub top_units: Vec<UnitCount>,
}

#[derive(Debug, Serialize)]
pub struct DetachmentCount {
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Serialize)]
pub struct FactionStatsResponse {
    pub factions: Vec<FactionStat>,
    pub total_placements: u32,
}

pub async fn faction_stats(
    State(state): State<AppState>,
    Query(params): Query<FactionStatsParams>,
) -> Result<Json<FactionStatsResponse>, ApiError> {
    let epoch = resolve_epoch(params.epoch.as_deref(), &state.epoch_mapper)?;

    // Parse optional date range filters
    let from_date = params
        .from
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let to_date = params
        .to
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

    let reader =
        JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, &epoch);
    let placements = reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let placements = dedup_by_id(placements, |p| p.id.as_str());

    // If date filtering, read events to get event dates and filter placements
    let placements = if from_date.is_some() || to_date.is_some() {
        let event_reader =
            JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, &epoch);
        let events = event_reader
            .read_all()
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let event_dates: HashMap<String, chrono::NaiveDate> = events
            .into_iter()
            .map(|e| (e.id.as_str().to_string(), e.date))
            .collect();

        placements
            .into_iter()
            .filter(|p| {
                let event_date = event_dates.get(p.event_id.as_str());
                match event_date {
                    Some(d) => {
                        from_date.map_or(true, |f| *d >= f) && to_date.map_or(true, |t| *d <= t)
                    }
                    None => true,
                }
            })
            .collect()
    } else {
        placements
    };

    // Read army lists for unit popularity
    let list_reader =
        JsonlReader::<ArmyList>::for_entity(&state.storage, EntityType::ArmyList, &epoch);
    let all_lists = list_reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let all_lists = dedup_by_id(all_lists, |l| l.id.as_str());

    // Index army lists by normalized faction name
    let mut lists_by_faction: HashMap<String, Vec<&ArmyList>> = HashMap::new();
    for l in &all_lists {
        if !l.faction.is_empty() && !l.units.is_empty() {
            lists_by_faction
                .entry(normalize_faction_name(&l.faction))
                .or_default()
                .push(l);
        }
    }

    let total = placements.len() as u32;

    // Group by normalized faction name
    let mut faction_map: HashMap<String, Vec<&Placement>> = HashMap::new();
    for p in &placements {
        faction_map.entry(normalize_faction_name(&p.faction)).or_default().push(p);
    }

    let min_players = params.min_players.unwrap_or(0);

    // Compute per-faction stats
    let mut factions: Vec<FactionStat> = faction_map
        .into_iter()
        .filter(|(_, ps)| ps.len() as u32 >= min_players)
        .map(|(faction, ps)| {
            let count = ps.len() as u32;
            let meta_share = if total > 0 {
                (count as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            let first_place_count = ps.iter().filter(|p| p.rank == 1).count() as u32;
            let top4_count = ps.iter().filter(|p| p.rank <= 4).count() as u32;
            let top4_rate = if count > 0 {
                (top4_count as f64 / count as f64) * 100.0
            } else {
                0.0
            };
            let win_rate = if count > 0 {
                (first_place_count as f64 / count as f64) * 100.0
            } else {
                0.0
            };

            // Top detachments
            let mut det_map: HashMap<String, u32> = HashMap::new();
            for p in &ps {
                if let Some(ref det) = p.detachment {
                    *det_map.entry(det.clone()).or_default() += 1;
                }
            }
            let mut top_detachments: Vec<DetachmentCount> = det_map
                .into_iter()
                .map(|(name, count)| DetachmentCount { name, count })
                .collect();
            top_detachments.sort_by(|a, b| b.count.cmp(&a.count));
            top_detachments.truncate(3);

            // Top units — find lists matching this faction (exact match only after normalization)
            let mut unit_map: HashMap<String, u32> = HashMap::new();
            for (list_faction, lists) in &lists_by_faction {
                if faction.eq_ignore_ascii_case(list_faction) {
                    for l in lists {
                        for u in &l.units {
                            // Skip characters — focus on non-character units for "common picks"
                            let is_char = u.keywords.iter().any(|k| k == "Character" || k == "Epic Hero");
                            if !is_char {
                                *unit_map.entry(u.name.clone()).or_default() += 1;
                            }
                        }
                    }
                }
            }
            let mut top_units: Vec<UnitCount> = unit_map
                .into_iter()
                .map(|(name, count)| UnitCount { name, count })
                .collect();
            top_units.sort_by(|a, b| b.count.cmp(&a.count));
            top_units.truncate(5);

            let info = super::events::lookup_faction(&faction);
            FactionStat {
                faction,
                allegiance: info.map(|i| i.allegiance.to_string()),
                allegiance_sub: info.map(|i| i.allegiance_sub.to_string()),
                count,
                meta_share: (meta_share * 10.0).round() / 10.0,
                first_place_count,
                top4_count,
                top4_rate: (top4_rate * 10.0).round() / 10.0,
                win_rate: (win_rate * 10.0).round() / 10.0,
                top_detachments,
                top_units,
            }
        })
        .collect();

    // Sort by count descending
    factions.sort_by(|a, b| b.count.cmp(&a.count));

    Ok(Json(FactionStatsResponse {
        factions,
        total_placements: total,
    }))
}

#[derive(Debug, Serialize)]
pub struct FactionWinner {
    pub rank: u32,
    pub player_name: String,
    pub detachment: Option<String>,
    pub event_name: String,
    pub event_id: String,
    pub event_date: String,
    pub army_list: Option<ArmyListDetail>,
}

#[derive(Debug, Serialize)]
pub struct UnitPopularity {
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Serialize)]
pub struct UnmatchedList {
    pub player_name: Option<String>,
    pub detachment: Option<String>,
    pub total_points: u32,
    pub unit_count: usize,
    pub event_name: Option<String>,
    pub event_id: Option<String>,
    pub event_date: Option<String>,
    pub list: ArmyListDetail,
}

#[derive(Debug, Serialize)]
pub struct FactionDetailResponse {
    pub faction: String,
    pub winners: Vec<FactionWinner>,
    pub top_units: Vec<UnitPopularity>,
    pub detachment_breakdown: Vec<DetachmentCount>,
    pub unmatched_lists: Vec<UnmatchedList>,
}

pub async fn faction_detail(
    State(state): State<AppState>,
    Path(faction_name): Path<String>,
    Query(params): Query<FactionDetailParams>,
) -> Result<Json<FactionDetailResponse>, ApiError> {
    let epoch = resolve_epoch(params.epoch.as_deref(), &state.epoch_mapper)?;
    // Read placements for this faction (winners and top-4)
    let placement_reader =
        JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, &epoch);
    let placements = placement_reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let placements = dedup_by_id(placements, |p| p.id.as_str());

    let normalized_query = normalize_faction_name(&faction_name);
    let faction_placements: Vec<_> = placements
        .into_iter()
        .filter(|p| normalize_faction_name(&p.faction).eq_ignore_ascii_case(&normalized_query) && p.rank <= 4)
        .collect();

    if faction_placements.is_empty() {
        return Err(ApiError::NotFound(format!("No placements for faction: {}", faction_name)));
    }

    // Read events to get names/dates
    let event_reader =
        JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, &epoch);
    let events = event_reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let events = dedup_by_id(events, |e| e.id.as_str());

    // Read army lists
    let list_reader =
        JsonlReader::<ArmyList>::for_entity(&state.storage, EntityType::ArmyList, &epoch);
    let all_lists = list_reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let all_lists = dedup_by_id(all_lists, |l| l.id.as_str());

    let normalize_name = |s: &str| -> String {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };

    // Track which list IDs have been claimed to prevent double-matching
    let mut claimed_list_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    let mut winners: Vec<FactionWinner> = Vec::new();
    for p in faction_placements {
        let event = events.iter().find(|e| e.id == p.event_id);
        let event_name = event.map(|e| e.name.clone()).unwrap_or_default();
        let event_date = event.map(|e| e.date.to_string()).unwrap_or_default();
        let source_url = event.map(|e| e.source_url.as_str()).unwrap_or("");

        let event_lists: Vec<_> = all_lists.iter()
            .filter(|l| {
                l.source_url.as_deref() == Some(source_url)
                    && !claimed_list_ids.contains(l.id.as_str())
            })
            .collect();

        let pname = normalize_name(&p.player_name);

        // 1. Player name match within same source article
        let mut matched_list: Option<&ArmyList> = event_lists.iter()
            .find(|l| {
                l.player_name.as_ref().is_some_and(|name| pname == normalize_name(name))
            })
            .copied();

        // 2. Player name + faction match across all unclaimed lists
        if matched_list.is_none() {
            matched_list = all_lists.iter()
                .find(|l| {
                    !claimed_list_ids.contains(l.id.as_str())
                        && l.player_name.as_ref().is_some_and(|name| {
                            pname == normalize_name(name)
                                && normalize_faction_name(&l.faction).eq_ignore_ascii_case(&normalized_query)
                        })
                });
        }

        // Only match by player name — anonymous lists stay unlinked

        if let Some(list) = matched_list {
            claimed_list_ids.insert(list.id.as_str().to_string());
        }
        let army_list = matched_list.map(|l| army_list_to_detail(l));

        winners.push(FactionWinner {
            rank: p.rank,
            player_name: p.player_name,
            detachment: p.detachment,
            event_name,
            event_id: p.event_id.as_str().to_string(),
            event_date,
            army_list,
        });
    }

    // Sort by rank then date descending
    winners.sort_by(|a, b| a.rank.cmp(&b.rank).then_with(|| b.event_date.cmp(&a.event_date)));

    // Find unmatched lists for this faction (using the claimed_list_ids from matching above)
    let faction_lists: Vec<_> = all_lists
        .iter()
        .filter(|l| {
            normalize_faction_name(&l.faction).eq_ignore_ascii_case(&normalized_query)
                && !l.units.is_empty()
                && !claimed_list_ids.contains(l.id.as_str())
        })
        .collect();

    let unmatched_lists: Vec<UnmatchedList> = faction_lists
        .iter()
        .map(|l| {
            let detail = army_list_to_detail(l);
            // Try to find the event this list belongs to via source_url
            let event = l.source_url.as_deref().and_then(|url| {
                events.iter().find(|e| e.source_url.as_str() == url)
            });
            UnmatchedList {
                player_name: l.player_name.clone(),
                detachment: l.detachment.clone(),
                total_points: l.total_points,
                unit_count: l.units.len(),
                event_name: event.map(|e| e.name.clone()),
                event_id: event.map(|e| e.id.as_str().to_string()),
                event_date: event.map(|e| e.date.to_string()),
                list: detail,
            }
        })
        .collect();

    // Compute unit popularity across ALL faction lists (matched + unmatched)
    let mut unit_counts: HashMap<String, u32> = HashMap::new();
    for w in &winners {
        if let Some(ref al) = w.army_list {
            for u in &al.units {
                *unit_counts.entry(u.name.clone()).or_default() += 1;
            }
        }
    }
    for ul in &unmatched_lists {
        for u in &ul.list.units {
            *unit_counts.entry(u.name.clone()).or_default() += 1;
        }
    }
    let mut top_units: Vec<UnitPopularity> = unit_counts
        .into_iter()
        .map(|(name, count)| UnitPopularity { name, count })
        .collect();
    top_units.sort_by(|a, b| b.count.cmp(&a.count));
    top_units.truncate(10);

    // Detachment breakdown
    let mut det_counts: HashMap<String, u32> = HashMap::new();
    for w in &winners {
        if let Some(ref det) = w.detachment {
            *det_counts.entry(det.clone()).or_default() += 1;
        }
    }
    let mut detachment_breakdown: Vec<DetachmentCount> = det_counts
        .into_iter()
        .map(|(name, count)| DetachmentCount { name, count })
        .collect();
    detachment_breakdown.sort_by(|a, b| b.count.cmp(&a.count));

    Ok(Json(FactionDetailResponse {
        faction: faction_name,
        winners,
        top_units,
        detachment_breakdown,
        unmatched_lists,
    }))
}

// ── Allegiance Stats ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AllegianceStatsParams {
    pub epoch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AllegianceFaction {
    pub faction: String,
    pub count: u32,
    pub meta_share: f64,
    pub win_rate: f64,
}

#[derive(Debug, Serialize)]
pub struct AllegianceGroup {
    pub allegiance: String,
    pub total_players: u32,
    pub meta_share: f64,
    pub factions: Vec<AllegianceFaction>,
}

#[derive(Debug, Serialize)]
pub struct AllegianceStatsResponse {
    pub allegiances: Vec<AllegianceGroup>,
    pub total_placements: u32,
}

pub async fn allegiance_stats(
    State(state): State<AppState>,
    Query(params): Query<AllegianceStatsParams>,
) -> Result<Json<AllegianceStatsResponse>, ApiError> {
    let epoch = resolve_epoch(params.epoch.as_deref(), &state.epoch_mapper)?;

    let reader =
        JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, &epoch);
    let placements = reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let placements = dedup_by_id(placements, |p| p.id.as_str());

    let total = placements.len() as u32;

    // Group by normalized faction → collect stats
    let mut faction_stats_map: HashMap<String, (u32, u32)> = HashMap::new(); // (count, wins)
    for p in &placements {
        let norm = normalize_faction_name(&p.faction);
        let entry = faction_stats_map.entry(norm).or_default();
        entry.0 += 1;
        if p.rank == 1 {
            entry.1 += 1;
        }
    }

    // Group factions by allegiance
    let mut allegiance_map: HashMap<String, Vec<AllegianceFaction>> = HashMap::new();
    for (faction, (count, wins)) in &faction_stats_map {
        let allegiance = faction_allegiance(faction)
            .unwrap_or("Unknown")
            .to_string();
        let meta_share = if total > 0 {
            (*count as f64 / total as f64) * 100.0
        } else {
            0.0
        };
        let win_rate = if *count > 0 {
            (*wins as f64 / *count as f64) * 100.0
        } else {
            0.0
        };
        allegiance_map.entry(allegiance).or_default().push(AllegianceFaction {
            faction: faction.clone(),
            count: *count,
            meta_share: (meta_share * 10.0).round() / 10.0,
            win_rate: (win_rate * 10.0).round() / 10.0,
        });
    }

    // Build response
    let order = ["Imperium", "Chaos", "Xenos", "Unknown"];
    let mut allegiances: Vec<AllegianceGroup> = Vec::new();
    for &name in &order {
        if let Some(mut factions) = allegiance_map.remove(name) {
            factions.sort_by(|a, b| b.count.cmp(&a.count));
            let total_players: u32 = factions.iter().map(|f| f.count).sum();
            let meta_share = if total > 0 {
                (total_players as f64 / total as f64) * 100.0
            } else {
                0.0
            };
            allegiances.push(AllegianceGroup {
                allegiance: name.to_string(),
                total_players,
                meta_share: (meta_share * 10.0).round() / 10.0,
                factions,
            });
        }
    }

    Ok(Json(AllegianceStatsResponse {
        allegiances,
        total_placements: total,
    }))
}

#[cfg(test)]
mod tests {
    use crate::api::build_router;
    use crate::api::state::AppState;
    use crate::models::{ArmyList, Event, Placement, Unit};
    use crate::models::EpochMapper;
    use crate::storage::StorageConfig;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use std::sync::Arc;
    use tower::util::ServiceExt;

    /// Create a test AppState with data written to a temp directory.
    fn setup_test_state(dir: &std::path::Path) -> AppState {
        let storage = StorageConfig::new(dir.to_path_buf());
        // Create epoch directory
        let epoch_dir = dir.join("normalized").join("current");
        std::fs::create_dir_all(&epoch_dir).unwrap();
        AppState {
            storage: Arc::new(storage),
            epoch_mapper: Arc::new(EpochMapper::new()),
        }
    }

    fn write_jsonl<T: serde::Serialize>(path: &std::path::Path, items: &[T]) {
        let mut content = String::new();
        for item in items {
            content.push_str(&serde_json::to_string(item).unwrap());
            content.push('\n');
        }
        std::fs::write(path, content).unwrap();
    }

    fn make_event(name: &str, date: &str, source_url: &str) -> Event {
        Event::new(
            name.to_string(),
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            source_url.to_string(),
            "test".to_string(),
            "current".into(),
        )
    }

    fn make_placement(event: &Event, rank: u32, player: &str, faction: &str) -> Placement {
        Placement::new(
            event.id.clone(),
            "current".into(),
            rank,
            player.to_string(),
            faction.to_string(),
        )
    }

    fn make_list(faction: &str, detachment: &str, player: Option<&str>, source_url: Option<&str>) -> ArmyList {
        let unit = Unit::new("Test Unit".to_string(), 1).with_points(100);
        let mut list = ArmyList::new(
            faction.to_string(),
            2000,
            vec![unit],
            "raw text".to_string(),
        )
        .with_detachment(detachment.to_string());
        if let Some(name) = player {
            list = list.with_player_name(name.to_string());
        }
        if let Some(url) = source_url {
            list = list.with_source_url(url.to_string());
        }
        list
    }

    async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, json)
    }

    // ── Matching Logic Tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_player_name_match_within_same_source() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let event = make_event("GT Alpha", "2026-01-15", "https://example.com/gt-alpha");
        let placement = make_placement(&event, 1, "Alice Smith", "Dark Angels");
        let list = make_list("Dark Angels", "Wrath of the Rock", Some("Alice Smith"), Some("https://example.com/gt-alpha"));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&event]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&placement]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/meta/factions/Dark%20Angels").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["winners"].as_array().unwrap().len(), 1);
        assert!(json["winners"][0]["army_list"].is_object(), "Should have matched list by player name");
        assert_eq!(json["unmatched_lists"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn test_player_name_match_cross_event() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let event = make_event("GT Alpha", "2026-01-15", "https://example.com/gt-alpha");
        let placement = make_placement(&event, 1, "Bob Jones", "Necrons");
        // List from a DIFFERENT source URL but same player + faction
        let list = make_list("Necrons", "Hypercrypt Legion", Some("Bob Jones"), Some("https://other.com/lists"));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&event]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&placement]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/meta/factions/Necrons").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["winners"][0]["army_list"].is_object(), "Should match by player name + faction across events");
    }

    #[tokio::test]
    async fn test_anonymous_list_stays_unlinked() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let event = make_event("GT Alpha", "2026-01-15", "https://example.com/gt-alpha");
        let placement = make_placement(&event, 1, "Charlie Brown", "Genestealer Cults");
        // Anonymous list — no player name, same event source
        let list = make_list("Genestealer Cults", "Strike Force", None, Some("https://example.com/gt-alpha"));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&event]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&placement]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/meta/factions/Genestealer%20Cults").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["winners"][0]["army_list"].is_null(), "Anonymous list must NOT be matched");
        assert_eq!(json["unmatched_lists"].as_array().unwrap().len(), 1, "Anonymous list should appear in unmatched");
    }

    #[tokio::test]
    async fn test_no_double_matching_same_list() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let event = make_event("GT Alpha", "2026-01-15", "https://example.com/gt-alpha");
        let p1 = make_placement(&event, 1, "Dave Wilson", "Dark Angels");
        let p2 = make_placement(&event, 3, "Eve Taylor", "Dark Angels");
        // Only ONE Dark Angels list, belonging to Dave
        let list = make_list("Dark Angels", "Wrath of the Rock", Some("Dave Wilson"), Some("https://example.com/gt-alpha"));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&event]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list]);

        let app = build_router(state);
        let (_, json) = get_json(app, "/api/meta/factions/Dark%20Angels").await;

        let winners = json["winners"].as_array().unwrap();
        let dave = winners.iter().find(|w| w["player_name"] == "Dave Wilson").unwrap();
        let eve = winners.iter().find(|w| w["player_name"] == "Eve Taylor").unwrap();

        assert!(dave["army_list"].is_object(), "Dave should have the list");
        assert!(eve["army_list"].is_null(), "Eve should NOT get Dave's list");
        assert_eq!(json["unmatched_lists"].as_array().unwrap().len(), 0, "No unmatched lists — Dave's list was claimed");
    }

    #[tokio::test]
    async fn test_wrong_player_name_stays_unlinked() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let event = make_event("GT Alpha", "2026-01-15", "https://example.com/gt-alpha");
        let placement = make_placement(&event, 2, "Frank Miller", "T'au Empire");
        // List belongs to a different player entirely
        let list = make_list("T'au Empire", "Kauyon", Some("Grace Hopper"), Some("https://example.com/gt-alpha"));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&event]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&placement]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list]);

        let app = build_router(state);
        let (_, json) = get_json(app, "/api/meta/factions/T%27au%20Empire").await;

        assert!(json["winners"][0]["army_list"].is_null(), "Should not match list to wrong player");
        assert_eq!(json["unmatched_lists"].as_array().unwrap().len(), 1, "Grace Hopper's list should be unlinked");
        assert_eq!(json["unmatched_lists"][0]["player_name"], "Grace Hopper");
    }

    #[tokio::test]
    async fn test_multiple_factions_no_cross_contamination() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let event = make_event("GT Alpha", "2026-01-15", "https://example.com/gt-alpha");
        let p_da = make_placement(&event, 1, "Hank", "Dark Angels");
        let p_ne = make_placement(&event, 2, "Iris", "Necrons");
        let list_ne = make_list("Necrons", "Hypercrypt Legion", Some("Iris"), Some("https://example.com/gt-alpha"));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&event]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p_da, &p_ne]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list_ne]);

        let app = build_router(state.clone());
        let (_, json_da) = get_json(app, "/api/meta/factions/Dark%20Angels").await;

        assert!(json_da["winners"][0]["army_list"].is_null(), "DA should not get Necrons list");
        assert_eq!(json_da["unmatched_lists"].as_array().unwrap().len(), 0);

        let app2 = build_router(state);
        let (_, json_ne) = get_json(app2, "/api/meta/factions/Necrons").await;
        assert!(json_ne["winners"][0]["army_list"].is_object(), "Necrons should get their list");
    }

    #[tokio::test]
    async fn test_only_top4_shown_as_winners() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let event = make_event("Big GT", "2026-01-20", "https://example.com/big");
        let placements: Vec<_> = (1..=8)
            .map(|rank| make_placement(&event, rank, &format!("Player {}", rank), "Aeldari"))
            .collect();

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&event]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &placements.iter().collect::<Vec<_>>());
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &Vec::<ArmyList>::new());

        let app = build_router(state);
        let (_, json) = get_json(app, "/api/meta/factions/Aeldari").await;

        assert_eq!(json["winners"].as_array().unwrap().len(), 4, "Only top 4 should be shown");
    }

    #[tokio::test]
    async fn test_faction_not_found_returns_404() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl(&epoch_dir.join("events.jsonl"), &Vec::<Event>::new());
        write_jsonl(&epoch_dir.join("placements.jsonl"), &Vec::<Placement>::new());
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &Vec::<ArmyList>::new());

        let app = build_router(state);
        let (status, _) = get_json(app, "/api/meta/factions/Nonexistent").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
