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
    let event_reader =
        JsonlReader::<Event>::for_entity(storage, EntityType::Event, source_epoch);
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
        events_by_epoch
            .entry(epoch_str)
            .or_default()
            .push(event);
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
        lists_by_epoch
            .entry(epoch_str)
            .or_default()
            .push(list);
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

        result
            .events_by_epoch
            .insert(epoch_id.clone(), n_events);
        result
            .placements_by_epoch
            .insert(epoch_id.clone(), n_placements);
        result
            .lists_by_epoch
            .insert(epoch_id.clone(), n_lists);

        info!(
            "  Epoch '{}': {} events, {} placements, {} lists",
            epoch_id, n_events, n_placements, n_lists
        );
    }

    // 7. Write (unless dry run)
    if !dry_run {
        for epoch_id in &all_epoch_ids {
            if let Some(evts) = events_by_epoch.get(epoch_id) {
                let writer =
                    JsonlWriter::<Event>::for_entity(storage, EntityType::Event, epoch_id);
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
                    info!("Backed up '{}' -> '{}'", src_dir.display(), bak_dir.display());
                } else {
                    info!("Backup already exists at '{}', skipping rename", bak_dir.display());
                }
            }
        }

        info!("Repartition complete: wrote {} epoch directories", all_epoch_ids.len());
    }

    Ok(result)
}
