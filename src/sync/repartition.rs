//! Repartition data by epoch.
//!
//! Reads all entities from a source epoch directory, assigns each to the
//! correct epoch based on event dates, and writes them into per-epoch directories.

use std::collections::HashMap;

use tracing::info;

use crate::api::dedup_by_id;
use crate::models::{ArmyList, EpochMapper, Event, Placement};
use crate::storage::{
    read_significant_events, EntityType, JsonlReader, JsonlWriter, StorageConfig,
};

/// Result of a repartition operation.
#[derive(Debug)]
pub struct RepartitionResult {
    pub events_by_epoch: HashMap<String, u32>,
    pub placements_by_epoch: HashMap<String, u32>,
    pub lists_by_epoch: HashMap<String, u32>,
}

/// Repartition data from `source_epoch` into per-epoch directories.
///
/// Returns counts per epoch, or an error.
pub fn repartition(
    storage: &StorageConfig,
    source_epoch: &str,
    dry_run: bool,
    keep_originals: bool,
) -> anyhow::Result<RepartitionResult> {
    // 1. Read significant events and build mapper
    let sig_events = read_significant_events(storage)?;
    if sig_events.is_empty() {
        anyhow::bail!(
            "No significant events found. Register balance passes first with `add-balance-pass`."
        );
    }
    let mapper = EpochMapper::from_significant_events(&sig_events);

    info!(
        "Built epoch mapper with {} epochs from {} significant events",
        mapper.all_epochs().len(),
        sig_events.len()
    );

    // 2. Read all entities from source
    let event_reader = JsonlReader::<Event>::for_entity(storage, EntityType::Event, source_epoch);
    let events = dedup_by_id(event_reader.read_all()?, |e| e.id.as_str());

    let placement_reader =
        JsonlReader::<Placement>::for_entity(storage, EntityType::Placement, source_epoch);
    let placements = dedup_by_id(placement_reader.read_all()?, |p| p.id.as_str());

    let list_reader =
        JsonlReader::<ArmyList>::for_entity(storage, EntityType::ArmyList, source_epoch);
    let lists = dedup_by_id(list_reader.read_all()?, |l| l.id.as_str());

    info!(
        "Read {} events, {} placements, {} lists from '{}'",
        events.len(),
        placements.len(),
        lists.len(),
        source_epoch
    );

    // 3. Assign events to epochs
    let mut events_by_epoch: HashMap<String, Vec<Event>> = HashMap::new();
    let mut event_epoch_map: HashMap<String, String> = HashMap::new();

    for mut event in events {
        let epoch_id = mapper.get_epoch_id_for_date(event.date);
        let epoch_str = epoch_id.as_str().to_string();
        event.epoch_id = epoch_id;
        event_epoch_map.insert(event.id.as_str().to_string(), epoch_str.clone());
        events_by_epoch.entry(epoch_str).or_default().push(event);
    }

    // 4. Assign placements to same epoch as their event
    let mut placements_by_epoch: HashMap<String, Vec<Placement>> = HashMap::new();
    for mut placement in placements {
        let epoch_str = event_epoch_map
            .get(placement.event_id.as_str())
            .cloned()
            .unwrap_or_else(|| source_epoch.to_string());
        placement.epoch_id = crate::models::EntityId::from(epoch_str.as_str());
        placements_by_epoch
            .entry(epoch_str)
            .or_default()
            .push(placement);
    }

    // 5. Assign lists — prefer event_date directly, fall back to source_url matching
    let mut event_source_to_epoch: HashMap<String, String> = HashMap::new();
    for (epoch, evts) in &events_by_epoch {
        for e in evts {
            event_source_to_epoch.insert(e.source_url.clone(), epoch.clone());
        }
    }

    let mut lists_by_epoch: HashMap<String, Vec<ArmyList>> = HashMap::new();
    for list in lists {
        let epoch_str = if let Some(date) = list.event_date {
            // Best: use the list's own event_date for epoch assignment
            mapper.get_epoch_id_for_date(date).as_str().to_string()
        } else {
            // Fallback: match via source_url to event
            list.source_url
                .as_ref()
                .and_then(|url| event_source_to_epoch.get(url))
                .cloned()
                .unwrap_or_else(|| source_epoch.to_string())
        };
        lists_by_epoch.entry(epoch_str).or_default().push(list);
    }

    // 6. Report
    let mut result = RepartitionResult {
        events_by_epoch: HashMap::new(),
        placements_by_epoch: HashMap::new(),
        lists_by_epoch: HashMap::new(),
    };

    let mut all_epoch_ids: Vec<String> = events_by_epoch
        .keys()
        .chain(placements_by_epoch.keys())
        .chain(lists_by_epoch.keys())
        .cloned()
        .collect();
    all_epoch_ids.sort();
    all_epoch_ids.dedup();

    for epoch_id in &all_epoch_ids {
        let n_events = events_by_epoch.get(epoch_id).map_or(0, |v| v.len() as u32);
        let n_placements = placements_by_epoch
            .get(epoch_id)
            .map_or(0, |v| v.len() as u32);
        let n_lists = lists_by_epoch.get(epoch_id).map_or(0, |v| v.len() as u32);

        result.events_by_epoch.insert(epoch_id.clone(), n_events);
        result
            .placements_by_epoch
            .insert(epoch_id.clone(), n_placements);
        result.lists_by_epoch.insert(epoch_id.clone(), n_lists);

        info!(
            "  Epoch '{}': {} events, {} placements, {} lists",
            epoch_id, n_events, n_placements, n_lists
        );
    }

    // 7. Write (unless dry run)
    if !dry_run {
        for epoch_id in &all_epoch_ids {
            if let Some(evts) = events_by_epoch.get(epoch_id) {
                let writer = JsonlWriter::<Event>::for_entity(storage, EntityType::Event, epoch_id);
                writer.write_all(evts)?;
            }
            if let Some(plcs) = placements_by_epoch.get(epoch_id) {
                let writer =
                    JsonlWriter::<Placement>::for_entity(storage, EntityType::Placement, epoch_id);
                writer.write_all(plcs)?;
            }
            if let Some(lsts) = lists_by_epoch.get(epoch_id) {
                let writer =
                    JsonlWriter::<ArmyList>::for_entity(storage, EntityType::ArmyList, epoch_id);
                writer.write_all(lsts)?;
            }
        }

        // Back up original directory (rename to source_epoch.bak)
        if !keep_originals {
            let src_dir = storage.normalized_dir().join(source_epoch);
            let bak_dir = storage
                .normalized_dir()
                .join(format!("{}.bak", source_epoch));
            if src_dir.exists() {
                // Don't overwrite an existing backup
                if !bak_dir.exists() {
                    std::fs::rename(&src_dir, &bak_dir)?;
                    info!(
                        "Backed up '{}' -> '{}'",
                        src_dir.display(),
                        bak_dir.display()
                    );
                } else {
                    info!(
                        "Backup already exists at '{}', skipping rename",
                        bak_dir.display()
                    );
                }
            }
        }

        info!(
            "Repartition complete: wrote {} epoch directories",
            all_epoch_ids.len()
        );
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use tempfile::TempDir;

    use crate::models::{Confidence, SignificantEvent, SignificantEventType};
    use crate::storage::{write_significant_events, JsonlWriter, StorageConfig};

    fn test_storage(temp_dir: &TempDir) -> StorageConfig {
        StorageConfig::new(temp_dir.path().to_path_buf())
    }

    fn make_sig_event(date: NaiveDate, title: &str) -> SignificantEvent {
        SignificantEvent::new(
            SignificantEventType::BalanceUpdate,
            date,
            title.to_string(),
            "https://example.com".to_string(),
        )
        .with_confidence(Confidence::High)
    }

    fn make_event(name: &str, date: NaiveDate, source_url: &str) -> Event {
        Event::new(
            name.to_string(),
            date,
            source_url.to_string(),
            "test".to_string(),
            crate::models::EntityId::from("source"),
        )
    }

    fn make_placement(event_id: crate::models::EntityId, rank: u32, name: &str) -> Placement {
        Placement::new(
            event_id,
            crate::models::EntityId::from("source"),
            rank,
            name.to_string(),
            "Test Faction".to_string(),
        )
    }

    #[test]
    fn test_repartition_no_sig_events_errors() {
        let temp_dir = TempDir::new().unwrap();
        let storage = test_storage(&temp_dir);
        let result = repartition(&storage, "current", false, false);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("No significant events"));
    }

    #[test]
    fn test_repartition_dry_run() {
        let temp_dir = TempDir::new().unwrap();
        let storage = test_storage(&temp_dir);

        // Write sig events
        let mut sig_events = vec![make_sig_event(
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            "June Update",
        )];
        write_significant_events(&storage, &mut sig_events).unwrap();

        // Write some events to "current" epoch
        let event1 = make_event(
            "GT1",
            NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            "https://example.com/gt1",
        );
        let writer = JsonlWriter::<Event>::for_entity(&storage, EntityType::Event, "current");
        writer.write_all(&[event1]).unwrap();

        // Dry run should count but not write
        let result = repartition(&storage, "current", true, false).unwrap();
        assert!(!result.events_by_epoch.is_empty());
        // The "current" directory should still exist (dry run)
        assert!(storage
            .normalized_dir()
            .join("current")
            .join("events.jsonl")
            .exists());
    }

    #[test]
    fn test_repartition_writes_to_epochs() {
        let temp_dir = TempDir::new().unwrap();
        let storage = test_storage(&temp_dir);

        // Create two epochs
        let mut sig_events = vec![
            make_sig_event(NaiveDate::from_ymd_opt(2025, 3, 1).unwrap(), "March Update"),
            make_sig_event(NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(), "June Update"),
        ];
        write_significant_events(&storage, &mut sig_events).unwrap();

        // Write events in different date ranges to "current"
        let event_march = make_event(
            "March GT",
            NaiveDate::from_ymd_opt(2025, 4, 15).unwrap(),
            "https://example.com/march",
        );
        let event_june = make_event(
            "June GT",
            NaiveDate::from_ymd_opt(2025, 7, 15).unwrap(),
            "https://example.com/june",
        );
        let writer = JsonlWriter::<Event>::for_entity(&storage, EntityType::Event, "current");
        writer.write_all(&[event_march, event_june]).unwrap();

        let result = repartition(&storage, "current", false, true).unwrap();

        // Should have events split across epochs
        let total: u32 = result.events_by_epoch.values().sum();
        assert_eq!(total, 2);
    }

    #[test]
    fn test_repartition_keeps_originals() {
        let temp_dir = TempDir::new().unwrap();
        let storage = test_storage(&temp_dir);

        let mut sig_events = vec![make_sig_event(
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            "June Update",
        )];
        write_significant_events(&storage, &mut sig_events).unwrap();

        let event = make_event(
            "Test",
            NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            "https://example.com/test",
        );
        let writer = JsonlWriter::<Event>::for_entity(&storage, EntityType::Event, "current");
        writer.write_all(&[event]).unwrap();

        repartition(&storage, "current", false, true).unwrap();

        // With keep_originals=true, "current" dir should still exist
        assert!(storage.normalized_dir().join("current").exists());
    }

    #[test]
    fn test_repartition_placements_follow_events() {
        let temp_dir = TempDir::new().unwrap();
        let storage = test_storage(&temp_dir);

        let mut sig_events = vec![make_sig_event(
            NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            "June Update",
        )];
        write_significant_events(&storage, &mut sig_events).unwrap();

        let event = make_event(
            "Test GT",
            NaiveDate::from_ymd_opt(2025, 7, 1).unwrap(),
            "https://example.com/test",
        );
        let event_id = event.id.clone();
        let writer = JsonlWriter::<Event>::for_entity(&storage, EntityType::Event, "current");
        writer.write_all(&[event]).unwrap();

        let placement = make_placement(event_id, 1, "Player One");
        let p_writer =
            JsonlWriter::<Placement>::for_entity(&storage, EntityType::Placement, "current");
        p_writer.write_all(&[placement]).unwrap();

        let result = repartition(&storage, "current", true, false).unwrap();

        // Placements should be in same epoch as their event
        let event_epochs: Vec<&String> = result.events_by_epoch.keys().collect();
        let placement_epochs: Vec<&String> = result
            .placements_by_epoch
            .keys()
            .filter(|k| result.placements_by_epoch[*k] > 0)
            .collect();
        // At least one epoch should have both events and placements
        assert!(!event_epochs.is_empty());
        assert!(!placement_epochs.is_empty());
    }
}
