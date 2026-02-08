# Operations and Scheduling

This document covers CLI usage, sync operations, and manual workflows.

## CLI Overview

The `meta-agent` CLI provides commands for syncing data, serving the API, and administrative tasks.

```bash
meta-agent <COMMAND> [OPTIONS]
```

### Global Options

| Option | Description |
|--------|-------------|
| `--config <path>` | Path to config file (default: `./config.toml`) |
| `--data-dir <path>` | Data directory (default: `./data`) |
| `--log-level <level>` | Log level: trace, debug, info, warn, error |
| `--json-logs` | Output logs as JSON |

---

## Commands

### sync — Fetch Tournament Data

Discovers and ingests tournament data from configured sources.

```bash
# One-time sync
meta-agent sync --once

# Watch mode with interval
meta-agent sync --watch --interval 6h

# Sync specific date range
meta-agent sync --once --from 2025-06-01 --to 2025-07-14

# Sync specific source only
meta-agent sync --once --source goonhammer

# Dry run (fetch but don't store)
meta-agent sync --once --dry-run
```

**Options**:
| Option | Description |
|--------|-------------|
| `--once` | Run sync once and exit |
| `--watch` | Run continuously at interval |
| `--interval <duration>` | Sync interval (e.g., `6h`, `30m`) |
| `--from <date>` | Start date for sync range |
| `--to <date>` | End date for sync range |
| `--source <name>` | Only sync from this source |
| `--dry-run` | Fetch and parse but don't store |

**Output**:
```
[2025-07-14T10:00:00Z] Starting sync...
[2025-07-14T10:00:01Z] Checking for balance updates...
[2025-07-14T10:00:02Z] Found 0 new balance updates
[2025-07-14T10:00:03Z] Scanning Goonhammer Competitive Innovations...
[2025-07-14T10:00:15Z] Found 3 new articles
[2025-07-14T10:00:16Z] Processing: Competitive Innovations in 10th: Star God Mode pt.1
[2025-07-14T10:01:02Z] Extracted 8 events, 64 placements
[2025-07-14T10:01:03Z] Fact-checking results...
[2025-07-14T10:01:15Z] 62 placements verified, 2 flagged for review
[2025-07-14T10:01:16Z] Writing to storage...
[2025-07-14T10:01:17Z] Sync complete: 8 events, 62 placements stored
```

---

### serve — Start API Server

Starts the REST API server.

```bash
# Default (localhost:8080)
meta-agent serve

# Custom host and port
meta-agent serve --host 0.0.0.0 --port 3000

# With request logging
meta-agent serve --access-log
```

**Options**:
| Option | Description |
|--------|-------------|
| `--host <addr>` | Bind address (default: `127.0.0.1`) |
| `--port <port>` | Port number (default: `8080`) |
| `--access-log` | Log all HTTP requests |
| `--cors-origin <url>` | Allowed CORS origin (default: `*`) |

**Output**:
```
[2025-07-14T10:00:00Z] Starting API server...
[2025-07-14T10:00:00Z] Loaded 5 epochs, 234 events
[2025-07-14T10:00:01Z] Server listening on http://127.0.0.1:8080
```

---

### build-parquet — Rebuild Analytics Files

Rebuilds Parquet files from JSONL source of truth.

```bash
# Rebuild specific epoch
meta-agent build-parquet --epoch a1b2c3d4

# Rebuild specific entity type
meta-agent build-parquet --epoch a1b2c3d4 --entity events

# Rebuild all epochs
meta-agent build-parquet --all

# Rebuild date range within epoch
meta-agent build-parquet --epoch a1b2c3d4 --from 2025-07-01 --to 2025-07-14
```

**Options**:
| Option | Description |
|--------|-------------|
| `--epoch <id>` | Epoch to rebuild |
| `--entity <type>` | Entity type: events, placements, army_lists |
| `--all` | Rebuild all epochs |
| `--from <date>` | Start date |
| `--to <date>` | End date |

---

### derive — Compute Derived Analytics

Runs the calculation engine to produce derived artifacts.

```bash
# Run all derivations for current epoch
meta-agent derive

# Run specific derivations
meta-agent derive --run themes,top-combos

# For specific epoch
meta-agent derive --epoch a1b2c3d4

# Force recompute (ignore cached)
meta-agent derive --force
```

**Options**:
| Option | Description |
|--------|-------------|
| `--epoch <id>` | Epoch to analyze (default: current) |
| `--run <list>` | Comma-separated: themes, top-combos, faction-stats, unit-frequency |
| `--force` | Recompute even if recent artifact exists |

---

### review — Manual Review Queue

