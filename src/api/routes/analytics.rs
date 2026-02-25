use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::state::AppState;
use crate::api::{dedup_by_id, ApiError};
use crate::models::{ArmyList, Event, Pairing, Placement};
use crate::storage::{self, EntityType, JsonlReader};
use crate::sync::normalize_player_name;

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
    let mapper = state.epoch_mapper.read().await;
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
        vec![crate::api::resolve_epoch(params.epoch.as_deref(), &mapper)?]
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
    let mapper = state.epoch_mapper.read().await;
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
    let mapper = state.epoch_mapper.read().await;
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
        vec![crate::api::resolve_epoch(params.epoch.as_deref(), &mapper)?]
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
    let mapper = state.epoch_mapper.read().await;
    let epochs = mapper.all_epochs();

    let epoch_ids: Vec<String> = if params.epoch.as_deref() == Some("all") || params.epoch.is_none()
    {
        if epochs.is_empty() {
            vec!["current".to_string()]
        } else {
            epochs.iter().map(|e| e.id.as_str().to_string()).collect()
        }
    } else {
        vec![crate::api::resolve_epoch(params.epoch.as_deref(), &mapper)?]
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

// ── Shared: join lists to placements ────────────────────────────

/// Join army lists to placements via list_id first, then fallback to
/// (event_id, normalized player name).
fn join_lists_to_placements(
    lists: &[ArmyList],
    placements: &[Placement],
) -> Vec<(ArmyList, Placement)> {
    let mut result = Vec::new();
    let mut matched_placement_ids: HashSet<String> = HashSet::new();

    // Index lists by id
    let list_by_id: HashMap<String, &ArmyList> = lists
        .iter()
        .map(|l| (l.id.as_str().to_string(), l))
        .collect();

    // First pass: match via list_id
    for p in placements {
        if let Some(ref lid) = p.list_id {
            if let Some(list) = list_by_id.get(lid.as_str()) {
                result.push((*list).clone());
                matched_placement_ids.insert(p.id.as_str().to_string());
            }
        }
    }

    // Rebuild result properly as tuples
    let mut joined = Vec::new();
    for p in placements {
        if let Some(ref lid) = p.list_id {
            if let Some(list) = list_by_id.get(lid.as_str()) {
                joined.push(((*list).clone(), p.clone()));
                continue;
            }
        }

        // Fallback: match by (event_id, normalized player name)
        let norm_name = normalize_player_name(&p.player_name);
        for list in lists {
            if list.event_id.as_ref().map(|e| e.as_str()) == Some(p.event_id.as_str()) {
                if let Some(ref lname) = list.player_name {
                    if normalize_player_name(lname) == norm_name {
                        joined.push((list.clone(), p.clone()));
                        break;
                    }
                }
            }
        }
    }

    joined
}

/// Load placements and lists for a set of epochs, deduplicating.
fn load_placements_and_lists(
    state: &AppState,
    epoch_ids: &[String],
) -> (Vec<Placement>, Vec<ArmyList>) {
    let mut all_placements = Vec::new();
    let mut all_lists = Vec::new();

    for epoch_id in epoch_ids {
        if let Ok(placements) =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id)
                .read_all()
        {
            all_placements.extend(placements);
        }
        if let Ok(lists) =
            JsonlReader::<ArmyList>::for_entity(&state.storage, EntityType::ArmyList, epoch_id)
                .read_all()
        {
            all_lists.extend(lists);
        }
    }

    all_placements = dedup_by_id(all_placements, |p| p.id.as_str());
    all_lists = dedup_by_id(all_lists, |l| l.id.as_str());

    (all_placements, all_lists)
}

/// Resolve epoch IDs from query params.
fn resolve_epoch_ids(
    epoch_param: Option<&str>,
    epochs: &[crate::models::MetaEpoch],
    mapper: &crate::models::EpochMapper,
) -> Result<Vec<String>, ApiError> {
    if epoch_param == Some("all") || epoch_param.is_none() {
        if epochs.is_empty() {
            Ok(vec!["current".to_string()])
        } else {
            Ok(epochs.iter().map(|e| e.id.as_str().to_string()).collect())
        }
    } else {
        Ok(vec![crate::api::resolve_epoch(epoch_param, mapper)?])
    }
}

