//! Best Coast Pairings (BCP) API client.
//!
//! Fetches event data, standings, and army lists from the BCP v1 API.
//! All BCP API specifics are isolated in this module so endpoint changes
//! are easy to fix.

use std::collections::HashMap;

use chrono::NaiveDate;
use regex::Regex;
use serde::Deserialize;
use tracing::{info, warn};
use url::Url;

use crate::fetch::{FetchError, Fetcher};
use crate::models::Unit;

// ── Custom deserializers for nested BCP fields ──────────────────────────────

/// Deserialize detachment from nested `warhammer: {detachment: "..."}`.
fn deserialize_warhammer_detachment<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Warhammer {
        detachment: Option<String>,
    }
    let maybe: Option<Warhammer> = Option::deserialize(deserializer)?;
    Ok(maybe.and_then(|w| w.detachment))
}

/// Deserialize faction name from nested `army: {name: "..."}`.
fn deserialize_army_name<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    struct Army {
        name: Option<String>,
    }
    let maybe: Option<Army> = Option::deserialize(deserializer)?;
    Ok(maybe.and_then(|a| a.name))
}

// ── BCP Authentication ──────────────────────────────────────────────────────

/// The BCP OAuth API base (API URL with /v1 stripped).
const BCP_OAUTH_BASE: &str = "https://newprod-api.bestcoastpairings.com";

/// The redirect_uri registered with BCP's OAuth endpoint.
const BCP_REDIRECT_URI: &str = "https://www.bestcoastpairings.com/login";

/// Authenticate with BCP using email/password and return an access token.
///
/// Flow:
/// 1. `GET /oauth/authorize` with Basic auth → authorization code
/// 2. `POST /oauth/token` with code → access token + refresh token
///
/// Note: Cognito token refresh is intentionally skipped because BCP's API
/// no longer accepts raw Cognito access tokens — only tokens from their own
/// OAuth flow work.
pub async fn bcp_authenticate() -> Result<String, String> {
    let email = std::env::var("BCP_EMAIL").map_err(|_| "BCP_EMAIL not set".to_string())?;
    let password = std::env::var("BCP_PASSWORD").map_err(|_| "BCP_PASSWORD not set".to_string())?;

    login_with_credentials(&email, &password).await
}

/// Login to BCP with email/password and return an access token.
async fn login_with_credentials(email: &str, password: &str) -> Result<String, String> {
    let client = reqwest::Client::new();

    // Step 1: Get authorization code (using reqwest's built-in basic auth)
    let auth_resp = client
        .get(format!("{}/oauth/authorize", BCP_OAUTH_BASE))
        .query(&[
            ("response_type", "code"),
            ("redirect_uri", BCP_REDIRECT_URI),
        ])
        .header("client-id", "web-app")
        .basic_auth(email, Some(password))
        .send()
        .await
        .map_err(|e| format!("BCP auth request failed: {}", e))?;

    let auth_json: serde_json::Value = auth_resp
        .json()
        .await
        .map_err(|e| format!("BCP auth response parse failed: {}", e))?;

    let code = auth_json["authorizationCode"]
        .as_str()
        .ok_or_else(|| format!("BCP auth: no authorizationCode in response: {}", auth_json))?
        .to_string();

    info!("BCP: got authorization code");

    // Step 2: Exchange code for tokens
    let token_resp = client
        .post(format!("{}/oauth/token", BCP_OAUTH_BASE))
        .header("client-id", "web-app")
        .json(&serde_json::json!({
            "redirect_uri": BCP_REDIRECT_URI,
            "code": code,
            "grant_type": "authorization_code"
        }))
        .send()
        .await
        .map_err(|e| format!("BCP token exchange failed: {}", e))?;

    let token_json: serde_json::Value = token_resp
        .json()
        .await
        .map_err(|e| format!("BCP token response parse failed: {}", e))?;

    let access_token = token_json["accessToken"]
        .as_str()
        .ok_or_else(|| format!("BCP token: no accessToken in response: {}", token_json))?
        .to_string();

    // Log refresh token availability for future use
    if let Some(refresh) = token_json["refreshToken"].as_str() {
        info!(
            "BCP: got refresh token ({}... chars), save as BCP_REFRESH_TOKEN for faster auth",
            &refresh[..20.min(refresh.len())]
        );
    }

    info!("BCP: login successful");
    Ok(access_token)
}

/// Build the extra headers map for BCP API requests.
///
/// Always includes `client-id: web-app`. If the `BCP_AUTH_TOKEN` environment
/// variable is set, also includes `Authorization: Bearer <token>` which
/// unlocks army list fetching (subscriber account required).
pub fn bcp_headers() -> HashMap<String, String> {
    let mut headers: HashMap<String, String> = [("client-id".to_string(), "web-app".to_string())]
        .into_iter()
        .collect();

    if let Ok(token) = std::env::var("BCP_AUTH_TOKEN") {
        if !token.is_empty() {
            info!("BCP: using auth token from BCP_AUTH_TOKEN");
            headers.insert("Authorization".to_string(), format!("Bearer {}", token));
        }
    }

    headers
}

/// Build headers with a fresh auth token (authenticates if needed).
///
/// This is the preferred way to get BCP headers for list fetching.
/// Tries: BCP_AUTH_TOKEN env → refresh token → full login.
pub async fn bcp_headers_authenticated() -> HashMap<String, String> {
    let mut headers: HashMap<String, String> = [("client-id".to_string(), "web-app".to_string())]
        .into_iter()
        .collect();

    // First check if there's a pre-set token
    if let Ok(token) = std::env::var("BCP_AUTH_TOKEN") {
        if !token.is_empty() {
            info!("BCP: using pre-set auth token");
            headers.insert("Authorization".to_string(), format!("Bearer {}", token));
            return headers;
        }
    }

    // Try automatic authentication
    match bcp_authenticate().await {
        Ok(token) => {
            info!("BCP: authenticated successfully");
            headers.insert("Authorization".to_string(), format!("Bearer {}", token));
        }
        Err(e) => {
            warn!("BCP: authentication failed: {}", e);
        }
    }

    headers
}

/// BCP API client.
pub struct BcpClient {
    fetcher: Fetcher,
    api_base: String,
    game_type: u32,
}

// ── BCP API response types ──────────────────────────────────────────────────

/// An event from the BCP events listing endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpEvent {
    /// BCP internal object ID
    #[serde(alias = "objectId", alias = "id")]
    pub id: String,

    /// Event name
    pub name: String,

    /// Start date (ISO 8601) — v1 uses `eventDate`
    #[serde(alias = "startDate", alias = "eventDate")]
    pub start_date: Option<String>,

    /// End date (ISO 8601) — v1 uses `eventEndDate`
    #[serde(alias = "endDate", alias = "eventEndDate")]
    pub end_date: Option<String>,

    /// Venue / location info
    #[serde(alias = "venue")]
    pub venue: Option<String>,

    /// City
    pub city: Option<String>,

    /// State / province
    pub state: Option<String>,

    /// Country
    pub country: Option<String>,

    /// Number of registered players
    #[serde(
        alias = "playerCount",
        alias = "numberOfPlayers",
        alias = "totalPlayers"
    )]
    pub player_count: Option<u32>,

    /// Number of rounds
    #[serde(alias = "numberOfRounds", alias = "roundCount")]
    pub round_count: Option<u32>,

    /// Game system ID (1 = Warhammer 40k)
    #[serde(alias = "gameType")]
    pub game_type: Option<u32>,

    /// Whether the event has ended
    pub ended: Option<bool>,

    /// Whether this is a team event
    #[serde(alias = "teamEvent")]
    pub team_event: Option<bool>,

    /// Whether placings are hidden
    #[serde(alias = "hidePlacings")]
    pub hide_placings: Option<bool>,
}

impl BcpEvent {
    /// Parse start_date string into NaiveDate.
    pub fn parsed_start_date(&self) -> Option<NaiveDate> {
        self.start_date.as_ref().and_then(|s| {
            // Handle both "2026-02-01" and "2026-02-01T00:00:00.000Z" formats
            let date_part = if s.len() >= 10 { &s[..10] } else { s };
            NaiveDate::parse_from_str(date_part, "%Y-%m-%d").ok()
        })
    }

    /// Build a human-readable location string.
    pub fn location_string(&self) -> Option<String> {
        let parts: Vec<&str> = [
            self.venue.as_deref(),
            self.city.as_deref(),
            self.state.as_deref(),
            self.country.as_deref(),
        ]
        .iter()
        .filter_map(|p| *p)
        .filter(|p| !p.is_empty())
        .collect();

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(", "))
        }
    }

    /// URL to the event page on BCP.
    pub fn event_url(&self) -> String {
        format!("https://www.bestcoastpairings.com/event/{}", self.id)
    }

    /// Whether this event should be skipped during sync.
    pub fn should_skip(&self) -> bool {
        self.team_event == Some(true) || self.hide_placings == Some(true)
    }
}

/// A player standing / placement from a BCP event.
///
/// This is the output format used by the rest of the pipeline.
/// For v1, standings are computed from pairings rather than fetched directly.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpStanding {
    /// Player's finishing position
    #[serde(alias = "placing", alias = "place")]
    pub placing: Option<u32>,

    /// Player name
    #[serde(alias = "playerName", alias = "name")]
    pub player_name: Option<String>,

    /// Faction / army name
    #[serde(alias = "armyName", alias = "faction")]
    pub faction: Option<String>,

    /// Wins
    pub wins: Option<u32>,

    /// Losses
    pub losses: Option<u32>,

    /// Draws
    pub draws: Option<u32>,

    /// Total battle points
    #[serde(alias = "totalBattlePoints")]
    pub total_battle_points: Option<u32>,

    /// Player BCP ID
    #[serde(alias = "playerId", alias = "userId")]
    pub player_id: Option<String>,

    /// Army list object ID (for fetching the full list)
    #[serde(alias = "armyListObjectId")]
    pub army_list_object_id: Option<String>,
}

