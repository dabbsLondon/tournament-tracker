# Storage Layout

This document defines the filesystem data lake structure.

## Design Principles

1. **JSONL is Source of Truth**: Append-only, human-readable, git-friendly
2. **Parquet for Analytics**: Derived from JSONL, optimized for queries
3. **Raw Archived Forever**: Original HTML/PDFs preserved for re-extraction
4. **Epoch Partitioning**: Data partitioned by epoch for efficient filtering
5. **Atomic Writes**: Write to `.tmp`, then rename to prevent corruption

---

## Directory Structure

```
./data/
├── raw/                          # Original fetched content
│   ├── goonhammer/
│   │   └── {yyyy}/{mm}/{dd}/
│   │       └── {content-hash}.html
│   ├── warhammer-community/
│   │   ├── pdfs/
│   │   │   └── {yyyy-mm-dd}-{slug}.pdf
│   │   └── announcements/
│   │       └── {yyyy-mm-dd}-{slug}.html
│   └── other-sources/
│       └── ...
│
├── normalized/                   # JSONL source of truth
│   ├── significant_events/
│   │   └── epoch={epoch_id}/
│   │       └── dt={yyyy-mm-dd}/
│   │           └── part-{uuid}.jsonl
│   ├── events/
│   │   └── epoch={epoch_id}/
│   │       └── dt={yyyy-mm-dd}/
│   │           └── part-{uuid}.jsonl
│   ├── placements/
│   │   └── epoch={epoch_id}/
│   │       └── dt={yyyy-mm-dd}/
│   │           └── part-{uuid}.jsonl
│   └── army_lists/
│       └── epoch={epoch_id}/
│           └── dt={yyyy-mm-dd}/
│               └── part-{uuid}.jsonl
│
├── parquet/                      # Derived analytics format
│   ├── events/
│   │   └── epoch={epoch_id}/
│   │       └── dt={yyyy-mm-dd}/
│   │           └── part-{uuid}.parquet
│   ├── placements/
│   │   └── epoch={epoch_id}/
│   │       └── dt={yyyy-mm-dd}/
│   │           └── part-{uuid}.parquet
│   └── army_lists/
│       └── epoch={epoch_id}/
│           └── dt={yyyy-mm-dd}/
│               └── part-{uuid}.parquet
│
├── derived/                      # Computed artifacts
│   ├── faction_stats/
│   │   └── epoch={epoch_id}/
│   │       └── dt={yyyy-mm-dd}/
│   │           └── stats-{uuid}.json
│   ├── unit_frequency/
│   │   └── epoch={epoch_id}/
│   │       └── dt={yyyy-mm-dd}/
│   │           └── frequency-{uuid}.json
│   ├── top_combos/
│   │   └── epoch={epoch_id}/
│   │       └── dt={yyyy-mm-dd}/
│   │           └── combos-{uuid}.json
│   └── themes/
│       └── epoch={epoch_id}/
│           └── dt={yyyy-mm-dd}/
│               └── themes-{uuid}.json
│
├── review_queue/                 # Items needing manual attention
│   └── {yyyy-mm-dd}/
│       └── {entity_type}-{uuid}.json
│
├── state/                        # Sync cursors and bookmarks
│   ├── balance_cursor.json
│   ├── ingest_cursor.json
│   ├── last_sync.json
│   └── epochs.json               # Cached epoch list
│
└── logs/                         # Application logs
    └── {yyyy-mm-dd}.jsonl
```

---

## Naming Conventions

### Content Hash

Raw files are named by SHA256 hash of content body (first 16 chars):

```
sha256(html_body)[0:16].html
```

This enables:
- Automatic deduplication of identical content
- Stable references across fetches
- Easy cache invalidation

### Partition Keys

| Key | Format | Example |
|-----|--------|---------|
| `epoch` | ID hash (first 8 chars) | `epoch=a1b2c3d4` |
| `dt` | ISO date | `dt=2025-07-14` |

### Part Files

Part files use UUIDs to prevent collisions:

```
part-550e8400-e29b-41d4-a716-446655440000.jsonl
```

---

## File Formats

### JSONL (Normalized)

One JSON object per line, newline-delimited:

```jsonl
{"id":"abc123","name":"London GT","date":"2025-07-12","epoch_id":"a1b2c3d4",...}
{"id":"def456","name":"Paris Open","date":"2025-07-13","epoch_id":"a1b2c3d4",...}
```

**Benefits**:
- Append-only writes
- Easy to stream/process
- Human-readable
- Git-friendly diffs

### Parquet (Analytics)

Columnar format with schema:

