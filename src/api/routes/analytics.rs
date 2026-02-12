use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::state::AppState;
use crate::api::{dedup_by_id, ApiError};
use crate::models::{ArmyList, Event, Placement};
use crate::storage::{self, EntityType, JsonlReader};

use super::events::{faction_allegiance, normalize_faction_name};

// ── Overview Endpoint ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OverviewParams {
    pub epoch: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct FactionHighlight {
    pub name: String,
    pub count: u32,
}

#[derive(Debug, Serialize)]
pub struct WinRateHighlight {
    pub name: String,
    pub win_rate: f64,
    pub min_count: u32,
}

#[derive(Debug, Serialize)]
pub struct DateRange {
    pub from: String,
    pub to: String,
}

#[derive(Debug, Serialize)]
pub struct OverviewResponse {
    pub total_events: u32,
    pub total_placements: u32,
    pub total_unique_players: u32,
    pub epochs_covered: u32,
    pub date_range: Option<DateRange>,
    pub most_popular_faction: Option<FactionHighlight>,
    pub highest_win_rate_faction: Option<WinRateHighlight>,
}

pub async fn overview(
    State(state): State<AppState>,
    Query(params): Query<OverviewParams>,
) -> Result<Json<OverviewResponse>, ApiError> {
    let mapper = &state.epoch_mapper;
    let epochs = mapper.all_epochs();

    // Determine which epochs to scan
    let epoch_ids: Vec<String> = if params.epoch.as_deref() == Some("all") || params.epoch.is_none()
    {
        if epochs.is_empty() {
            vec!["current".to_string()]
        } else {
            epochs.iter().map(|e| e.id.as_str().to_string()).collect()
        }
    } else {
        vec![crate::api::resolve_epoch(params.epoch.as_deref(), mapper)?]
    };

    let mut all_placements: Vec<Placement> = Vec::new();
    let mut all_events: Vec<Event> = Vec::new();

    for epoch_id in &epoch_ids {
        let event_reader =
            JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, epoch_id);
        if let Ok(events) = event_reader.read_all() {
            all_events.extend(events);
        }
        let placement_reader =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id);
        if let Ok(placements) = placement_reader.read_all() {
            all_placements.extend(placements);
        }
    }

    all_events = dedup_by_id(all_events, |e| e.id.as_str());
    all_placements = dedup_by_id(all_placements, |p| p.id.as_str());

    let total_events = all_events.len() as u32;
    let total_placements = all_placements.len() as u32;

    // Unique players
    let unique_players: std::collections::HashSet<String> = all_placements
        .iter()
        .map(|p| p.player_name.to_lowercase().trim().to_string())
        .collect();
    let total_unique_players = unique_players.len() as u32;

    // Date range
    let date_range = if !all_events.is_empty() {
        let min_date = all_events.iter().map(|e| e.date).min().unwrap();
        let max_date = all_events.iter().map(|e| e.date).max().unwrap();
        Some(DateRange {
            from: min_date.to_string(),
            to: max_date.to_string(),
        })
    } else {
        None
    };

    // Faction counts and win rates
    let mut faction_counts: HashMap<String, u32> = HashMap::new();
    let mut faction_wins: HashMap<String, u32> = HashMap::new();
    for p in &all_placements {
        let norm = normalize_faction_name(&p.faction);
        *faction_counts.entry(norm.clone()).or_default() += 1;
        if p.rank == 1 {
            *faction_wins.entry(norm).or_default() += 1;
        }
    }

    let most_popular_faction =
        faction_counts
            .iter()
            .max_by_key(|(_, c)| *c)
            .map(|(name, count)| FactionHighlight {
                name: name.clone(),
                count: *count,
            });

    let min_count_threshold = 10u32;
    let highest_win_rate_faction = faction_counts
        .iter()
        .filter(|(_, c)| **c >= min_count_threshold)
        .map(|(name, count)| {
            let wins = faction_wins.get(name).copied().unwrap_or(0);
            let win_rate = if *count > 0 {
                (wins as f64 / *count as f64) * 100.0
            } else {
                0.0
            };
            (name.clone(), win_rate, *count)
        })
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(name, win_rate, count)| WinRateHighlight {
            name,
            win_rate: (win_rate * 10.0).round() / 10.0,
            min_count: count,
        });

    Ok(Json(OverviewResponse {
        total_events,
        total_placements,
        total_unique_players,
        epochs_covered: epoch_ids.len() as u32,
        date_range,
        most_popular_faction,
        highest_win_rate_faction,
    }))
}