Manage items flagged for human attention.

```bash
# List pending items
meta-agent review list

# List with filters
meta-agent review list --entity-type placement --limit 10

# Show item details
meta-agent review show review-123

# Open source file for inspection
meta-agent review open review-123

# Resolve item
meta-agent review resolve review-123 --action accept
meta-agent review resolve review-123 --action reject --reason "Duplicate entry"
meta-agent review resolve review-123 --action edit --field faction --value "Aeldari"
```

**Actions**:
| Action | Description |
|--------|-------------|
| `accept` | Accept as-is, remove from queue |
| `reject` | Reject and remove from storage |
| `edit` | Modify field value, then accept |

---

### debug — Development Utilities

Tools for debugging and development.

```bash
# Parse a fixture file (no storage)
meta-agent debug parse-fixture path/to/file.html

# Test AI extraction on content
meta-agent debug extract --agent event-scout --file article.html

# Validate storage integrity
meta-agent debug validate-storage

# Show epoch timeline
meta-agent debug epochs

# Export entity to JSON (for inspection)
meta-agent debug export --entity-type event --id evt456
```

---

## Configuration File

`config.toml` example:

```toml
[general]
data_dir = "./data"
log_level = "info"

[ai]
backend = "ollama"
base_url = "http://localhost:11434"
model = "llama3.2"
timeout_seconds = 120
max_retries = 3

[sources.goonhammer]
enabled = true
base_url = "https://www.goonhammer.com"
competitive_innovations_path = "/category/columns/40k-competitive-innovations/"
rate_limit_ms = 2000

[sources.warhammer_community]
enabled = true
base_url = "https://www.warhammer-community.com"
rate_limit_ms = 3000

[server]
host = "127.0.0.1"
port = 8080
cors_origin = "*"

[sync]
default_interval = "6h"
max_articles_per_run = 10
```

---

## Scheduling Recommendations

### Manual Sync (Recommended for MVP)

Run sync manually when you want fresh data:

```bash
meta-agent sync --once
```

### Cron Schedule (Optional)

For automated syncing, add to crontab:

```bash
# Every 6 hours
0 */6 * * * /path/to/meta-agent sync --once >> /var/log/meta-agent.log 2>&1

# Daily at 6 AM
0 6 * * * /path/to/meta-agent sync --once && /path/to/meta-agent derive
```

### Watch Mode (Development)

For continuous development:

```bash
# Terminal 1: API server
meta-agent serve

# Terminal 2: Watch for new data
meta-agent sync --watch --interval 1h
```

---

## Log Interpretation

### Standard Log Format

```
[2025-07-14T10:00:00Z] [INFO] [sync] Starting sync...
[2025-07-14T10:00:01Z] [DEBUG] [agent:event-scout] Extracting from article...
[2025-07-14T10:00:02Z] [WARN] [agent:fact-checker] Low confidence: placement rank unclear
[2025-07-14T10:00:03Z] [ERROR] [storage] Failed to write: disk full
```

### JSON Log Format (--json-logs)

```json
{"timestamp":"2025-07-14T10:00:00Z","level":"INFO","target":"sync","message":"Starting sync...","span":{"sync_id":"abc123"}}
```

### Key Log Patterns

| Pattern | Meaning |
|---------|---------|
| `[agent:X] Extracting` | AI agent processing content |
| `[agent:fact-checker] Low confidence` | Item will be flagged for review |
| `[storage] Writing` | Data being persisted |
| `[api] Request` | HTTP request received (with --access-log) |

---

## Manual Recovery Procedures

### Corrupted JSONL File

1. Identify the corrupted file from logs
2. Delete the corrupted part file
3. Re-sync the affected date range:
   ```bash
   meta-agent sync --once --from 2025-07-14 --to 2025-07-14
   ```

### Missing Epoch Data

1. Check epochs.json for gaps
2. Re-sync the missing period:
   ```bash
   meta-agent sync --once --from <epoch_start> --to <epoch_end>
   ```

### Rebuild After Schema Change

1. Delete parquet files: `rm -rf data/parquet/*`
2. Rebuild: `meta-agent build-parquet --all`

### AI Extraction Quality Issues

1. Update AI model in config
2. Delete normalized data for affected period
3. Re-sync with new model

---

## Monitoring Checklist

Daily/Weekly checks:

- [ ] Review queue not growing excessively
- [ ] Last sync completed successfully
- [ ] Disk space adequate
- [ ] No error patterns in logs

```bash
# Quick health check
meta-agent debug validate-storage
curl http://localhost:8080/api/v1/health
meta-agent review list --limit 5
```
