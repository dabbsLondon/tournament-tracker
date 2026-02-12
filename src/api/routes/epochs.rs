use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;

use crate::api::state::AppState;
use crate::api::ApiError;
use crate::models::{BalanceChanges, Event, SignificantEventType};
use crate::storage::{self, EntityType, JsonlReader};

#[derive(Debug, Serialize)]
pub struct Epoch {
    pub id: String,
    pub label: String,
    pub is_current: bool,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub event_count: u32,
    pub balance_pass_id: Option<String>,
    pub balance_pass_title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EpochsResponse {
    pub epochs: Vec<Epoch>,
}

pub async fn list_epochs(
    State(state): State<AppState>,
) -> Result<Json<EpochsResponse>, ApiError> {
    let mapper = &state.epoch_mapper;

    // Load significant events for balance pass info
    let sig_events = storage::read_significant_events(&state.storage).unwrap_or_default();

    if mapper.all_epochs().is_empty() {
        let count = JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, "current")
            .read_all()
            .map(|v| v.len() as u32)
            .unwrap_or(0);
        return Ok(Json(EpochsResponse {
            epochs: vec![Epoch {
                id: "current".to_string(),
                label: "Current Meta".to_string(),
                is_current: true,
                start_date: None,
                end_date: None,
                event_count: count,
                balance_pass_id: None,
                balance_pass_title: None,
            }],
        }));
    }

    let epochs = mapper
        .all_epochs()
        .iter()
        .map(|e| {
            let epoch_id = e.id.as_str();
            let count =
                JsonlReader::<Event>::for_entity(&state.storage, EntityType::Event, epoch_id)
                    .read_all()
                    .map(|v| v.len() as u32)
                    .unwrap_or(0);

            // Find the balance pass that started this epoch
            let balance_pass = sig_events.iter().find(|se| se.id == e.start_event_id);

            Epoch {
                id: epoch_id.to_string(),
                label: e.name.clone(),
                is_current: e.is_current,
                start_date: Some(e.start_date.to_string()),
                end_date: e.end_date.map(|d| d.to_string()),
                event_count: count,
                balance_pass_id: balance_pass.map(|bp| bp.id.as_str().to_string()),
                balance_pass_title: balance_pass.map(|bp| bp.title.clone()),
            }
        })
        .collect();

    Ok(Json(EpochsResponse { epochs }))
}

// ── Balance Pass Endpoints ──────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct BalancePassSummary {
    pub id: String,
    pub title: String,
    pub date: String,
    pub source_url: String,
    pub summary: Option<String>,
    pub has_details: bool,
}

#[derive(Debug, Serialize)]
pub struct BalancePassListResponse {
    pub balance_passes: Vec<BalancePassSummary>,
}

pub async fn list_balance_passes(
    State(state): State<AppState>,
) -> Result<Json<BalancePassListResponse>, ApiError> {
    let sig_events = storage::read_significant_events(&state.storage)
        .map_err(|e| ApiError::Internal(format!("Failed to read significant events: {}", e)))?;

    let balance_passes = sig_events
        .iter()
        .filter(|e| e.event_type == SignificantEventType::BalanceUpdate)
        .map(|e| BalancePassSummary {
            id: e.id.as_str().to_string(),
            title: e.title.clone(),
            date: e.date.to_string(),
            source_url: e.source_url.clone(),
            summary: e.summary.clone(),
            has_details: e.changes.is_some(),
        })
        .collect();

    Ok(Json(BalancePassListResponse { balance_passes }))
}

#[derive(Debug, Serialize)]
pub struct BalancePassDetail {
    pub id: String,
    pub title: String,
    pub date: String,
    pub source_url: String,
    pub pdf_url: Option<String>,
    pub summary: Option<String>,
    pub changes: Option<BalanceChanges>,
}

