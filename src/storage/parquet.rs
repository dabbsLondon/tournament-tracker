//! Parquet storage for analytics.
//!
//! Parquet files are derived from JSONL for fast analytical queries.
//! They are rebuilt from source JSONL when needed.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::array::{ArrayRef, StringArray, TimestampMillisecondArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
use arrow::record_batch::RecordBatch;
use chrono::{DateTime, NaiveDate, Utc};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use tracing::{debug, info};

use super::{StorageConfig, StorageError};

/// Parquet table types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableType {
    Events,
    Placements,
    FactionStats,
}

impl TableType {
    /// Get the filename for this table.
    pub fn filename(&self) -> &'static str {
        match self {
            TableType::Events => "events.parquet",
            TableType::Placements => "placements.parquet",
            TableType::FactionStats => "faction_stats.parquet",
        }
    }
}

/// Schema definitions for Parquet tables.
pub mod schemas {
    use super::*;

    /// Schema for events table.
    pub fn events_schema() -> Schema {
        Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("date", DataType::Utf8, true),
            Field::new("location", DataType::Utf8, true),
            Field::new("player_count", DataType::UInt32, true),
            Field::new("round_count", DataType::UInt32, true),
            Field::new("event_type", DataType::Utf8, true),
            Field::new("epoch_id", DataType::Utf8, false),
            Field::new(
                "created_at",
                DataType::Timestamp(TimeUnit::Millisecond, None),
                false,
            ),
        ])
    }

    /// Schema for placements table.
    pub fn placements_schema() -> Schema {
        Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("event_id", DataType::Utf8, false),
            Field::new("rank", DataType::UInt32, false),
            Field::new("player_name", DataType::Utf8, false),
            Field::new("faction", DataType::Utf8, false),
            Field::new("subfaction", DataType::Utf8, true),
            Field::new("detachment", DataType::Utf8, true),
            Field::new("wins", DataType::UInt32, true),
            Field::new("losses", DataType::UInt32, true),
            Field::new("draws", DataType::UInt32, true),
            Field::new("battle_points", DataType::UInt32, true),
            Field::new("epoch_id", DataType::Utf8, false),
        ])
    }

    /// Schema for faction stats table.
    pub fn faction_stats_schema() -> Schema {
        Schema::new(vec![
            Field::new("faction", DataType::Utf8, false),
            Field::new("epoch_id", DataType::Utf8, false),
            Field::new("player_count", DataType::UInt32, false),
            Field::new("games_played", DataType::UInt32, false),
            Field::new("wins", DataType::UInt32, false),
            Field::new("losses", DataType::UInt32, false),
            Field::new("draws", DataType::UInt32, false),
            Field::new("win_rate", DataType::Float64, false),
            Field::new("meta_share", DataType::Float64, false),
            Field::new("tier", DataType::Utf8, false),
        ])
    }
}

/// Event data for Parquet writing.
#[derive(Debug, Clone)]
pub struct EventRecord {
    pub id: String,
    pub name: String,
    pub date: Option<NaiveDate>,
    pub location: Option<String>,
    pub player_count: Option<u32>,
    pub round_count: Option<u32>,
    pub event_type: Option<String>,
    pub epoch_id: String,
    pub created_at: DateTime<Utc>,
}

/// Placement data for Parquet writing.
#[derive(Debug, Clone)]
pub struct PlacementRecord {
    pub id: String,
    pub event_id: String,
    pub rank: u32,
    pub player_name: String,
    pub faction: String,
    pub subfaction: Option<String>,
    pub detachment: Option<String>,
    pub wins: Option<u32>,
    pub losses: Option<u32>,
    pub draws: Option<u32>,
    pub battle_points: Option<u32>,
    pub epoch_id: String,
}

/// Parquet file writer.
pub struct ParquetWriter {
    config: StorageConfig,
}

impl ParquetWriter {
    pub fn new(config: StorageConfig) -> Self {
        Self { config }
    }

    /// Get the path for a table in an epoch.
    fn table_path(&self, table: TableType, epoch_id: &str) -> PathBuf {
        self.config
            .parquet_dir()
            .join(epoch_id)
            .join(table.filename())
    }

