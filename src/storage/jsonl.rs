//! JSONL (JSON Lines) storage.
//!
//! JSONL is the source of truth for all normalized data.
//! Each line is a valid JSON object representing one entity.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::marker::PhantomData;
use std::path::PathBuf;

use serde::{de::DeserializeOwned, Serialize};
use tracing::{debug, info, warn};

use super::{StorageConfig, StorageError};

/// Entity types for JSONL storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityType {
    SignificantEvent,
    Event,
    Placement,
    ArmyList,
    ReviewItem,
    Pairing,
}

impl EntityType {
    /// Get the filename for this entity type.
    pub fn filename(&self) -> &'static str {
        match self {
            EntityType::SignificantEvent => "significant_events.jsonl",
            EntityType::Event => "events.jsonl",
            EntityType::Placement => "placements.jsonl",
            EntityType::ArmyList => "army_lists.jsonl",
            EntityType::ReviewItem => "review_items.jsonl",
            EntityType::Pairing => "pairings.jsonl",
        }
    }
}

/// JSONL file writer.
pub struct JsonlWriter<T> {
    path: PathBuf,
    _marker: PhantomData<T>,
}

impl<T: Serialize> JsonlWriter<T> {
    /// Create a new JSONL writer for the given path.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            _marker: PhantomData,
        }
    }

    /// Create a writer for a specific entity type and epoch.
    pub fn for_entity(config: &StorageConfig, entity: EntityType, epoch_id: &str) -> Self {
        let path = config
            .normalized_dir()
            .join(epoch_id)
            .join(entity.filename());
        Self::new(path)
    }

    /// Ensure the parent directory exists.
    fn ensure_dir(&self) -> Result<(), StorageError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Append a single entity to the file.
    pub fn append(&self, entity: &T) -> Result<(), StorageError> {
        self.ensure_dir()?;

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let mut writer = BufWriter::new(file);
        let json = serde_json::to_string(entity)?;
        writeln!(writer, "{}", json)?;
        writer.flush()?;

        debug!("Appended entity to {:?}", self.path);
        Ok(())
    }

    /// Append multiple entities to the file.
    pub fn append_batch(&self, entities: &[T]) -> Result<usize, StorageError> {
        if entities.is_empty() {
            return Ok(0);
        }

        self.ensure_dir()?;

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let mut writer = BufWriter::new(file);
        let mut count = 0;

        for entity in entities {
            let json = serde_json::to_string(entity)?;
            writeln!(writer, "{}", json)?;
            count += 1;
        }

        writer.flush()?;
        info!("Appended {} entities to {:?}", count, self.path);

        Ok(count)
    }

    /// Write entities, replacing the entire file.
    pub fn write_all(&self, entities: &[T]) -> Result<usize, StorageError> {
        self.ensure_dir()?;

        let file = File::create(&self.path)?;
        let mut writer = BufWriter::new(file);
        let mut count = 0;

        for entity in entities {
            let json = serde_json::to_string(entity)?;
            writeln!(writer, "{}", json)?;
            count += 1;
        }

        writer.flush()?;
        info!("Wrote {} entities to {:?}", count, self.path);

        Ok(count)
    }
}

/// JSONL file reader.
pub struct JsonlReader<T> {
    path: PathBuf,
    _marker: PhantomData<T>,
}