/// An army list from BCP.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpArmyList {
    /// The raw list text — v1 uses `armyListText`
    #[serde(alias = "armyList", alias = "armyListText", alias = "list")]
    pub army_list: Option<String>,

    /// Faction
    #[serde(alias = "armyName", alias = "faction")]
    pub faction: Option<String>,

    /// Detachment info from the `warhammer` nested object
    #[serde(
        default,
        rename = "warhammer",
        deserialize_with = "deserialize_warhammer_detachment"
    )]
    pub detachment: Option<String>,

    /// Army/faction name from the `army` nested object
    #[serde(default, rename = "army", deserialize_with = "deserialize_army_name")]
    pub army_faction: Option<String>,
}

/// Wrapper for paginated BCP responses.
#[derive(Debug, Deserialize)]
pub struct BcpListResponse<T> {
    /// The array of results
    #[serde(alias = "data", alias = "results")]
    pub data: Vec<T>,
}

// ── v1 player / pairings types ──────────────────────────────────────────────

/// A user nested in a player record.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpUser {
    #[serde(alias = "firstName")]
    pub first_name: Option<String>,
    #[serde(alias = "lastName")]
    pub last_name: Option<String>,
}

/// A faction nested in a player record.
#[derive(Debug, Clone, Deserialize)]
pub struct BcpFaction {
    pub name: Option<String>,
}

/// A player record from the v1 `/events/{id}/players` endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpPlayerV1 {
    /// The player-event record ID
    #[serde(alias = "_id")]
    pub id: Option<String>,

    /// Nested user info
    pub user: Option<BcpUser>,

    /// Nested faction info
    pub faction: Option<BcpFaction>,

    /// Army list object ID
    #[serde(alias = "armyListObjectId", alias = "listId")]
    pub list_id: Option<String>,

    /// Whether the player dropped
    pub dropped: Option<bool>,

    /// Army/faction name directly on the player (some events)
    #[serde(alias = "armyName")]
    pub army_name: Option<String>,
}

impl BcpPlayerV1 {
    /// Get the player's full name.
    pub fn full_name(&self) -> String {
        match &self.user {
            Some(u) => {
                let first = u.first_name.as_deref().unwrap_or("");
                let last = u.last_name.as_deref().unwrap_or("");
                format!("{} {}", first, last).trim().to_string()
            }
            None => String::new(),
        }
    }

    /// Get the faction name from either nested faction or direct field.
    pub fn faction_name(&self) -> Option<String> {
        self.faction
            .as_ref()
            .and_then(|f| f.name.clone())
            .or_else(|| self.army_name.clone())
    }
}

/// Wrapper for v1 player list response: `{active: [...], deleted: [...]}`
#[derive(Debug, Deserialize)]
pub struct BcpPlayersResponse {
    #[serde(default)]
    pub active: Vec<BcpPlayerV1>,
    #[serde(default)]
    pub deleted: Vec<BcpPlayerV1>,
}

/// A player embedded in a pairing record (expanded).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpPairingPlayer {
    /// Player ID
    #[serde(alias = "_id")]
    pub id: Option<String>,

    #[serde(alias = "firstName")]
    pub first_name: Option<String>,

    #[serde(alias = "lastName")]
    pub last_name: Option<String>,

    #[serde(alias = "armyName")]
    pub army_name: Option<String>,

    #[serde(alias = "armyListObjectId")]
    pub army_list_object_id: Option<String>,
}

impl BcpPairingPlayer {
    pub fn full_name(&self) -> String {
        let first = self.first_name.as_deref().unwrap_or("");
        let last = self.last_name.as_deref().unwrap_or("");
        format!("{} {}", first, last).trim().to_string()
    }
}

/// Deserialize a value that may be a number or a string containing a number.
fn deserialize_string_or_number_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    Ok(val.and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_u64().map(|x| x as u32),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }))
}

fn deserialize_string_or_number_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    Ok(val.and_then(|v| match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse().ok(),
        _ => None,
    }))
}

/// Metadata from a pairing record.
#[derive(Debug, Clone, Deserialize)]
pub struct BcpPairingMeta {
    /// Player 1 game result: 2=win, 0=loss, 1=draw
    #[serde(
        alias = "p1-gameResult",
        rename = "p1-gameResult",
        deserialize_with = "deserialize_string_or_number_u32",
        default
    )]
    pub p1_game_result: Option<u32>,

    /// Player 1 game points
    #[serde(
        alias = "p1-gamePoints",
        rename = "p1-gamePoints",
        deserialize_with = "deserialize_string_or_number_f64",
        default
    )]
    pub p1_game_points: Option<f64>,

    /// Player 2 game result
    #[serde(
        alias = "p2-gameResult",
        rename = "p2-gameResult",
        deserialize_with = "deserialize_string_or_number_u32",
        default
    )]
    pub p2_game_result: Option<u32>,

    /// Player 2 game points
    #[serde(
        alias = "p2-gamePoints",
        rename = "p2-gamePoints",
        deserialize_with = "deserialize_string_or_number_f64",
        default
    )]
    pub p2_game_points: Option<f64>,
}

/// A pairing record from the v1 pairings endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BcpPairing {
    pub player1: Option<BcpPairingPlayer>,
    pub player2: Option<BcpPairingPlayer>,

    #[serde(alias = "metaData")]
    pub meta_data: Option<BcpPairingMeta>,

    pub round: Option<u32>,
}

/// Accumulated stats for a single player, used in standings computation.
#[derive(Debug, Clone, Default)]
struct PlayerStats {
    name: String,
    faction: Option<String>,
    player_id: Option<String>,
    army_list_object_id: Option<String>,
    wins: u32,
    losses: u32,
    draws: u32,
    battle_points: u32,
}

// ── BCP client implementation ───────────────────────────────────────────────

impl BcpClient {
    /// Create a new BCP client.
    pub fn new(fetcher: Fetcher, api_base: String, game_type: u32) -> Self {
        let api_base = api_base.trim_end_matches('/').to_string();
        Self {
            fetcher,
            api_base,
            game_type,
        }
    }

    /// Discover events in a date range.
    pub async fn discover_events(
        &self,
        date_from: NaiveDate,
        date_to: NaiveDate,
    ) -> Result<Vec<BcpEvent>, FetchError> {
        let url_str = format!(
            "{}/events?startDate={}&endDate={}&gameType={}&limit=100",
            self.api_base, date_from, date_to, self.game_type
        );
        let url = Url::parse(&url_str)
            .map_err(|e| FetchError::InvalidUrl(format!("Bad BCP events URL: {}", e)))?;

        info!("BCP: discovering events {} to {}", date_from, date_to);
        let fetch_result = self.fetcher.fetch(&url).await?;
        let json_text = self.fetcher.read_cached_text(&fetch_result).await?;

        // Try paginated response first, then plain array
        let events: Vec<BcpEvent> =
            if let Ok(response) = serde_json::from_str::<BcpListResponse<BcpEvent>>(&json_text) {
                response.data
            } else if let Ok(events) = serde_json::from_str::<Vec<BcpEvent>>(&json_text) {
                events
            } else {
                let paginated_err =
                    serde_json::from_str::<BcpListResponse<BcpEvent>>(&json_text).unwrap_err();
                let array_err = serde_json::from_str::<Vec<BcpEvent>>(&json_text).unwrap_err();
                warn!(
                    "BCP: could not parse events response. Paginated: {}. Array: {}. Preview: {}",
                    paginated_err,
                    array_err,
                    &json_text[..json_text.len().min(500)]
                );
                return Err(FetchError::InvalidUrl(
                    "Could not parse BCP events response".to_string(),
                ));
            };

        info!("BCP: found {} events", events.len());
        Ok(events)
    }

    /// Fetch players for an event from the v1 API.
    pub async fn fetch_players(&self, event_id: &str) -> Result<Vec<BcpPlayerV1>, FetchError> {
        let url_str = format!("{}/events/{}/players?limit=500", self.api_base, event_id);
        let url = Url::parse(&url_str)
            .map_err(|e| FetchError::InvalidUrl(format!("Bad BCP players URL: {}", e)))?;

        info!("BCP: fetching players for event {}", event_id);
        let fetch_result = self.fetcher.fetch(&url).await?;
        let json_text = self.fetcher.read_cached_text(&fetch_result).await?;

        // v1 returns {active: [...], deleted: [...]}
        if let Ok(response) = serde_json::from_str::<BcpPlayersResponse>(&json_text) {
            info!(
                "BCP: got {} active players for event {}",
                response.active.len(),
                event_id
            );
            return Ok(response.active);
        }

        // Fallback: try paginated or plain array
        if let Ok(response) = serde_json::from_str::<BcpListResponse<BcpPlayerV1>>(&json_text) {
            return Ok(response.data);
        }
        if let Ok(players) = serde_json::from_str::<Vec<BcpPlayerV1>>(&json_text) {
            return Ok(players);
        }

        warn!("BCP: could not parse players response for {}", event_id);
        Ok(Vec::new())
    }

    /// Fetch pairings for an event from the v1 API.
    pub async fn fetch_pairings(&self, event_id: &str) -> Result<Vec<BcpPairing>, FetchError> {
        let url_str = format!(
            "{}/pairings?eventId={}&pairingType=Pairing&expand[]=player1&expand[]=player2&limit=500",
            self.api_base, event_id
        );
        let url = Url::parse(&url_str)
            .map_err(|e| FetchError::InvalidUrl(format!("Bad BCP pairings URL: {}", e)))?;

        info!("BCP: fetching pairings for event {}", event_id);
        let fetch_result = self.fetcher.fetch(&url).await?;
        let json_text = self.fetcher.read_cached_text(&fetch_result).await?;

        // Try paginated response, then plain array
        let pairings: Vec<BcpPairing> =
            if let Ok(response) = serde_json::from_str::<BcpListResponse<BcpPairing>>(&json_text) {
                response.data
            } else if let Ok(pairings) = serde_json::from_str::<Vec<BcpPairing>>(&json_text) {
                pairings
            } else {
                warn!("BCP: could not parse pairings response for {}", event_id);
                Vec::new()
            };

        info!(
            "BCP: got {} pairings for event {}",
            pairings.len(),
            event_id
        );
        Ok(pairings)
    }

