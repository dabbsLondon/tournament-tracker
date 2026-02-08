# Data Model

This document defines the core entities and their schemas for the Warhammer Meta Agent.

## Design Principles

1. **Deterministic IDs**: Entity IDs are SHA256 hashes of canonical fields for deduplication
2. **Epoch Awareness**: All analytical entities reference their epoch
3. **Extraction Metadata**: All AI-extracted data includes confidence and audit trail
4. **Review Flags**: Uncertain data is flagged for manual attention
5. **Timestamps**: All timestamps in ISO 8601 UTC

---

## Core Entities

### SignificantEvent

Marks epoch boundaries — either balance updates or edition releases.

```json
{
  "id": "sha256-hash",
  "event_type": "balance_update | edition_release",
  "date": "2025-06-15",
  "title": "Balance Dataslate June 2025",
  "source_url": "https://www.warhammer-community.com/...",
  "pdf_url": "https://assets.warhammer-community.com/....pdf",
  "summary": "AI-extracted summary of key changes",
  "created_at": "2025-06-15T10:00:00Z",
  "extraction_confidence": "high | medium | low",
  "needs_review": false,
  "raw_source_path": "raw/warhammer-community/pdfs/2025-06-15-balance-dataslate.pdf"
}
```

**ID Derivation**: `sha256(event_type + date + title)`

---

### MetaEpoch

A contiguous time window between significant events.

```json
{
  "id": "sha256-hash",
  "name": "Post June 2025 Dataslate",
  "start_event_id": "significant-event-id",
  "start_date": "2025-06-15",
  "end_date": "2025-09-20",
  "end_event_id": "next-significant-event-id",
  "is_current": false
}
```

**ID Derivation**: `sha256(start_event_id)`

**Notes**:
- `end_date` and `end_event_id` are null for the current epoch
- `is_current` is true for the active epoch

---

### Event (Tournament)

A competitive tournament discovered from sources.

```json
{
  "id": "sha256-hash",
  "name": "London GT 2025",
  "date": "2025-07-12",
  "location": "London, UK",
  "player_count": 120,
  "round_count": 6,
  "source_url": "https://www.goonhammer.com/competitive-innovations-...",
  "source_name": "goonhammer",
  "epoch_id": "epoch-hash",
  "created_at": "2025-07-14T08:00:00Z",
  "extraction_confidence": "high | medium | low",
  "needs_review": false,
  "raw_source_path": "raw/goonhammer/2025/07/14/abc123.html"
}
```

**ID Derivation**: `sha256(name + date + location)`

---

### Placement

A player's result at an event.

```json
{
  "id": "sha256-hash",
  "event_id": "event-hash",
  "epoch_id": "epoch-hash",
  "rank": 1,
  "player_name": "John Smith",
  "faction": "Aeldari",
  "subfaction": "Ynnari",
  "detachment": "Seer Council",
  "record": {
    "wins": 5,
    "losses": 1,
    "draws": 0
  },
  "battle_points": 450,
  "list_id": "army-list-hash",
  "created_at": "2025-07-14T08:00:00Z",
  "extraction_confidence": "high | medium | low",
  "needs_review": false
}
```

**ID Derivation**: `sha256(event_id + rank + player_name)`

**Notes**:
- `record` and `battle_points` may be null if not available from source
- `list_id` links to the army list (may be null if list not published)

---

### ArmyList

A normalized army list extracted by AI.

```json
{
  "id": "sha256-hash",
  "faction": "Aeldari",
  "subfaction": "Ynnari",
  "detachment": "Seer Council",
  "total_points": 2000,
  "units": [
    {
      "name": "Yvraine",
      "count": 1,
      "points": 120,
      "wargear": ["The Cronesword"],
      "keywords": ["Character", "Infantry", "Ynnari"]
    },
    {
      "name": "Wraithguard",
      "count": 5,
      "points": 180,
      "wargear": ["Wraithcannons"],
      "keywords": ["Infantry", "Wraith Construct"]
    }
  ],
  "raw_text": "Original unprocessed list text...",
  "source_url": "https://...",
  "created_at": "2025-07-14T08:00:00Z",
  "extraction_confidence": "high | medium | low",
  "needs_review": false,
  "raw_source_path": "raw/goonhammer/2025/07/14/abc123.html"
}
```

**ID Derivation**: `sha256(faction + detachment + sorted_unit_names + total_points)`

**Notes**:
- `raw_text` preserved for audit and re-extraction
- `keywords` are AI-inferred and may be incomplete

---

### ReviewQueueItem

Items flagged for manual attention.