impl<T: DeserializeOwned> JsonlReader<T> {
    /// Create a new JSONL reader for the given path.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            _marker: PhantomData,
        }
    }

    /// Create a reader for a specific entity type and epoch.
    pub fn for_entity(config: &StorageConfig, entity: EntityType, epoch_id: &str) -> Self {
        let path = config
            .normalized_dir()
            .join(epoch_id)
            .join(entity.filename());
        Self::new(path)
    }

    /// Check if the file exists.
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Read all entities from the file.
    pub fn read_all(&self) -> Result<Vec<T>, StorageError> {
        if !self.path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut entities = Vec::new();
        let mut line_num = 0;

        for line in reader.lines() {
            line_num += 1;
            let line = line?;

            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str(&line) {
                Ok(entity) => entities.push(entity),
                Err(e) => {
                    warn!(
                        "Failed to parse line {} in {:?}: {}",
                        line_num, self.path, e
                    );
                }
            }
        }

        debug!("Read {} entities from {:?}", entities.len(), self.path);
        Ok(entities)
    }

    /// Read entities matching a predicate.
    pub fn read_where<F>(&self, predicate: F) -> Result<Vec<T>, StorageError>
    where
        F: Fn(&T) -> bool,
    {
        let all = self.read_all()?;
        Ok(all.into_iter().filter(predicate).collect())
    }

    /// Count entities in the file.
    pub fn count(&self) -> Result<usize, StorageError> {
        if !self.path.exists() {
            return Ok(0);
        }

        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let count = reader.lines().filter(|l| l.is_ok()).count();

        Ok(count)
    }

    /// Create an iterator over the file.
    pub fn iter(&self) -> Result<JsonlIterator<T>, StorageError> {
        if !self.path.exists() {
            return Err(StorageError::PathNotFound(self.path.clone()));
        }

        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);

        Ok(JsonlIterator {
            reader,
            _marker: PhantomData,
        })
    }
}

/// Iterator over JSONL file entries.
pub struct JsonlIterator<T> {
    reader: BufReader<File>,
    _marker: PhantomData<T>,
}

impl<T: DeserializeOwned> Iterator for JsonlIterator<T> {
    type Item = Result<T, StorageError>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = String::new();

        loop {
            line.clear();
            match self.reader.read_line(&mut line) {
                Ok(0) => return None, // EOF
                Ok(_) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    return Some(serde_json::from_str(&line).map_err(StorageError::Json));
                }
                Err(e) => return Some(Err(StorageError::Io(e))),
            }
        }
    }
}

/// Find all epoch directories.
pub fn list_epochs(config: &StorageConfig) -> Result<Vec<String>, StorageError> {
    let dir = config.normalized_dir();
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut epochs = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                epochs.push(name.to_string());
            }
        }
    }

    epochs.sort();
    Ok(epochs)
}

/// Get the path for an epoch's entity file.
pub fn entity_path(config: &StorageConfig, entity: EntityType, epoch_id: &str) -> PathBuf {
    config
        .normalized_dir()
        .join(epoch_id)
        .join(entity.filename())
}

/// Read significant events from the global file.
pub fn read_significant_events(
    config: &StorageConfig,
) -> Result<Vec<crate::models::SignificantEvent>, StorageError> {
    let reader = JsonlReader::new(config.significant_events_path());
    reader.read_all()
}