    /// Compute standings from pairings data.
    ///
    /// Aggregates W/L/D and battle points per player from pairings,
    /// then sorts by wins desc, battle points desc to assign placing.
    /// Also enriches with player/faction info from the players list.
    pub fn compute_standings(
        &self,
        pairings: &[BcpPairing],
        players: &[BcpPlayerV1],
    ) -> Vec<BcpStanding> {
        let mut stats: HashMap<String, PlayerStats> = HashMap::new();

        // Build player info lookup by ID
        let player_info: HashMap<String, &BcpPlayerV1> = players
            .iter()
            .filter_map(|p| p.id.as_ref().map(|id| (id.clone(), p)))
            .collect();

        for pairing in pairings {
            let meta = match &pairing.meta_data {
                Some(m) => m,
                None => continue,
            };

            // Process player 1
            if let Some(ref p1) = pairing.player1 {
                if let Some(ref p1_id) = p1.id {
                    let entry = stats.entry(p1_id.clone()).or_insert_with(|| {
                        let mut ps = PlayerStats {
                            name: p1.full_name(),
                            player_id: Some(p1_id.clone()),
                            army_list_object_id: p1.army_list_object_id.clone(),
                            ..Default::default()
                        };
                        // Get faction from player info or pairing
                        ps.faction = player_info
                            .get(p1_id)
                            .and_then(|pi| pi.faction_name())
                            .or_else(|| p1.army_name.clone());
                        ps
                    });

                    match meta.p1_game_result {
                        Some(2) => entry.wins += 1,
                        Some(0) => entry.losses += 1,
                        Some(1) => entry.draws += 1,
                        _ => {}
                    }
                    if let Some(pts) = meta.p1_game_points {
                        entry.battle_points += pts as u32;
                    }
                }
            }

            // Process player 2
            if let Some(ref p2) = pairing.player2 {
                if let Some(ref p2_id) = p2.id {
                    let entry = stats.entry(p2_id.clone()).or_insert_with(|| {
                        let mut ps = PlayerStats {
                            name: p2.full_name(),
                            player_id: Some(p2_id.clone()),
                            army_list_object_id: p2.army_list_object_id.clone(),
                            ..Default::default()
                        };
                        ps.faction = player_info
                            .get(p2_id)
                            .and_then(|pi| pi.faction_name())
                            .or_else(|| p2.army_name.clone());
                        ps
                    });

                    match meta.p2_game_result {
                        Some(2) => entry.wins += 1,
                        Some(0) => entry.losses += 1,
                        Some(1) => entry.draws += 1,
                        _ => {}
                    }
                    if let Some(pts) = meta.p2_game_points {
                        entry.battle_points += pts as u32;
                    }
                }
            }
        }

        // Sort by wins desc, then battle points desc
        let mut player_stats: Vec<PlayerStats> = stats.into_values().collect();
        player_stats.sort_by(|a, b| {
            b.wins
                .cmp(&a.wins)
                .then(b.battle_points.cmp(&a.battle_points))
        });

        // Convert to BcpStanding with placing
        player_stats
            .into_iter()
            .enumerate()
            .map(|(i, ps)| BcpStanding {
                placing: Some((i + 1) as u32),
                player_name: Some(ps.name),
                faction: ps.faction,
                wins: Some(ps.wins),
                losses: Some(ps.losses),
                draws: Some(ps.draws),
                total_battle_points: Some(ps.battle_points),
                player_id: ps.player_id,
                army_list_object_id: ps.army_list_object_id,
            })
            .collect()
    }

    /// Fetch standings for an event by fetching pairings and computing results.
    ///
    /// This is the main entry point for getting standings data from the v1 API.
    pub async fn fetch_standings(&self, event_id: &str) -> Result<Vec<BcpStanding>, FetchError> {
        let players = self.fetch_players(event_id).await?;
        let pairings = self.fetch_pairings(event_id).await?;

        if pairings.is_empty() {
            info!(
                "BCP: no pairings for event {}, returning empty standings",
                event_id
            );
            return Ok(Vec::new());
        }

        let standings = self.compute_standings(&pairings, &players);
        info!(
            "BCP: computed {} standings for event {}",
            standings.len(),
            event_id
        );
        Ok(standings)
    }

    /// Fetch a single army list from Listhammer.
    ///
    /// Listhammer mirrors BCP army list data publicly using the same
    /// event and player IDs, with no CAPTCHA or auth requirements.
    pub async fn fetch_army_list(
        &self,
        event_id: &str,
        player_id: &str,
    ) -> Result<Option<BcpArmyList>, FetchError> {
        self.fetch_army_list_from_listhammer(event_id, player_id)
            .await
    }

