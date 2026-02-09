//! Ingestion pipeline for testing.
//!
//! Provides functions to test the full ingestion flow with real or mock AI backends.

use std::sync::Arc;

use chrono::NaiveDate;
use tracing::{error, info, warn};

use crate::agents::backend::{AiBackend, ChatRequest};
use crate::agents::balance_watcher::{BalanceWatcherAgent, BalanceWatcherInput};
use crate::agents::event_scout::{EventScoutAgent, EventScoutInput};
use crate::agents::result_harvester::{ResultHarvesterAgent, ResultHarvesterInput};
use crate::agents::Agent;

/// Result of an ingestion test.
#[derive(Debug)]
pub struct IngestResult {
    pub events_found: usize,
    pub placements_found: usize,
    pub lists_found: usize,
    pub errors: Vec<String>,
}

/// Test ingestion from a local HTML fixture.
pub async fn ingest_from_fixture(
    fixture_path: &str,
    backend: Arc<dyn AiBackend>,
) -> Result<IngestResult, String> {
    info!("Testing ingestion from fixture: {}", fixture_path);

    let content = std::fs::read_to_string(fixture_path)
        .map_err(|e| format!("Failed to read fixture: {}", e))?;

    let mut result = IngestResult {
        events_found: 0,
        placements_found: 0,
        lists_found: 0,
        errors: Vec::new(),
    };

    // Run Event Scout
    let event_scout = EventScoutAgent::new(backend.clone());
    let scout_input = EventScoutInput {
        article_html: content.clone(),
        article_url: format!("file://{}", fixture_path),
        article_date: NaiveDate::from_ymd_opt(2025, 6, 23).unwrap(),
    };

    match event_scout.execute(scout_input).await {
        Ok(output) => {
            result.events_found = output.events.len();
            info!("Event Scout found {} events", output.events.len());

            for event in &output.events {
                info!(
                    "  - {} ({:?} players, confidence: {:?})",
                    event.data.name, event.data.player_count, event.confidence
                );

                // Run Result Harvester for each event
                let harvester = ResultHarvesterAgent::new(backend.clone());
                let harvest_input = ResultHarvesterInput {
                    article_html: content.clone(),
                    event_stub: event.data.clone(),
                };

                match harvester.execute(harvest_input).await {
                    Ok(harvest_output) => {
                        result.placements_found += harvest_output.placements.len();
                        result.lists_found += harvest_output.raw_lists.len();

                        info!(
                            "    Found {} placements, {} lists",
                            harvest_output.placements.len(),
                            harvest_output.raw_lists.len()
                        );

                        for placement in &harvest_output.placements {
                            info!(
                                "      #{} {} - {} ({:?})",
                                placement.data.rank,
                                placement.data.player_name,
                                placement.data.faction,
                                placement.confidence
                            );
                        }
                    }
                    Err(e) => {
                        let err = format!("Result Harvester error: {}", e);
                        warn!("{}", err);
                        result.errors.push(err);
                    }
                }
            }
        }
        Err(e) => {
            let err = format!("Event Scout error: {}", e);
            error!("{}", err);
            result.errors.push(err);
        }
    }

    Ok(result)
}

/// Test balance watcher from a fixture.
pub async fn ingest_balance_update(
    fixture_path: &str,
    backend: Arc<dyn AiBackend>,
) -> Result<IngestResult, String> {
    info!("Testing balance ingestion from: {}", fixture_path);

    let content = std::fs::read_to_string(fixture_path)
        .map_err(|e| format!("Failed to read fixture: {}", e))?;

    let mut result = IngestResult {
        events_found: 0,
        placements_found: 0,
        lists_found: 0,
        errors: Vec::new(),
    };

    let watcher = BalanceWatcherAgent::new(backend);
    let input = BalanceWatcherInput {
        html_content: content,
        source_url: format!("file://{}", fixture_path),
        known_event_ids: vec![],
    };

    match watcher.execute(input).await {
        Ok(output) => {
            result.events_found = output.events.len();
            info!("Balance Watcher found {} events", output.events.len());

            for event in &output.events {
                info!(
                    "  - {} (date: {}, confidence: {:?})",
                    event.data.title, event.data.date, event.confidence
                );
                if let Some(pdf) = &event.data.pdf_url {
                    info!("    PDF: {}", pdf);
                }
            }
        }
        Err(e) => {
            let err = format!("Balance Watcher error: {}", e);
            error!("{}", err);
            result.errors.push(err);
        }
    }

    Ok(result)
}

