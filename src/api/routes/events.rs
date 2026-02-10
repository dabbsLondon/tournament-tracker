use axum::extract::{Path, Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::api::state::AppState;
use crate::api::{dedup_by_id, resolve_epoch, ApiError, Pagination, PaginationMeta};
use crate::models::{ArmyList, Event, Placement};
use crate::storage::{EntityType, JsonlReader};

#[derive(Debug, Deserialize)]
pub struct ListEventsParams {
    pub page: Option<u32>,
    pub page_size: Option<u32>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub epoch: Option<String>,
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
    let epoch = resolve_epoch(params.epoch.as_deref(), &state.epoch_mapper)?;
    let reader = JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, &epoch);
    let mut events = reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    events = dedup_by_id(events, |e| e.id.as_str());

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

    // Sort by date descending
    events.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| a.name.cmp(&b.name)));

    // Read placements to find winners
    let placement_reader =
        JsonlReader::<Placement>::for_entity(&state.storage, EntityType::Placement, &epoch);
    let placements = placement_reader
        .read_all()
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let placements = dedup_by_id(placements, |p| p.id.as_str());

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

            EventSummary {
                id: event.id.as_str().to_string(),
                name: event.name.clone(),
                date: event.date.to_string(),
                location: event.location.clone(),
                player_count: event.player_count,
                round_count: event.round_count,
                source_url: event.source_url.clone(),
                winner,
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
pub struct EventDetailResponse {
    pub id: String,
    pub name: String,
    pub date: String,
    pub location: Option<String>,
    pub player_count: Option<u32>,
    pub round_count: Option<u32>,
    pub source_url: String,
    pub placements: Vec<PlacementDetail>,
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
        "Astra Militarum", "Space Marines", "Necrons", "T'au Empire",
        "Aeldari", "Drukhari", "Blood Angels", "Dark Angels",
        "Death Guard", "Thousand Sons", "Chaos Space Marines",
        "Chaos Daemons", "Adeptus Custodes", "Adepta Sororitas",
        "Grey Knights", "Orks", "Tyranids", "Genestealer Cults",
        "Imperial Knights", "Chaos Knights", "Adeptus Mechanicus",
        "World Eaters", "Leagues of Votann", "Emperor's Children",
        "Agents of the Imperium", "Black Templars", "Space Wolves",
        "Ultramarines", "Raven Guard",
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

/// Parse detachment from raw_text.
pub fn parse_detachment_from_raw(raw: &str) -> Option<String> {
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
    None
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
pub fn normalize_faction_name(name: &str) -> String {
    let trimmed = name.trim();
    let lower = trimmed.to_lowercase();
    match lower.as_str() {
        "genestealer cult" => "Genestealer Cults".to_string(),
        "genestealer cults" => "Genestealer Cults".to_string(),
        "adeptus astartes" => "Space Marines".to_string(),
        "astra militarum" | "imperial guard" => "Astra Militarum".to_string(),
        "t'au" | "tau" | "tau empire" => "T'au Empire".to_string(),
        "craftworlds" | "craftworld" => "Aeldari".to_string(),
        "harlequins" => "Aeldari".to_string(),
        "black templars" => "Black Templars".to_string(),
        "blood angels" => "Blood Angels".to_string(),
        "dark angels" => "Dark Angels".to_string(),
        "space wolves" => "Space Wolves".to_string(),
        "deathwatch" => "Deathwatch".to_string(),
        _ => trimmed.to_string(),
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

/// Match army lists to placements by player name, falling back to faction+detachment
/// for legacy lists that don't have player_name set.
///
/// Lists are filtered to the same event (via source_url), so date matching
/// is implicit.
fn match_lists_to_placements(
    placements: &mut [PlacementDetail],
    lists: Vec<ArmyList>,
    event_source_url: &str,
) {
    let mut candidates: Vec<(ArmyList, ArmyListDetail)> = lists
        .into_iter()
        .filter(|l| l.source_url.as_deref() == Some(event_source_url))
        .map(|l| {
            let detail = army_list_to_detail(&l);
            (l, detail)
        })
        .collect();

    for placement in placements.iter_mut() {
        // First: try exact player name match
        let matched = candidates.iter().position(|(l, _)| {
            l.player_name
                .as_ref()
                .is_some_and(|name| player_names_match(&placement.player_name, name))
        });

        // Fallback: faction + detachment match for legacy lists without player_name
        let matched = matched.or_else(|| {
            let mut best_idx: Option<usize> = None;
            let mut best_score: u32 = 0;

            for (i, (_, detail)) in candidates.iter().enumerate() {
                let mut score: u32 = 0;
                let list_faction = detail.parsed_faction.as_deref().unwrap_or("");
                if !list_faction.is_empty() {
                    score += faction_match_score(&placement.faction, list_faction);
                }
                if let (Some(ref pd), Some(ref ld)) =
                    (&placement.detachment, &detail.parsed_detachment)
                {
                    if pd.eq_ignore_ascii_case(ld) {
                        score += 5;
                    }
                }
                if score > best_score {
                    best_score = score;
                    best_idx = Some(i);
                }
            }

            // Require faction + detachment match (8 = faction 3 + detachment 5)
            // A bare faction match alone is too loose — it would match any list
            // of the same faction regardless of detachment/player
            if best_score >= 8 {
                best_idx
            } else {
                None
            }
        });

        if let Some(idx) = matched {
            let (_, detail) = candidates.remove(idx);
            placement.army_list = Some(detail);
        }
    }
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
    let epoch = resolve_epoch(params.epoch.as_deref(), &state.epoch_mapper)?;
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

    match_lists_to_placements(&mut event_placements, lists, &event.source_url);

    Ok(Json(EventDetailResponse {
        id: event.id.as_str().to_string(),
        name: event.name,
        date: event.date.to_string(),
        location: event.location,
        player_count: event.player_count,
        round_count: event.round_count,
        source_url: event.source_url,
        placements: event_placements,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_faction_name() {
        assert_eq!(normalize_faction_name("Genestealer Cult"), "Genestealer Cults");
        assert_eq!(normalize_faction_name("Genestealer Cults"), "Genestealer Cults");
        assert_eq!(normalize_faction_name("Adeptus Astartes"), "Space Marines");
        assert_eq!(normalize_faction_name("Space Marines"), "Space Marines");
        assert_eq!(normalize_faction_name("Chaos Space Marines"), "Chaos Space Marines");
        assert_eq!(normalize_faction_name("T'au Empire"), "T'au Empire");
        assert_eq!(normalize_faction_name("tau empire"), "T'au Empire");
        assert_eq!(normalize_faction_name("Blood Angels"), "Blood Angels");
    }

    #[test]
    fn test_faction_match_score_exact() {
        assert_eq!(faction_match_score("Space Marines", "Space Marines"), 3);
        assert_eq!(faction_match_score("space marines", "Space Marines"), 3);
        // Adeptus Astartes normalizes to Space Marines
        assert_eq!(faction_match_score("Adeptus Astartes", "Space Marines"), 3);
        assert_eq!(faction_match_score("Genestealer Cult", "Genestealer Cults"), 3);
    }

    #[test]
    fn test_faction_match_score_no_cross_contamination() {
        // Chaos Space Marines must NOT match Space Marines
        assert_eq!(faction_match_score("Chaos Space Marines", "Space Marines"), 0);
        assert_eq!(faction_match_score("Space Marines", "Chaos Space Marines"), 0);
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
}