    /// Fetch an army list from Listhammer (listhammer.info).
    ///
    /// Listhammer mirrors BCP army list data publicly using the same
    /// event and player IDs. Returns plain text lists for top-performing
    /// players at events with 20+ players.
    async fn fetch_army_list_from_listhammer(
        &self,
        event_id: &str,
        player_id: &str,
    ) -> Result<Option<BcpArmyList>, FetchError> {
        let url_str = format!(
            "https://listhammer.info/api/eventList?eventId={}&playerId={}",
            event_id, player_id
        );
        let url = Url::parse(&url_str)
            .map_err(|e| FetchError::InvalidUrl(format!("Bad listhammer URL: {}", e)))?;

        match self.fetcher.fetch(&url).await {
            Ok(fetch_result) => {
                let json_text = self.fetcher.read_cached_text(&fetch_result).await?;

                // Listhammer returns { "list": "..." }
                #[derive(Deserialize)]
                struct ListhammerResponse {
                    list: Option<String>,
                }

                if let Ok(resp) = serde_json::from_str::<ListhammerResponse>(&json_text) {
                    if let Some(text) = resp.list {
                        if !text.trim().is_empty() {
                            return Ok(Some(BcpArmyList {
                                army_list: Some(text),
                                faction: None,
                                detachment: None,
                                army_faction: None,
                            }));
                        }
                    }
                }
                Ok(None)
            }
            Err(FetchError::HttpStatus { status: 404, .. }) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Detect a specific Space Marine chapter from army list raw text.
///
/// BCP often returns "Space Marines" or "Space Marines (Astartes)" as the faction,
/// but the raw text contains the actual chapter (Ultramarines, Salamanders, etc.).
/// Returns `Some("Ultramarines")` etc. if detected, `None` if truly generic.
pub fn detect_chapter_from_raw_text(raw_text: &str) -> Option<&'static str> {
    static CHAPTERS: &[&str] = &[
        "Ultramarines",
        "Iron Hands",
        "Salamanders",
        "Imperial Fists",
        "Raven Guard",
        "White Scars",
        "Crimson Fists",
        "Flesh Tearers",
    ];

    // Pattern 1: "Space Marines\nChapterName\n"
    let re_line = Regex::new(r"(?m)Space Marines\n(\w[\w\s]+)\n").unwrap();
    if let Some(caps) = re_line.captures(raw_text) {
        let candidate = caps[1].trim();
        for &ch in CHAPTERS {
            if candidate.eq_ignore_ascii_case(ch) {
                return Some(ch);
            }
        }
    }

    // Pattern 2: "Adeptus Astartes - ChapterName"
    let re_astartes =
        Regex::new(r"(?i)Adeptus Astartes\s*-\s*(\w[\w\s]+?)(?:\s*-|\s*\n|\s*\[)").unwrap();
    if let Some(caps) = re_astartes.captures(raw_text) {
        let candidate = caps[1].trim();
        for &ch in CHAPTERS {
            if candidate.eq_ignore_ascii_case(ch) {
                return Some(ch);
            }
        }
    }

    // Pattern 3: "Space Marines (ChapterName)"
    let re_parens = Regex::new(r"(?i)Space Marines\s*\((\w[\w\s]+?)\)").unwrap();
    if let Some(caps) = re_parens.captures(raw_text) {
        let candidate = caps[1].trim();
        for &ch in CHAPTERS {
            if candidate.eq_ignore_ascii_case(ch) {
                return Some(ch);
            }
        }
    }

    // Pattern 4: Chapter-specific detachments
    static DET_MAP: &[(&str, &str)] = &[
        ("Blade of Ultramar", "Ultramarines"),
        ("Anvil Siege Force", "Iron Hands"),
        ("Firestorm Assault Force", "Salamanders"),
        ("Forgefather", "Salamanders"),
        ("Stormlance Task Force", "White Scars"),
        ("Emperor's Shield", "Imperial Fists"),
    ];
    let lower = raw_text.to_lowercase();
    for &(det, ch) in DET_MAP {
        if lower.contains(&det.to_lowercase()) {
            return Some(ch);
        }
    }

    // Pattern 5: Named characters unique to a chapter
    static CHAR_MAP: &[(&str, &str)] = &[
        ("Marneus Calgar", "Ultramarines"),
        ("Cato Sicarius", "Ultramarines"),
        ("Roboute Guilliman", "Ultramarines"),
        ("Uriel Ventris", "Ultramarines"),
        ("Kayvaan Shrike", "Raven Guard"),
        ("Iron Father Feirros", "Iron Hands"),
        ("Adrax Agatone", "Salamanders"),
        ("Vulkan He'stan", "Salamanders"),
        ("Tor Garadon", "Imperial Fists"),
        ("Darnath Lysander", "Imperial Fists"),
        ("Pedro Kantor", "Crimson Fists"),
    ];
    for &(name, ch) in CHAR_MAP {
        if raw_text.contains(name) {
            return Some(ch);
        }
    }

    None
}

/// Strip common line prefixes used in BCP army list formats.
///
/// Handles: `Char1:`, `EH1:`, `CH2:`, `BL3:`, `IN4:`, `VE1:`, `MO1:`, `BE1:`, `DT1:`.
/// Returns `(stripped_line, prefix_hint)` where prefix_hint helps classify unit roles.
fn strip_line_prefix(line: &str) -> (&str, Option<&str>) {
    let re = Regex::new(r"^(?:(Char|EH|CH|BL|IN|VE|MO|BE|DT)\d+:\s*)").unwrap();
    match re.captures(line) {
        Some(caps) => {
            let prefix = caps.get(1).map(|m| m.as_str());
            (&line[caps.get(0).unwrap().end()..], prefix)
        }
        None => (line, None),
    }
}

/// Map a line prefix or section header to unit keywords.
fn keywords_for_section(section: &str) -> Vec<String> {
    match section {
        "CHARACTERS" | "CHARACTER" | "Char" | "EH" | "CH" => {
            vec!["Character".to_string()]
        }
        "BATTLELINE" | "BL" | "BE" => vec!["Battleline".to_string()],
        "OTHER DATASHEETS" => vec!["Other".to_string()],
        "IN" => vec!["Infantry".to_string()],
        "DEDICATED TRANSPORTS" | "DT" => vec!["Dedicated Transport".to_string()],
        "VE" => vec!["Vehicle".to_string()],
        "MO" => vec!["Monster".to_string()],
        "ALLIED UNITS" => vec!["Allied".to_string()],
        "FORTIFICATIONS" => vec!["Fortification".to_string()],
        _ => Vec::new(),
    }
}

/// Process buffered gear lines for a unit, classifying model types vs weapons.
///
/// Uses indentation depth to distinguish:
/// - Shallow indent (≤ min+1): model components → sum into unit count
/// - Deeper indent: weapons → add to wargear with "Nx" prefix when qty > 1
///
/// If all lines are at the same indent (flat), treats them all as weapons.
fn flush_gear_buffer(buffer: &mut Vec<(usize, u32, String)>, units: &mut [Unit]) {
    if buffer.is_empty() || units.is_empty() {
        buffer.clear();
        return;
    }
    let unit = units.last_mut().unwrap();

    let min_indent = buffer.iter().map(|g| g.0).min().unwrap_or(0);
    let max_indent = buffer.iter().map(|g| g.0).max().unwrap_or(0);
    let has_sub_levels = max_indent > min_indent + 1;

    if has_sub_levels {
        // Multi-level: top-level items are model components, deeper are weapons
        let mut model_count = 0u32;
        for (indent, qty, name) in buffer.iter() {
            if *indent <= min_indent + 1 {
                // Model component → contributes to squad count
                model_count += qty;
            } else {
                // Weapon line
                let formatted = if *qty > 1 {
                    format!("{}x {}", qty, name)
                } else {
                    name.clone()
                };
                unit.wargear.push(formatted);
            }
        }
        if model_count > 0 {
            unit.count = model_count;
        }
    } else {
        // Flat: all lines are direct weapons/gear
        for (_, qty, name) in buffer.iter() {
            if name.starts_with("Enhancement") || name.starts_with("Warlord") {
                continue;
            }
            let formatted = if *qty > 1 {
                format!("{}x {}", qty, name)
            } else {
                name.clone()
            };
            unit.wargear.push(formatted);
        }
    }
    buffer.clear();
}

/// Parse inline wargear from text after the points value.
///
/// Handles: `Unit (120 pts) 4x Multi-melta, 1x Inferno pistol`
/// Returns wargear items like `["4x Multi-melta", "Inferno pistol"]`.
fn parse_inline_wargear(text_after_points: &str) -> Vec<String> {
    let re_item = Regex::new(r"(\d+)x\s+([^,]+)").unwrap();
    let mut wargear = Vec::new();
    for caps in re_item.captures_iter(text_after_points) {
        let qty: u32 = caps[1].parse().unwrap_or(1);
        let name = caps[2].trim().to_string();
        if !name.is_empty() {
            let formatted = if qty > 1 {
                format!("{}x {}", qty, name)
            } else {
                name
            };
            wargear.push(formatted);
        }
    }
    wargear
}

/// Parse units from BCP army list raw text using regex.
///
/// Extracts unit names, points, counts, wargear (from bullet/indented lines
/// and inline comma-separated gear), and keywords (from section headers / line
/// prefixes).
///
/// Handles three BCP list formats:
/// 1. **Parenthesized**: `Unit Name (XXpts)` — most common (~60%)
/// 2. **Bracket**: `Unit Name [XXX pts]` — Russian tournament template (~20%)
/// 3. **Dash**: `1 Unit Name - XXpts` — a few tournament organizers (~1%)
///
/// Returns an empty Vec if no units could be parsed (signals AI fallback).
pub fn parse_units_from_raw_text(raw_text: &str) -> Vec<Unit> {
    // Format 1: "Unit Name (XXpts)" or "2x Unit Name (XX points)"
    // Captures optional trailing text after the closing paren for inline wargear
    let re_parens =
        Regex::new(r"(?i)^(?:(\d+)x?\s+)?(.+?)\s*\((\d+)\s*(?:pts?|points?)\)(.*)").unwrap();
    // Format 2: "Unit Name [XXX pts]" (bracket format, Russian template)
    let re_bracket =
        Regex::new(r"(?i)^(?:\((\d+)\)\s+)?(.+?)\s*\[(\d+)\s*(?:pts?|points?)\](.*)").unwrap();
    // Format 3: "1 Unit Name - XXpts" (dash format)
    let re_dash = Regex::new(r"(?i)^(\d+)\s+(.+?)\s*[-–]\s*(\d+)\s*(?:pts?|points?)(.*)").unwrap();
    // Wargear line: "• 1x Storm bolter" or "  1x Bolt rifle" or "- 1x Weapon"
    let re_wargear = Regex::new(r"(?:^[\u{2022}\u{25e6}\u{2013}*\-]\s*)?(\d+)x\s+(.+)").unwrap();
    // Skip "N with ..." wargear description lines
    let re_n_with = Regex::new(r"^\d+ with ").unwrap();
    // Extract (N) model count from name, e.g. "Retributor Squad (5)" → count=5
    let re_name_count = Regex::new(r"^(.+?)\s*\((\d+)\)\s*$").unwrap();

    let section_headers: std::collections::HashSet<&str> = [
        "CHARACTERS",
        "BATTLELINE",
        "OTHER DATASHEETS",
        "ALLIED UNITS",
        "CHARACTER",
        "DEDICATED TRANSPORTS",
        "FORTIFICATIONS",
    ]
    .into_iter()
    .collect();

    let skip_names: std::collections::HashSet<&str> =
        ["strike force", "incursion", "onslaught", "army roster"]
            .into_iter()
            .collect();

    let mut units: Vec<Unit> = Vec::new();
    let mut current_section = String::new();
    // Buffer for gear lines: (indent, qty, name)
    let mut gear_buffer: Vec<(usize, u32, String)> = Vec::new();

    for raw_line in raw_text.lines() {
        let indent = raw_line.len() - raw_line.trim_start().len();
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        // Track section headers for keyword assignment
        if section_headers.contains(line) {
            flush_gear_buffer(&mut gear_buffer, &mut units);
            current_section = line.to_string();
            continue;
        }
        // Also handle "Epic Hero:", "Character:", "Battleline:" labels
        let stripped_colon = line.trim_end_matches(':');
        if line.ends_with(':') && line.len() < 25 && !line.contains('[') && !line.contains('(') {
            flush_gear_buffer(&mut gear_buffer, &mut units);
            match stripped_colon {
                "Epic Hero" | "Character" | "Characters" => {
                    current_section = "CHARACTER".to_string();
                }
                "Battleline" => current_section = "BATTLELINE".to_string(),
                "Other Datasheets" | "Other" => {
                    current_section = "OTHER DATASHEETS".to_string();
                }
                "Dedicated Transports" => {
                    current_section = "DEDICATED TRANSPORTS".to_string();
                }
                _ => {}
            }
            continue;
        }

        // Gear line detection: bullet-prefixed OR indented Nx lines (without points)
        let starts_with_bullet = line.starts_with('\u{2022}')
            || line.starts_with('\u{25e6}')
            || line.starts_with('\u{2013}')
            || line.starts_with('*')
            || line.starts_with("- ");
        let has_points = line.contains("pts") || line.contains("points") || line.contains("Points");
        let is_indented_nx =
            indent >= 2 && !units.is_empty() && !has_points && re_wargear.is_match(line);

        if starts_with_bullet || is_indented_nx {
            if !units.is_empty() {
                if let Some(caps) = re_wargear.captures(line) {
                    let qty: u32 = caps[1].parse().unwrap_or(1);
                    let name = caps[2].trim().to_string();
                    if !name.is_empty() {
                        gear_buffer.push((indent, qty, name));
                    }
                }
            }
            continue;
        }

        // Skip non-unit lines
        if line.starts_with("Enhancement:") || line.starts_with("Warlord") {
            continue;
        }
        if line.starts_with("Exported with") || line == "undefined" {
            continue;
        }
        if line.starts_with('+') || line.starts_with('#') {
            continue;
        }
        if line.contains("(+") {
            continue;
        }
        if line.starts_with("==") {
            continue;
        }
        if re_n_with.is_match(line) {
            continue;
        }

        // Strip common line prefixes (Char1:, EH1:, CH2:, etc.)
        let (stripped, prefix_hint) = strip_line_prefix(line);

        // Try parenthesized format first, then bracket, then dash
        let parsed = re_parens
            .captures(stripped)
            .map(|caps| {
                let count: u32 = caps.get(1).map_or(1, |m| m.as_str().parse().unwrap_or(1));
                let name = caps[2].trim().to_string();
                let points: u32 = caps[3].parse().unwrap_or(0);
                let trailing = caps.get(4).map_or("", |m| m.as_str());
                (count, name, points, trailing.to_string())
            })
            .or_else(|| {
                re_bracket.captures(stripped).map(|caps| {
                    let count: u32 = caps.get(1).map_or(1, |m| m.as_str().parse().unwrap_or(1));
                    let raw_name = caps[2].trim();
                    let name = raw_name
                        .split(',')
                        .next()
                        .unwrap_or(raw_name)
                        .trim()
                        .to_string();
                    let points: u32 = caps[3].parse().unwrap_or(0);
                    let trailing = caps.get(4).map_or("", |m| m.as_str());
                    (count, name, points, trailing.to_string())
                })
            })
            .or_else(|| {
                re_dash.captures(stripped).map(|caps| {
                    let count: u32 = caps[1].parse().unwrap_or(1);
                    let name = caps[2].trim().to_string();
                    let points: u32 = caps[3].parse().unwrap_or(0);
                    let trailing = caps.get(4).map_or("", |m| m.as_str());
                    (count, name, points, trailing.to_string())
                })
            });

        if let Some((count, name, points, trailing)) = parsed {
            if skip_names.contains(name.to_lowercase().as_str()) {
                continue;
            }
            if name.is_empty() || points == 0 {
                continue;
            }
            // Skip army total lines (no single unit costs 800+ pts)
            if points >= 800 {
                continue;
            }
            if name.starts_with("ENHANCEMENT") {
                continue;
            }

            let (clean_name, count) = if let Some(nc) = re_name_count.captures(&name) {
                let n = nc[1].trim().to_string();
                let c: u32 = nc[2].parse().unwrap_or(count);
                (n, c)
            } else {
                (
                    name.replace(": Warlord", "")
                        .replace(": ENHANCEMENT", "")
                        .trim()
                        .to_string(),
                    count,
                )
            };

            // Determine keywords from section header or line prefix
            let keywords = if let Some(prefix) = prefix_hint {
                keywords_for_section(prefix)
            } else if !current_section.is_empty() {
                keywords_for_section(&current_section)
            } else {
                Vec::new()
            };

            // Flush gear buffer for the previous unit before pushing new one
            flush_gear_buffer(&mut gear_buffer, &mut units);

            // Parse inline wargear from trailing text after points
            let inline_wargear = parse_inline_wargear(&trailing);

            let mut unit = Unit::new(clean_name, count)
                .with_points(points)
                .with_keywords(keywords);
            if !inline_wargear.is_empty() {
                unit = unit.with_wargear(inline_wargear);
            }
            units.push(unit);
        }
    }

    // Final flush for the last unit's gear lines
    flush_gear_buffer(&mut gear_buffer, &mut units);

    units
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bcp_event_parsed_start_date() {
        let event = BcpEvent {
            id: "abc123".to_string(),
            name: "Test GT".to_string(),
            start_date: Some("2026-02-01T00:00:00.000Z".to_string()),
            end_date: None,
            venue: None,
            city: Some("London".to_string()),
            state: None,
            country: Some("UK".to_string()),
            player_count: Some(64),
            round_count: Some(5),
            game_type: Some(1),
            ended: None,
            team_event: None,
            hide_placings: None,
        };

        assert_eq!(
            event.parsed_start_date(),
            Some(NaiveDate::from_ymd_opt(2026, 2, 1).unwrap())
        );
    }

    #[test]
    fn test_bcp_event_location_string() {
        let event = BcpEvent {
            id: "abc".to_string(),
            name: "Test".to_string(),
            start_date: None,
            end_date: None,
            venue: Some("Convention Center".to_string()),
            city: Some("London".to_string()),
            state: None,
            country: Some("UK".to_string()),
            player_count: None,
            round_count: None,
            game_type: None,
            ended: None,
            team_event: None,
            hide_placings: None,
        };

        assert_eq!(
            event.location_string(),
            Some("Convention Center, London, UK".to_string())
        );
    }

    #[test]
    fn test_bcp_event_url() {
        let event = BcpEvent {
            id: "abc123".to_string(),
            name: "Test".to_string(),
            start_date: None,
            end_date: None,
            venue: None,
            city: None,
            state: None,
            country: None,
            player_count: None,
            round_count: None,
            game_type: None,
            ended: None,
            team_event: None,
            hide_placings: None,
        };

        assert_eq!(
            event.event_url(),
            "https://www.bestcoastpairings.com/event/abc123"
        );
    }

    #[test]
    fn test_bcp_event_should_skip() {
        let mut event = BcpEvent {
            id: "abc".to_string(),
            name: "Test".to_string(),
            start_date: None,
            end_date: None,
            venue: None,
            city: None,
            state: None,
            country: None,
            player_count: None,
            round_count: None,
            game_type: None,
            ended: None,
            team_event: None,
            hide_placings: None,
        };

        assert!(!event.should_skip());

        event.team_event = Some(true);
        assert!(event.should_skip());

        event.team_event = None;
        event.hide_placings = Some(true);
        assert!(event.should_skip());
    }

    #[test]
    fn test_bcp_event_deserialize_v3() {
        let json = r#"{
            "id": "abc123",
            "name": "Test GT 2026",
            "startDate": "2026-02-01",
            "endDate": "2026-02-02",
            "city": "London",
            "country": "UK",
            "numberOfPlayers": 64,
            "numberOfRounds": 5,
            "gameSystemId": 1
        }"#;

        let event: BcpEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.id, "abc123");
        assert_eq!(event.name, "Test GT 2026");
        assert_eq!(event.player_count, Some(64));
        assert_eq!(event.round_count, Some(5));
    }