```
events.parquet
├── id: string
├── name: string
├── date: date32
├── location: string (nullable)
├── player_count: uint32 (nullable)
├── epoch_id: string
├── extraction_confidence: string
└── needs_review: bool
```

**Benefits**:
- Fast analytical queries
- Efficient compression
- Predicate pushdown

### State Files (JSON)

```json
// balance_cursor.json
{
  "last_checked": "2025-07-14T10:00:00Z",
  "last_event_id": "abc123",
  "last_pdf_url": "https://..."
}

// ingest_cursor.json
{
  "last_article_url": "https://www.goonhammer.com/competitive-innovations-...",
  "last_article_date": "2025-07-14",
  "processed_event_ids": ["abc123", "def456"]
}

// last_sync.json
{
  "started_at": "2025-07-14T10:00:00Z",
  "completed_at": "2025-07-14T10:05:32Z",
  "events_added": 12,
  "placements_added": 156,
  "errors": []
}

// epochs.json
{
  "epochs": [
    {
      "id": "a1b2c3d4",
      "name": "Post June 2025 Dataslate",
      "start_date": "2025-06-15",
      "end_date": null,
      "is_current": true
    }
  ],
  "updated_at": "2025-07-14T10:00:00Z"
}
```

---

## Write Operations

### Atomic Writes

All writes follow this pattern to prevent corruption:

```rust
fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
    let tmp_path = path.with_extension("tmp");

    // Write to temp file
    fs::write(&tmp_path, content)?;

    // Atomic rename
    fs::rename(&tmp_path, path)?;

    Ok(())
}
```

### Append to JSONL

```rust
fn append_jsonl<T: Serialize>(dir: &Path, items: &[T]) -> io::Result<PathBuf> {
    let part_file = dir.join(format!("part-{}.jsonl", Uuid::new_v4()));

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&part_file)?;

    for item in items {
        serde_json::to_writer(&mut file, item)?;
        writeln!(file)?;
    }

    Ok(part_file)
}
```

---

## Read Operations

### List Entities in Epoch

```rust
fn list_events_in_epoch(epoch_id: &str, date_range: Option<DateRange>) -> Vec<Event> {
    let pattern = format!("normalized/events/epoch={}/dt=*/part-*.jsonl", epoch_id);

    glob(&pattern)
        .filter(|path| matches_date_range(path, date_range))
        .flat_map(|path| read_jsonl::<Event>(path))
        .collect()
}
```

### Parquet Query

```rust
fn query_placements(epoch_id: &str, faction: Option<&str>) -> Vec<Placement> {
    let path = format!("parquet/placements/epoch={}", epoch_id);

    ParquetReader::new(&path)
        .with_filter(col("faction").eq(lit(faction)))
        .read()
}
```

---

## Parquet Rebuild

Parquet files are derived from JSONL and can be rebuilt anytime:

```bash
meta-agent build-parquet --epoch a1b2c3d4 --entity events
meta-agent build-parquet --epoch a1b2c3d4 --entity placements
meta-agent build-parquet --all  # Rebuild everything
```

The process:
1. Read all JSONL files for entity/epoch
2. Deduplicate by ID (latest wins)
3. Write to Parquet with schema validation
4. Update state file with rebuild timestamp

---

## Review Queue

Items flagged for manual attention:

```json
// review_queue/2025-07-14/placement-550e8400.json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "entity_type": "placement",
  "entity_id": "abc123",
  "entity_data": { ... },
  "reason": "low_confidence",
  "details": "Faction 'Craftworlds' not recognized, may be 'Aeldari'",
  "source_path": "raw/goonhammer/2025/07/14/abc123.html",
  "created_at": "2025-07-14T10:00:00Z",
  "resolved": false
}
```

Review workflow:
1. `meta-agent review list` — Show pending items
2. `meta-agent review show <id>` — Display item details
3. `meta-agent review resolve <id> --action accept|reject|edit`

---

## Disk Space Management

Estimated storage per week of data:

| Component | Size Estimate |
|-----------|---------------|
| Raw HTML (20 articles) | ~5 MB |
| Raw PDFs (1-2 balance updates) | ~2 MB |
| JSONL (normalized) | ~500 KB |
| Parquet (compressed) | ~200 KB |
| Derived JSON | ~100 KB |

**Monthly estimate**: ~30 MB

**Cleanup policy** (manual, not automatic):
- Raw files: Keep forever (for re-extraction)
- JSONL: Keep forever (source of truth)
- Parquet: Can be rebuilt, safe to delete if needed
- Logs: Rotate after 30 days