// ── Trends Endpoint ─────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TrendsParams {
    pub factions: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TrendEpoch {
    pub epoch_id: String,
    pub label: String,
    pub start_date: String,
}

#[derive(Debug, Serialize)]
pub struct TrendDataPoint {
    pub epoch_id: String,
    pub meta_share: f64,
    pub win_rate: f64,
    pub count: u32,
}

#[derive(Debug, Serialize)]
pub struct FactionTrend {
    pub faction: String,
    pub allegiance: String,
    pub data_points: Vec<TrendDataPoint>,
}

#[derive(Debug, Serialize)]
pub struct BalancePassMarker {
    pub date: String,
    pub title: String,
    pub id: String,
}

#[derive(Debug, Serialize)]
pub struct TrendsResponse {
    pub epochs: Vec<TrendEpoch>,
    pub factions: Vec<FactionTrend>,
    pub balance_passes: Vec<BalancePassMarker>,
}

pub async fn faction_trends(
    State(state): State<AppState>,
    Query(params): Query<TrendsParams>,
) -> Result<Json<TrendsResponse>, ApiError> {
    let mapper = &state.epoch_mapper;
    let epochs = mapper.all_epochs();

    if epochs.is_empty() {
        return Ok(Json(TrendsResponse {
            epochs: vec![],
            factions: vec![],
            balance_passes: vec![],
        }));
    }

    // Parse requested factions
    let requested_factions: Option<Vec<String>> = params.factions.as_ref().map(|f| {
        f.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });

    // Collect per-epoch stats
    let mut epoch_infos: Vec<TrendEpoch> = Vec::new();
    // faction -> epoch_id -> (count, wins)
    let mut faction_epoch_stats: HashMap<String, HashMap<String, (u32, u32)>> = HashMap::new();
    // Track global faction counts to determine top-10 if no factions param
    let mut global_faction_counts: HashMap<String, u32> = HashMap::new();

    for epoch in epochs {
        let epoch_id = epoch.id.as_str();
        epoch_infos.push(TrendEpoch {
            epoch_id: epoch_id.to_string(),
            label: epoch.name.clone(),
            start_date: epoch.start_date.to_string(),
        });

        let reader =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id);
        let placements = reader.read_all().unwrap_or_default();
        let placements = dedup_by_id(placements, |p| p.id.as_str());

        // Group by faction
        let mut epoch_faction_map: HashMap<String, (u32, u32)> = HashMap::new();
        for p in &placements {
            let norm = normalize_faction_name(&p.faction);
            let entry = epoch_faction_map.entry(norm).or_default();
            entry.0 += 1;
            if p.rank == 1 {
                entry.1 += 1;
            }
        }

        for (faction, (count, _wins)) in &epoch_faction_map {
            *global_faction_counts.entry(faction.clone()).or_default() += count;
        }

        // Store raw counts per epoch for share/rate computation at output time
        for (faction, (count, wins)) in epoch_faction_map {
            faction_epoch_stats
                .entry(faction)
                .or_default()
                .insert(epoch_id.to_string(), (count, wins));
        }
    }

    // Determine which factions to include
    let target_factions: Vec<String> = if let Some(ref factions) = requested_factions {
        factions.iter().map(|f| normalize_faction_name(f)).collect()
    } else {
        // Top 10 by global count
        let mut sorted: Vec<_> = global_faction_counts.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        sorted
            .into_iter()
            .take(10)
            .map(|(f, _)| f.clone())
            .collect()
    };

    // Compute epoch totals
    let mut epoch_totals: HashMap<String, u32> = HashMap::new();
    for epoch in epochs {
        let epoch_id = epoch.id.as_str();
        let reader =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id);
        let placements = reader.read_all().unwrap_or_default();
        epoch_totals.insert(epoch_id.to_string(), placements.len() as u32);
    }

    // Build faction trends
    let mut faction_trends: Vec<FactionTrend> = Vec::new();
    for faction in &target_factions {
        let allegiance = faction_allegiance(faction).unwrap_or("Unknown").to_string();
        let stats = faction_epoch_stats.get(faction);
        let data_points: Vec<TrendDataPoint> = epoch_infos
            .iter()
            .map(|ei| {
                let (count, wins) = stats
                    .and_then(|s| s.get(&ei.epoch_id))
                    .copied()
                    .unwrap_or((0, 0));
                let total = epoch_totals.get(&ei.epoch_id).copied().unwrap_or(0);
                let meta_share = if total > 0 {
                    (count as f64 / total as f64) * 100.0
                } else {
                    0.0
                };
                let win_rate = if count > 0 {
                    (wins as f64 / count as f64) * 100.0
                } else {
                    0.0
                };
                TrendDataPoint {
                    epoch_id: ei.epoch_id.clone(),
                    meta_share: (meta_share * 10.0).round() / 10.0,
                    win_rate: (win_rate * 10.0).round() / 10.0,
                    count,
                }
            })
            .collect();

        faction_trends.push(FactionTrend {
            faction: faction.clone(),
            allegiance,
            data_points,
        });
    }

    // Balance passes
    let sig_events = storage::read_significant_events(&state.storage).unwrap_or_default();
    let balance_passes: Vec<BalancePassMarker> = sig_events
        .iter()
        .filter(|e| e.event_type == crate::models::SignificantEventType::BalanceUpdate)
        .map(|e| BalancePassMarker {
            date: e.date.to_string(),
            title: e.title.clone(),
            id: e.id.as_str().to_string(),
        })
        .collect();

    Ok(Json(TrendsResponse {
        epochs: epoch_infos,
        factions: faction_trends,
        balance_passes,
    }))
}