    /// Ensure the directory exists.
    fn ensure_dir(&self, path: &Path) -> Result<(), StorageError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Write events to Parquet.
    pub fn write_events(&self, epoch_id: &str, events: &[EventRecord]) -> Result<(), StorageError> {
        let path = self.table_path(TableType::Events, epoch_id);
        self.ensure_dir(&path)?;

        let schema = Arc::new(schemas::events_schema());

        let ids: Vec<&str> = events.iter().map(|e| e.id.as_str()).collect();
        let names: Vec<&str> = events.iter().map(|e| e.name.as_str()).collect();
        let locations: Vec<Option<&str>> = events.iter().map(|e| e.location.as_deref()).collect();
        let player_counts: Vec<Option<u32>> = events.iter().map(|e| e.player_count).collect();
        let round_counts: Vec<Option<u32>> = events.iter().map(|e| e.round_count).collect();
        let event_types: Vec<Option<&str>> =
            events.iter().map(|e| e.event_type.as_deref()).collect();
        let epoch_ids: Vec<&str> = events.iter().map(|e| e.epoch_id.as_str()).collect();
        let created_ats: Vec<i64> = events
            .iter()
            .map(|e| e.created_at.timestamp_millis())
            .collect();

        // Convert dates to strings
        let date_strings: Vec<Option<String>> = events
            .iter()
            .map(|e| e.date.map(|d| d.to_string()))
            .collect();
        let date_refs: Vec<Option<&str>> = date_strings.iter().map(|d| d.as_deref()).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)) as ArrayRef,
                Arc::new(StringArray::from(names)) as ArrayRef,
                Arc::new(StringArray::from(date_refs)) as ArrayRef,
                Arc::new(StringArray::from(locations)) as ArrayRef,
                Arc::new(UInt32Array::from(player_counts)) as ArrayRef,
                Arc::new(UInt32Array::from(round_counts)) as ArrayRef,
                Arc::new(StringArray::from(event_types)) as ArrayRef,
                Arc::new(StringArray::from(epoch_ids)) as ArrayRef,
                Arc::new(TimestampMillisecondArray::from(created_ats)) as ArrayRef,
            ],
        )
        .map_err(|e| StorageError::InvalidPath(e.to_string()))?;

        self.write_batch(&path, &schema, &batch)?;

        info!("Wrote {} events to {:?}", events.len(), path);
        Ok(())
    }

    /// Write placements to Parquet.
    pub fn write_placements(
        &self,
        epoch_id: &str,
        placements: &[PlacementRecord],
    ) -> Result<(), StorageError> {
        let path = self.table_path(TableType::Placements, epoch_id);
        self.ensure_dir(&path)?;

        let schema = Arc::new(schemas::placements_schema());

        let ids: Vec<&str> = placements.iter().map(|p| p.id.as_str()).collect();
        let event_ids: Vec<&str> = placements.iter().map(|p| p.event_id.as_str()).collect();
        let ranks: Vec<u32> = placements.iter().map(|p| p.rank).collect();
        let player_names: Vec<&str> = placements.iter().map(|p| p.player_name.as_str()).collect();
        let factions: Vec<&str> = placements.iter().map(|p| p.faction.as_str()).collect();
        let subfactions: Vec<Option<&str>> =
            placements.iter().map(|p| p.subfaction.as_deref()).collect();
        let detachments: Vec<Option<&str>> =
            placements.iter().map(|p| p.detachment.as_deref()).collect();
        let wins: Vec<Option<u32>> = placements.iter().map(|p| p.wins).collect();
        let losses: Vec<Option<u32>> = placements.iter().map(|p| p.losses).collect();
        let draws: Vec<Option<u32>> = placements.iter().map(|p| p.draws).collect();
        let battle_points: Vec<Option<u32>> = placements.iter().map(|p| p.battle_points).collect();
        let epoch_ids: Vec<&str> = placements.iter().map(|p| p.epoch_id.as_str()).collect();

        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(StringArray::from(ids)) as ArrayRef,
                Arc::new(StringArray::from(event_ids)) as ArrayRef,
                Arc::new(UInt32Array::from(ranks)) as ArrayRef,
                Arc::new(StringArray::from(player_names)) as ArrayRef,
                Arc::new(StringArray::from(factions)) as ArrayRef,
                Arc::new(StringArray::from(subfactions)) as ArrayRef,
                Arc::new(StringArray::from(detachments)) as ArrayRef,
                Arc::new(UInt32Array::from(wins)) as ArrayRef,
                Arc::new(UInt32Array::from(losses)) as ArrayRef,
                Arc::new(UInt32Array::from(draws)) as ArrayRef,
                Arc::new(UInt32Array::from(battle_points)) as ArrayRef,
                Arc::new(StringArray::from(epoch_ids)) as ArrayRef,
            ],
        )
        .map_err(|e| StorageError::InvalidPath(e.to_string()))?;

        self.write_batch(&path, &schema, &batch)?;

        info!("Wrote {} placements to {:?}", placements.len(), path);
        Ok(())
    }

    /// Write a record batch to a Parquet file.
    fn write_batch(
        &self,
        path: &PathBuf,
        schema: &Arc<Schema>,
        batch: &RecordBatch,
    ) -> Result<(), StorageError> {
        let file = File::create(path)?;

        let props = WriterProperties::builder()
            .set_compression(Compression::SNAPPY)
            .build();

        let mut writer = ArrowWriter::try_new(file, schema.clone(), Some(props))
            .map_err(|e| StorageError::InvalidPath(e.to_string()))?;

        writer
            .write(batch)
            .map_err(|e| StorageError::InvalidPath(e.to_string()))?;

        writer
            .close()
            .map_err(|e| StorageError::InvalidPath(e.to_string()))?;

        Ok(())
    }
}