    #[test]
    fn test_bcp_event_deserialize_v1() {
        let json = r#"{
            "id": "abc123",
            "name": "Test GT 2026",
            "eventDate": "2026-02-01T00:00:00.000Z",
            "eventEndDate": "2026-02-02T00:00:00.000Z",
            "city": "London",
            "country": "UK",
            "totalPlayers": 64,
            "numberOfRounds": 5,
            "gameType": 1,
            "ended": true,
            "teamEvent": false,
            "hidePlacings": false
        }"#;

        let event: BcpEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.id, "abc123");
        assert_eq!(event.name, "Test GT 2026");
        assert_eq!(event.player_count, Some(64));
        assert_eq!(event.ended, Some(true));
        assert_eq!(event.team_event, Some(false));
        assert_eq!(event.hide_placings, Some(false));
        assert_eq!(
            event.parsed_start_date(),
            Some(NaiveDate::from_ymd_opt(2026, 2, 1).unwrap())
        );
    }

    #[test]
    fn test_bcp_standing_deserialize() {
        let json = r#"{
            "placing": 1,
            "playerName": "John Smith",
            "armyName": "Aeldari",
            "wins": 5,
            "losses": 0,
            "draws": 0,
            "totalBattlePoints": 94,
            "playerId": "player-1",
            "armyListObjectId": "list-1"
        }"#;

        let standing: BcpStanding = serde_json::from_str(json).unwrap();
        assert_eq!(standing.placing, Some(1));
        assert_eq!(standing.player_name, Some("John Smith".to_string()));
        assert_eq!(standing.faction, Some("Aeldari".to_string()));
        assert_eq!(standing.wins, Some(5));
    }

    #[test]
    fn test_bcp_army_list_deserialize_v3() {
        let json = r#"{
            "armyList": "++ Army Roster ++\nFaction: Aeldari\n...",
            "armyName": "Aeldari"
        }"#;

        let list: BcpArmyList = serde_json::from_str(json).unwrap();
        assert!(list.army_list.unwrap().contains("Aeldari"));
        assert_eq!(list.faction, Some("Aeldari".to_string()));
    }

    #[test]
    fn test_bcp_army_list_deserialize_v1() {
        let json = r#"{
            "armyListText": "GK Army (1975 points)\nGrey Knights\n...",
            "armyName": "Grey Knights",
            "warhammer": {"detachment": "Teleport Strike Force"},
            "army": {"name": "Grey Knights"}
        }"#;

        let list: BcpArmyList = serde_json::from_str(json).unwrap();
        assert!(list.army_list.unwrap().contains("Grey Knights"));
        assert_eq!(list.faction, Some("Grey Knights".to_string()));
        assert_eq!(list.detachment, Some("Teleport Strike Force".to_string()));
        assert_eq!(list.army_faction, Some("Grey Knights".to_string()));
    }

    #[test]
    fn test_bcp_player_v1_deserialize() {
        let json = r#"{
            "_id": "player-123",
            "user": {
                "firstName": "John",
                "lastName": "Smith"
            },
            "faction": {
                "name": "Aeldari"
            },
            "armyListObjectId": "list-abc",
            "dropped": false
        }"#;

        let player: BcpPlayerV1 = serde_json::from_str(json).unwrap();
        assert_eq!(player.id, Some("player-123".to_string()));
        assert_eq!(player.full_name(), "John Smith");
        assert_eq!(player.faction_name(), Some("Aeldari".to_string()));
        assert_eq!(player.list_id, Some("list-abc".to_string()));
    }

    #[test]
    fn test_bcp_players_response_deserialize() {
        let json = r#"{
            "active": [
                {
                    "_id": "p1",
                    "user": {"firstName": "Alice", "lastName": "B"},
                    "faction": {"name": "Necrons"}
                }
            ],
            "deleted": []
        }"#;

        let response: BcpPlayersResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.active.len(), 1);
        assert_eq!(response.active[0].full_name(), "Alice B");
    }

    #[test]
    fn test_bcp_pairing_deserialize() {
        let json = r#"{
            "player1": {
                "_id": "p1",
                "firstName": "Alice",
                "lastName": "A",
                "armyName": "Necrons",
                "armyListObjectId": "list-1"
            },
            "player2": {
                "_id": "p2",
                "firstName": "Bob",
                "lastName": "B",
                "armyName": "Aeldari",
                "armyListObjectId": "list-2"
            },
            "metaData": {
                "p1-gameResult": 2,
                "p1-gamePoints": 85.0,
                "p2-gameResult": 0,
                "p2-gamePoints": 60.0
            },
            "round": 1
        }"#;

        let pairing: BcpPairing = serde_json::from_str(json).unwrap();
        assert_eq!(pairing.player1.as_ref().unwrap().full_name(), "Alice A");
        assert_eq!(pairing.player2.as_ref().unwrap().full_name(), "Bob B");
        assert_eq!(pairing.meta_data.as_ref().unwrap().p1_game_result, Some(2));
        assert_eq!(pairing.meta_data.as_ref().unwrap().p2_game_result, Some(0));
        assert_eq!(pairing.round, Some(1));
    }

    #[test]
    fn test_compute_standings_basic() {
        let fetcher = Fetcher::new(crate::fetch::FetcherConfig {
            cache_dir: std::path::PathBuf::from("/tmp/test-bcp"),
            ..Default::default()
        })
        .unwrap();
        let client = BcpClient::new(fetcher, "https://example.com/v1".to_string(), 1);

        let pairings = vec![
            BcpPairing {
                player1: Some(BcpPairingPlayer {
                    id: Some("p1".to_string()),
                    first_name: Some("Alice".to_string()),
                    last_name: Some("A".to_string()),
                    army_name: Some("Necrons".to_string()),
                    army_list_object_id: Some("list-1".to_string()),
                }),
                player2: Some(BcpPairingPlayer {
                    id: Some("p2".to_string()),
                    first_name: Some("Bob".to_string()),
                    last_name: Some("B".to_string()),
                    army_name: Some("Aeldari".to_string()),
                    army_list_object_id: Some("list-2".to_string()),
                }),
                meta_data: Some(BcpPairingMeta {
                    p1_game_result: Some(2), // Alice wins
                    p1_game_points: Some(85.0),
                    p2_game_result: Some(0), // Bob loses
                    p2_game_points: Some(60.0),
                }),
                round: Some(1),
            },
            BcpPairing {
                player1: Some(BcpPairingPlayer {
                    id: Some("p1".to_string()),
                    first_name: Some("Alice".to_string()),
                    last_name: Some("A".to_string()),
                    army_name: Some("Necrons".to_string()),
                    army_list_object_id: Some("list-1".to_string()),
                }),
                player2: Some(BcpPairingPlayer {
                    id: Some("p3".to_string()),
                    first_name: Some("Charlie".to_string()),
                    last_name: Some("C".to_string()),
                    army_name: Some("Orks".to_string()),
                    army_list_object_id: None,
                }),
                meta_data: Some(BcpPairingMeta {
                    p1_game_result: Some(2), // Alice wins again
                    p1_game_points: Some(90.0),
                    p2_game_result: Some(0),
                    p2_game_points: Some(50.0),
                }),
                round: Some(2),
            },
        ];

        let standings = client.compute_standings(&pairings, &[]);

        // Alice should be first (2 wins)
        assert_eq!(standings.len(), 3);
        assert_eq!(standings[0].player_name, Some("Alice A".to_string()));
        assert_eq!(standings[0].wins, Some(2));
        assert_eq!(standings[0].placing, Some(1));
        assert_eq!(standings[0].total_battle_points, Some(175));
        assert_eq!(standings[0].army_list_object_id, Some("list-1".to_string()));

        // Bob and Charlie both have 0 wins
        let bob = standings
            .iter()
            .find(|s| s.player_name == Some("Bob B".to_string()))
            .unwrap();
        assert_eq!(bob.wins, Some(0));
        assert_eq!(bob.losses, Some(1));

        let charlie = standings
            .iter()
            .find(|s| s.player_name == Some("Charlie C".to_string()))
            .unwrap();
        assert_eq!(charlie.wins, Some(0));
    }

    #[test]
    fn test_compute_standings_with_draw() {
        let fetcher = Fetcher::new(crate::fetch::FetcherConfig {
            cache_dir: std::path::PathBuf::from("/tmp/test-bcp"),
            ..Default::default()
        })
        .unwrap();
        let client = BcpClient::new(fetcher, "https://example.com/v1".to_string(), 1);

        let pairings = vec![BcpPairing {
            player1: Some(BcpPairingPlayer {
                id: Some("p1".to_string()),
                first_name: Some("Alice".to_string()),
                last_name: Some("A".to_string()),
                army_name: None,
                army_list_object_id: None,
            }),
            player2: Some(BcpPairingPlayer {
                id: Some("p2".to_string()),
                first_name: Some("Bob".to_string()),
                last_name: Some("B".to_string()),
                army_name: None,
                army_list_object_id: None,
            }),
            meta_data: Some(BcpPairingMeta {
                p1_game_result: Some(1), // Draw
                p1_game_points: Some(70.0),
                p2_game_result: Some(1), // Draw
                p2_game_points: Some(70.0),
            }),
            round: Some(1),
        }];

        let standings = client.compute_standings(&pairings, &[]);
        assert_eq!(standings.len(), 2);

        for s in &standings {
            assert_eq!(s.draws, Some(1));
            assert_eq!(s.wins, Some(0));
            assert_eq!(s.losses, Some(0));
        }
    }

    #[test]
    fn test_compute_standings_empty() {
        let fetcher = Fetcher::new(crate::fetch::FetcherConfig {
            cache_dir: std::path::PathBuf::from("/tmp/test-bcp"),
            ..Default::default()
        })
        .unwrap();
        let client = BcpClient::new(fetcher, "https://example.com/v1".to_string(), 1);

        let standings = client.compute_standings(&[], &[]);
        assert!(standings.is_empty());
    }

    #[test]
    fn test_compute_standings_with_player_info() {
        let fetcher = Fetcher::new(crate::fetch::FetcherConfig {
            cache_dir: std::path::PathBuf::from("/tmp/test-bcp"),
            ..Default::default()
        })
        .unwrap();
        let client = BcpClient::new(fetcher, "https://example.com/v1".to_string(), 1);

        let players = vec![BcpPlayerV1 {
            id: Some("p1".to_string()),
            user: Some(BcpUser {
                first_name: Some("Alice".to_string()),
                last_name: Some("A".to_string()),
            }),
            faction: Some(BcpFaction {
                name: Some("Necrons".to_string()),
            }),
            list_id: Some("list-1".to_string()),
            dropped: None,
            army_name: None,
        }];

        let pairings = vec![BcpPairing {
            player1: Some(BcpPairingPlayer {
                id: Some("p1".to_string()),
                first_name: Some("Alice".to_string()),
                last_name: Some("A".to_string()),
                army_name: None, // Not on pairing
                army_list_object_id: Some("list-1".to_string()),
            }),
            player2: Some(BcpPairingPlayer {
                id: Some("p2".to_string()),
                first_name: Some("Bob".to_string()),
                last_name: Some("B".to_string()),
                army_name: Some("Aeldari".to_string()),
                army_list_object_id: None,
            }),
            meta_data: Some(BcpPairingMeta {
                p1_game_result: Some(2),
                p1_game_points: Some(85.0),
                p2_game_result: Some(0),
                p2_game_points: Some(60.0),
            }),
            round: Some(1),
        }];

        let standings = client.compute_standings(&pairings, &players);

        // Alice should get faction from player info
        let alice = standings
            .iter()
            .find(|s| s.player_name == Some("Alice A".to_string()))
            .unwrap();
        assert_eq!(alice.faction, Some("Necrons".to_string()));

        // Bob gets faction from pairing
        let bob = standings
            .iter()
            .find(|s| s.player_name == Some("Bob B".to_string()))
            .unwrap();
        assert_eq!(bob.faction, Some("Aeldari".to_string()));
    }

    // ── parse_units_from_raw_text tests ────────────────────────────────────

    #[test]
    fn test_parse_standard_bcp_format() {
        let raw = r#"GK Army (1975 points)

Grey Knights
Strike Force (2000 points)
Teleport Strike Force


CHARACTERS

Brotherhood Librarian (150 points)
  • 1x Combi-weapon
    1x Nemesis force weapon
  • Enhancement: Sigil of Exigence

Castellan Crowe (90 points)
  • 1x Black Blade of Antwyr
    1x Storm bolter

Grand Master Voldus (95 points)
  • Warlord
  • 1x Malleus Argyrum

BATTLELINE

Brotherhood Terminator Squad (200 points)
  • 4x Brotherhood Terminator

Brotherhood Terminator Squad (200 points)
  • 4x Brotherhood Terminator

OTHER DATASHEETS

Nemesis Dreadknight (245 points)
  • 1x Heavy psycannon

Nemesis Dreadknight (245 points)
  • 1x Heavy psycannon
"#;
        let units = parse_units_from_raw_text(raw);
        // Should parse: Librarian(150), Crowe(90), Voldus(95), 2x Terminator(200),
        //               2x Dreadknight(245) = 7 units minimum
        // "Strike Force" and army name lines should be skipped
        assert!(units.len() >= 7, "Expected >= 7 units, got {}", units.len());

        // Check specific units
        let librarian = units
            .iter()
            .find(|u| u.name.contains("Brotherhood Librarian"));
        assert!(librarian.is_some(), "Should find Brotherhood Librarian");
        assert_eq!(librarian.unwrap().points, Some(150));

        let crowe = units.iter().find(|u| u.name.contains("Crowe"));
        assert!(crowe.is_some(), "Should find Castellan Crowe");
        assert_eq!(crowe.unwrap().points, Some(90));

        // Strike Force should be skipped
        let sf = units
            .iter()
            .find(|u| u.name.to_lowercase().contains("strike force"));
        assert!(sf.is_none(), "Strike Force should be skipped");
    }

    #[test]
    fn test_parse_char_prefix_pts_format() {
        let raw = r#"+++++++++++++++++++++++++++++++++++++++++++++++
+ FACTION KEYWORD: Xenos - Tyranids
+ DETACHMENT: Crusher Stampede
+ TOTAL ARMY POINTS: 2000pts
+++++++++++++++++++++++++++++++++++++++++++++++

CHARACTER

Char1: 1x Old One Eye (150 pts)
1 with Old One Eye's claws and talons

Char2: 1x Hive Tyrant (235 pts)
1 with Monstrous Bonesword and Lash Whip
  • Warlord

Char3: 1x Neurotyrant (105 pts)

BATTLELINE

10x Gargoyles (85 pts)

OTHER DATASHEETS

1x Biovores (50 pts)
3x Screamer-Killers (375 pts)
2x Carnifexes (180 pts)
"#;
        let units = parse_units_from_raw_text(raw);
        assert!(units.len() >= 7, "Expected >= 7 units, got {}", units.len());

        // Old One Eye with Char prefix
        let ooe = units.iter().find(|u| u.name.contains("Old One Eye"));
        assert!(ooe.is_some(), "Should find Old One Eye");
        assert_eq!(ooe.unwrap().points, Some(150));
        assert_eq!(ooe.unwrap().count, 1);

        // 10x Gargoyles
        let gargs = units.iter().find(|u| u.name.contains("Gargoyles"));
        assert!(gargs.is_some(), "Should find Gargoyles");
        assert_eq!(gargs.unwrap().count, 10);
        assert_eq!(gargs.unwrap().points, Some(85));

        // 3x Screamer-Killers
        let sk = units.iter().find(|u| u.name.contains("Screamer-Killers"));
        assert!(sk.is_some(), "Should find Screamer-Killers");
        assert_eq!(sk.unwrap().count, 3);
        assert_eq!(sk.unwrap().points, Some(375));
    }

    #[test]
    fn test_parse_bracket_format() {
        let raw = r#"+++++++++++++++++++++++++++++++++++++++++++++++
+FACTION KEYWORD: Imperium - Adeptus Custodes
+DETACHMENT: Lions of the Emperor
+TOTAL ARMY POINTS: 2000 pts
+++++++++++++++++++++++++++++++++++++++++++++++
EH1: Trajann Valoris [140 pts]
BL1: (4) Custodian Guard, Guardian Spear [150 pts]
CH1: Blade Champion [120 pts]
CH2: Shield-captain In Allarus Terminator Armour, Castellan axe, Admonimortis [140 pts]
IN1: (2) Allarus Custodians, Guardian Spear [110 pts]
IN2: (4) Custodian Wardens, Guardian Spear, Vexilla [210 pts]
IN3: (5) Witchseekers, Witchseeker Flamer [55 pts]
VE1: Venerable Land Raider, Hunter-killer missile [220 pts]
"#;
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 8, "Expected 8 units, got {:?}", units);

        // Trajann Valoris (EH prefix, no count)
        let trajann = units.iter().find(|u| u.name.contains("Trajann Valoris"));
        assert!(trajann.is_some(), "Should find Trajann Valoris");
        assert_eq!(trajann.unwrap().points, Some(140));
        assert_eq!(trajann.unwrap().count, 1);

        // Custodian Guard with (4) count prefix
        let guard = units.iter().find(|u| u.name.contains("Custodian Guard"));
        assert!(guard.is_some(), "Should find Custodian Guard");
        assert_eq!(guard.unwrap().count, 4);
        assert_eq!(guard.unwrap().points, Some(150));

        // Shield-captain — should strip wargear after comma
        let sc = units.iter().find(|u| u.name.contains("Shield-captain"));
        assert!(sc.is_some(), "Should find Shield-captain");
        assert_eq!(sc.unwrap().points, Some(140));

        // Allarus Custodians with (2) count
        let allarus = units.iter().find(|u| u.name.contains("Allarus Custodians"));
        assert!(allarus.is_some(), "Should find Allarus Custodians");
        assert_eq!(allarus.unwrap().count, 2);
    }

    #[test]
    fn test_parse_dash_format() {
        let raw = r#"Sneaky Beaky Bois
Detachment: Vanguard Spearhead

1 Kayvaan Shrike - 100 pts
- 1x Blackout
- 1x The Raven's Talons

1 Captain - 80 pts
- 1x Master-crafted power sword

1 Librarian in Phobos armour - 70 pts
- 1x Smite
- 1x Force weapon

5 Vanguard Veterans - 110 pts
- 5x Vanguard Veteran Weapon
"#;
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 4, "Expected 4 units, got {:?}", units);

        let shrike = units
            .iter()
            .find(|u| u.name.contains("Kayvaan Shrike"))
            .unwrap();
        assert_eq!(shrike.points, Some(100));
        assert_eq!(shrike.count, 1);
        assert_eq!(shrike.wargear, vec!["Blackout", "The Raven's Talons"]);

        let captain = units.iter().find(|u| u.name == "Captain").unwrap();
        assert_eq!(captain.wargear, vec!["Master-crafted power sword"]);

        let librarian = units.iter().find(|u| u.name.contains("Librarian")).unwrap();
        assert_eq!(librarian.wargear, vec!["Smite", "Force weapon"]);

        let vets = units
            .iter()
            .find(|u| u.name.contains("Vanguard Veterans"))
            .unwrap();
        assert_eq!(vets.count, 5);
        assert_eq!(vets.points, Some(110));
        assert_eq!(vets.wargear, vec!["5x Vanguard Veteran Weapon"]);
    }

    #[test]
    fn test_parse_bracket_format_with_warlord_suffix() {
        // Some bracket-format lists have ": Warlord" after the unit name
        let raw = r#"EH1: Mortarion: Warlord [380 pts]
EH2: Typhus [100 pts]
CH1: Daemon Prince of Nurgle [215 pts]
IN1: Blightlord Terminators (10), 2x blight launcher [370 pts]
IN2: Deathshroud Terminators (3), 1x Icon of Despair [160 pts]
BL1: Poxwalkers (10) [65 pts]
"#;
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 6, "Expected 6 units, got {:?}", units);

        // Mortarion should have Warlord stripped from name
        let mort = units.iter().find(|u| u.name.contains("Mortarion"));
        assert!(mort.is_some(), "Should find Mortarion");
        assert_eq!(mort.unwrap().points, Some(380));
        assert!(
            !mort.unwrap().name.contains("Warlord"),
            "Warlord should be stripped from name"
        );
    }

    #[test]
    fn test_parse_inline_pts_on_same_line_as_wargear() {
        // Russian format: unit name + model count in parens + points, wargear on same line
        let raw = r#"Bjorn the Fell-Handed (160 points) 1x Multi-melta
Iron Priest (80 points) Warlord Enhancement: Chariots of the Storm
Logan Grimnar (110 points)
Ragnar Blackmane (100 points)
Blood Claws (20) (285 points) 1x Power weapon
Intercessor Squad (5) (80 points) 1x Astartes grenade launcher
Fenrisian Wolves (40 points)
Gladiator Lancer (160 points) 2x Storm bolter
"#;
        let units = parse_units_from_raw_text(raw);
        assert!(units.len() >= 6, "Expected >= 6 units, got {}", units.len());

        let bjorn = units.iter().find(|u| u.name.contains("Bjorn"));
        assert!(bjorn.is_some(), "Should find Bjorn");
        assert_eq!(bjorn.unwrap().points, Some(160));

        // Fenrisian Wolves (40 points) — number looks like count but it's points
        let wolves = units.iter().find(|u| u.name.contains("Fenrisian Wolves"));
        assert!(wolves.is_some(), "Should find Fenrisian Wolves");
        assert_eq!(wolves.unwrap().points, Some(40));
    }

    #[test]
    fn test_parse_skips_enhancement_lines() {
        let raw = r#"CHARACTERS

Brotherhood Librarian (150 points)
  • Enhancement: Sigil of Exigence
  • (+20 pts)

OTHER DATASHEETS

Nemesis Dreadknight (245 points)
"#;
        let units = parse_units_from_raw_text(raw);
        // Should only find 2 units, not the enhancement line
        assert_eq!(units.len(), 2, "Expected 2 units, got {:?}", units);
        assert_eq!(units[0].keywords, vec!["Character"]);
        assert_eq!(units[1].keywords, vec!["Other"]);
    }

    #[test]
    fn test_parse_skips_wargear_bullet_lines() {
        let raw = r#"Castellan Crowe (90 points)
  • 1x Black Blade of Antwyr
  - 1x Storm bolter
Nemesis Dreadknight (245 points)
  • 1x Heavy psycannon
"#;
        let units = parse_units_from_raw_text(raw);
        // Should only find 2 units (Crowe, Dreadknight), not wargear lines
        assert_eq!(units.len(), 2, "Expected 2 units, got {:?}", units);
        // Wargear should be attached to the correct units
        assert_eq!(
            units[0].wargear,
            vec!["Black Blade of Antwyr", "Storm bolter"]
        );
        assert_eq!(units[1].wargear, vec!["Heavy psycannon"]);
    }

    #[test]
    fn test_parse_skips_header_lines() {
        let raw = r#"+++++++++++++++++++++++++++++++++++++++++++++++
+ FACTION KEYWORD: Space Marines
+ DETACHMENT: Ironstorm
+ TOTAL ARMY POINTS: 2000pts
+++++++++++++++++++++++++++++++++++++++++++++++

# Army Roster

## Character [200 pts]

== Ironstorm Army Roster ==

Captain (80 points)
"#;
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 1, "Expected 1 unit, got {:?}", units);
        assert_eq!(units[0].name, "Captain");
    }

    #[test]
    fn test_parse_returns_empty_for_no_points_format() {
        // Lists without per-unit points should return empty (needs AI)
        let raw = r#"Aeldari - Warhost
2000 Points

Maugan Ra
- Maugetar

5x Swooping Hawks
- 4x Lasblasters

10x Dark Reapers
- 10x Reaper Launchers
"#;
        let units = parse_units_from_raw_text(raw);
        assert!(
            units.is_empty(),
            "Should return empty for no-points format, got {:?}",
            units
        );
    }

    #[test]
    fn test_parse_returns_empty_for_whitespace_tabulated() {
        // Whitespace-aligned columns — impossible to parse with simple regex
        let raw = r#"GREY KNIGHTS

Brotherhood Strike

GrandMaster                                                                95
       Purity of purpose                                                   15

Paladin Squad  (10)                                                       450

Librarians  x2                                                 80        160
"#;
        let units = parse_units_from_raw_text(raw);
        assert!(
            units.is_empty(),
            "Should return empty for whitespace-tabulated format, got {:?}",
            units
        );
    }

    #[test]
    fn test_parse_total_points_accuracy() {
        // Test that total points sum makes sense for a standard list
        let raw = r#"CHARACTERS

Captain (80 points)
Librarian (70 points)

BATTLELINE

Intercessor Squad (75 points)
Intercessor Squad (75 points)

OTHER DATASHEETS

Redemptor Dreadnought (210 points)
Gladiator Lancer (160 points)
"#;
        let units = parse_units_from_raw_text(raw);
        let total: u32 = units.iter().filter_map(|u| u.points).sum();
        assert_eq!(total, 670, "Total should be 670pts, got {}", total);
        assert_eq!(units.len(), 6);
        // Verify section-based keyword assignment
        assert_eq!(units[0].keywords, vec!["Character"]);
        assert_eq!(units[1].keywords, vec!["Character"]);
        assert_eq!(units[2].keywords, vec!["Battleline"]);
        assert_eq!(units[3].keywords, vec!["Battleline"]);
        assert_eq!(units[4].keywords, vec!["Other"]);
        assert_eq!(units[5].keywords, vec!["Other"]);
    }

    #[test]
    fn test_parse_mixed_count_formats() {
        // Various ways counts appear
        let raw = r#"1x Old One Eye (150 pts)
3x Screamer-Killers (375 pts)
10x Gargoyles (85 pts)
Neurotyrant (105 pts)
"#;
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 4);
        assert_eq!(units[0].count, 1); // 1x
        assert_eq!(units[1].count, 3); // 3x
        assert_eq!(units[2].count, 10); // 10x
        assert_eq!(units[3].count, 1); // no prefix = 1
    }

    #[test]
    fn test_parse_bracket_count_in_parens() {
        // Bracket format uses (N) before unit name for count
        let raw = r#"EH1: Trajann Valoris [140 pts]
BL1: (4) Custodian Guard, Guardian Spear [150 pts]
IN1: (2) Allarus Custodians [110 pts]
VE1: Venerable Land Raider [220 pts]
"#;
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 4);

        let trajann = units.iter().find(|u| u.name.contains("Trajann")).unwrap();
        assert_eq!(trajann.count, 1);
        assert_eq!(trajann.keywords, vec!["Character"]); // EH prefix

        let guard = units
            .iter()
            .find(|u| u.name.contains("Custodian Guard"))
            .unwrap();
        assert_eq!(guard.count, 4);
        assert_eq!(guard.keywords, vec!["Battleline"]); // BL prefix

        let allarus = units.iter().find(|u| u.name.contains("Allarus")).unwrap();
        assert_eq!(allarus.count, 2);
        assert_eq!(allarus.keywords, vec!["Infantry"]); // IN prefix

        let raider = units
            .iter()
            .find(|u| u.name.contains("Land Raider"))
            .unwrap();
        assert_eq!(raider.keywords, vec!["Vehicle"]); // VE prefix
    }

    #[test]
    fn test_parse_does_not_include_game_size() {
        let raw = r#"Strike Force (2000 points)
Incursion (1000 points)
Onslaught (3000 points)
Captain (80 points)
"#;
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].name, "Captain");
    }

    #[test]
    fn test_strip_line_prefix() {
        assert_eq!(
            strip_line_prefix("Char1: 1x Old One Eye (150 pts)"),
            ("1x Old One Eye (150 pts)", Some("Char"))
        );
        assert_eq!(
            strip_line_prefix("EH1: Trajann Valoris [140 pts]"),
            ("Trajann Valoris [140 pts]", Some("EH"))
        );
        assert_eq!(
            strip_line_prefix("CH2: Shield-captain [140 pts]"),
            ("Shield-captain [140 pts]", Some("CH"))
        );
        assert_eq!(
            strip_line_prefix("BL1: (4) Custodian Guard [150 pts]"),
            ("(4) Custodian Guard [150 pts]", Some("BL"))
        );
        assert_eq!(
            strip_line_prefix("IN3: Poxwalkers (10) [65 pts]"),
            ("Poxwalkers (10) [65 pts]", Some("IN"))
        );
        assert_eq!(
            strip_line_prefix("VE1: Venerable Land Raider [220 pts]"),
            ("Venerable Land Raider [220 pts]", Some("VE"))
        );
        assert_eq!(
            strip_line_prefix("MO1: Carnifex [90 pts]"),
            ("Carnifex [90 pts]", Some("MO"))
        );
        assert_eq!(
            strip_line_prefix("Captain (80 points)"),
            ("Captain (80 points)", None)
        );
    }

    #[test]
    fn test_detect_chapter_line_after_sm() {
        let raw = "Army Name (2000 Points)\n\nSpace Marines\nUltramarines\nStrike Force (2000 Points)\nBlade of Ultramar\n";
        assert_eq!(detect_chapter_from_raw_text(raw), Some("Ultramarines"));
    }

    #[test]
    fn test_detect_chapter_astartes_dash() {
        let raw = "+ FACTION KEYWORD: Imperium - Adeptus Astartes - Iron Hands\n+ DETACHMENT: Anvil Siege Force\n";
        assert_eq!(detect_chapter_from_raw_text(raw), Some("Iron Hands"));
    }

    #[test]
    fn test_detect_chapter_parens() {
        let raw =
            "Army Faction Used: Space Marines (Ultramarines)\nDetachment: Blade of Ultramar\n";
        assert_eq!(detect_chapter_from_raw_text(raw), Some("Ultramarines"));
    }

    #[test]
    fn test_detect_chapter_from_detachment() {
        let raw = "FACTION KEYWORD: Adeptus Astartes\nDETACHMENT: Firestorm Assault Force\nCaptain (80 points)\n";
        assert_eq!(detect_chapter_from_raw_text(raw), Some("Salamanders"));
    }

    #[test]
    fn test_detect_chapter_from_character() {
        let raw = "1 Kayvaan Shrike - 100 pts\n1 Captain - 80 pts\n";
        assert_eq!(detect_chapter_from_raw_text(raw), Some("Raven Guard"));
    }

    #[test]
    fn test_detect_chapter_generic_sm() {
        // Truly generic SM list with no chapter markers
        let raw = "FACTION KEYWORD: Space Marines\nDETACHMENT: Ironstorm Spearhead\nTechmarine (65 points)\n";
        assert_eq!(detect_chapter_from_raw_text(raw), None);
    }

    #[test]
    fn test_parse_multi_level_wargear_retributors() {
        // BCP app format with model-type lines and sub-weapon lines
        let raw = "OTHER DATASHEETS\n\nRetributor Squad (120 points)\n  \u{2022} 1x Retributor Superior\n    \u{2022} 1x Bolt pistol\n      1x Close combat weapon\n      1x Condemnor boltgun\n      1x Power weapon\n  \u{2022} 4x Retributor\n    \u{2022} 4x Bolt pistol\n      4x Close combat weapon\n      4x Multi-melta\n";
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 1, "Expected 1 unit, got {:?}", units);
        let ret = &units[0];
        assert_eq!(ret.name, "Retributor Squad");
        assert_eq!(ret.count, 5, "Squad should be 5 models (1 superior + 4)");
        assert!(
            ret.wargear.iter().any(|w| w.contains("Multi-melta")),
            "Should have Multi-melta in wargear: {:?}",
            ret.wargear
        );
        assert!(
            ret.wargear.iter().any(|w| w == "4x Multi-melta"),
            "Should show 4x Multi-melta: {:?}",
            ret.wargear
        );
        assert!(
            ret.wargear.iter().any(|w| w.contains("Power weapon")),
            "Should have Power weapon: {:?}",
            ret.wargear
        );
    }

    #[test]
    fn test_parse_multi_level_wargear_paragons() {
        let raw = "Paragon Warsuits (210 points)\n  \u{2022} 1x Paragon Superior\n    \u{2022} 1x Bolt pistol\n      1x Multi-melta\n      1x Paragon grenade launchers\n      1x Paragon war mace\n  \u{2022} 2x Paragon\n    \u{2022} 2x Bolt pistol\n      2x Multi-melta\n      2x Paragon grenade launchers\n      2x Paragon war mace\n";
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 1);
        let para = &units[0];
        assert_eq!(para.name, "Paragon Warsuits");
        assert_eq!(para.count, 3, "Should be 3 models (1 superior + 2)");
        assert!(
            para.wargear.iter().any(|w| w.contains("war mace")),
            "Should show Paragon war mace: {:?}",
            para.wargear
        );
        assert!(
            para.wargear.iter().any(|w| w.contains("Multi-melta")),
            "Should show Multi-melta: {:?}",
            para.wargear
        );
    }

    #[test]
    fn test_parse_inline_wargear_after_points() {
        // Russian format with wargear on same line after points
        let raw = "Retributor Squad (5) (120 pts) 4x Multi-melta, 1x Inferno pistol, 1x Power weapon\nParagon Warsuits (3) (210 pts) 3x Multi-melta, 3x Paragon war mace\n";
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 2, "Expected 2 units, got {:?}", units);

        let ret = &units[0];
        assert_eq!(ret.name, "Retributor Squad");
        assert_eq!(ret.count, 5, "Count from (5) in name");
        assert!(
            ret.wargear.iter().any(|w| w == "4x Multi-melta"),
            "Should have 4x Multi-melta: {:?}",
            ret.wargear
        );
        assert!(
            ret.wargear.iter().any(|w| w == "Inferno pistol"),
            "Should have Inferno pistol: {:?}",
            ret.wargear
        );

        let para = &units[1];
        assert_eq!(para.name, "Paragon Warsuits");
        assert_eq!(para.count, 3, "Count from (3) in name");
        assert!(
            para.wargear.iter().any(|w| w == "3x Multi-melta"),
            "Should have 3x Multi-melta: {:?}",
            para.wargear
        );
        assert!(
            para.wargear.iter().any(|w| w.contains("war mace")),
            "Should have war mace: {:?}",
            para.wargear
        );
    }

    #[test]
    fn test_parse_flat_wargear_single_model() {
        // Flat format: single-model unit, all gear at same indent
        let raw = "Castellan Crowe (90 points)\n  \u{2022} 1x Black Blade of Antwyr\n  \u{2022} 1x Storm bolter\n";
        let units = parse_units_from_raw_text(raw);
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].count, 1, "Single model, count should stay 1");
        assert_eq!(
            units[0].wargear,
            vec!["Black Blade of Antwyr", "Storm bolter"]
        );
    }
}