// ── Players Endpoint ────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PlayersParams {
    pub epoch: Option<String>,
    pub min_events: Option<u32>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct RecentResult {
    pub event_name: String,
    pub event_date: String,
    pub rank: u32,
    pub faction: String,
}

#[derive(Debug, Serialize)]
pub struct PlayerSummary {
    pub name: String,
    pub total_events: u32,
    pub total_wins: u32,
    pub total_top4: u32,
    pub win_rate: f64,
    pub top4_rate: f64,
    pub primary_faction: String,
    pub recent_results: Vec<RecentResult>,
}

#[derive(Debug, Serialize)]
pub struct PlayersResponse {
    pub players: Vec<PlayerSummary>,
    pub total_unique_players: u32,
}

pub async fn top_players(
    State(state): State<AppState>,
    Query(params): Query<PlayersParams>,
) -> Result<Json<PlayersResponse>, ApiError> {
    let mapper = &state.epoch_mapper;
    let epochs = mapper.all_epochs();

    // Determine which epochs to scan
    let epoch_ids: Vec<String> = if params.epoch.as_deref() == Some("all") || params.epoch.is_none()
    {
        if epochs.is_empty() {
            vec!["current".to_string()]
        } else {
            epochs.iter().map(|e| e.id.as_str().to_string()).collect()
        }
    } else {
        vec![crate::api::resolve_epoch(params.epoch.as_deref(), mapper)?]
    };

    let mut all_placements: Vec<Placement> = Vec::new();
    let mut all_events: Vec<Event> = Vec::new();

    for epoch_id in &epoch_ids {
        let event_reader =
            JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, epoch_id);
        if let Ok(events) = event_reader.read_all() {
            all_events.extend(events);
        }
        let placement_reader =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id);
        if let Ok(placements) = placement_reader.read_all() {
            all_placements.extend(placements);
        }
    }

    all_events = dedup_by_id(all_events, |e| e.id.as_str());
    all_placements = dedup_by_id(all_placements, |p| p.id.as_str());

    // Build event lookup
    let event_map: HashMap<String, &Event> = all_events
        .iter()
        .map(|e| (e.id.as_str().to_string(), e))
        .collect();

    // Group placements by normalized player name
    let normalize_name = |s: &str| -> String {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };

    struct PlayerData {
        display_name: String,
        placements: Vec<(Placement, Option<String>, Option<String>)>, // (placement, event_name, event_date)
    }

    let mut player_map: HashMap<String, PlayerData> = HashMap::new();
    for p in all_placements.iter() {
        let key = normalize_name(&p.player_name);
        let event = event_map.get(p.event_id.as_str());
        let event_name = event.map(|e| e.name.clone());
        let event_date = event.map(|e| e.date.to_string());

        let entry = player_map.entry(key).or_insert_with(|| PlayerData {
            display_name: p.player_name.clone(),
            placements: Vec::new(),
        });
        entry.placements.push((p.clone(), event_name, event_date));
    }

    let min_events = params.min_events.unwrap_or(2);
    let limit = params.limit.unwrap_or(25).min(100);

    let total_unique_players = player_map.len() as u32;

    // Count unique events per player
    let mut player_summaries: Vec<PlayerSummary> = player_map
        .into_iter()
        .filter_map(|(_, data)| {
            let unique_events: std::collections::HashSet<&str> = data
                .placements
                .iter()
                .map(|(p, _, _)| p.event_id.as_str())
                .collect();
            let total_events = unique_events.len() as u32;
            if total_events < min_events {
                return None;
            }

            let total_wins = data
                .placements
                .iter()
                .filter(|(p, _, _)| p.rank == 1)
                .count() as u32;
            let total_top4 = data
                .placements
                .iter()
                .filter(|(p, _, _)| p.rank <= 4)
                .count() as u32;
            let win_rate = if total_events > 0 {
                (total_wins as f64 / total_events as f64) * 100.0
            } else {
                0.0
            };
            let top4_rate = if total_events > 0 {
                (total_top4 as f64 / total_events as f64) * 100.0
            } else {
                0.0
            };

            // Primary faction = most common
            let mut faction_counts: HashMap<String, u32> = HashMap::new();
            for (p, _, _) in &data.placements {
                *faction_counts
                    .entry(normalize_faction_name(&p.faction))
                    .or_default() += 1;
            }
            let primary_faction = faction_counts
                .into_iter()
                .max_by_key(|(_, c)| *c)
                .map(|(f, _)| f)
                .unwrap_or_default();

            // Recent results (sorted by date desc, take 5)
            let mut sorted_placements = data.placements.clone();
            sorted_placements.sort_by(|a, b| {
                let date_a = a.2.as_deref().unwrap_or("");
                let date_b = b.2.as_deref().unwrap_or("");
                date_b.cmp(date_a)
            });
            let recent_results: Vec<RecentResult> = sorted_placements
                .iter()
                .take(5)
                .map(|(p, event_name, event_date)| RecentResult {
                    event_name: event_name.clone().unwrap_or_default(),
                    event_date: event_date.clone().unwrap_or_default(),
                    rank: p.rank,
                    faction: normalize_faction_name(&p.faction),
                })
                .collect();

            Some(PlayerSummary {
                name: data.display_name,
                total_events,
                total_wins,
                total_top4,
                win_rate: (win_rate * 10.0).round() / 10.0,
                top4_rate: (top4_rate * 10.0).round() / 10.0,
                primary_faction,
                recent_results,
            })
        })
        .collect();

    // Sort by wins desc, then top4 desc
    player_summaries.sort_by(|a, b| {
        b.total_wins
            .cmp(&a.total_wins)
            .then_with(|| b.total_top4.cmp(&a.total_top4))
            .then_with(|| b.total_events.cmp(&a.total_events))
    });
    player_summaries.truncate(limit as usize);

    Ok(Json(PlayersResponse {
        players: player_summaries,
        total_unique_players,
    }))
}