```json
{
  "id": "uuid",
  "entity_type": "event | placement | army_list | significant_event",
  "entity_id": "hash-of-entity",
  "reason": "low_confidence | fact_check_failed | duplicate_suspected",
  "details": "Faction name 'Dark Angels' not found in extracted text",
  "source_path": "raw/goonhammer/2025/07/14/abc123.html",
  "created_at": "2025-07-14T08:00:00Z",
  "resolved": false,
  "resolved_at": null,
  "resolution_notes": null
}
```

---

## Derived Entities (Calculated, Not Extracted)

These are computed by the Rust calculation engine, not extracted by AI.

### FactionStats

Per-epoch faction statistics. This is the primary data structure for tier charts and stats tables.

```json
{
  "epoch_id": "epoch-hash",
  "epoch_name": "Post June 2025 Dataslate",
  "computed_at": "2025-07-14T10:00:00Z",
  "date_range": {
    "from": "2025-06-15",
    "to": "2025-07-14"
  },
  "totals": {
    "events": 45,
    "players": 1856,
    "games": 5568
  },
  "factions": [
    {
      "name": "Aeldari",
      "tier": "S",
      "player_count": 234,
      "games_played": 702,
      "event_appearances": 45,
      "wins": 379,
      "losses": 298,
      "draws": 25,
      "win_rate": 0.54,
      "win_rate_delta": 0.03,
      "placement_counts": {
        "first": 12,
        "top_4": 38,
        "top_10": 67,
        "top_half": 134
      },
      "podium_rate": 0.162,
      "meta_share": 0.126,
      "over_representation": 1.28,
      "average_placement_percentile": 0.32,
      "four_zero_starts": 18,
      "five_zero_starts": 8,
      "top_detachments": [
        {"name": "Seer Council", "count": 89, "win_rate": 0.58},
        {"name": "Battle Host", "count": 67, "win_rate": 0.51}
      ]
    }
  ]
}
```

**Tier Calculation**:
| Win Rate | Tier |
|----------|------|
| ≥ 55% | S |
| 52-55% | A |
| 48-52% | B |
| 45-48% | C |
| < 45% | D |

**Derived Metrics**:
- `win_rate_delta`: Change from previous epoch (null if first epoch)
- `podium_rate`: `top_4 / player_count`
- `meta_share`: `player_count / total_players`
- `over_representation`: `(top_4 / total_top_4) / meta_share` — values > 1.0 indicate over-performance
- `four_zero_starts` / `five_zero_starts`: Players with perfect starts (requires win/loss data)
```

---

### UnitFrequency

Unit usage across top-performing lists.

```json
{
  "epoch_id": "epoch-hash",
  "faction": "Aeldari",
  "computed_at": "2025-07-14T10:00:00Z",
  "top_n_lists": 50,
  "units": [
    {
      "name": "Wraithguard",
      "count": 42,
      "percentage": 0.84,
      "average_per_list": 1.2,
      "sample_list_ids": ["list-1", "list-2", "list-3"]
    }
  ]
}
```

---

### TopCombo

Common unit combinations in winning lists.

```json
{
  "epoch_id": "epoch-hash",
  "faction": "Aeldari",
  "computed_at": "2025-07-14T10:00:00Z",
  "combos": [
    {
      "units": ["Yvraine", "Wraithguard", "Wave Serpent"],
      "count": 28,
      "win_rate_with_combo": 0.62,
      "sample_list_ids": ["list-1", "list-2"],
      "confidence": "high"
    }
  ]
}
```

---

## Entity Relationships

```
SignificantEvent
       │
       ▼
   MetaEpoch ──────────────────────────────┐
       │                                   │
       ▼                                   ▼
    Event ─────────────────────────► FactionStats
       │                                   │
       ▼                                   │
  Placement ──────► ArmyList              │
       │               │                   │
       │               ▼                   ▼
       │         UnitFrequency ◄───── TopCombo
       │
       ▼
ReviewQueueItem
```

---

## Confidence Levels

| Level | Meaning | Action |
|-------|---------|--------|
| `high` | AI confident, fact-checker verified | Store normally |
| `medium` | AI somewhat confident, minor discrepancies | Store with flag |
| `low` | AI uncertain, fact-checker flagged issues | Store + add to review queue |

---

## Epoch Mapping Rules

Given an event date, determine its epoch:

1. Find the most recent SignificantEvent where `event.date <= significant_event.date`
2. The epoch is the one started by that SignificantEvent
3. If no SignificantEvent exists before the event date, use a synthetic "pre-tracking" epoch

```rust
fn get_epoch_for_date(date: NaiveDate, events: &[SignificantEvent]) -> EpochId {
    events
        .iter()
        .filter(|e| e.date <= date)
        .max_by_key(|e| e.date)
        .map(|e| epoch_id_from_event(e))
        .unwrap_or(PRE_TRACKING_EPOCH_ID)
}
```