pub async fn get_balance_pass(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<BalancePassDetail>, ApiError> {
    let sig_events = storage::read_significant_events(&state.storage)
        .map_err(|e| ApiError::Internal(format!("Failed to read significant events: {}", e)))?;

    let event = sig_events
        .iter()
        .find(|e| e.id.as_str() == id && e.event_type == SignificantEventType::BalanceUpdate)
        .ok_or_else(|| ApiError::NotFound(format!("Balance pass not found: {}", id)))?;

    Ok(Json(BalancePassDetail {
        id: event.id.as_str().to_string(),
        title: event.title.clone(),
        date: event.date.to_string(),
        source_url: event.source_url.clone(),
        pdf_url: event.pdf_url.clone(),
        summary: event.summary.clone(),
        changes: event.changes.clone(),
    }))
}

#[cfg(test)]
mod tests {
    use crate::api::build_router;
    use crate::api::state::AppState;
    use crate::models::{
        BalanceChanges, EpochMapper, Event, FactionChange, SignificantEvent,
        SignificantEventType,
    };
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
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        (status, json)
    }

    fn make_balance_pass(title: &str, date: &str, with_changes: bool) -> SignificantEvent {
        let mut event = SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").unwrap(),
            title.to_string(),
            format!("https://example.com/{}", title.to_lowercase().replace(' ', "-")),
        );
        if with_changes {
            event.summary = Some("Test balance update summary".to_string());
            event.changes = Some(BalanceChanges {
                core_rules: vec!["Deep Strike changed".to_string()],
                faction_changes: vec![FactionChange {
                    faction: "Aeldari".to_string(),
                    direction: "nerf".to_string(),
                    summary: "Points increases".to_string(),
                    points_changes: vec![],
                    rules_changes: vec!["Star Engines reworked".to_string()],
                    new_detachments: vec![],
                }],
            });
        }
        event
    }

    fn setup_with_balance_passes(dir: &std::path::Path, passes: &[SignificantEvent]) -> AppState {
        let storage = StorageConfig::new(dir.to_path_buf());
        std::fs::create_dir_all(dir.join("normalized")).unwrap();
        write_jsonl(&dir.join("normalized").join("significant_events.jsonl"), passes);

        let mapper = EpochMapper::from_significant_events(passes);

        // Create epoch directories so event counts can be read
        for epoch in mapper.all_epochs() {
            let epoch_dir = dir.join("normalized").join(epoch.id.as_str());
            std::fs::create_dir_all(&epoch_dir).unwrap();
            write_jsonl(&epoch_dir.join("events.jsonl"), &Vec::<Event>::new());
        }

        AppState {
            storage: Arc::new(storage),
            epoch_mapper: Arc::new(mapper),
        }
    }

    // ── Balance Pass List Tests ──────────────────────────────────

    #[tokio::test]
    async fn test_list_balance_passes() {
        let tmp = tempfile::tempdir().unwrap();
        let passes = vec![
            make_balance_pass("Dataslate January 2025", "2025-01-20", false),
            make_balance_pass("Dataslate December 2025", "2025-12-11", true),
        ];
        let state = setup_with_balance_passes(tmp.path(), &passes);
        let app = build_router(state);

        let (status, json) = get_json(app, "/api/balance").await;

        assert_eq!(status, StatusCode::OK);
        let bps = json["balance_passes"].as_array().unwrap();
        assert_eq!(bps.len(), 2);
        assert_eq!(bps[0]["has_details"], false);
        assert_eq!(bps[1]["has_details"], true);
        assert_eq!(bps[1]["title"], "Dataslate December 2025");
        assert!(bps[1]["summary"].is_string());
    }

    #[tokio::test]
    async fn test_get_balance_pass_with_details() {
        let tmp = tempfile::tempdir().unwrap();
        let pass = make_balance_pass("Dataslate December 2025", "2025-12-11", true);
        let pass_id = pass.id.as_str().to_string();
        let state = setup_with_balance_passes(tmp.path(), &[pass]);
        let app = build_router(state);

        let (status, json) = get_json(app, &format!("/api/balance/{}", pass_id)).await;

        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["title"], "Dataslate December 2025");
        assert!(json["changes"].is_object());
        let changes = &json["changes"];
        assert_eq!(changes["core_rules"].as_array().unwrap().len(), 1);
        assert_eq!(changes["faction_changes"].as_array().unwrap().len(), 1);
        assert_eq!(changes["faction_changes"][0]["faction"], "Aeldari");
        assert_eq!(changes["faction_changes"][0]["direction"], "nerf");
    }

    #[tokio::test]
    async fn test_get_balance_pass_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let state = setup_with_balance_passes(tmp.path(), &[
            make_balance_pass("Dataslate January 2025", "2025-01-20", false),
        ]);
        let app = build_router(state);

        let (status, _) = get_json(app, "/api/balance/nonexistent").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_get_balance_pass_without_details() {
        let tmp = tempfile::tempdir().unwrap();
        let pass = make_balance_pass("Dataslate January 2025", "2025-01-20", false);
        let pass_id = pass.id.as_str().to_string();
        let state = setup_with_balance_passes(tmp.path(), &[pass]);
        let app = build_router(state);

        let (status, json) = get_json(app, &format!("/api/balance/{}", pass_id)).await;

        assert_eq!(status, StatusCode::OK);
        assert!(json["changes"].is_null(), "Pass without details should have null changes");
    }

    // ── Epoch List Tests ─────────────────────────────────────────

    #[tokio::test]
    async fn test_list_epochs_with_balance_pass_info() {
        let tmp = tempfile::tempdir().unwrap();
        let passes = vec![
            make_balance_pass("Dataslate December 2025", "2025-12-11", true),
            make_balance_pass("Dataslate January 2026", "2026-01-07", false),
        ];
        let state = setup_with_balance_passes(tmp.path(), &passes);
        let app = build_router(state);

        let (status, json) = get_json(app, "/api/epochs").await;

        assert_eq!(status, StatusCode::OK);
        let epochs = json["epochs"].as_array().unwrap();
        assert_eq!(epochs.len(), 2);

        // First epoch should reference December balance pass
        assert_eq!(epochs[0]["balance_pass_title"], "Dataslate December 2025");
        assert!(epochs[0]["balance_pass_id"].is_string());
        assert_eq!(epochs[0]["is_current"], false);

        // Second epoch should be current and reference January pass
        assert_eq!(epochs[1]["balance_pass_title"], "Dataslate January 2026");
        assert_eq!(epochs[1]["is_current"], true);
    }

    #[tokio::test]
    async fn test_list_epochs_empty_falls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let storage = StorageConfig::new(tmp.path().to_path_buf());
        let epoch_dir = tmp.path().join("normalized").join("current");
        std::fs::create_dir_all(&epoch_dir).unwrap();
        write_jsonl(&epoch_dir.join("events.jsonl"), &Vec::<Event>::new());

        let state = AppState {
            storage: Arc::new(storage),
            epoch_mapper: Arc::new(EpochMapper::new()),
        };
        let app = build_router(state);

        let (status, json) = get_json(app, "/api/epochs").await;

        assert_eq!(status, StatusCode::OK);
        let epochs = json["epochs"].as_array().unwrap();
        assert_eq!(epochs.len(), 1);
        assert_eq!(epochs[0]["id"], "current");
        assert_eq!(epochs[0]["label"], "Current Meta");
    }
}
