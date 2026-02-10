use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::api::state::AppState;
use crate::api::ApiError;
use crate::models::Event;
use crate::storage::{EntityType, JsonlReader};

#[derive(Debug, Serialize)]
pub struct Epoch {
    pub id: String,
    pub label: String,
    pub is_current: bool,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub event_count: u32,
}

#[derive(Debug, Serialize)]
pub struct EpochsResponse {
    pub epochs: Vec<Epoch>,
}

pub async fn list_epochs(
    State(state): State<AppState>,
) -> Result<Json<EpochsResponse>, ApiError> {
    let mapper = &state.epoch_mapper;

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
            Epoch {
                id: epoch_id.to_string(),
                label: e.name.clone(),
                is_current: e.is_current,
                start_date: Some(e.start_date.to_string()),
                end_date: e.end_date.map(|d| d.to_string()),
                event_count: count,
            }
        })
        .collect();

    Ok(Json(EpochsResponse { epochs }))
}