/// Write significant events to the global file, sorted by date.
pub fn write_significant_events(
    config: &StorageConfig,
    events: &mut [crate::models::SignificantEvent],
) -> Result<usize, StorageError> {
    events.sort_by_key(|e| e.date);
    let writer = JsonlWriter::new(config.significant_events_path());
    writer.write_all(events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestEntity {
        id: String,
        name: String,
        value: u32,
    }

    fn test_config(temp_dir: &TempDir) -> StorageConfig {
        StorageConfig::new(temp_dir.path().to_path_buf())
    }

    #[test]
    fn test_jsonl_write_and_read() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("test.jsonl");

        let entities = vec![
            TestEntity {
                id: "1".to_string(),
                name: "First".to_string(),
                value: 100,
            },
            TestEntity {
                id: "2".to_string(),
                name: "Second".to_string(),
                value: 200,
            },
        ];

        // Write
        let writer: JsonlWriter<TestEntity> = JsonlWriter::new(path.clone());
        let count = writer.write_all(&entities).unwrap();
        assert_eq!(count, 2);

        // Read
        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        let read_entities = reader.read_all().unwrap();

        assert_eq!(read_entities.len(), 2);
        assert_eq!(read_entities[0], entities[0]);
        assert_eq!(read_entities[1], entities[1]);
    }

    #[test]
    fn test_jsonl_append() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("append.jsonl");

        let writer: JsonlWriter<TestEntity> = JsonlWriter::new(path.clone());
        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);

        // Append first
        writer
            .append(&TestEntity {
                id: "1".to_string(),
                name: "First".to_string(),
                value: 100,
            })
            .unwrap();

        // Append second
        writer
            .append(&TestEntity {
                id: "2".to_string(),
                name: "Second".to_string(),
                value: 200,
            })
            .unwrap();

        let entities = reader.read_all().unwrap();
        assert_eq!(entities.len(), 2);
    }

    #[test]
    fn test_jsonl_read_empty() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("nonexistent.jsonl");

        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        let entities = reader.read_all().unwrap();

        assert!(entities.is_empty());
    }

    #[test]
    fn test_jsonl_count() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("count.jsonl");

        let writer: JsonlWriter<TestEntity> = JsonlWriter::new(path.clone());
        writer
            .write_all(&[
                TestEntity {
                    id: "1".to_string(),
                    name: "A".to_string(),
                    value: 1,
                },
                TestEntity {
                    id: "2".to_string(),
                    name: "B".to_string(),
                    value: 2,
                },
                TestEntity {
                    id: "3".to_string(),
                    name: "C".to_string(),
                    value: 3,
                },
            ])
            .unwrap();

        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        assert_eq!(reader.count().unwrap(), 3);
    }

    #[test]
    fn test_jsonl_read_where() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("filter.jsonl");

        let writer: JsonlWriter<TestEntity> = JsonlWriter::new(path.clone());
        writer
            .write_all(&[
                TestEntity {
                    id: "1".to_string(),
                    name: "A".to_string(),
                    value: 50,
                },
                TestEntity {
                    id: "2".to_string(),
                    name: "B".to_string(),
                    value: 150,
                },
                TestEntity {
                    id: "3".to_string(),
                    name: "C".to_string(),
                    value: 250,
                },
            ])
            .unwrap();

        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        let filtered = reader.read_where(|e| e.value > 100).unwrap();

        assert_eq!(filtered.len(), 2);
        assert_eq!(filtered[0].name, "B");
        assert_eq!(filtered[1].name, "C");
    }

    #[test]
    fn test_entity_type_filename() {
        assert_eq!(EntityType::Event.filename(), "events.jsonl");
        assert_eq!(EntityType::Placement.filename(), "placements.jsonl");
        assert_eq!(EntityType::ArmyList.filename(), "army_lists.jsonl");
    }

    #[test]
    fn test_list_epochs() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);

        // Create epoch directories
        fs::create_dir_all(config.normalized_dir().join("epoch-001")).unwrap();
        fs::create_dir_all(config.normalized_dir().join("epoch-002")).unwrap();
        fs::create_dir_all(config.normalized_dir().join("epoch-003")).unwrap();

        let epochs = list_epochs(&config).unwrap();

        assert_eq!(epochs.len(), 3);
        assert_eq!(epochs[0], "epoch-001");
        assert_eq!(epochs[1], "epoch-002");
        assert_eq!(epochs[2], "epoch-003");
    }

    #[test]
    fn test_for_entity() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);

        let writer: JsonlWriter<TestEntity> =
            JsonlWriter::for_entity(&config, EntityType::Event, "epoch-001");

        // Check the path is constructed correctly
        let expected = config
            .normalized_dir()
            .join("epoch-001")
            .join("events.jsonl");
        assert_eq!(writer.path, expected);
    }

    #[test]
    fn test_append_batch() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("batch.jsonl");

        let writer: JsonlWriter<TestEntity> = JsonlWriter::new(path.clone());
        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);

        let entities = vec![
            TestEntity {
                id: "1".to_string(),
                name: "A".to_string(),
                value: 10,
            },
            TestEntity {
                id: "2".to_string(),
                name: "B".to_string(),
                value: 20,
            },
            TestEntity {
                id: "3".to_string(),
                name: "C".to_string(),
                value: 30,
            },
        ];

        let count = writer.append_batch(&entities).unwrap();
        assert_eq!(count, 3);

        let read = reader.read_all().unwrap();
        assert_eq!(read.len(), 3);
        assert_eq!(read[0].name, "A");
        assert_eq!(read[2].name, "C");
    }

    #[test]
    fn test_append_batch_empty() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("empty_batch.jsonl");

        let writer: JsonlWriter<TestEntity> = JsonlWriter::new(path);
        let count = writer.append_batch(&[]).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_write_all_overwrites_existing() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("overwrite.jsonl");

        let writer: JsonlWriter<TestEntity> = JsonlWriter::new(path.clone());
        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);

        writer
            .write_all(&[TestEntity {
                id: "1".to_string(),
                name: "Old".to_string(),
                value: 1,
            }])
            .unwrap();
        assert_eq!(reader.read_all().unwrap().len(), 1);

        writer
            .write_all(&[
                TestEntity {
                    id: "2".to_string(),
                    name: "New1".to_string(),
                    value: 2,
                },
                TestEntity {
                    id: "3".to_string(),
                    name: "New2".to_string(),
                    value: 3,
                },
            ])
            .unwrap();

        let read = reader.read_all().unwrap();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].name, "New1");
    }

    #[test]
    fn test_read_all_skips_bad_lines() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("bad_lines.jsonl");

        // Write a mix of valid and invalid lines
        std::fs::write(
            &path,
            r#"{"id":"1","name":"Good","value":1}
not-valid-json
{"id":"2","name":"Also Good","value":2}
"#,
        )
        .unwrap();

        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        let entities = reader.read_all().unwrap();
        assert_eq!(entities.len(), 2);
        assert_eq!(entities[0].name, "Good");
        assert_eq!(entities[1].name, "Also Good");
    }

    #[test]
    fn test_reader_exists_true() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("exists.jsonl");
        std::fs::write(&path, "").unwrap();

        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        assert!(reader.exists());
    }

    #[test]
    fn test_reader_exists_false() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("nonexistent.jsonl");

        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        assert!(!reader.exists());
    }

    #[test]
    fn test_jsonl_iterator() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("iter.jsonl");

        let writer: JsonlWriter<TestEntity> = JsonlWriter::new(path.clone());
        writer
            .write_all(&[
                TestEntity {
                    id: "1".to_string(),
                    name: "A".to_string(),
                    value: 10,
                },
                TestEntity {
                    id: "2".to_string(),
                    name: "B".to_string(),
                    value: 20,
                },
            ])
            .unwrap();

        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        let items: Vec<TestEntity> = reader.iter().unwrap().filter_map(|r| r.ok()).collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].name, "A");
    }

    #[test]
    fn test_iterator_skips_empty_lines() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("empty_lines.jsonl");

        std::fs::write(
            &path,
            r#"{"id":"1","name":"A","value":1}

{"id":"2","name":"B","value":2}
"#,
        )
        .unwrap();

        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        let items: Vec<TestEntity> = reader.iter().unwrap().filter_map(|r| r.ok()).collect();
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn test_entity_path_construction() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);

        let path = entity_path(&config, EntityType::Placement, "epoch-001");
        assert!(path.ends_with("epoch-001/placements.jsonl"));
    }

    #[test]
    fn test_read_significant_events_empty() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);

        let events = read_significant_events(&config).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn test_write_and_read_significant_events() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);

        let mut events = vec![
            crate::models::SignificantEvent::new(
                crate::models::SignificantEventType::BalanceUpdate,
                chrono::NaiveDate::from_ymd_opt(2025, 9, 15).unwrap(),
                "September Update".to_string(),
                "https://example.com".to_string(),
            ),
            crate::models::SignificantEvent::new(
                crate::models::SignificantEventType::BalanceUpdate,
                chrono::NaiveDate::from_ymd_opt(2025, 3, 15).unwrap(),
                "March Update".to_string(),
                "https://example.com".to_string(),
            ),
        ];

        write_significant_events(&config, &mut events).unwrap();

        let read = read_significant_events(&config).unwrap();
        assert_eq!(read.len(), 2);
        // Should be sorted by date
        assert!(read[0].date <= read[1].date);
    }

    #[test]
    fn test_list_epochs_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let config = test_config(&temp_dir);
        // normalized dir doesn't exist yet
        let epochs = list_epochs(&config).unwrap();
        assert!(epochs.is_empty());
    }

    #[test]
    fn test_count_nonexistent_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("nonexistent.jsonl");
        let reader: JsonlReader<TestEntity> = JsonlReader::new(path);
        assert_eq!(reader.count().unwrap(), 0);
    }

    #[test]
    fn test_entity_type_all_filenames() {
        assert_eq!(
            EntityType::SignificantEvent.filename(),
            "significant_events.jsonl"
        );
        assert_eq!(EntityType::ReviewItem.filename(), "review_items.jsonl");
    }
}
