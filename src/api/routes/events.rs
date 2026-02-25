use std::collections::HashMap;
use std::sync::LazyLock;

use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::state::AppState;
use crate::api::{dedup_by_id, resolve_epoch, ApiError, Pagination, PaginationMeta};
use crate::models::{ArmyList, Event, Placement};
use crate::storage::{EntityType, JsonlReader};

// ── Faction Taxonomy ─────────────────────────────────────────────

/// Information about a canonical faction.
#[derive(Debug, Clone)]
pub struct FactionInfo {
    pub canonical_name: &'static str,
    pub allegiance: &'static str,
    pub allegiance_sub: &'static str,
}

/// Result of resolving a raw faction string.
#[derive(Debug, Clone)]
pub struct ResolvedFaction {
    pub faction: String,
    pub subfaction: Option<String>,
    pub allegiance: String,
    pub allegiance_sub: String,
}

static FACTION_MAP: LazyLock<HashMap<&'static str, FactionInfo>> = LazyLock::new(|| {
    let entries: Vec<(&str, FactionInfo)> = vec![
        // Space Marines chapters (distinct factions with codex supplements)
        (
            "space marines",
            FactionInfo {
                canonical_name: "Space Marines",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "blood angels",
            FactionInfo {
                canonical_name: "Blood Angels",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "dark angels",
            FactionInfo {
                canonical_name: "Dark Angels",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "space wolves",
            FactionInfo {
                canonical_name: "Space Wolves",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "black templars",
            FactionInfo {
                canonical_name: "Black Templars",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "deathwatch",
            FactionInfo {
                canonical_name: "Deathwatch",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "grey knights",
            FactionInfo {
                canonical_name: "Grey Knights",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        // Other chapters → each is its own faction
        (
            "adeptus astartes",
            FactionInfo {
                canonical_name: "Space Marines",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "ultramarines",
            FactionInfo {
                canonical_name: "Ultramarines",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "iron hands",
            FactionInfo {
                canonical_name: "Iron Hands",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "raven guard",
            FactionInfo {
                canonical_name: "Raven Guard",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "salamanders",
            FactionInfo {
                canonical_name: "Salamanders",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "imperial fists",
            FactionInfo {
                canonical_name: "Imperial Fists",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "white scars",
            FactionInfo {
                canonical_name: "White Scars",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "crimson fists",
            FactionInfo {
                canonical_name: "Crimson Fists",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "black dragons",
            FactionInfo {
                canonical_name: "Black Dragons",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        (
            "flesh tearers",
            FactionInfo {
                canonical_name: "Flesh Tearers",
                allegiance: "Imperium",
                allegiance_sub: "Space Marines",
            },
        ),
        // Armies of the Imperium
        (
            "adepta sororitas",
            FactionInfo {
                canonical_name: "Adepta Sororitas",
                allegiance: "Imperium",
                allegiance_sub: "Armies of the Imperium",
            },
        ),
        (
            "sisters of battle",
            FactionInfo {
                canonical_name: "Adepta Sororitas",
                allegiance: "Imperium",
                allegiance_sub: "Armies of the Imperium",
            },
        ),
        (
            "adeptus custodes",
            FactionInfo {
                canonical_name: "Adeptus Custodes",
                allegiance: "Imperium",
                allegiance_sub: "Armies of the Imperium",
            },
        ),
        (
            "adeptus mechanicus",
            FactionInfo {
                canonical_name: "Adeptus Mechanicus",
                allegiance: "Imperium",
                allegiance_sub: "Armies of the Imperium",
            },
        ),
        (
            "astra militarum",
            FactionInfo {
                canonical_name: "Astra Militarum",
                allegiance: "Imperium",
                allegiance_sub: "Armies of the Imperium",
            },
        ),
        (
            "imperial guard",
            FactionInfo {
                canonical_name: "Astra Militarum",
                allegiance: "Imperium",
                allegiance_sub: "Armies of the Imperium",
            },
        ),
        (
            "imperial knights",
            FactionInfo {
                canonical_name: "Imperial Knights",
                allegiance: "Imperium",
                allegiance_sub: "Armies of the Imperium",
            },
        ),
        (
            "agents of the imperium",
            FactionInfo {
                canonical_name: "Agents of the Imperium",
                allegiance: "Imperium",
                allegiance_sub: "Armies of the Imperium",
            },
        ),
        // Forces of Chaos
        (
            "chaos space marines",
            FactionInfo {
                canonical_name: "Chaos Space Marines",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        (
            "death guard",
            FactionInfo {
                canonical_name: "Death Guard",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        (
            "thousand sons",
            FactionInfo {
                canonical_name: "Thousand Sons",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        (
            "chaos thousand sons",
            FactionInfo {
                canonical_name: "Thousand Sons",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        (
            "world eaters",
            FactionInfo {
                canonical_name: "World Eaters",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        (
            "emperor's children",
            FactionInfo {
                canonical_name: "Emperor's Children",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        (
            "chaos daemons",
            FactionInfo {
                canonical_name: "Chaos Daemons",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        (
            "daemons of chaos",
            FactionInfo {
                canonical_name: "Chaos Daemons",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        (
            "chaos knights",
            FactionInfo {
                canonical_name: "Chaos Knights",
                allegiance: "Chaos",
                allegiance_sub: "Forces of Chaos",
            },
        ),
        // Xenos
        (
            "aeldari",
            FactionInfo {
                canonical_name: "Aeldari",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "craftworlds",
            FactionInfo {
                canonical_name: "Aeldari",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "craftworld",
            FactionInfo {
                canonical_name: "Aeldari",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "harlequins",
            FactionInfo {
                canonical_name: "Aeldari",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "drukhari",
            FactionInfo {
                canonical_name: "Drukhari",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "dark eldar",
            FactionInfo {
                canonical_name: "Drukhari",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "tyranids",
            FactionInfo {
                canonical_name: "Tyranids",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "genestealer cults",
            FactionInfo {
                canonical_name: "Genestealer Cults",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "genestealer cult",
            FactionInfo {
                canonical_name: "Genestealer Cults",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "leagues of votann",
            FactionInfo {
                canonical_name: "Leagues of Votann",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "votann",
            FactionInfo {
                canonical_name: "Leagues of Votann",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "necrons",
            FactionInfo {
                canonical_name: "Necrons",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "orks",
            FactionInfo {
                canonical_name: "Orks",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "t'au empire",
            FactionInfo {
                canonical_name: "T'au Empire",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "t'au",
            FactionInfo {
                canonical_name: "T'au Empire",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "tau",
            FactionInfo {
                canonical_name: "T'au Empire",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
        (
            "tau empire",
            FactionInfo {
                canonical_name: "T'au Empire",
                allegiance: "Xenos",
                allegiance_sub: "Xenos",
            },
        ),
    ];
    entries.into_iter().collect()
});

/// Chapters that should be promoted from subfaction to faction.
/// When faction is "Space Marines" and subfaction matches one of these,
/// the subfaction becomes the faction.
const CHAPTER_FACTIONS: &[&str] = &[
    "Blood Angels",
    "Dark Angels",
    "Space Wolves",
    "Black Templars",
    "Deathwatch",
    "Grey Knights",
    "Ultramarines",
    "Iron Hands",
    "Raven Guard",
    "Salamanders",
    "Imperial Fists",
    "White Scars",
    "Crimson Fists",
    "Black Dragons",
    "Flesh Tearers",
];

/// Look up faction info from the taxonomy map.
pub fn lookup_faction(name: &str) -> Option<&'static FactionInfo> {
    FACTION_MAP.get(name.trim().to_lowercase().as_str())
}

/// Get the allegiance for a faction name. Returns None if not found.
pub fn faction_allegiance(name: &str) -> Option<&'static str> {
    lookup_faction(name).map(|info| info.allegiance)
}

/// Resolve a raw faction + subfaction into canonical faction, subfaction, and allegiance.
///
/// Handles cases like:
/// - `faction: "Space Marines", subfaction: "Blood Angels"` → `faction: "Blood Angels", subfaction: None`
/// - `faction: "Ultramarines"` → `faction: "Space Marines", subfaction: "Ultramarines"`
/// - `faction: "Adeptus Astartes"` → `faction: "Space Marines"`
/// - `faction: "Blood Angels"` → `faction: "Blood Angels"`
pub fn resolve_faction(faction: &str, subfaction: Option<&str>) -> ResolvedFaction {
    let trimmed = faction.trim();
    let lower = trimmed.to_lowercase();

    // Step 1: If subfaction is a chapter-level faction, promote it
    if let Some(sub) = subfaction {
        let sub_lower = sub.trim().to_lowercase();
        // Check if subfaction is a codex-supplement chapter
        if CHAPTER_FACTIONS
            .iter()
            .any(|c| c.to_lowercase() == sub_lower)
        {
            if let Some(info) = FACTION_MAP.get(sub_lower.as_str()) {
                return ResolvedFaction {
                    faction: info.canonical_name.to_string(),
                    subfaction: None,
                    allegiance: info.allegiance.to_string(),
                    allegiance_sub: info.allegiance_sub.to_string(),
                };
            }
        }
    }

    // Step 2: Look up the faction itself
    if let Some(info) = FACTION_MAP.get(lower.as_str()) {
        return ResolvedFaction {
            faction: info.canonical_name.to_string(),
            subfaction: subfaction.map(|s| s.to_string()),
            allegiance: info.allegiance.to_string(),
            allegiance_sub: info.allegiance_sub.to_string(),
        };
    }

    // Step 3: Unknown faction — return as-is with no allegiance
    ResolvedFaction {
        faction: trimmed.to_string(),
        subfaction: subfaction.map(|s| s.to_string()),
        allegiance: "Unknown".to_string(),
        allegiance_sub: "Unknown".to_string(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ListEventsParams {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub epoch: Option<String>,
    pub has_results: Option<bool>,
    pub q: Option<String>,
    pub min_players: Option<u32>,
    pub max_players: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct WinnerSummary {
    pub player_name: String,
    pub faction: String,
    pub detachment: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EventSummary {
    pub id: String,
    pub name: String,
    pub date: String,
    pub location: Option<String>,
    pub player_count: Option<u32>,
    pub round_count: Option<u32>,
    pub source_url: String,
    pub winner: Option<WinnerSummary>,
    pub has_lists: bool,
    pub completed: bool,
}

#[derive(Debug, Serialize)]
pub struct EventListResponse {
    pub events: Vec<EventSummary>,
    pub pagination: PaginationMeta,
}

pub async fn list_events(
    State(state): State<AppState>,
    Query(params): Query<ListEventsParams>,
) -> Result<Json<EventListResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;

    // Support epoch=all to load events from every epoch (used by calendar)
    let epoch_ids: Vec<String> = if params.epoch.as_deref() == Some("all") {
        let epochs = mapper.all_epochs();
        if epochs.is_empty() {
            vec!["current".to_string()]
        } else {
            epochs.iter().map(|e| e.id.as_str().to_string()).collect()
        }
    } else {
        vec![resolve_epoch(params.epoch.as_deref(), &mapper)?]
    };

    let mut events: Vec<Event> = Vec::new();
    let mut placements: Vec<Placement> = Vec::new();
    let mut lists: Vec<ArmyList> = Vec::new();

    for epoch_id in &epoch_ids {
        let reader = JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, epoch_id);
        if let Ok(mut epoch_events) = reader.read_all() {
            events.append(&mut epoch_events);
        }

        let p_reader =
            JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, epoch_id);
        if let Ok(mut epoch_placements) = p_reader.read_all() {
            placements.append(&mut epoch_placements);
        }

        let l_reader =
            JsonlReader::<ArmyList>::for_entity(&state.storage, EntityType::ArmyList, epoch_id);
        if let Ok(mut epoch_lists) = l_reader.read_all() {
            lists.append(&mut epoch_lists);
        }
    }

    events = dedup_by_id(events, |e| e.id.as_str());
    let placements = dedup_by_id(placements, |p| p.id.as_str());

    // Filter by date range
    if let Some(ref from) = params.from {
        if let Ok(from_date) = from.parse::<chrono::NaiveDate>() {
            events.retain(|e| e.date >= from_date);
        }
    }
    if let Some(ref to) = params.to {
        if let Ok(to_date) = to.parse::<chrono::NaiveDate>() {
            events.retain(|e| e.date <= to_date);
        }
    }

    // Filter by search query (name or location, case-insensitive)
    if let Some(ref query) = params.q {
        let q_lower = query.to_lowercase();
        events.retain(|e| {
            e.name.to_lowercase().contains(&q_lower)
                || e.location
                    .as_ref()
                    .is_some_and(|l| l.to_lowercase().contains(&q_lower))
        });
    }

    // Filter by player count
    if let Some(min) = params.min_players {
        events.retain(|e| e.player_count.unwrap_or(0) >= min);
    }
    if let Some(max) = params.max_players {
        events.retain(|e| e.player_count.unwrap_or(0) <= max);
    }

    // Sort by date descending
    events.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.name.cmp(&b.name)));

    let today = chrono::Utc::now().date_naive();
    let event_ids_with_placements: std::collections::HashSet<&str> =
        placements.iter().map(|p| p.event_id.as_str()).collect();

    // Filter to only events that have at least one placement (results)
    // Also exclude future events — they can't have legitimate results
    if params.has_results.unwrap_or(false) {
        events.retain(|e| event_ids_with_placements.contains(e.id.as_str()));
    }
    let list_player_names: std::collections::HashSet<String> = lists
        .iter()
        .filter_map(|l| l.player_name.as_ref())
        .map(|n| {
            n.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .collect();
    let events_with_lists: std::collections::HashSet<&str> = placements
        .iter()
        .filter(|p| {
            let normalized = p
                .player_name
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            list_player_names.contains(&normalized)
        })
        .map(|p| p.event_id.as_str())
        .collect();

    let pagination = Pagination::new(params.page, params.page_size);
    let total_items = events.len() as u32;
    let meta = PaginationMeta::new(&pagination, total_items);

    let start = pagination.offset() as usize;
    let end = (start + pagination.page_size as usize).min(events.len());
    let page_events = if start < events.len() {
        &events[start..end]
    } else {
        &[]
    };

    let summaries: Vec<EventSummary> = page_events
        .iter()
        .map(|event| {
            let winner = placements
                .iter()
                .find(|p| p.event_id == event.id && p.rank == 1)
                .map(|p| WinnerSummary {
                    player_name: p.player_name.clone(),
                    faction: p.faction.clone(),
                    detachment: p.detachment.clone(),
                });

            let has_placements = event_ids_with_placements.contains(event.id.as_str());
            // "completed" = has placement data.
            // "expected" = we expect this event to have results (enough players, old enough).
            // Events with <10 players or within last 3 days are not expected to have data.
            let too_small = event.player_count.unwrap_or(0) < 10;
            let too_recent = event.date >= today - chrono::Days::new(3);
            let completed = if has_placements {
                true
            } else if event.date > today {
                false // future
            } else if too_small || too_recent {
                true // don't show as missing — too small/recent
            } else {
                false
            };

            EventSummary {
                id: event.id.as_str().to_string(),
                name: event.name.clone(),
                date: event.date.to_string(),
                location: event.location.clone(),
                player_count: event.player_count,
                round_count: event.round_count,
                source_url: event.source_url.clone(),
                winner,
                has_lists: events_with_lists.contains(event.id.as_str()),
                completed,
            }
        })
        .collect();

    Ok(Json(EventListResponse {
        events: summaries,
        pagination: meta,
    }))
}

#[derive(Debug, Serialize)]
pub struct PlacementDetail {
    pub rank: u32,
    pub player_name: String,
    pub faction: String,
    pub subfaction: Option<String>,
    pub detachment: Option<String>,
    pub record: Option<RecordDetail>,
    pub army_list: Option<ArmyListDetail>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecordDetail {
    pub wins: u32,
    pub losses: u32,
    pub draws: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnitDetail {
    pub name: String,
    pub count: u32,
    pub points: Option<u32>,
    pub wargear: Vec<String>,
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArmyListDetail {
    pub id: String,
    pub raw_text: String,
    pub parsed_faction: Option<String>,
    pub parsed_detachment: Option<String>,
    pub total_points: u32,
    pub units: Vec<UnitDetail>,
}

#[derive(Debug, Serialize)]
pub struct UnmatchedEventList {
    pub player_name: Option<String>,
    pub faction: Option<String>,
    pub detachment: Option<String>,
    pub list: ArmyListDetail,
}

#[derive(Debug, Serialize)]
pub struct EventDetailResponse {
    pub id: String,
    pub name: String,
    pub date: String,
    pub location: Option<String>,
    pub player_count: Option<u32>,
    pub round_count: Option<u32>,
    pub source_url: String,
    pub placements: Vec<PlacementDetail>,
    pub unmatched_lists: Vec<UnmatchedEventList>,
}

/// Parse the actual faction name from an army list's raw_text.
pub fn parse_faction_from_raw(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let line = line.trim().trim_start_matches('+').trim();
        let upper = line.to_uppercase();
        if upper.starts_with("FACTION KEYWORD:") || upper.starts_with("FACTION:") {
            // e.g. "FACTION KEYWORD: Imperium – Astra Militarum"
            let val = line.split_once(':')?.1.trim();
            // Take the part after the last dash/em-dash
            let faction = val
                .rsplit_once('–')
                .or_else(|| val.rsplit_once('-'))
                .map(|(_, f)| f.trim())
                .unwrap_or(val);
            return Some(faction.to_string());
        }
    }
    // Fallback: look for known faction names in first ~10 non-empty, non-header lines
    let factions = [
        "Astra Militarum",
        "Space Marines",
        "Necrons",
        "T'au Empire",
        "Aeldari",
        "Drukhari",
        "Blood Angels",
        "Dark Angels",
        "Death Guard",
        "Thousand Sons",
        "Chaos Space Marines",
        "Chaos Daemons",
        "Adeptus Custodes",
        "Adepta Sororitas",
        "Grey Knights",
        "Orks",
        "Tyranids",
        "Genestealer Cults",
        "Imperial Knights",
        "Chaos Knights",
        "Adeptus Mechanicus",
        "World Eaters",
        "Leagues of Votann",
        "Emperor's Children",
        "Agents of the Imperium",
        "Black Templars",
        "Space Wolves",
        "Ultramarines",
        "Raven Guard",
    ];
    for line in raw.lines().take(15) {
        let line = line.trim();
        for f in &factions {
            if line.contains(f) {
                return Some(f.to_string());
            }
        }
    }
    None
}

/// Game sizes that must NOT be treated as detachment names.
const GAME_SIZES: &[&str] = &[
    "strike force",
    "incursion",
    "onslaught",
    "combat patrol",
    "battalion",
    "patrol",
    "brigade",
    "vanguard",
    "spearhead",
    "outrider",
];

/// Check if a line is a game-size label (not a detachment).
fn is_game_size_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    GAME_SIZES.iter().any(|gs| lower.starts_with(gs))
}

/// Parse detachment from raw_text.
///
/// Handles multiple formats:
/// 1. Header style: `DETACHMENT: Grizzled Company (Ruthless Discipline)`
/// 2. Free-standing: detachment on its own line near the top, between faction
///    and game-size line, or right after the game-size line.
pub fn parse_detachment_from_raw(raw: &str) -> Option<String> {
    // Pass 1: Look for explicit DETACHMENT: header
    for line in raw.lines() {
        let line = line.trim().trim_start_matches('+').trim();
        let upper = line.to_uppercase();
        if upper.starts_with("DETACHMENT:") || upper.starts_with("DETACHMENT RULE:") {
            let val = line.split_once(':')?.1.trim();
            // Strip parenthetical suffixes like "(Ruthless Discipline)"
            let det = val.split('(').next().unwrap_or(val).trim();
            return Some(det.to_string());
        }
    }

    // Pass 2: Free-standing format — scan the first ~15 non-empty lines
    // for a line that is NOT a faction, NOT a game size, NOT a section header,
    // and appears near the faction/game-size cluster at the top.
    let non_empty: Vec<&str> = raw
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('+') && !l.starts_with('#'))
        .take(15)
        .collect();

    // Find the game-size line index
    let gs_idx = non_empty.iter().position(|l| is_game_size_line(l));

    if let Some(gi) = gs_idx {
        // Check the line immediately after the game-size line
        if gi + 1 < non_empty.len() {
            let candidate = non_empty[gi + 1];
            if !is_section_header(candidate)
                && !is_game_size_line(candidate)
                && lookup_faction(candidate).is_none()
                && !candidate.contains("points")
                && !candidate.contains("Points")
            {
                return Some(candidate.to_string());
            }
        }
        // Check the line immediately before the game-size line
        if gi > 0 {
            let candidate = non_empty[gi - 1];
            if !is_section_header(candidate)
                && !is_game_size_line(candidate)
                && lookup_faction(candidate).is_none()
                && !candidate.contains("points")
                && !candidate.contains("Points")
            {
                return Some(candidate.to_string());
            }
        }
    }

    None
}

/// Check if a line is a section header like "CHARACTERS", "BATTLELINE", etc.
fn is_section_header(line: &str) -> bool {
    let headers = [
        "CHARACTERS",
        "BATTLELINE",
        "OTHER DATASHEETS",
        "DEDICATED TRANSPORTS",
        "ALLIED UNITS",
        "FORTIFICATIONS",
    ];
    let upper = line.to_uppercase();
    headers.iter().any(|h| upper == *h)
}

/// Convert a model Unit to an API UnitDetail.
pub fn unit_to_detail(u: &crate::models::Unit) -> UnitDetail {
    UnitDetail {
        name: u.name.clone(),
        count: u.count,
        points: u.points,
        wargear: u.wargear.clone(),
        keywords: u.keywords.clone(),
    }
}

/// Normalize faction names to canonical forms.
/// Handles common variants and abbreviations found in tournament data.
/// Uses the FACTION_MAP taxonomy for consistent resolution.
pub fn normalize_faction_name(name: &str) -> String {
    let trimmed = name.trim();
    if let Some(info) = lookup_faction(trimmed) {
        info.canonical_name.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Score how well two faction names match.
/// Returns: 3 = exact match, 2 = one contains the other, 0 = no match.
/// Applies faction name normalization before comparing.
pub fn faction_match_score(a: &str, b: &str) -> u32 {
    let na = normalize_faction_name(a);
    let nb = normalize_faction_name(b);
    if na.eq_ignore_ascii_case(&nb) {
        return 3;
    }
    let la = na.to_lowercase();
    let lb = nb.to_lowercase();
    // Only allow contains-match if one name is a true prefix/suffix of the other
    // and they don't belong to different faction families (e.g. "Space Marines" vs "Chaos Space Marines")
    if (la.contains(&lb) || lb.contains(&la)) && !is_conflicting_contains(&la, &lb) {
        return 2;
    }
    0
}

/// Check if a contains-match between two faction names is a false positive.
/// E.g. "chaos space marines" contains "space marines" but they are different factions.
fn is_conflicting_contains(a: &str, b: &str) -> bool {
    let pairs = [
        ("space marines", "chaos space marines"),
        ("knights", "chaos knights"),
        ("knights", "imperial knights"),
        ("chaos knights", "imperial knights"),
    ];
    for (x, y) in &pairs {
        if (a == *x && b == *y) || (a == *y && b == *x) {
            return true;
        }
    }
    false
}

/// Check if two player names match (case-insensitive, whitespace-normalized).
fn player_names_match(a: &str, b: &str) -> bool {
    let normalize = |s: &str| {
        s.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    normalize(a) == normalize(b)
}

/// Build an ArmyListDetail from an ArmyList, using structured fields with
/// raw-text parsing as fallback.
pub fn army_list_to_detail(l: &ArmyList) -> ArmyListDetail {
    let faction = if !l.faction.is_empty() && !l.faction.contains("presents") {
        Some(l.faction.clone())
    } else {
        parse_faction_from_raw(&l.raw_text)
    };
    let detachment = if l.detachment.is_some() {
        l.detachment.clone()
    } else {
        parse_detachment_from_raw(&l.raw_text)
    };
    ArmyListDetail {
        id: l.id.as_str().to_string(),
        raw_text: l.raw_text.clone(),
        parsed_faction: faction,
        parsed_detachment: detachment,
        total_points: l.total_points,
        units: l.units.iter().map(unit_to_detail).collect(),
    }
}

/// Match army lists to placements using two passes:
///
/// 1. **Player name** (across ALL lists) — definitive match.
/// 2. **Faction + detachment** (same source URL only) — weaker signal,
///    restricted to lists from the same article so we don't cross-contaminate
///    across tournaments.
///
/// Any lists that remain unmatched are NOT returned — they will appear on
/// faction pages instead of the tournament page.
fn match_lists_to_placements(
    placements: &mut [PlacementDetail],
    lists: Vec<ArmyList>,
    event_source_url: &str,
    event_id: &str,
) -> Vec<UnmatchedEventList> {
    let mut candidates: Vec<(ArmyList, ArmyListDetail)> = lists
        .into_iter()
        .map(|l| {
            let detail = army_list_to_detail(&l);
            (l, detail)
        })
        .collect();

    // Pass 1: Match by player name, preferring same-event lists
    for placement in placements.iter_mut() {
        // First try: same event_id + player name (definitive)
        let matched = candidates
            .iter()
            .position(|(l, _)| {
                l.event_id
                    .as_ref()
                    .is_some_and(|eid| eid.as_str() == event_id)
                    && l.player_name
                        .as_ref()
                        .is_some_and(|name| player_names_match(&placement.player_name, name))
            })
            .or_else(|| {
                // Fallback: any list with matching player name
                candidates.iter().position(|(l, _)| {
                    l.player_name
                        .as_ref()
                        .is_some_and(|name| player_names_match(&placement.player_name, name))
                })
            });

        if let Some(idx) = matched {
            let (_, detail) = candidates.remove(idx);
            placement.army_list = Some(detail);
        }
    }

    // Pass 2: Match by faction+detachment (same source URL only)
    // Only considers lists from the same article to avoid cross-event matches.
    for placement in placements.iter_mut() {
        if placement.army_list.is_some() {
            continue;
        }
        let det = match &placement.detachment {
            Some(d) if !d.is_empty() => d.as_str(),
            _ => continue,
        };
        let matched = candidates.iter().position(|(l, detail)| {
            l.source_url.as_deref() == Some(event_source_url)
                && detail
                    .parsed_detachment
                    .as_deref()
                    .is_some_and(|d| d.eq_ignore_ascii_case(det))
                && detail
                    .parsed_faction
                    .as_ref()
                    .is_some_and(|f| faction_match_score(f, &placement.faction) > 0)
        });
        if let Some(idx) = matched {
            let (_, detail) = candidates.remove(idx);
            placement.army_list = Some(detail);
        }
    }

    // Remaining candidates either belong to other events or don't match
    // any placement — they'll appear on faction pages instead.
    Vec::new()
}

#[derive(Debug, Deserialize)]
pub struct GetEventParams {
    pub epoch: Option<String>,
}

pub async fn get_event(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(params): Query<GetEventParams>,
) -> Result<Json<EventDetailResponse>, ApiError> {
    let mapper = state.epoch_mapper.read().await;
    let epoch = resolve_epoch(params.epoch.as_deref(), &mapper)?;
    let reader = JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, &epoch);
    let events = reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let events = dedup_by_id(events, |e| e.id.as_str());

    let event = events
        .into_iter()
        .find(|e| e.id.as_str() == id)
        .ok_or_else(|| ApiError::NotFound(format!("Event not found: {}", id)))?;

    // Read placements for this event
    let placement_reader =
        JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, &epoch);
    let placements = placement_reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let placements = dedup_by_id(placements, |p| p.id.as_str());

    let mut event_placements: Vec<PlacementDetail> = placements
        .into_iter()
        .filter(|p| p.event_id == event.id)
        .map(|p| PlacementDetail {
            rank: p.rank,
            player_name: p.player_name,
            faction: p.faction,
            subfaction: p.subfaction,
            detachment: p.detachment,
            record: p.record.map(|r| RecordDetail {
                wins: r.wins,
                losses: r.losses,
                draws: r.draws,
            }),
            army_list: None,
        })
        .collect();
    event_placements.sort_by_key(|p| p.rank);

    // Read army lists and match to placements
    let list_reader =
        JsonlReader::<ArmyList>::for_entity(&state.storage, EntityType::ArmyList, &epoch);
    let lists = list_reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let lists = dedup_by_id(lists, |l| l.id.as_str());

    let unmatched_lists = match_lists_to_placements(
        &mut event_placements,
        lists,
        &event.source_url,
        event.id.as_str(),
    );

    Ok(Json(EventDetailResponse {
        id: event.id.as_str().to_string(),
        name: event.name,
        date: event.date.to_string(),
        location: event.location,
        player_count: event.player_count,
        round_count: event.round_count,
        source_url: event.source_url,
        placements: event_placements,
        unmatched_lists,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::build_router;
    use crate::api::state::AppState;
    use crate::models::{ArmyList, EpochMapper, Unit};
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

    // ── Endpoint Tests ──────────────────────────────────────────

    #[tokio::test]
    async fn test_list_events_has_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2025-01-15", "https://example.com/a");
        let e2 = make_event("GT Beta", "2025-01-22", "https://example.com/b");

        // e1 has a placement whose player name matches the list
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");

        let list = ArmyList::new(
            "Aeldari".to_string(),
            2000,
            vec![Unit::new("Wraithguard".to_string(), 5)],
            "raw".to_string(),
        )
        .with_source_url("https://example.com/a".to_string())
        .with_player_name("Alice".to_string());

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        // e2 is first (newer date, sorted desc)
        assert_eq!(events[0]["has_lists"], false);
        assert_eq!(events[1]["has_lists"], true);
    }

    #[tokio::test]
    async fn test_list_events_completed() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        // Past event with placements = completed
        let e1 = make_event("GT Alpha", "2025-01-15", "https://example.com/a");
        // Future event = not completed
        let e2 = make_event("GT Future", "2099-12-31", "https://example.com/b");

        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1]);
        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        // Future event first (sorted by date desc)
        assert_eq!(events[0]["completed"], false);
        assert_eq!(events[1]["completed"], true);
    }

    #[tokio::test]
    async fn test_list_events_date_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Jan", "2025-01-15", "https://example.com/a");
        let e2 = make_event("GT Mar", "2025-03-15", "https://example.com/b");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);
        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events?from=2025-02-01").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "GT Mar");
    }

    #[tokio::test]
    async fn test_list_events_has_results_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT With Results", "2025-01-15", "https://example.com/a");
        let e2 = make_event("GT No Results", "2025-01-22", "https://example.com/b");

        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1]);
        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events?has_results=true").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "GT With Results");
    }

    #[tokio::test]
    async fn test_list_events_winner() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT Alpha", "2025-01-15", "https://example.com/a");
        let p1 = make_placement(&e1, 1, "Alice", "Aeldari");
        let p2 = make_placement(&e1, 2, "Bob", "Necrons");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2]);
        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        assert_eq!(events[0]["winner"]["player_name"], "Alice");
        assert_eq!(events[0]["winner"]["faction"], "Aeldari");
    }

    #[tokio::test]
    async fn test_list_events_pagination() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("GT A", "2025-01-01", "https://example.com/a");
        let e2 = make_event("GT B", "2025-01-02", "https://example.com/b");
        let e3 = make_event("GT C", "2025-01-03", "https://example.com/c");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2, &e3]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);
        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events?page=1&page_size=2").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(json["pagination"]["total_items"], 3);
        assert_eq!(json["pagination"]["total_pages"], 2);
        assert_eq!(json["pagination"]["has_next"], true);
    }

    // ── Helper Function Tests ──────────────────────────────────

    #[test]
    fn test_parse_faction_from_raw_keyword_line() {
        let raw = "++ Army ++\nFACTION KEYWORD: Imperium – Astra Militarum\n++ HQ ++";
        assert_eq!(
            parse_faction_from_raw(raw),
            Some("Astra Militarum".to_string())
        );
    }

    #[test]
    fn test_parse_faction_from_raw_faction_line() {
        let raw = "Faction: Necrons\nDetachment: Awakened Dynasty";
        assert_eq!(parse_faction_from_raw(raw), Some("Necrons".to_string()));
    }

    #[test]
    fn test_parse_faction_from_raw_fallback() {
        let raw = "++ Army Roster ++\nSome list with Aeldari units\n++ HQ ++";
        assert_eq!(parse_faction_from_raw(raw), Some("Aeldari".to_string()));
    }

    #[test]
    fn test_parse_faction_from_raw_none() {
        let raw = "++ Random stuff ++\nNo factions here\n++ End ++";
        assert_eq!(parse_faction_from_raw(raw), None);
    }

    #[test]
    fn test_parse_detachment_from_raw() {
        let raw = "Detachment: Ironstorm Spearhead\nSome units";
        assert_eq!(
            parse_detachment_from_raw(raw),
            Some("Ironstorm Spearhead".to_string())
        );
    }

    #[test]
    fn test_parse_detachment_from_raw_header_with_parenthetical() {
        let raw = "+++++\n+ FACTION KEYWORD: Imperium – Astra Militarum\n+ DETACHMENT: Grizzled Company (Ruthless Discipline)\n+ TOTAL ARMY POINTS: 1990pts";
        assert_eq!(
            parse_detachment_from_raw(raw),
            Some("Grizzled Company".to_string())
        );
    }

    #[test]
    fn test_parse_detachment_freestanding_before_strike_force() {
        // Format: Faction\nDetachment\nStrike Force (points)
        let raw = "My Cool Army (1990 Points)\n\nNecrons\n\nAwakened Dynasty\n\nStrike Force (2,000 Points)\n\nCHARACTERS\n\nC'tan Shard";
        assert_eq!(
            parse_detachment_from_raw(raw),
            Some("Awakened Dynasty".to_string())
        );
    }

    #[test]
    fn test_parse_detachment_freestanding_after_strike_force() {
        // Format: Faction\nStrike Force (points)\nDetachment
        let raw = "My Army (2000 points)\n\nAstra Militarum\n\nStrike Force (2000 points)\n\nGrizzled Company\n\nCHARACTERS";
        assert_eq!(
            parse_detachment_from_raw(raw),
            Some("Grizzled Company".to_string())
        );
    }

    #[test]
    fn test_parse_detachment_with_subfaction_line() {
        // Format: Faction\nSubfaction\nStrike Force (points)\nDetachment
        let raw = "Don't worry about me (2000 points)\n\nSpace Marines\n\nUltramarines\n\nStrike Force (2000 points)\n\nBlade of Ultramar\n\nCHARACTERS";
        assert_eq!(
            parse_detachment_from_raw(raw),
            Some("Blade of Ultramar".to_string())
        );
    }

    #[test]
    fn test_parse_detachment_never_returns_strike_force() {
        // Must NEVER return "Strike Force" as a detachment
        let raw = "Army Name (2000 points)\n\nNecrons\n\nStrike Force (2000 points)\n\nCHARACTERS";
        let det = parse_detachment_from_raw(raw);
        assert!(
            det.as_ref()
                .is_none_or(|d| !d.to_lowercase().starts_with("strike force")),
            "parse_detachment_from_raw must never return Strike Force, got: {:?}",
            det
        );
    }

    #[test]
    fn test_parse_detachment_from_raw_none() {
        let raw = "++ Army Roster ++\nNo detachment line";
        assert_eq!(parse_detachment_from_raw(raw), None);
    }

    #[test]
    fn test_unit_to_detail() {
        let unit = Unit::new("Leman Russ".to_string(), 2)
            .with_points(160)
            .with_keywords(vec!["Vehicle".to_string()]);
        let detail = unit_to_detail(&unit);
        assert_eq!(detail.name, "Leman Russ");
        assert_eq!(detail.count, 2);
        assert_eq!(detail.points, Some(160));
        assert_eq!(detail.keywords, vec!["Vehicle"]);
    }

    #[test]
    fn test_is_conflicting_contains() {
        // Function expects lowercase input (called internally with lowercased strings)
        assert!(is_conflicting_contains(
            "space marines",
            "chaos space marines"
        ));
        assert!(is_conflicting_contains(
            "chaos space marines",
            "space marines"
        ));
        assert!(!is_conflicting_contains("necrons", "aeldari"));
    }

    #[test]
    fn test_normalize_faction_name() {
        assert_eq!(
            normalize_faction_name("Genestealer Cult"),
            "Genestealer Cults"
        );
        assert_eq!(
            normalize_faction_name("Genestealer Cults"),
            "Genestealer Cults"
        );
        assert_eq!(normalize_faction_name("Adeptus Astartes"), "Space Marines");
        assert_eq!(normalize_faction_name("Space Marines"), "Space Marines");
        assert_eq!(
            normalize_faction_name("Chaos Space Marines"),
            "Chaos Space Marines"
        );
        assert_eq!(normalize_faction_name("T'au Empire"), "T'au Empire");
        assert_eq!(normalize_faction_name("tau empire"), "T'au Empire");
        assert_eq!(normalize_faction_name("Blood Angels"), "Blood Angels");
    }

    #[test]
    fn test_faction_allegiance() {
        assert_eq!(faction_allegiance("Space Marines"), Some("Imperium"));
        assert_eq!(faction_allegiance("Blood Angels"), Some("Imperium"));
        assert_eq!(faction_allegiance("Chaos Space Marines"), Some("Chaos"));
        assert_eq!(faction_allegiance("Death Guard"), Some("Chaos"));
        assert_eq!(faction_allegiance("Aeldari"), Some("Xenos"));
        assert_eq!(faction_allegiance("Necrons"), Some("Xenos"));
        assert_eq!(faction_allegiance("Unknown Faction"), None);
    }

    #[test]
    fn test_resolve_faction_chapter_promotion() {
        // subfaction "Blood Angels" should be promoted to faction
        let resolved = resolve_faction("Space Marines", Some("Blood Angels"));
        assert_eq!(resolved.faction, "Blood Angels");
        assert!(resolved.subfaction.is_none());
        assert_eq!(resolved.allegiance, "Imperium");
    }

    #[test]
    fn test_resolve_faction_generic_chapter() {
        // "Ultramarines" should be its own faction
        let resolved = resolve_faction("Ultramarines", None);
        assert_eq!(resolved.faction, "Ultramarines");
        assert_eq!(resolved.subfaction, None);
        assert_eq!(resolved.allegiance, "Imperium");
    }

    #[test]
    fn test_resolve_faction_already_correct() {
        let resolved = resolve_faction("Blood Angels", None);
        assert_eq!(resolved.faction, "Blood Angels");
        assert!(resolved.subfaction.is_none());
        assert_eq!(resolved.allegiance, "Imperium");
        assert_eq!(resolved.allegiance_sub, "Space Marines");
    }

    #[test]
    fn test_resolve_faction_old_name() {
        let resolved = resolve_faction("Adeptus Astartes", None);
        assert_eq!(resolved.faction, "Space Marines");
        assert_eq!(resolved.allegiance, "Imperium");
    }

    #[test]
    fn test_faction_match_score_exact() {
        assert_eq!(faction_match_score("Space Marines", "Space Marines"), 3);
        assert_eq!(faction_match_score("space marines", "Space Marines"), 3);
        // Adeptus Astartes normalizes to Space Marines
        assert_eq!(faction_match_score("Adeptus Astartes", "Space Marines"), 3);
        assert_eq!(
            faction_match_score("Genestealer Cult", "Genestealer Cults"),
            3
        );
    }

    #[test]
    fn test_faction_match_score_no_cross_contamination() {
        // Chaos Space Marines must NOT match Space Marines
        assert_eq!(
            faction_match_score("Chaos Space Marines", "Space Marines"),
            0
        );
        assert_eq!(
            faction_match_score("Space Marines", "Chaos Space Marines"),
            0
        );
        // Chaos Knights must NOT match Imperial Knights
        assert_eq!(faction_match_score("Chaos Knights", "Imperial Knights"), 0);
    }

    #[test]
    fn test_faction_match_score_no_match() {
        assert_eq!(faction_match_score("Necrons", "Tyranids"), 0);
        assert_eq!(faction_match_score("Orks", "Aeldari"), 0);
    }

    #[test]
    fn test_player_names_match() {
        assert!(player_names_match("John Smith", "John Smith"));
        assert!(player_names_match("john smith", "John Smith"));
        assert!(player_names_match("John  Smith", "John Smith"));
        assert!(!player_names_match("John Smith", "Jane Smith"));
    }

    #[tokio::test]
    async fn test_no_cross_event_list_contamination() {
        // Two events share the same source_url (like a Goonhammer article)
        // Each has a list for their winner but NOT for the other's players.
        // The fix ensures lists only match by player name, not faction fallback.
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let shared_url = "https://example.com/article";
        let e1 = make_event("GT Alpha", "2025-01-15", shared_url);
        let e2 = make_event("GT Beta", "2025-01-22", shared_url);

        // GT Alpha: Alice wins with Dark Angels
        let p1 = make_placement(&e1, 1, "Alice", "Dark Angels");
        // GT Alpha: Bob plays Dark Angels too — but has no list
        let p2 = make_placement(&e1, 2, "Bob", "Dark Angels");

        // GT Beta: Charlie wins with Dark Angels
        let p3 = make_placement(&e2, 1, "Charlie", "Dark Angels");

        // Lists: Alice and Charlie each have a list at the shared URL
        let list_alice = ArmyList::new(
            "Dark Angels".to_string(),
            2000,
            vec![Unit::new("Deathwing Knights".to_string(), 5)],
            "raw".to_string(),
        )
        .with_source_url(shared_url.to_string())
        .with_player_name("Alice".to_string());

        let list_charlie = ArmyList::new(
            "Dark Angels".to_string(),
            2000,
            vec![Unit::new("Ravenwing Knights".to_string(), 3)],
            "raw".to_string(),
        )
        .with_source_url(shared_url.to_string())
        .with_player_name("Charlie".to_string());

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2, &p3]);
        write_jsonl(
            &epoch_dir.join("army_lists.jsonl"),
            &[&list_alice, &list_charlie],
        );

        let app = build_router(state);

        // Check GT Alpha: Alice should have her list, Bob should NOT get Charlie's
        let (status, json) = get_json(
            app,
            &format!("/api/events/{}?epoch=current", e1.id.as_str()),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let placements = json["placements"].as_array().unwrap();
        assert_eq!(placements.len(), 2);

        // Alice (rank 1) should have her list
        assert!(
            placements[0]["army_list"].is_object(),
            "Alice should have her own list"
        );
        // Bob (rank 2) should NOT have Charlie's list via faction fallback
        assert!(
            placements[1]["army_list"].is_null(),
            "Bob must NOT get Charlie's list via faction fallback"
        );

        // Charlie's list should NOT appear on GT Alpha — it belongs to GT Beta
        // and will show up on faction pages instead.
        let unmatched = json["unmatched_lists"].as_array().unwrap();
        assert_eq!(
            unmatched.len(),
            0,
            "No unmatched lists — Charlie's list belongs to GT Beta"
        );
    }

    // ── Detachment Consistency Integration Tests ─────────────────

    /// Helper: cross-check placement detachments against army list raw_text.
    /// Returns a list of mismatch descriptions. Empty = all consistent.
    fn check_detachment_consistency(
        placements: &[crate::models::Placement],
        lists: &[ArmyList],
    ) -> Vec<String> {
        let name_to_list: std::collections::HashMap<String, &ArmyList> = lists
            .iter()
            .filter_map(|l| {
                l.player_name.as_ref().map(|n| {
                    (
                        n.split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .to_lowercase(),
                        l,
                    )
                })
            })
            .collect();

        let mut issues: Vec<String> = Vec::new();

        for p in placements {
            let norm_name = p
                .player_name
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            let list = match name_to_list.get(&norm_name) {
                Some(l) => l,
                None => continue,
            };

            // Check: list structured detachment should not be a game size
            if let Some(ref det) = list.detachment {
                let lower = det.to_lowercase();
                if lower.starts_with("strike force")
                    || lower.starts_with("incursion")
                    || lower.starts_with("combat patrol")
                {
                    issues.push(format!(
                        "{}: list.detachment is game size '{}'",
                        p.player_name, det
                    ));
                }
            }

            // Check: placement detachment matches what raw_text says
            let placement_det = match &p.detachment {
                Some(d) if !d.is_empty() => d,
                _ => continue,
            };
            let raw_det = match parse_detachment_from_raw(&list.raw_text) {
                Some(d) => d,
                None => continue,
            };

            if !placement_det.eq_ignore_ascii_case(&raw_det) {
                issues.push(format!(
                    "{}: placement='{}' vs list_raw='{}'",
                    p.player_name, placement_det, raw_det
                ));
            }
        }

        issues
    }

    /// Consistent data should produce zero issues.
    #[tokio::test]
    async fn test_detachment_consistency_passes_when_correct() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let event = make_event("GT Test", "2025-01-15", "https://example.com/a");

        // Placement detachment matches the raw_text detachment
        let mut p1 = make_placement(&event, 1, "Justin Moore", "Ultramarines");
        p1.detachment = Some("Blade of Ultramar".to_string());

        let list = ArmyList::new(
            "Space Marines".to_string(),
            2000,
            vec![Unit::new("Captain Sicarius".to_string(), 1)],
            "Army (2000 points)\n\nSpace Marines\n\nUltramarines\n\nStrike Force (2000 points)\n\nBlade of Ultramar\n\nCHARACTERS".to_string(),
        )
        .with_source_url("https://example.com/a".to_string())
        .with_player_name("Justin Moore".to_string());

        let mut p2 = make_placement(&event, 2, "Sean Murray", "Necrons");
        p2.detachment = Some("Awakened Dynasty".to_string());

        let list2 = ArmyList::new(
            "Necrons".to_string(),
            1990,
            vec![Unit::new("C'tan Shard".to_string(), 1)],
            "Army (1990 Points)\n\nNecrons\n\nAwakened Dynasty\n\nStrike Force (2,000 Points)\n\nCHARACTERS".to_string(),
        )
        .with_source_url("https://example.com/a".to_string())
        .with_player_name("Sean Murray".to_string());

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&event]);
        write_jsonl(&epoch_dir.join("placements.jsonl"), &[&p1, &p2]);
        write_jsonl(&epoch_dir.join("army_lists.jsonl"), &[&list, &list2]);

        let lists_loaded: Vec<ArmyList> = crate::storage::JsonlReader::for_entity(
            &state.storage,
            crate::storage::EntityType::ArmyList,
            "current",
        )
        .read_all()
        .unwrap();
        let placements_loaded: Vec<crate::models::Placement> =
            crate::storage::JsonlReader::for_entity(
                &state.storage,
                crate::storage::EntityType::Placement,
                "current",
            )
            .read_all()
            .unwrap();

        let issues = check_detachment_consistency(&placements_loaded, &lists_loaded);
        assert!(
            issues.is_empty(),
            "Expected no issues but found:\n{}",
            issues.join("\n")
        );
    }

    /// Detects when placement detachment doesn't match list raw_text.
    #[test]
    fn test_detachment_consistency_catches_mismatch() {
        let event = make_event("GT Test", "2025-01-15", "https://example.com/a");

        let mut p1 = make_placement(&event, 1, "Justin Moore", "Ultramarines");
        p1.detachment = Some("Gladius Task Force".to_string());

        let list = ArmyList::new(
            "Space Marines".to_string(),
            2000,
            vec![Unit::new("Captain Sicarius".to_string(), 1)],
            "Army (2000 points)\n\nSpace Marines\n\nUltramarines\n\nStrike Force (2000 points)\n\nBlade of Ultramar\n\nCHARACTERS".to_string(),
        )
        .with_source_url("https://example.com/a".to_string())
        .with_player_name("Justin Moore".to_string());

        let issues = check_detachment_consistency(&[p1], &[list]);
        assert_eq!(issues.len(), 1, "Should detect exactly one mismatch");
        assert!(
            issues[0].contains("Gladius Task Force") && issues[0].contains("Blade of Ultramar"),
            "Mismatch should name both detachments: {}",
            issues[0]
        );
    }

    /// Detects when a list's structured detachment field is a game size.
    #[test]
    fn test_detachment_consistency_catches_game_size() {
        let event = make_event("GT Test", "2025-01-15", "https://example.com/a");

        let mut p1 = make_placement(&event, 1, "Sean Murray", "Necrons");
        p1.detachment = Some("Awakened Dynasty".to_string());

        let mut list = ArmyList::new(
            "Necrons".to_string(),
            1990,
            vec![Unit::new("C'tan Shard".to_string(), 1)],
            "Army\n\nNecrons\n\nAwakened Dynasty\n\nStrike Force (2,000 Points)\n\nCHARACTERS"
                .to_string(),
        );
        list.detachment = Some("Strike Force".to_string());
        let list = list
            .with_source_url("https://example.com/a".to_string())
            .with_player_name("Sean Murray".to_string());

        let issues = check_detachment_consistency(&[p1], &[list]);
        assert!(
            issues.iter().any(|i| i.contains("game size")),
            "Should detect Strike Force as game size, got: {:?}",
            issues
        );
    }

    #[tokio::test]
    async fn test_list_events_min_players_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 =
            make_event("Small GT", "2025-01-15", "https://example.com/a").with_player_count(10);
        let e2 = make_event("Big GT", "2025-01-22", "https://example.com/b").with_player_count(50);
        let e3 = make_event("No Count GT", "2025-01-20", "https://example.com/c");

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2, &e3]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);
        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events?min_players=20").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "Big GT");
    }

    #[tokio::test]
    async fn test_list_events_max_players_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 =
            make_event("Small GT", "2025-01-15", "https://example.com/a").with_player_count(10);
        let e2 = make_event("Big GT", "2025-01-22", "https://example.com/b").with_player_count(50);

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);
        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events?max_players=30").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "Small GT");
    }

    #[tokio::test]
    async fn test_list_events_min_max_players_range() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_test_state(tmp.path());
        let epoch_dir = tmp.path().join("normalized").join("current");

        let e1 = make_event("Tiny", "2025-01-10", "https://example.com/a").with_player_count(5);
        let e2 = make_event("Medium", "2025-01-15", "https://example.com/b").with_player_count(30);
        let e3 = make_event("Large", "2025-01-20", "https://example.com/c").with_player_count(100);

        write_jsonl(&epoch_dir.join("events.jsonl"), &[&e1, &e2, &e3]);
        write_jsonl::<Placement>(&epoch_dir.join("placements.jsonl"), &[]);
        write_jsonl::<ArmyList>(&epoch_dir.join("army_lists.jsonl"), &[]);

        let app = build_router(state);
        let (status, json) = get_json(app, "/api/events?min_players=10&max_players=50").await;

        assert_eq!(status, StatusCode::OK);
        let events = json["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["name"], "Medium");
    }
}