/// Parquet file reader.
pub struct ParquetReader {
    config: StorageConfig,
}

impl ParquetReader {
    pub fn new(config: StorageConfig) -> Self {
        Self { config }
    }

    /// Get the path for a table in an epoch.
    fn table_path(&self, table: TableType, epoch_id: &str) -> PathBuf {
        self.config
            .parquet_dir()
            .join(epoch_id)
            .join(table.filename())
    }

    /// Check if a table exists for an epoch.
    pub fn exists(&self, table: TableType, epoch_id: &str) -> bool {
        self.table_path(table, epoch_id).exists()
    }

    /// Read all record batches from a Parquet file.
    pub fn read_batches(
        &self,
        table: TableType,
        epoch_id: &str,
    ) -> Result<Vec<RecordBatch>, StorageError> {
        let path = self.table_path(table, epoch_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&path)?;
        let builder = ParquetRecordBatchReaderBuilder::try_new(file)
            .map_err(|e| StorageError::InvalidPath(e.to_string()))?;

        let reader = builder
            .build()
            .map_err(|e| StorageError::InvalidPath(e.to_string()))?;

        let batches: Result<Vec<_>, _> = reader.collect();
        let batches = batches.map_err(|e| StorageError::InvalidPath(e.to_string()))?;

        debug!("Read {} batches from {:?}", batches.len(), path);
        Ok(batches)
    }

    /// Get row count for a table.
    pub fn count(&self, table: TableType, epoch_id: &str) -> Result<usize, StorageError> {
        let batches = self.read_batches(table, epoch_id)?;
        Ok(batches.iter().map(|b| b.num_rows()).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_config(temp_dir: &TempDir) -> StorageConfig {
        StorageConfig::new(temp_dir.path().to_path_buf())
    }

    #[test]
    fn test_table_type_filename() {
        assert_eq!(TableType::Events.filename(), "events.parquet");
        assert_eq!(TableType::Placements.filename(), "placements.parquet");
    }

    #[test]
    fn test_events_schema() {
        let schema = schemas::events_schema();
        assert_eq!(schema.fields().len(), 9);
        assert!(schema.field_with_name("id").is_ok());
        assert!(schema.field_with_name("name").is_ok());
    }

    #[test]
    fn test_placements_schema() {
        let schema = schemas::placements_schema();
        assert_eq!(schema.fields().len(), 12);
        assert!(schema.field_with_name("faction").is_ok());
        assert!(schema.field_with_name("wins").is_ok());
    }

    #[test]
    fn test_write_and_read_events() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);

        let writer = ParquetWriter::new(config.clone());
        let reader = ParquetReader::new(config);

        let events = vec![
            EventRecord {
                id: "evt-001".to_string(),
                name: "London GT".to_string(),
                date: Some(NaiveDate::from_ymd_opt(2025, 6, 15).unwrap()),
                location: Some("London, UK".to_string()),
                player_count: Some(96),
                round_count: Some(5),
                event_type: Some("GT".to_string()),
                epoch_id: "epoch-001".to_string(),
                created_at: Utc::now(),
            },
            EventRecord {
                id: "evt-002".to_string(),
                name: "Birmingham Open".to_string(),
                date: None,
                location: Some("Birmingham, UK".to_string()),
                player_count: Some(48),
                round_count: None,
                event_type: None,
                epoch_id: "epoch-001".to_string(),
                created_at: Utc::now(),
            },
        ];

        writer.write_events("epoch-001", &events).unwrap();

        assert!(reader.exists(TableType::Events, "epoch-001"));
        assert_eq!(reader.count(TableType::Events, "epoch-001").unwrap(), 2);

        let batches = reader.read_batches(TableType::Events, "epoch-001").unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 2);
    }

    #[test]
    fn test_write_and_read_placements() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);

        let writer = ParquetWriter::new(config.clone());
        let reader = ParquetReader::new(config);

        let placements = vec![PlacementRecord {
            id: "plc-001".to_string(),
            event_id: "evt-001".to_string(),
            rank: 1,
            player_name: "John Smith".to_string(),
            faction: "Aeldari".to_string(),
            subfaction: Some("Ynnari".to_string()),
            detachment: Some("Soulrender".to_string()),
            wins: Some(5),
            losses: Some(0),
            draws: Some(0),
            battle_points: Some(94),
            epoch_id: "epoch-001".to_string(),
        }];

        writer.write_placements("epoch-001", &placements).unwrap();

        assert!(reader.exists(TableType::Placements, "epoch-001"));
        assert_eq!(reader.count(TableType::Placements, "epoch-001").unwrap(), 1);
    }

    #[test]
    fn test_read_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        let reader = ParquetReader::new(config);

        assert!(!reader.exists(TableType::Events, "nonexistent"));
        let batches = reader
            .read_batches(TableType::Events, "nonexistent")
            .unwrap();
        assert!(batches.is_empty());
    }
}