/// Check if AI backend is available.
pub async fn check_backend(backend: &dyn AiBackend) -> bool {
    match backend.health_check().await {
        Ok(healthy) => {
            if healthy {
                info!("AI backend '{}' is healthy", backend.name());
            } else {
                warn!("AI backend '{}' health check failed", backend.name());
            }
            healthy
        }
        Err(e) => {
            error!("AI backend error: {}", e);
            false
        }
    }
}

/// Create a mock backend that returns pre-defined responses for testing.
pub struct TestMockBackend {
    event_response: String,
    placement_response: String,
    balance_response: String,
}

impl TestMockBackend {
    pub fn new() -> Self {
        Self {
            event_response: r#"{
                "events": [
                    {
                        "name": "London GT 2025",
                        "date": "2025-06-15",
                        "location": "London, UK",
                        "player_count": 96,
                        "round_count": 5,
                        "event_type": "GT",
                        "article_section": "London GT Results",
                        "confidence": "high"
                    },
                    {
                        "name": "Birmingham Open",
                        "date": null,
                        "location": "Birmingham, UK",
                        "player_count": 48,
                        "round_count": null,
                        "event_type": "Open",
                        "article_section": null,
                        "confidence": "medium"
                    }
                ]
            }"#
            .to_string(),
            placement_response: r#"{
                "placements": [
                    {
                        "rank": 1,
                        "player_name": "John Smith",
                        "faction": "Aeldari",
                        "subfaction": "Ynnari",
                        "detachment": "Soulrender",
                        "wins": 5,
                        "losses": 0,
                        "draws": 0,
                        "battle_points": 94,
                        "army_list": "++ Battalion ++\nYvraine [120pts]\nWraithguard x5 [180pts]",
                        "confidence": "high"
                    },
                    {
                        "rank": 2,
                        "player_name": "Jane Doe",
                        "faction": "Space Marines",
                        "subfaction": "Ultramarines",
                        "detachment": "Gladius Task Force",
                        "wins": 4,
                        "losses": 1,
                        "draws": 0,
                        "battle_points": 85,
                        "army_list": null,
                        "confidence": "high"
                    },
                    {
                        "rank": 3,
                        "player_name": "Bob Wilson",
                        "faction": "Death Guard",
                        "subfaction": null,
                        "detachment": "Plague Company",
                        "wins": 4,
                        "losses": 1,
                        "draws": 0,
                        "battle_points": 82,
                        "army_list": null,
                        "confidence": "medium"
                    }
                ]
            }"#
            .to_string(),
            balance_response: r#"{
                "updates": [
                    {
                        "title": "Balance Dataslate Spring 2025",
                        "date": "2025-03-15",
                        "event_type": "balance_update",
                        "pdf_url": "https://www.warhammer-community.com/wp-content/uploads/2025/03/balance-dataslate-spring-2025.pdf",
                        "summary": "Major changes to Aeldari, Space Marines adjustments",
                        "confidence": "high"
                    }
                ]
            }"#
            .to_string(),
        }
    }
}

impl Default for TestMockBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AiBackend for TestMockBackend {
    fn name(&self) -> &'static str {
        "test_mock"
    }

    async fn chat(
        &self,
        request: ChatRequest,
    ) -> Result<crate::agents::ChatResponse, crate::agents::AgentError> {
        // Determine which response to return based on the prompt content
        let prompt = request
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase();

        // Check for balance watcher prompts
        let response = if prompt.contains("balance dataslate")
            || prompt.contains("warhammer community")
            || prompt.contains("balance updates")
        {
            &self.balance_response
        // Check for result harvester prompts (more specific than event scout)
        } else if prompt.contains("tournament results")
            || prompt.contains("placing player")
            || prompt.contains("player_name")
        {
            &self.placement_response
        // Default to event scout
        } else {
            &self.event_response
        };

        Ok(crate::agents::ChatResponse {
            content: response.clone(),
            model: "test_mock".to_string(),
            tokens_used: None,
        })
    }

    async fn health_check(&self) -> Result<bool, crate::agents::AgentError> {
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_ingest_fixture_with_mock() {
        let backend: Arc<dyn AiBackend> = Arc::new(TestMockBackend::new());

        let result = ingest_from_fixture(
            "tests/fixtures/goonhammer_sample.html",
            backend,
        )
        .await
        .unwrap();

        assert_eq!(result.events_found, 2);
        assert!(result.placements_found > 0);
        assert!(result.errors.is_empty());
    }

    #[tokio::test]
    async fn test_balance_ingest_with_mock() {
        let backend: Arc<dyn AiBackend> = Arc::new(TestMockBackend::new());

        let result = ingest_balance_update(
            "tests/fixtures/warhammer_community_balance.html",
            backend,
        )
        .await
        .unwrap();

        assert_eq!(result.events_found, 1);
        assert!(result.errors.is_empty());
    }
}