// ── Units Endpoint ──────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UnitsParams {
    pub epoch: Option<String>,
    pub faction: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct UnitStat {
    pub name: String,
    pub total_appearances: u32,
    pub lists_containing: u32,
    pub avg_count_per_list: f64,
    pub avg_points: Option<u32>,
    pub factions: Vec<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct FactionUnitBreakdown {
    pub faction: String,
    pub allegiance: String,
    pub top_units: Vec<UnitStat>,
}

#[derive(Debug, Serialize)]
pub struct UnitsResponse {
    pub top_units: Vec<UnitStat>,
    pub total_lists_analysed: u32,
    pub faction_breakdowns: Vec<FactionUnitBreakdown>,
}

pub async fn top_units(
    State(state): State<AppState>,
    Query(params): Query<UnitsParams>,
) -> Result<Json<UnitsResponse>, ApiError> {
    let mapper = &state.epoch_mapper;
    let epochs = mapper.all_epochs();

    let epoch_ids: Vec<String> = if params.epoch.as_deref() == Some("all") || params.epoch.is_none()
    {
        if epochs.is_empty() {
            vec!["current".to_string()]
        } else {
            epochs.iter().map(|e| e.id.as_str().to_string()).collect()
        }
    } else {
        vec![crate::api::resolve_epoch(params.epoch.as_deref(), mapper)?]
    };

    let mut all_lists: Vec<ArmyList> = Vec::new();
    for epoch_id in &epoch_ids {
        let reader =
            JsonlReader::<ArmyList>::for_entity(&state.storage, EntityType::ArmyList, epoch_id);
        if let Ok(lists) = reader.read_all() {
            all_lists.extend(lists);
        }
    }

    all_lists = dedup_by_id(all_lists, |l| l.id.as_str());

    // Optional faction filter
    if let Some(ref faction_filter) = params.faction {
        let norm = normalize_faction_name(faction_filter);
        all_lists.retain(|l| normalize_faction_name(&l.faction) == norm);
    }

    let total_lists = all_lists.len() as u32;
    let limit = params.limit.unwrap_or(30).min(100) as usize;

    // Aggregate unit stats across all lists
    struct UnitAgg {
        total_models: u32,
        lists_containing: u32,
        total_points: u64,
        points_count: u32,
        factions: HashMap<String, bool>,
        keywords: HashMap<String, bool>,
    }

    let mut global_units: HashMap<String, UnitAgg> = HashMap::new();
    // Per-faction unit counts: faction -> unit_name -> UnitAgg
    let mut faction_units: HashMap<String, HashMap<String, UnitAgg>> = HashMap::new();

    for list in &all_lists {
        let faction_norm = normalize_faction_name(&list.faction);
        for unit in &list.units {
            let name = unit.name.clone();

            // Global
            let entry = global_units.entry(name.clone()).or_insert_with(|| UnitAgg {
                total_models: 0,
                lists_containing: 0,
                total_points: 0,
                points_count: 0,
                factions: HashMap::new(),
                keywords: HashMap::new(),
            });
            entry.total_models += unit.count;
            entry.lists_containing += 1;
            if let Some(pts) = unit.points {
                entry.total_points += pts as u64;
                entry.points_count += 1;
            }
            entry.factions.insert(faction_norm.clone(), true);
            for kw in &unit.keywords {
                entry.keywords.insert(kw.clone(), true);
            }

            // Per-faction
            let faction_entry = faction_units
                .entry(faction_norm.clone())
                .or_default()
                .entry(name.clone())
                .or_insert_with(|| UnitAgg {
                    total_models: 0,
                    lists_containing: 0,
                    total_points: 0,
                    points_count: 0,
                    factions: HashMap::new(),
                    keywords: HashMap::new(),
                });
            faction_entry.total_models += unit.count;
            faction_entry.lists_containing += 1;
            if let Some(pts) = unit.points {
                faction_entry.total_points += pts as u64;
                faction_entry.points_count += 1;
            }
            for kw in &unit.keywords {
                faction_entry.keywords.insert(kw.clone(), true);
            }
        }
    }

    fn to_unit_stat(name: &str, agg: &UnitAgg) -> UnitStat {
        let avg_count = if agg.lists_containing > 0 {
            (agg.total_models as f64 / agg.lists_containing as f64 * 10.0).round() / 10.0
        } else {
            0.0
        };
        let avg_points = if agg.points_count > 0 {
            Some((agg.total_points / agg.points_count as u64) as u32)
        } else {
            None
        };
        let mut factions: Vec<String> = agg.factions.keys().cloned().collect();
        factions.sort();
        let mut keywords: Vec<String> = agg.keywords.keys().cloned().collect();
        keywords.sort();
        UnitStat {
            name: name.to_string(),
            total_appearances: agg.total_models,
            lists_containing: agg.lists_containing,
            avg_count_per_list: avg_count,
            avg_points,
            factions,
            keywords,
        }
    }

    // Build top units globally sorted by lists_containing
    let mut global_sorted: Vec<_> = global_units.iter().collect();
    global_sorted.sort_by(|a, b| b.1.lists_containing.cmp(&a.1.lists_containing));
    let top_units: Vec<UnitStat> = global_sorted
        .iter()
        .take(limit)
        .map(|(name, agg)| to_unit_stat(name, agg))
        .collect();

    // Build per-faction breakdowns (top 5 factions by list count, top 10 units each)
    let mut faction_list_counts: Vec<_> = faction_units
        .keys()
        .map(|f| {
            let count = all_lists
                .iter()
                .filter(|l| normalize_faction_name(&l.faction) == *f)
                .count();
            (f.clone(), count)
        })
        .collect();
    faction_list_counts.sort_by(|a, b| b.1.cmp(&a.1));

    let faction_breakdowns: Vec<FactionUnitBreakdown> = faction_list_counts
        .iter()
        .take(8)
        .filter_map(|(faction, _)| {
            let units_map = faction_units.get(faction)?;
            let mut sorted: Vec<_> = units_map.iter().collect();
            sorted.sort_by(|a, b| b.1.lists_containing.cmp(&a.1.lists_containing));
            let top: Vec<UnitStat> = sorted
                .iter()
                .take(10)
                .map(|(name, agg)| to_unit_stat(name, agg))
                .collect();
            let allegiance = faction_allegiance(faction).unwrap_or("Unknown").to_string();
            Some(FactionUnitBreakdown {
                faction: faction.clone(),
                allegiance,
                top_units: top,
            })
        })
        .collect();

    Ok(Json(UnitsResponse {
        top_units,
        total_lists_analysed: total_lists,
        faction_breakdowns,
    }))
}

#[cfg(test)]
mod tests {
    use crate::api::build_router;
    use crate::api::state::AppState;
    use crate::models::{EpochMapper, Event, Placement};
    use crate::storage::StorageConfig;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use serde_json::Value;
    use std::sync::Arc;
    use tower::util::ServiceExt;

    fn write_jsonl<T: serde::Serialize>(path: &std::path::Path, items: &[T]) {
        let mut content = String::new();
        for item in items {
            content.push_str(&serde_json::to_string(item).unwrap());
            content.push('\n');
        }
        std::fs::write(path, content).unwrap();
    }

    async fn get_json(app: axum::Router, uri: &str) -> (StatusCode, Value) {
        let resp = app
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, json)
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

    fn setup_test_state(dir: &std::path::Path) -> AppState {
        let storage = StorageConfig::new(dir.to_path_buf());
        let epoch_dir = dir.join("normalized").join("current");
        std::fs::create_dir_all(&epoch_dir).unwrap();
        AppState {
            storage: Arc::new(storage),
            epoch_mapper: Arc::new(EpochMapper::new()),
        }
    }

    #[tokio::test]
    async fn test_analytics_overview() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        let e2 = make_event("GT Beta", "2026-01-22", "https://example.com/b");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        let p2 = make_placement(&e1, 2, "Bob", "Necrons");
        let p3 = make_placement(&e2, 1, "Alice", "Aeldari");
        let p4 = make_placement(&e2, 2, "Charlie", "Orks");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2, &p3, &p4]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/overview").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["total_events"], 2);
        assert_eq!(json["total_placements"], 4);
        assert_eq!(json["total_unique_players"], 3);
    }

    #[tokio::test]
    async fn test_analytics_overview_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/overview").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["total_events"], 0);
        assert_eq!(json["total_placements"], 0);
        assert_eq!(json["total_unique_players"], 0);
        assert!(json["most_popular_faction"].is_null());
    }

    fn setup_test_state_with_epoch(dir: &std::path::Path) -> AppState {
        use crate::models::{SignificantEvent, SignificantEventType};
        let storage = StorageConfig::new(dir.to_path_buf());
        let sig_event = SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            chrono::NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            "Jan 2025 Balance".to_string(),
            "https://example.com".to_string(),
        );
        let mapper = crate::models::EpochMapper::from_significant_events(&[sig_event]);
        // Create the epoch directory using the first epoch id
        let epoch_id = mapper.all_epochs()[0].id.as_str();
        let epoch_dir = dir.join("normalized").join(epoch_id);
        std::fs::create_dir_all(&epoch_dir).unwrap();
        AppState {
            storage: Arc::new(storage),
            epoch_mapper: Arc::new(mapper),
        }
    }

    #[tokio::test]
    async fn test_analytics_trends() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state_with_epoch(tmp.path());
        let epoch_id = state.epoch_mapper.all_epochs()[0].id.as_str().to_string();
        let epoch_dir = tmp.path().join("normalized").join(&epoch_id);

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        let p2 = make_placement(&e1, 2, "Bob", "Necrons");
        let p3 = make_placement(&e1, 3, "Charlie", "Aeldari");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2, &p3]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/trends").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();
        assert!(!factions.is_empty());
        for f in factions {
            assert!(!f["data_points"].as_array().unwrap().is_empty());
            assert!(!f["faction"].as_str().unwrap().is_empty());
            assert!(!f["allegiance"].as_str().unwrap().is_empty());
        }
    }

    #[tokio::test]
    async fn test_analytics_trends_with_faction_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state_with_epoch(tmp.path());
        let epoch_id = state.epoch_mapper.all_epochs()[0].id.as_str().to_string();
        let epoch_dir = tmp.path().join("normalized").join(&epoch_id);

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        let p2 = make_placement(&e1, 2, "Bob", "Necrons");
        let p3 = make_placement(&e1, 3, "Charlie", "Orks");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2, &p3]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/trends?factions=Aeldari,Necrons").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();
        assert_eq!(factions.len(), 2);
        let names: Vec<&str> = factions
            .iter()
            .map(|f| f["faction"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"Aeldari"));
        assert!(names.contains(&"Necrons"));
    }

    #[tokio::test]
    async fn test_analytics_trends_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/trends").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["factions"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_analytics_units() {
        use crate::models::{ArmyList, Unit};

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let u1 = Unit::new("Leman Russ".to_string(), 2).with_points(160);
        let u2 = Unit::new("Infantry Squad".to_string(), 10).with_points(65);
        let list1 = ArmyList::new(
            "Astra Militarum".to_string(),
            2000,
            vec![u1.clone(), u2.clone()],
            "raw".to_string(),
        );

        let u3 = Unit::new("Leman Russ".to_string(), 1).with_points(160);
        let list2 = ArmyList::new(
            "Astra Militarum".to_string(),
            2000,
            vec![u3],
            "raw".to_string(),
        );

        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list1, &list2]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/units").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["total_lists_analysed"], 2);

        let top = json["top_units"].as_array().unwrap();
        assert!(!top.is_empty());
        // Leman Russ appears in 2 lists, Infantry Squad in 1
        assert_eq!(top[0]["name"], "Leman Russ");
        assert_eq!(top[0]["lists_containing"], 2);
        assert_eq!(top[0]["avg_points"], 160);
    }

    #[tokio::test]
    async fn test_analytics_units_with_faction_filter() {
        use crate::models::{ArmyList, Unit};

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let list1 = ArmyList::new(
            "Astra Militarum".to_string(),
            2000,
            vec![Unit::new("Leman Russ".to_string(), 1)],
            "raw".to_string(),
        );
        let list2 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![Unit::new("Wraithlord".to_string(), 1)],
            "raw".to_string(),
        );

        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list1, &list2]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/units?faction=Astra%20Militarum").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["total_lists_analysed"], 1);
        let top = json["top_units"].as_array().unwrap();
        assert_eq!(top.len(), 1);
        assert_eq!(top[0]["name"], "Leman Russ");
    }

    #[tokio::test]
    async fn test_analytics_units_empty() {
        use crate::models::ArmyList;

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/units").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["total_lists_analysed"], 0);
        assert!(json["top_units"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_analytics_players_min_events_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        let p2 = make_placement(&e1, 2, "Bob", "Necrons");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2]);

        let app = build_router(state);
        // min_events=2 means nobody qualifies (1 event each)
        let (status, json) = get_json(app, "/api/analytics/players?min_events=2").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["players"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_analytics_players() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        let e2 = make_event("GT Beta", "2026-01-22", "https://example.com/b");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        let p2 = make_placement(&e1, 2, "Bob", "Necrons");
        let p3 = make_placement(&e2, 1, "Alice", "Aeldari");
        let p4 = make_placement(&e2, 3, "Bob", "Necrons");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2, &p3, &p4]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/players?min_events=2").await;

        assert_eq!(status, StatusCode::OK);
        let players = json["players"].as_array().unwrap();
        assert_eq!(players.len(), 2);
        // Alice should be first (2 wins)
        assert_eq!(players[0]["name"], "Alice");
        assert_eq!(players[0]["total_wins"], 2);
        assert_eq!(players[0]["primary_faction"], "Aeldari");
    }
}