// ── Detachments Endpoint ────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct DetachmentParams {
    pub epoch: Option<String>,
    pub faction: Option<String>,
    pub min_count: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct DetachmentStat {
    pub faction: String,
    pub detachment: String,
    pub count: u32,
    pub avg_win_rate: f64,
    pub avg_rank: f64,
    pub top4_count: u32,
    pub avg_battle_points: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct DetachmentResponse {
    pub detachments: Vec<DetachmentStat>,
}

pub async fn detachment_stats(
    State(state): State<AppState>,
    Query(params): Query<DetachmentParams>,
) -> Result<Json<DetachmentResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;
    let epochs = mapper.all_epochs();
    let epoch_ids = resolve_epoch_ids(params.epoch.as_deref(), epochs, &mapper)?;
    let (placements, lists) = load_placements_and_lists(&state, &epoch_ids);

    let joined = join_lists_to_placements(&lists, &placements);

    let min_count = params.min_count.unwrap_or(3);
    let faction_filter = params.faction.as_deref().map(normalize_faction_name);

    // Group by (faction, detachment)
    struct DetachmentAgg {
        faction: String,
        detachment: String,
        win_rates: Vec<f64>,
        ranks: Vec<f64>,
        top4: u32,
        battle_points: Vec<f64>,
    }

    let mut groups: HashMap<(String, String), DetachmentAgg> = HashMap::new();

    // Use joined pairs for placements with lists, standalone placements for those with detachment
    let mut seen_placement_ids: HashSet<String> = HashSet::new();

    for (list, placement) in &joined {
        let faction = normalize_faction_name(&placement.faction);
        if let Some(ref ff) = faction_filter {
            if &faction != ff {
                continue;
            }
        }
        let detachment = list
            .detachment
            .as_deref()
            .or(placement.detachment.as_deref())
            .unwrap_or("Unknown")
            .to_string();

        seen_placement_ids.insert(placement.id.as_str().to_string());

        let key = (faction.clone(), detachment.clone());
        let agg = groups.entry(key).or_insert_with(|| DetachmentAgg {
            faction: faction.clone(),
            detachment,
            win_rates: Vec::new(),
            ranks: Vec::new(),
            top4: 0,
            battle_points: Vec::new(),
        });

        if let Some(ref record) = placement.record {
            agg.win_rates.push(record.win_rate());
        }
        agg.ranks.push(placement.rank as f64);
        if placement.rank <= 4 {
            agg.top4 += 1;
        }
        if let Some(bp) = placement.battle_points {
            agg.battle_points.push(bp as f64);
        }
    }

    // Also include placements with detachment but no list match
    for placement in &placements {
        if seen_placement_ids.contains(placement.id.as_str()) {
            continue;
        }
        let det = match &placement.detachment {
            Some(d) if !d.is_empty() => d.clone(),
            _ => continue,
        };
        let faction = normalize_faction_name(&placement.faction);
        if let Some(ref ff) = faction_filter {
            if &faction != ff {
                continue;
            }
        }

        let key = (faction.clone(), det.clone());
        let agg = groups.entry(key).or_insert_with(|| DetachmentAgg {
            faction: faction.clone(),
            detachment: det,
            win_rates: Vec::new(),
            ranks: Vec::new(),
            top4: 0,
            battle_points: Vec::new(),
        });

        if let Some(ref record) = placement.record {
            agg.win_rates.push(record.win_rate());
        }
        agg.ranks.push(placement.rank as f64);
        if placement.rank <= 4 {
            agg.top4 += 1;
        }
        if let Some(bp) = placement.battle_points {
            agg.battle_points.push(bp as f64);
        }
    }

    let mut detachments: Vec<DetachmentStat> = groups
        .into_values()
        .filter(|agg| agg.ranks.len() as u32 >= min_count)
        .map(|agg| {
            let count = agg.ranks.len() as u32;
            let avg_win_rate = if agg.win_rates.is_empty() {
                0.0
            } else {
                (agg.win_rates.iter().sum::<f64>() / agg.win_rates.len() as f64 * 1000.0).round()
                    / 10.0
            };
            let avg_rank =
                (agg.ranks.iter().sum::<f64>() / agg.ranks.len() as f64 * 10.0).round() / 10.0;
            let avg_battle_points = if agg.battle_points.is_empty() {
                None
            } else {
                Some(
                    (agg.battle_points.iter().sum::<f64>() / agg.battle_points.len() as f64 * 10.0)
                        .round()
                        / 10.0,
                )
            };
            DetachmentStat {
                faction: agg.faction,
                detachment: agg.detachment,
                count,
                avg_win_rate,
                avg_rank,
                top4_count: agg.top4,
                avg_battle_points,
            }
        })
        .collect();

    detachments.sort_by(|a, b| {
        b.avg_win_rate
            .partial_cmp(&a.avg_win_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(Json(DetachmentResponse { detachments }))
}

// ── Unit Performance Endpoint ───────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UnitPerfParams {
    pub epoch: Option<String>,
    pub faction: Option<String>,
    pub min_appearances: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct UnitPerfStat {
    pub name: String,
    pub faction: String,
    pub total_lists: u32,
    pub in_top4_lists: u32,
    pub in_bottom_half_lists: u32,
    pub top4_rate: f64,
    pub overall_list_rate: f64,
    pub overrepresentation: f64,
    pub avg_rank_when_present: f64,
    pub avg_win_rate_when_present: f64,
}

#[derive(Debug, Serialize)]
pub struct UnitPerfResponse {
    pub units: Vec<UnitPerfStat>,
    pub linked_lists: u32,
    pub total_lists: u32,
}

pub async fn unit_performance(
    State(state): State<AppState>,
    Query(params): Query<UnitPerfParams>,
) -> Result<Json<UnitPerfResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;
    let epochs = mapper.all_epochs();
    let epoch_ids = resolve_epoch_ids(params.epoch.as_deref(), epochs, &mapper)?;
    let (placements, lists) = load_placements_and_lists(&state, &epoch_ids);

    let faction_filter = params.faction.as_deref().map(normalize_faction_name);
    let min_appearances = params.min_appearances.unwrap_or(3);

    let joined = join_lists_to_placements(&lists, &placements);

    // Filter by faction if specified
    let joined: Vec<_> = if let Some(ref ff) = faction_filter {
        joined
            .into_iter()
            .filter(|(_, p)| normalize_faction_name(&p.faction) == *ff)
            .collect()
    } else {
        joined
    };

    let total_lists = joined.len() as u32;
    // Determine player count for "bottom half" threshold
    // Get max rank per event to determine bottom half
    let mut event_max_rank: HashMap<String, u32> = HashMap::new();
    for (_, p) in &joined {
        let entry = event_max_rank
            .entry(p.event_id.as_str().to_string())
            .or_default();
        if p.rank > *entry {
            *entry = p.rank;
        }
    }

    struct UnitAgg {
        faction: String,
        total: u32,
        top4: u32,
        bottom_half: u32,
        ranks: Vec<f64>,
        win_rates: Vec<f64>,
    }

    let mut unit_map: HashMap<String, UnitAgg> = HashMap::new();
    let top4_lists = joined.iter().filter(|(_, p)| p.rank <= 4).count() as u32;

    for (list, placement) in &joined {
        let is_top4 = placement.rank <= 4;
        let max_rank = event_max_rank
            .get(placement.event_id.as_str())
            .copied()
            .unwrap_or(1);
        let is_bottom_half = placement.rank > max_rank / 2;

        let win_rate = placement.record.as_ref().map(|r| r.win_rate());
        let faction = normalize_faction_name(&list.faction);

        for unit in &list.units {
            let agg = unit_map
                .entry(unit.name.clone())
                .or_insert_with(|| UnitAgg {
                    faction: faction.clone(),
                    total: 0,
                    top4: 0,
                    bottom_half: 0,
                    ranks: Vec::new(),
                    win_rates: Vec::new(),
                });

            agg.total += 1;
            if is_top4 {
                agg.top4 += 1;
            }
            if is_bottom_half {
                agg.bottom_half += 1;
            }
            agg.ranks.push(placement.rank as f64);
            if let Some(wr) = win_rate {
                agg.win_rates.push(wr);
            }
        }
    }

    let mut units: Vec<UnitPerfStat> = unit_map
        .into_iter()
        .filter(|(_, agg)| agg.total >= min_appearances)
        .map(|(name, agg)| {
            let overall_list_rate = if total_lists > 0 {
                agg.total as f64 / total_lists as f64
            } else {
                0.0
            };
            let top4_rate = if top4_lists > 0 {
                agg.top4 as f64 / top4_lists as f64
            } else {
                0.0
            };
            let overrepresentation = if overall_list_rate > 0.0 {
                top4_rate / overall_list_rate
            } else {
                0.0
            };
            let avg_rank = if agg.ranks.is_empty() {
                0.0
            } else {
                (agg.ranks.iter().sum::<f64>() / agg.ranks.len() as f64 * 10.0).round() / 10.0
            };
            let avg_win_rate = if agg.win_rates.is_empty() {
                0.0
            } else {
                (agg.win_rates.iter().sum::<f64>() / agg.win_rates.len() as f64 * 1000.0).round()
                    / 10.0
            };

            UnitPerfStat {
                name,
                faction: agg.faction,
                total_lists: agg.total,
                in_top4_lists: agg.top4,
                in_bottom_half_lists: agg.bottom_half,
                top4_rate: (top4_rate * 1000.0).round() / 10.0,
                overall_list_rate: (overall_list_rate * 1000.0).round() / 10.0,
                overrepresentation: (overrepresentation * 100.0).round() / 100.0,
                avg_rank_when_present: avg_rank,
                avg_win_rate_when_present: avg_win_rate,
            }
        })
        .collect();

    units.sort_by(|a, b| {
        b.overrepresentation
            .partial_cmp(&a.overrepresentation)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(Json(UnitPerfResponse {
        units,
        linked_lists: total_lists,
        total_lists: lists.len() as u32,
    }))
}

// ── Points Efficiency Endpoint ──────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct PointsEffParams {
    pub epoch: Option<String>,
    pub faction: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UnitEfficiency {
    pub unit_name: String,
    pub faction: String,
    pub avg_points: u32,
    pub avg_win_rate_when_present: f64,
    pub efficiency_score: f64,
    pub appearances: u32,
}

#[derive(Debug, Serialize)]
pub struct PointsEffResponse {
    pub units: Vec<UnitEfficiency>,
}

pub async fn points_efficiency(
    State(state): State<AppState>,
    Query(params): Query<PointsEffParams>,
) -> Result<Json<PointsEffResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;
    let epochs = mapper.all_epochs();
    let epoch_ids = resolve_epoch_ids(params.epoch.as_deref(), epochs, &mapper)?;
    let (placements, lists) = load_placements_and_lists(&state, &epoch_ids);

    let faction_filter = params.faction.as_deref().map(normalize_faction_name);

    let joined = join_lists_to_placements(&lists, &placements);
    let joined: Vec<_> = if let Some(ref ff) = faction_filter {
        joined
            .into_iter()
            .filter(|(_, p)| normalize_faction_name(&p.faction) == *ff)
            .collect()
    } else {
        joined
    };

    struct EffAgg {
        faction: String,
        points: Vec<u32>,
        win_rates: Vec<f64>,
    }

    let mut unit_map: HashMap<String, EffAgg> = HashMap::new();

    for (list, placement) in &joined {
        let win_rate = match placement.record.as_ref() {
            Some(r) => r.win_rate(),
            None => continue,
        };
        let faction = normalize_faction_name(&list.faction);

        for unit in &list.units {
            let pts = match unit.points {
                Some(p) if p > 0 => p,
                _ => continue,
            };

            let agg = unit_map.entry(unit.name.clone()).or_insert_with(|| EffAgg {
                faction: faction.clone(),
                points: Vec::new(),
                win_rates: Vec::new(),
            });
            agg.points.push(pts);
            agg.win_rates.push(win_rate);
        }
    }

    let mut units: Vec<UnitEfficiency> = unit_map
        .into_iter()
        .filter(|(_, agg)| agg.points.len() >= 3)
        .map(|(name, agg)| {
            let avg_points =
                (agg.points.iter().sum::<u32>() as f64 / agg.points.len() as f64).round() as u32;
            let avg_win_rate = agg.win_rates.iter().sum::<f64>() / agg.win_rates.len() as f64;
            let efficiency_score = if avg_points > 0 {
                avg_win_rate / (avg_points as f64 / 100.0)
            } else {
                0.0
            };

            UnitEfficiency {
                unit_name: name,
                faction: agg.faction,
                avg_points,
                avg_win_rate_when_present: (avg_win_rate * 1000.0).round() / 10.0,
                efficiency_score: (efficiency_score * 1000.0).round() / 1000.0,
                appearances: agg.points.len() as u32,
            }
        })
        .collect();

    units.sort_by(|a, b| {
        b.efficiency_score
            .partial_cmp(&a.efficiency_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    Ok(Json(PointsEffResponse { units }))
}

// ── Matchups Endpoint ───────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MatchupsParams {
    pub epoch: Option<String>,
    pub min_games: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct MatchupStat {
    pub faction1: String,
    pub faction2: String,
    pub faction1_wins: u32,
    pub faction2_wins: u32,
    pub draws: u32,
    pub total_games: u32,
    pub faction1_win_rate: f64,
}

#[derive(Debug, Serialize)]
pub struct MatchupsResponse {
    pub factions: Vec<String>,
    pub matchups: Vec<MatchupStat>,
}

pub async fn matchups(
    State(state): State<AppState>,
    Query(params): Query<MatchupsParams>,
) -> Result<Json<MatchupsResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;
    let epochs = mapper.all_epochs();
    let epoch_ids = resolve_epoch_ids(params.epoch.as_deref(), epochs, &mapper)?;

    let min_games = params.min_games.unwrap_or(5);

    // Load pairings
    let mut all_pairings: Vec<Pairing> = Vec::new();
    for epoch_id in &epoch_ids {
        if let Ok(pairings) =
            JsonlReader::<Pairing>::for_entity(&state.storage, EntityType::Pairing, epoch_id)
                .read_all()
        {
            all_pairings.extend(pairings);
        }
    }
    all_pairings = dedup_by_id(all_pairings, |p| p.id.as_str());

    // Group by (faction1, faction2) — normalize ordering so faction1 < faction2
    struct MatchupAgg {
        faction1: String,
        faction2: String,
        faction1_wins: u32,
        faction2_wins: u32,
        draws: u32,
    }

    let mut matchup_map: HashMap<(String, String), MatchupAgg> = HashMap::new();
    let mut all_factions: HashSet<String> = HashSet::new();

    for pairing in &all_pairings {
        let f1 = match &pairing.player1_faction {
            Some(f) if !f.is_empty() => normalize_faction_name(f),
            _ => continue,
        };
        let f2 = match &pairing.player2_faction {
            Some(f) if !f.is_empty() => normalize_faction_name(f),
            _ => continue,
        };

        // Skip mirror matches
        if f1 == f2 {
            continue;
        }

        all_factions.insert(f1.clone());
        all_factions.insert(f2.clone());

        let (key_f1, key_f2, is_swapped) = if f1 <= f2 {
            (f1.clone(), f2.clone(), false)
        } else {
            (f2.clone(), f1.clone(), true)
        };

        let agg = matchup_map
            .entry((key_f1.clone(), key_f2.clone()))
            .or_insert_with(|| MatchupAgg {
                faction1: key_f1,
                faction2: key_f2,
                faction1_wins: 0,
                faction2_wins: 0,
                draws: 0,
            });

        match pairing.player1_result.as_deref() {
            Some("win") => {
                if is_swapped {
                    agg.faction2_wins += 1;
                } else {
                    agg.faction1_wins += 1;
                }
            }
            Some("loss") => {
                if is_swapped {
                    agg.faction1_wins += 1;
                } else {
                    agg.faction2_wins += 1;
                }
            }
            Some("draw") => agg.draws += 1,
            _ => {}
        }
    }

    let mut matchup_stats: Vec<MatchupStat> = matchup_map
        .into_values()
        .filter(|agg| agg.faction1_wins + agg.faction2_wins + agg.draws >= min_games)
        .map(|agg| {
            let total = agg.faction1_wins + agg.faction2_wins + agg.draws;
            let win_rate = if total > 0 {
                (agg.faction1_wins as f64 / total as f64 * 1000.0).round() / 10.0
            } else {
                0.0
            };
            MatchupStat {
                faction1: agg.faction1,
                faction2: agg.faction2,
                faction1_wins: agg.faction1_wins,
                faction2_wins: agg.faction2_wins,
                draws: agg.draws,
                total_games: total,
                faction1_win_rate: win_rate,
            }
        })
        .collect();

    matchup_stats.sort_by(|a, b| b.total_games.cmp(&a.total_games));

    let mut factions: Vec<String> = all_factions.into_iter().collect();
    factions.sort();

    Ok(Json(MatchupsResponse {
        factions,
        matchups: matchup_stats,
    }))
}

// ── Archetypes Endpoint ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ArchetypesParams {
    pub epoch: Option<String>,
    pub faction: String,
}

#[derive(Debug, Serialize)]
pub struct ArchetypeUnit {
    pub name: String,
    pub count: u32,
    pub points: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct ArchetypeListEntry {
    pub player_name: String,
    pub rank: u32,
    pub event_id: String,
    pub total_points: u32,
    pub units: Vec<ArchetypeUnit>,
}

#[derive(Debug, Serialize)]
pub struct ArchetypeStat {
    pub name: String,
    pub detachment: String,
    pub defining_units: Vec<String>,
    pub list_count: u32,
    pub avg_rank: f64,
    pub avg_win_rate: f64,
    pub sample_lists: Vec<ArchetypeListEntry>,
}

#[derive(Debug, Serialize)]
pub struct ArchetypesResponse {
    pub faction: String,
    pub archetypes: Vec<ArchetypeStat>,
    pub total_lists: u32,
}

/// Jaccard similarity between two sets of unit names.
fn jaccard_similarity(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

pub async fn archetypes(
    State(state): State<AppState>,
    Query(params): Query<ArchetypesParams>,
) -> Result<Json<ArchetypesResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;
    let epochs = mapper.all_epochs();
    let epoch_ids = resolve_epoch_ids(params.epoch.as_deref(), epochs, &mapper)?;
    let (placements, lists) = load_placements_and_lists(&state, &epoch_ids);

    let faction_norm = normalize_faction_name(&params.faction);

    // Filter lists to this faction
    let faction_lists: Vec<&ArmyList> = lists
        .iter()
        .filter(|l| normalize_faction_name(&l.faction) == faction_norm && !l.units.is_empty())
        .collect();

    let total_lists = faction_lists.len() as u32;

    if faction_lists.is_empty() {
        return Ok(Json(ArchetypesResponse {
            faction: faction_norm,
            archetypes: vec![],
            total_lists: 0,
        }));
    }

    // Build unit name sets for each list
    let list_unit_sets: Vec<HashSet<String>> = faction_lists
        .iter()
        .map(|l| l.units.iter().map(|u| u.name.clone()).collect())
        .collect();

    // Global unit frequencies across all faction lists
    let mut global_unit_freq: HashMap<String, u32> = HashMap::new();
    for set in &list_unit_sets {
        for unit in set {
            *global_unit_freq.entry(unit.clone()).or_default() += 1;
        }
    }

    // Group by detachment first, then cluster within each detachment
    let mut detachment_groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, list) in faction_lists.iter().enumerate() {
        let det = list.detachment.as_deref().unwrap_or("Unknown").to_string();
        detachment_groups.entry(det).or_default().push(idx);
    }

    // Simple greedy clustering within each detachment group
    let mut archetypes = Vec::new();

    for (detachment, indices) in &detachment_groups {
        let mut assigned: Vec<bool> = vec![false; indices.len()];
        let mut clusters: Vec<Vec<usize>> = Vec::new();

        for i in 0..indices.len() {
            if assigned[i] {
                continue;
            }
            let mut cluster = vec![indices[i]];
            assigned[i] = true;

            for j in (i + 1)..indices.len() {
                if assigned[j] {
                    continue;
                }
                let sim =
                    jaccard_similarity(&list_unit_sets[indices[i]], &list_unit_sets[indices[j]]);
                if sim >= 0.5 {
                    cluster.push(indices[j]);
                    assigned[j] = true;
                }
            }

            if cluster.len() >= 2 {
                clusters.push(cluster);
            }
        }

        // For each cluster, find defining units and compute stats
        let joined = join_lists_to_placements(&lists, &placements);
        let placement_by_list_id: HashMap<String, &Placement> = joined
            .iter()
            .map(|(l, p)| (l.id.as_str().to_string(), p))
            .collect();

        for cluster in &clusters {
            // Unit frequency within cluster
            let mut cluster_unit_freq: HashMap<String, u32> = HashMap::new();
            for &idx in cluster {
                for unit in &list_unit_sets[idx] {
                    *cluster_unit_freq.entry(unit.clone()).or_default() += 1;
                }
            }

            let cluster_size = cluster.len() as f64;

            // Defining units: present in >=60% of cluster, <30% of faction overall
            let defining_units: Vec<String> = cluster_unit_freq
                .iter()
                .filter(|(unit, count)| {
                    let cluster_rate = **count as f64 / cluster_size;
                    let global_rate =
                        *global_unit_freq.get(*unit).unwrap_or(&0) as f64 / total_lists as f64;
                    cluster_rate >= 0.6 && global_rate < 0.3
                })
                .map(|(unit, _)| unit.clone())
                .collect();

            // Compute performance stats
            let mut ranks: Vec<f64> = Vec::new();
            let mut win_rates: Vec<f64> = Vec::new();

            for &idx in cluster {
                let list = faction_lists[idx];
                if let Some(p) = placement_by_list_id.get(list.id.as_str()) {
                    ranks.push(p.rank as f64);
                    if let Some(ref record) = p.record {
                        win_rates.push(record.win_rate());
                    }
                }
            }

            let avg_rank = if ranks.is_empty() {
                0.0
            } else {
                (ranks.iter().sum::<f64>() / ranks.len() as f64 * 10.0).round() / 10.0
            };
            let avg_win_rate = if win_rates.is_empty() {
                0.0
            } else {
                (win_rates.iter().sum::<f64>() / win_rates.len() as f64 * 1000.0).round() / 10.0
            };

            let name = if defining_units.is_empty() {
                format!("{} {}", detachment, cluster.len())
            } else {
                defining_units
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" + ")
            };

            // Collect all lists in the cluster with placement info
            let mut sample_lists: Vec<ArchetypeListEntry> = Vec::new();
            for &idx in cluster {
                let list = faction_lists[idx];
                let player_name;
                let rank;
                let event_id;
                if let Some(p) = placement_by_list_id.get(list.id.as_str()) {
                    player_name = p.player_name.clone();
                    rank = p.rank;
                    event_id = p.event_id.as_str().to_string();
                } else {
                    player_name = list
                        .player_name
                        .clone()
                        .unwrap_or_else(|| "Unknown".to_string());
                    rank = 0;
                    event_id = list
                        .event_id
                        .as_ref()
                        .map(|e| e.as_str().to_string())
                        .unwrap_or_default();
                }
                let units = list
                    .units
                    .iter()
                    .map(|u| ArchetypeUnit {
                        name: u.name.clone(),
                        count: u.count,
                        points: u.points,
                    })
                    .collect();
                sample_lists.push(ArchetypeListEntry {
                    player_name,
                    rank,
                    event_id,
                    total_points: list.total_points,
                    units,
                });
            }
            sample_lists.sort_by_key(|e| e.rank);

            archetypes.push(ArchetypeStat {
                name,
                detachment: detachment.clone(),
                defining_units,
                list_count: cluster.len() as u32,
                avg_rank,
                avg_win_rate,
                sample_lists,
            });
        }
    }

    archetypes.sort_by(|a, b| b.list_count.cmp(&a.list_count));

    Ok(Json(ArchetypesResponse {
        faction: faction_norm,
        archetypes,
        total_lists,
    }))
}

// ── Win Rates Endpoint ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WinRatesParams {
    pub epoch: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub min_games: Option<u32>,
    pub min_players: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct FactionWinRate {
    pub faction: String,
    pub allegiance: String,
    pub win_rate: f64,
    pub adjusted_win_rate: f64,
    pub games_played: u32,
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
    pub player_count: u32,
}

#[derive(Debug, Serialize)]
pub struct WinRatesResponse {
    pub factions: Vec<FactionWinRate>,
    pub total_games: u32,
    pub average_win_rate: f64,
}

pub async fn win_rates(
    State(state): State<AppState>,
    Query(params): Query<WinRatesParams>,
) -> Result<Json<WinRatesResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;
    let epochs = mapper.all_epochs();
    let epoch_ids = resolve_epoch_ids(params.epoch.as_deref(), epochs, &mapper)?;

    let min_players_filter = params.min_players.unwrap_or(0);
    // Prior weight for regression to the mean: adding K imaginary games at 50%.
    // Higher K = more conservative (small samples pulled harder toward 50%).
    let prior_weight: f64 = params.min_games.unwrap_or(40) as f64;

    // Parse optional date range filters
    let from_date = params
        .from
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let to_date = params
        .to
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

    // Load placements and events
    let mut all_placements = Vec::new();
    let mut all_events = Vec::new();
    for epoch_id in &epoch_ids {
        if let Ok(placements) =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id)
                .read_all()
        {
            all_placements.extend(placements);
        }
        if let Ok(events) =
            JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, epoch_id).read_all()
        {
            all_events.extend(events);
        }
    }

    all_placements = dedup_by_id(all_placements, |p| p.id.as_str());
    all_events = dedup_by_id(all_events, |e| e.id.as_str());

    // Build event lookup for date filtering and player count filtering
    let event_map: HashMap<String, &Event> = all_events
        .iter()
        .map(|e| (e.id.as_str().to_string(), e))
        .collect();

    // Filter placements by date range if specified
    if from_date.is_some() || to_date.is_some() {
        all_placements.retain(|p| match event_map.get(p.event_id.as_str()) {
            Some(e) => from_date.is_none_or(|f| e.date >= f) && to_date.is_none_or(|t| e.date <= t),
            None => true,
        });
    }

    // Filter by min_players (tournament size)
    if min_players_filter > 0 {
        all_placements.retain(|p| match event_map.get(p.event_id.as_str()) {
            Some(e) => e.player_count.unwrap_or(0) >= min_players_filter,
            None => true,
        });
    }

    // Filter to events with full standings to avoid survivorship bias.
    // Top-only sources (e.g. Goonhammer articles reporting only top 4-8)
    // inflate win rates because they only capture winners.
    // We detect full standings by checking the max rank per event — if the
    // highest rank is <= 8, we likely only have top finishers.
    let mut event_max_rank: HashMap<String, u32> = HashMap::new();
    for p in &all_placements {
        let entry = event_max_rank
            .entry(p.event_id.as_str().to_string())
            .or_default();
        if p.rank > *entry {
            *entry = p.rank;
        }
    }
    let full_event_ids: HashSet<String> = event_max_rank
        .into_iter()
        .filter(|(_, max_rank)| *max_rank > 20)
        .map(|(eid, _)| eid)
        .collect();

    // Only use placements from events with full standings
    all_placements.retain(|p| full_event_ids.contains(p.event_id.as_str()));

    // Accumulate W/L/D per faction
    struct FactionAgg {
        wins: u32,
        losses: u32,
        draws: u32,
        players: HashSet<String>,
    }

    let mut faction_stats: HashMap<String, FactionAgg> = HashMap::new();

    for p in &all_placements {
        let record = match &p.record {
            Some(r) if r.total_games() > 0 => r,
            _ => continue,
        };
        let faction = normalize_faction_name(&p.faction);
        let agg = faction_stats.entry(faction).or_insert_with(|| FactionAgg {
            wins: 0,
            losses: 0,
            draws: 0,
            players: HashSet::new(),
        });
        agg.wins += record.wins;
        agg.losses += record.losses;
        agg.draws += record.draws;
        agg.players.insert(normalize_player_name(&p.player_name));
    }

    // Compute win rates with regression to the mean
    let mut factions: Vec<FactionWinRate> = faction_stats
        .into_iter()
        .map(|(faction, agg)| {
            let total = agg.wins + agg.losses + agg.draws;
            let raw_wins = agg.wins as f64 + 0.5 * agg.draws as f64;
            let win_rate = if total > 0 {
                (raw_wins / total as f64 * 1000.0).round() / 10.0
            } else {
                0.0
            };
            // Regression to the mean: blend raw rate with 50% prior
            // adjusted = (actual_wins + K * 0.5) / (games + K)
            let adjusted_win_rate = if total > 0 {
                ((raw_wins + prior_weight * 0.5) / (total as f64 + prior_weight) * 1000.0).round()
                    / 10.0
            } else {
                50.0
            };
            let allegiance = faction_allegiance(&faction)
                .unwrap_or("Unknown")
                .to_string();
            FactionWinRate {
                faction,
                allegiance,
                win_rate,
                adjusted_win_rate,
                games_played: total,
                wins: agg.wins,
                losses: agg.losses,
                draws: agg.draws,
                player_count: agg.players.len() as u32,
            }
        })
        .collect();

    factions.sort_by(|a, b| {
        b.adjusted_win_rate
            .partial_cmp(&a.adjusted_win_rate)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let total_games: u32 = factions.iter().map(|f| f.games_played).sum();
    let average_win_rate = if factions.is_empty() {
        0.0
    } else {
        let sum: f64 = factions.iter().map(|f| f.win_rate).sum();
        (sum / factions.len() as f64 * 10.0).round() / 10.0
    };

    Ok(Json(WinRatesResponse {
        factions,
        total_games,
        average_win_rate,
    }))
}

// ── Composite Scores Endpoint ───────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CompositeScoresParams {
    pub epoch: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub min_players: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct FactionCompositeScore {
    pub faction: String,
    pub allegiance: String,
    // Raw inputs
    pub adjusted_win_rate: f64,
    pub meta_share: f64,
    pub top4_rate: f64,
    pub first_place_rate: f64,
    pub games_played: u32,
    pub placement_count: u32,
    // Composite scores
    pub meta_threat: f64,
    pub expected_podiums: f64,
    pub balance_deviation: f64,
    pub power_index: f64,
}

#[derive(Debug, Serialize)]
pub struct CompositeScoresResponse {
    pub factions: Vec<FactionCompositeScore>,
    pub total_placements: u32,
    pub total_games: u32,
}

fn percentile_ranks(values: &[f64]) -> Vec<f64> {
    let n = values.len() as f64;
    if n == 0.0 {
        return vec![];
    }
    values
        .iter()
        .map(|v| {
            let below = values.iter().filter(|x| **x < *v).count() as f64;
            below / (n - 1.0).max(1.0)
        })
        .collect()
}

pub async fn composite_scores(
    State(state): State<AppState>,
    Query(params): Query<CompositeScoresParams>,
) -> Result<Json<CompositeScoresResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;
    let epochs = mapper.all_epochs();
    let epoch_ids = resolve_epoch_ids(params.epoch.as_deref(), epochs, &mapper)?;

    let min_players_filter = params.min_players.unwrap_or(0);
    let prior_weight: f64 = 40.0;

    // Parse optional date range filters
    let from_date = params
        .from
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let to_date = params
        .to
        .as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());

    // Load placements and events
    let mut all_placements = Vec::new();
    let mut all_events = Vec::new();
    for epoch_id in &epoch_ids {
        if let Ok(placements) =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id)
                .read_all()
        {
            all_placements.extend(placements);
        }
        if let Ok(events) =
            JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, epoch_id).read_all()
        {
            all_events.extend(events);
        }
    }

    all_placements = dedup_by_id(all_placements, |p| p.id.as_str());
    all_events = dedup_by_id(all_events, |e| e.id.as_str());

    // Build event lookup
    let event_map: HashMap<String, &Event> = all_events
        .iter()
        .map(|e| (e.id.as_str().to_string(), e))
        .collect();

    // Filter by date range
    if from_date.is_some() || to_date.is_some() {
        all_placements.retain(|p| match event_map.get(p.event_id.as_str()) {
            Some(e) => from_date.is_none_or(|f| e.date >= f) && to_date.is_none_or(|t| e.date <= t),
            None => true,
        });
    }

    // Filter by min_players (tournament size)
    if min_players_filter > 0 {
        all_placements.retain(|p| match event_map.get(p.event_id.as_str()) {
            Some(e) => e.player_count.unwrap_or(0) >= min_players_filter,
            None => true,
        });
    }

    let total_placements_count = all_placements.len() as u32;

    // ── Faction stats (meta share, top4, first place) ──
    let mut faction_placement_map: HashMap<String, Vec<&Placement>> = HashMap::new();
    for p in &all_placements {
        faction_placement_map
            .entry(normalize_faction_name(&p.faction))
            .or_default()
            .push(p);
    }

    struct FactionMeta {
        count: u32,
        meta_share: f64,
        top4_rate: f64,
        first_place_rate: f64,
    }

    let mut faction_meta: HashMap<String, FactionMeta> = HashMap::new();
    for (faction, ps) in &faction_placement_map {
        let count = ps.len() as u32;
        let meta_share = if total_placements_count > 0 {
            (count as f64 / total_placements_count as f64) * 100.0
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
        let first_place_rate = if count > 0 {
            (first_place_count as f64 / count as f64) * 100.0
        } else {
            0.0
        };
        faction_meta.insert(
            faction.clone(),
            FactionMeta {
                count,
                meta_share,
                top4_rate,
                first_place_rate,
            },
        );
    }

    // ── Win rates (full-standings filter + regression to mean) ──
    let mut event_max_rank: HashMap<String, u32> = HashMap::new();
    for p in &all_placements {
        let entry = event_max_rank
            .entry(p.event_id.as_str().to_string())
            .or_default();
        if p.rank > *entry {
            *entry = p.rank;
        }
    }
    let full_event_ids: HashSet<String> = event_max_rank
        .into_iter()
        .filter(|(_, max_rank)| *max_rank > 20)
        .map(|(eid, _)| eid)
        .collect();

    struct WinRateAgg {
        wins: u32,
        losses: u32,
        draws: u32,
    }

    let mut wr_stats: HashMap<String, WinRateAgg> = HashMap::new();
    for p in &all_placements {
        if !full_event_ids.contains(p.event_id.as_str()) {
            continue;
        }
        let record = match &p.record {
            Some(r) if r.total_games() > 0 => r,
            _ => continue,
        };
        let faction = normalize_faction_name(&p.faction);
        let agg = wr_stats.entry(faction).or_insert(WinRateAgg {
            wins: 0,
            losses: 0,
            draws: 0,
        });
        agg.wins += record.wins;
        agg.losses += record.losses;
        agg.draws += record.draws;
    }

    // ── Join and compute composites ──
    // Only include factions present in both datasets
    let mut composite_factions: Vec<(String, f64, f64, f64, f64, u32, u32)> = Vec::new();
    for (faction, meta) in &faction_meta {
        if let Some(wr) = wr_stats.get(faction) {
            let total_games = wr.wins + wr.losses + wr.draws;
            if total_games == 0 {
                continue;
            }
            let raw_wins = wr.wins as f64 + 0.5 * wr.draws as f64;
            let adjusted_win_rate =
                ((raw_wins + prior_weight * 0.5) / (total_games as f64 + prior_weight) * 1000.0)
                    .round()
                    / 10.0;
            composite_factions.push((
                faction.clone(),
                adjusted_win_rate,
                meta.meta_share,
                meta.top4_rate,
                meta.first_place_rate,
                total_games,
                meta.count,
            ));
        }
    }

    // Compute percentile ranks for power index
    let wr_vals: Vec<f64> = composite_factions.iter().map(|f| f.1).collect();
    let ms_vals: Vec<f64> = composite_factions.iter().map(|f| f.2).collect();
    let t4_vals: Vec<f64> = composite_factions.iter().map(|f| f.3).collect();
    let fp_vals: Vec<f64> = composite_factions.iter().map(|f| f.4).collect();

    let wr_ranks = percentile_ranks(&wr_vals);
    let ms_ranks = percentile_ranks(&ms_vals);
    let t4_ranks = percentile_ranks(&t4_vals);
    let fp_ranks = percentile_ranks(&fp_vals);

    let mut factions: Vec<FactionCompositeScore> = composite_factions
        .iter()
        .enumerate()
        .map(|(i, (faction, adj_wr, ms, t4r, fpr, games, placements))| {
            let meta_threat = *adj_wr * ms.sqrt();
            let expected_podiums = *ms * *t4r / 100.0;
            let balance_deviation = (*adj_wr - 50.0) * ms.sqrt();
            let power_index =
                ((wr_ranks[i] + ms_ranks[i] + t4_ranks[i] + fp_ranks[i]) / 4.0 * 1000.0).round()
                    / 10.0;
            let allegiance = faction_allegiance(faction).unwrap_or("Unknown").to_string();
            FactionCompositeScore {
                faction: faction.clone(),
                allegiance,
                adjusted_win_rate: (adj_wr * 10.0).round() / 10.0,
                meta_share: (ms * 10.0).round() / 10.0,
                top4_rate: (t4r * 10.0).round() / 10.0,
                first_place_rate: (fpr * 10.0).round() / 10.0,
                games_played: *games,
                placement_count: *placements,
                meta_threat: (meta_threat * 10.0).round() / 10.0,
                expected_podiums: (expected_podiums * 100.0).round() / 100.0,
                balance_deviation: (balance_deviation * 10.0).round() / 10.0,
                power_index,
            }
        })
        .collect();

    factions.sort_by(|a, b| {
        b.meta_threat
            .partial_cmp(&a.meta_threat)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let total_games: u32 = factions.iter().map(|f| f.games_played).sum();

    Ok(Json(CompositeScoresResponse {
        factions,
        total_placements: total_placements_count,
        total_games,
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
            epoch_mapper: Arc::new(tokio::sync::RwLock::new(EpochMapper::new())),
            refresh_state: Arc::new(tokio::sync::RwLock::new(
                crate::api::routes::refresh::RefreshState::default(),
            )),
            ai_backend: Arc::new(crate::agents::backend::MockBackend::new("{}")),
            traffic_stats: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::api::routes::traffic::TrafficStats::new(),
            )),
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
            epoch_mapper: Arc::new(tokio::sync::RwLock::new(mapper)),
            refresh_state: Arc::new(tokio::sync::RwLock::new(
                crate::api::routes::refresh::RefreshState::default(),
            )),
            ai_backend: Arc::new(crate::agents::backend::MockBackend::new("{}")),
            traffic_stats: std::sync::Arc::new(tokio::sync::RwLock::new(
                crate::api::routes::traffic::TrafficStats::new(),
            )),
        }
    }

    #[tokio::test]
    async fn test_analytics_trends() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state_with_epoch(tmp.path());
        let epoch_id = state.epoch_mapper.read().await.all_epochs()[0]
            .id
            .as_str()
            .to_string();
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
        let epoch_id = state.epoch_mapper.read().await.all_epochs()[0]
            .id
            .as_str()
            .to_string();
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

    // ── Win Rates Tests ─────────────────────────────────────────

    /// Create filler placements to make an event look like full standings
    /// (max rank > 20), so the win rate endpoint doesn't filter it out.
    fn fill_event(event: &Event, start_rank: u32, count: u32) -> Vec<Placement> {
        (0..count)
            .map(|i| {
                let rank = start_rank + i;
                make_placement(event, rank, &format!("Filler{}", rank), "Orks").with_record(2, 3, 0)
            })
            .collect()
    }

    #[tokio::test]
    async fn test_win_rates_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari").with_record(5, 1, 0);
        let p2 = make_placement(&e1, 2, "Bob", "Necrons").with_record(3, 3, 0);
        let p3 = make_placement(&e1, 3, "Charlie", "Aeldari").with_record(4, 2, 0);
        let mut all_p: Vec<Placement> = vec![p1, p2, p3];
        all_p.extend(fill_event(&e1, 4, 20));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(
            &epoch_dir.join("placements.jsonl"),
            &all_p.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/win-rates?min_games=0").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();
        // Aeldari, Necrons, and Orks (filler)
        assert!(factions.len() >= 2);
        // Aeldari: (5+4) wins, (1+2) losses = 9/12 = 75%
        let aeldari = factions.iter().find(|f| f["faction"] == "Aeldari").unwrap();
        assert_eq!(aeldari["wins"], 9);
        assert_eq!(aeldari["losses"], 3);
        assert_eq!(aeldari["games_played"], 12);
        assert_eq!(aeldari["win_rate"], 75.0);
        // Necrons: 3/6 = 50%
        let necrons = factions.iter().find(|f| f["faction"] == "Necrons").unwrap();
        assert_eq!(necrons["win_rate"], 50.0);
    }

    #[tokio::test]
    async fn test_win_rates_draws_counted_half() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        // 3 wins, 1 loss, 2 draws = (3 + 0.5*2) / 6 = 4/6 = 66.7%
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari").with_record(3, 1, 2);
        let mut all_p = vec![p1];
        all_p.extend(fill_event(&e1, 2, 22));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(
            &epoch_dir.join("placements.jsonl"),
            &all_p.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/win-rates?min_games=0").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();
        let f = factions.iter().find(|f| f["faction"] == "Aeldari").unwrap();
        assert_eq!(f["wins"], 3);
        assert_eq!(f["draws"], 2);
        assert_eq!(f["games_played"], 6);
        // (3 + 0.5*2) / 6 * 100 = 66.7
        assert_eq!(f["win_rate"], 66.7);
    }

    #[tokio::test]
    async fn test_win_rates_skips_no_record() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari").with_record(5, 1, 0);
        let p2 = make_placement(&e1, 2, "Bob", "Necrons"); // no record
        let mut all_p = vec![p1, p2];
        all_p.extend(fill_event(&e1, 3, 20));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(
            &epoch_dir.join("placements.jsonl"),
            &all_p.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/win-rates?min_games=0").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();
        // Necrons should NOT appear (no record), but Aeldari and Orks (filler) should
        assert!(!factions.iter().any(|f| f["faction"] == "Necrons"));
        assert!(factions.iter().any(|f| f["faction"] == "Aeldari"));
    }

    #[tokio::test]
    async fn test_win_rates_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/win-rates").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["factions"].as_array().unwrap().is_empty());
        assert_eq!(json["total_games"], 0);
        assert_eq!(json["average_win_rate"], 0.0);
    }

    #[tokio::test]
    async fn test_win_rates_regression_to_mean() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        // Small sample: 5W 1L = 83.3% raw, but adjusted should be pulled toward 50%
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari").with_record(5, 1, 0);
        let mut all_p = vec![p1];
        all_p.extend(fill_event(&e1, 2, 22));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(
            &epoch_dir.join("placements.jsonl"),
            &all_p.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        // min_games=40 (prior weight) — 6 real games + 40 imaginary at 50%
        // adjusted = (5 + 40*0.5) / (6 + 40) = 25/46 = 54.3%
        let (status, json) = get_json(app, "/api/analytics/win-rates").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();
        let f = factions.iter().find(|f| f["faction"] == "Aeldari").unwrap();
        assert_eq!(f["win_rate"], 83.3); // raw rate unchanged
        let adjusted = f["adjusted_win_rate"].as_f64().unwrap();
        // Should be much closer to 50% than the raw 83.3%
        assert!(
            adjusted > 50.0 && adjusted < 60.0,
            "adjusted={adjusted} should be 50-60"
        );
    }

    #[tokio::test]
    async fn test_win_rates_excludes_top_only_events() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        // Event with only top 4 placements (top-only source, should be excluded)
        let e1 = make_event("Small GT", "2026-01-15", "https://example.com/a");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari").with_record(5, 0, 0);
        let p2 = make_placement(&e1, 2, "Bob", "Aeldari").with_record(4, 1, 0);
        let p3 = make_placement(&e1, 3, "Charlie", "Necrons").with_record(3, 2, 0);
        let p4 = make_placement(&e1, 4, "Dave", "Necrons").with_record(3, 2, 0);

        // Event with full standings (should be included)
        let e2 = make_event("Big GT", "2026-01-22", "https://example.com/b");
        let q1 = make_placement(&e2, 1, "Eve", "Necrons").with_record(4, 1, 0);
        let mut all_p = vec![p1, p2, p3, p4, q1];
        all_p.extend(fill_event(&e2, 2, 22));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl(
            &epoch_dir.join("placements.jsonl"),
            &all_p.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/win-rates?min_games=0").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();
        // Aeldari should NOT appear — they only exist in the top-only event
        assert!(!factions.iter().any(|f| f["faction"] == "Aeldari"));
        // Necrons should appear from the full event only (4W 1L from Eve)
        let necrons = factions.iter().find(|f| f["faction"] == "Necrons").unwrap();
        assert_eq!(necrons["wins"], 4);
        assert_eq!(necrons["games_played"], 5);
    }

    // ── Composite Scores Tests ──────────────────────────────────

    #[tokio::test]
    async fn test_composite_scores_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("Big GT", "2026-01-15", "https://example.com/a");
        // Aeldari: strong win record
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari").with_record(5, 0, 0);
        let p2 = make_placement(&e1, 3, "Charlie", "Aeldari").with_record(3, 2, 0);
        // Necrons: mediocre
        let p3 = make_placement(&e1, 2, "Bob", "Necrons").with_record(3, 2, 0);
        let p4 = make_placement(&e1, 5, "Dave", "Necrons").with_record(2, 3, 0);
        let mut all_p: Vec<Placement> = vec![p1, p2, p3, p4];
        all_p.extend(fill_event(&e1, 6, 20));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(
            &epoch_dir.join("placements.jsonl"),
            &all_p.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/composite-scores").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();
        assert!(factions.len() >= 2);

        let aeldari = factions.iter().find(|f| f["faction"] == "Aeldari").unwrap();
        let necrons = factions.iter().find(|f| f["faction"] == "Necrons").unwrap();

        // Aeldari should have higher meta_threat than Necrons
        assert!(
            aeldari["meta_threat"].as_f64().unwrap() > necrons["meta_threat"].as_f64().unwrap()
        );
        // Both should have non-zero power_index
        assert!(aeldari["power_index"].as_f64().unwrap() > 0.0);
        assert!(necrons["power_index"].as_f64().unwrap() >= 0.0);
        // expected_podiums should be non-negative
        assert!(aeldari["expected_podiums"].as_f64().unwrap() >= 0.0);
        // Check total_placements and total_games are populated
        assert!(json["total_placements"].as_u64().unwrap() > 0);
        assert!(json["total_games"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_composite_scores_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/composite-scores").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["factions"].as_array().unwrap().is_empty());
        assert_eq!(json["total_placements"], 0);
        assert_eq!(json["total_games"], 0);
    }

    #[tokio::test]
    async fn test_composite_scores_balance_deviation_sign() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Balance", "2026-01-15", "https://example.com/a");
        // Strong faction (above 50%)
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari").with_record(5, 0, 0);
        // Weak faction (below 50%)
        let p2 = make_placement(&e1, 10, "Bob", "Necrons").with_record(1, 4, 0);
        let mut all_p: Vec<Placement> = vec![p1, p2];
        all_p.extend(fill_event(&e1, 2, 22));

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(
            &epoch_dir.join("placements.jsonl"),
            &all_p.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/composite-scores").await;

        assert_eq!(status, StatusCode::OK);
        let factions = json["factions"].as_array().unwrap();

        let aeldari = factions.iter().find(|f| f["faction"] == "Aeldari").unwrap();
        let necrons = factions.iter().find(|f| f["faction"] == "Necrons").unwrap();

        // Aeldari: above 50% → positive balance_deviation
        assert!(
            aeldari["balance_deviation"].as_f64().unwrap() > 0.0,
            "Strong faction should have positive balance deviation"
        );
        // Necrons: below 50% → negative balance_deviation
        assert!(
            necrons["balance_deviation"].as_f64().unwrap() < 0.0,
            "Weak faction should have negative balance deviation"
        );
    }

    // ── Jaccard Similarity Tests ─────────────────────────────────

    #[test]
    fn test_jaccard_similarity_identical() {
        use super::jaccard_similarity;
        use std::collections::HashSet;
        let a: HashSet<String> = ["Wraithguard", "Wave Serpent"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!((jaccard_similarity(&a, &a) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_jaccard_similarity_disjoint() {
        use super::jaccard_similarity;
        use std::collections::HashSet;
        let a: HashSet<String> = ["Wraithguard"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["Wave Serpent"].iter().map(|s| s.to_string()).collect();
        assert!((jaccard_similarity(&a, &b)).abs() < f64::EPSILON);
    }

    #[test]
    fn test_jaccard_similarity_empty() {
        use super::jaccard_similarity;
        use std::collections::HashSet;
        let empty: HashSet<String> = HashSet::new();
        assert!((jaccard_similarity(&empty, &empty) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_jaccard_similarity_partial() {
        use super::jaccard_similarity;
        use std::collections::HashSet;
        let a: HashSet<String> = ["A", "B", "C"].iter().map(|s| s.to_string()).collect();
        let b: HashSet<String> = ["B", "C", "D"].iter().map(|s| s.to_string()).collect();
        // intersection = {B,C} = 2, union = {A,B,C,D} = 4 → 0.5
        assert!((jaccard_similarity(&a, &b) - 0.5).abs() < f64::EPSILON);
    }

    // ── Detachment Stats Tests ──────────────────────────────────

    #[tokio::test]
    async fn test_detachment_stats_basic() {
        use crate::models::{ArmyList, Unit};

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");
        let list1 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![Unit::new("Wraithguard".to_string(), 5)],
            "raw".to_string(),
        )
        .with_detachment("Seer Council".to_string())
        .with_player_name("Alice".to_string())
        .with_event_id(e1.id.clone());

        let list2 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![Unit::new("Fire Prism".to_string(), 1)],
            "raw".to_string(),
        )
        .with_detachment("Seer Council".to_string())
        .with_player_name("Bob".to_string())
        .with_event_id(e1.id.clone());

        let list3 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![Unit::new("Wraithknight".to_string(), 1)],
            "raw".to_string(),
        )
        .with_detachment("Seer Council".to_string())
        .with_player_name("Charlie".to_string())
        .with_event_id(e1.id.clone());

        let mut p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        p1.list_id = Some(list1.id.clone());
        p1 = p1.with_record(5, 0, 0);
        let mut p2 = make_placement(&e1, 2, "Bob", "Aeldari");
        p2.list_id = Some(list2.id.clone());
        p2 = p2.with_record(4, 1, 0);
        let mut p3 = make_placement(&e1, 3, "Charlie", "Aeldari");
        p3.list_id = Some(list3.id.clone());
        p3 = p3.with_record(3, 2, 0);

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2, &p3]);
        write_jsonl(
            &epoch_dir.join("army_lists.jsonl"),
            &[&list1, &list2, &list3],
        );

        let app = build_router(state);
        let (status, json) = get_json(
            app,
            "/api/analytics/detachments?faction=Aeldari&min_count=1",
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let detachments = json["detachments"].as_array().unwrap();
        assert!(!detachments.is_empty());
        let sc = detachments
            .iter()
            .find(|d| d["detachment"] == "Seer Council")
            .unwrap();
        assert_eq!(sc["count"], 3);
        assert_eq!(sc["faction"], "Aeldari");
    }

    #[tokio::test]
    async fn test_detachment_stats_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/detachments").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["detachments"].as_array().unwrap().is_empty());
    }

    // ── Unit Performance Tests ──────────────────────────────────

    #[tokio::test]
    async fn test_unit_performance_basic() {
        use crate::models::{ArmyList, Unit};

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");

        let list1 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![
                Unit::new("Wraithguard".to_string(), 5).with_points(180),
                Unit::new("Wave Serpent".to_string(), 1).with_points(120),
            ],
            "raw".to_string(),
        )
        .with_player_name("Alice".to_string())
        .with_event_id(e1.id.clone());

        let list2 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![
                Unit::new("Wraithguard".to_string(), 5).with_points(180),
                Unit::new("Fire Prism".to_string(), 1).with_points(150),
            ],
            "raw".to_string(),
        )
        .with_player_name("Bob".to_string())
        .with_event_id(e1.id.clone());

        let mut p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        p1.list_id = Some(list1.id.clone());
        p1 = p1.with_record(5, 0, 0);
        let mut p2 = make_placement(&e1, 8, "Bob", "Aeldari");
        p2.list_id = Some(list2.id.clone());
        p2 = p2.with_record(2, 3, 0);

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list1, &list2]);

        let app = build_router(state);
        let (status, json) = get_json(
            app,
            "/api/analytics/unit-performance?faction=Aeldari&min_appearances=1",
        )
        .await;

        assert_eq!(status, StatusCode::OK);
        let units = json["units"].as_array().unwrap();
        assert!(!units.is_empty());
        // Wraithguard appears in both lists
        let wg = units.iter().find(|u| u["name"] == "Wraithguard");
        assert!(wg.is_some());
        assert_eq!(wg.unwrap()["total_lists"], 2);
    }

    #[tokio::test]
    async fn test_unit_performance_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/unit-performance").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["units"].as_array().unwrap().is_empty());
    }

    // ── Points Efficiency Tests ─────────────────────────────────

    #[tokio::test]
    async fn test_points_efficiency_basic() {
        use crate::models::{ArmyList, Unit};

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");

        // Need >=3 appearances per unit for the filter
        let mut lists = Vec::new();
        let mut placements_vec = Vec::new();
        for (i, name) in ["Alice", "Bob", "Charlie"].iter().enumerate() {
            let list = ArmyList::new(
                "Aeldari".to_string(),
                2000,
                vec![Unit::new("Wraithguard".to_string(), 5).with_points(180)],
                format!("raw{}", i),
            )
            .with_player_name(name.to_string())
            .with_event_id(e1.id.clone());

            let mut p = make_placement(&e1, (i + 1) as u32, name, "Aeldari");
            p.list_id = Some(list.id.clone());
            p = p.with_record(4, 1, 0);
            placements_vec.push(p);
            lists.push(list);
        }

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(
            &epoch_dir.join("placements.jsonl"),
            &placements_vec.iter().collect::<Vec<_>>(),
        );
        write_jsonl(
            &epoch_dir.join("army_lists.jsonl"),
            &lists.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        let (status, json) =
            get_json(app, "/api/analytics/points-efficiency?faction=Aeldari").await;

        assert_eq!(status, StatusCode::OK);
        let units = json["units"].as_array().unwrap();
        assert!(!units.is_empty());
        assert_eq!(units[0]["unit_name"], "Wraithguard");
        assert_eq!(units[0]["avg_points"], 180);
        assert_eq!(units[0]["appearances"], 3);
    }

    #[tokio::test]
    async fn test_points_efficiency_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/points-efficiency").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["units"].as_array().unwrap().is_empty());
    }

    // ── Matchups Tests ──────────────────────────────────────────

    #[tokio::test]
    async fn test_matchups_basic() {
        use crate::models::Pairing;

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");

        // Create pairings: Aeldari vs Necrons, 3 wins for Aeldari, 2 for Necrons
        let mut pairings = Vec::new();
        for i in 0..3u32 {
            let mut p = Pairing::new(
                e1.id.clone(),
                "current".into(),
                i + 1,
                format!("Aeldari{}", i),
                format!("Necron{}", i),
            );
            p.player1_faction = Some("Aeldari".to_string());
            p.player2_faction = Some("Necrons".to_string());
            p.player1_result = Some("win".to_string());
            pairings.push(p);
        }
        for i in 0..2u32 {
            let mut p = Pairing::new(
                e1.id.clone(),
                "current".into(),
                i + 4,
                format!("NecronWin{}", i),
                format!("AeldariLoss{}", i),
            );
            p.player1_faction = Some("Necrons".to_string());
            p.player2_faction = Some("Aeldari".to_string());
            p.player1_result = Some("win".to_string());
            pairings.push(p);
        }

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(
            &epoch_dir.join("pairings.jsonl"),
            &pairings.iter().collect::<Vec<_>>(),
        );

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/matchups?min_games=1").await;

        assert_eq!(status, StatusCode::OK);
        let matchups = json["matchups"].as_array().unwrap();
        assert!(!matchups.is_empty());
        assert_eq!(json["factions"].as_array().unwrap().len(), 2);
        // Total games should be 5
        assert_eq!(matchups[0]["total_games"], 5);
    }

    #[tokio::test]
    async fn test_matchups_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<Event>(&epoch_dir.join("events.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/matchups").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["matchups"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_matchups_skips_mirror() {
        use crate::models::Pairing;

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");

        let mut p = Pairing::new(
            e1.id.clone(),
            "current".into(),
            1,
            "Alice".to_string(),
            "Bob".to_string(),
        );
        p.player1_faction = Some("Aeldari".to_string());
        p.player2_faction = Some("Aeldari".to_string());
        p.player1_result = Some("win".to_string());

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("pairings.jsonl"), &[&p]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/matchups?min_games=0").await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["matchups"].as_array().unwrap().is_empty());
    }

    // ── Archetypes Tests ────────────────────────────────────────

    #[tokio::test]
    async fn test_archetypes_basic() {
        use crate::models::{ArmyList, Unit};

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");

        // Two similar lists (same units) → should cluster together
        let list1 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![
                Unit::new("Wraithguard".to_string(), 5).with_points(180),
                Unit::new("Wave Serpent".to_string(), 1).with_points(120),
                Unit::new("Farseer".to_string(), 1).with_points(90),
            ],
            "raw".to_string(),
        )
        .with_detachment("Seer Council".to_string())
        .with_player_name("Alice".to_string())
        .with_event_id(e1.id.clone());

        let list2 = ArmyList::new(
            "Aeldari".to_string(),
            1990,
            vec![
                Unit::new("Wraithguard".to_string(), 5).with_points(180),
                Unit::new("Wave Serpent".to_string(), 2).with_points(240),
                Unit::new("Farseer".to_string(), 1).with_points(90),
            ],
            "raw".to_string(),
        )
        .with_detachment("Seer Council".to_string())
        .with_player_name("Bob".to_string())
        .with_event_id(e1.id.clone());

        let mut p1 = make_placement(&e1, 2, "Alice", "Aeldari");
        p1.list_id = Some(list1.id.clone());
        p1 = p1.with_record(4, 1, 0);
        let mut p2 = make_placement(&e1, 5, "Bob", "Aeldari");
        p2.list_id = Some(list2.id.clone());
        p2 = p2.with_record(3, 2, 0);

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list1, &list2]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/archetypes?faction=Aeldari").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["faction"], "Aeldari");
        assert_eq!(json["total_lists"], 2);

        let archetypes = json["archetypes"].as_array().unwrap();
        assert!(!archetypes.is_empty());

        // Should have a cluster with 2 lists
        let a = &archetypes[0];
        assert_eq!(a["list_count"], 2);
        assert_eq!(a["detachment"], "Seer Council");

        // sample_lists should be populated
        let sample_lists = a["sample_lists"].as_array().unwrap();
        assert_eq!(sample_lists.len(), 2);
        // Sorted by rank: Alice (#2) first, then Bob (#5)
        assert_eq!(sample_lists[0]["player_name"], "Alice");
        assert_eq!(sample_lists[0]["rank"], 2);
        assert_eq!(sample_lists[1]["player_name"], "Bob");
        assert_eq!(sample_lists[1]["rank"], 5);

        // Each entry should have units
        let units = sample_lists[0]["units"].as_array().unwrap();
        assert!(!units.is_empty());
        let names: Vec<&str> = units.iter().map(|u| u["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"Wraithguard"));
        assert!(names.contains(&"Wave Serpent"));
    }

    #[tokio::test]
    async fn test_archetypes_empty_faction() {
        use crate::models::ArmyList;

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/archetypes?faction=Tyranids").await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["total_lists"], 0);
        assert!(json["archetypes"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_archetypes_no_cluster_when_dissimilar() {
        use crate::models::{ArmyList, Unit};

        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2026-01-15", "https://example.com/a");

        // Two completely different lists → no cluster (jaccard < 0.5)
        let list1 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![
                Unit::new("Wraithguard".to_string(), 5),
                Unit::new("Wave Serpent".to_string(), 1),
            ],
            "raw".to_string(),
        )
        .with_detachment("Seer Council".to_string())
        .with_player_name("Alice".to_string())
        .with_event_id(e1.id.clone());

        let list2 = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![
                Unit::new("Fire Prism".to_string(), 1),
                Unit::new("Night Spinner".to_string(), 1),
            ],
            "raw".to_string(),
        )
        .with_detachment("Seer Council".to_string())
        .with_player_name("Bob".to_string())
        .with_event_id(e1.id.clone());

        let mut p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        p1.list_id = Some(list1.id.clone());
        let mut p2 = make_placement(&e1, 2, "Bob", "Aeldari");
        p2.list_id = Some(list2.id.clone());

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list1, &list2]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/analytics/archetypes?faction=Aeldari").await;

        assert_eq!(status, StatusCode::OK);
        // No clusters because lists are completely different (0% jaccard)
        assert!(json["archetypes"].as_array().unwrap().is_empty());
    }
}
