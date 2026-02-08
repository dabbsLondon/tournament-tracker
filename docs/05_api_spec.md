# API Specification

This document defines the REST API for the Warhammer Meta Agent.

## Design Principles

1. **Epoch-Aware**: All analytical endpoints filter by epoch
2. **Pagination**: Large collections use cursor-based pagination
3. **Consistent Errors**: Structured error responses
4. **CORS Permissive**: Allow local frontend development
5. **JSON Only**: All requests and responses are JSON

---

## Base URL

```
http://localhost:8080/api/v1
```

---

## Common Headers

### Request Headers

| Header | Required | Description |
|--------|----------|-------------|
| `Accept` | No | Should be `application/json` (default) |

### Response Headers

| Header | Description |
|--------|-------------|
| `Content-Type` | Always `application/json` |
| `X-Request-Id` | Unique request identifier for debugging |

---

## Pagination

Paginated endpoints use these query parameters:

| Parameter | Type | Default | Max | Description |
|-----------|------|---------|-----|-------------|
| `page` | integer | 1 | - | Page number (1-indexed) |
| `page_size` | integer | 50 | 100 | Items per page |

Response includes pagination metadata:

```json
{
  "data": [...],
  "pagination": {
    "page": 1,
    "page_size": 50,
    "total_items": 234,
    "total_pages": 5,
    "has_next": true,
    "has_prev": false
  }
}
```

---

## Error Responses

All errors return consistent structure:

```json
{
  "error": {
    "code": "NOT_FOUND",
    "message": "Event with ID 'abc123' not found",
    "details": null
  }
}
```

### Error Codes

| HTTP Status | Code | Description |
|-------------|------|-------------|
| 400 | `BAD_REQUEST` | Invalid query parameters |
| 404 | `NOT_FOUND` | Resource not found |
| 500 | `INTERNAL_ERROR` | Server error |
| 503 | `SERVICE_UNAVAILABLE` | Data not yet available |

---

## Endpoints

### Health Check

```
GET /api/v1/health
```

**Response** `200 OK`:
```json
{
  "status": "healthy",
  "version": "0.1.0",
  "last_sync": {
    "completed_at": "2025-07-14T10:05:32Z",
    "events_added": 12,
    "placements_added": 156
  },
  "current_epoch": {
    "id": "a1b2c3d4",
    "name": "Post June 2025 Dataslate",
    "start_date": "2025-06-15"
  }
}
```

---

### Epochs

#### List All Epochs

```
GET /api/v1/epochs
```

**Response** `200 OK`:
```json
{
  "data": [
    {
      "id": "a1b2c3d4",
      "name": "Post June 2025 Dataslate",
      "start_date": "2025-06-15",
      "end_date": null,
      "is_current": true,
      "start_event": {
        "id": "evt123",
        "type": "balance_update",
        "title": "Balance Dataslate June 2025"
      },
      "stats": {
        "event_count": 45,
        "placement_count": 1234
      }
    },
    {
      "id": "b2c3d4e5",
      "name": "Post March 2025 Dataslate",
      "start_date": "2025-03-15",
      "end_date": "2025-06-14",
      "is_current": false,
      "start_event": {
        "id": "evt122",
        "type": "balance_update",
        "title": "Balance Dataslate March 2025"
      },
      "stats": {
        "event_count": 120,
        "placement_count": 3456
      }
    }
  ]
}
```

#### Get Current Epoch

```
GET /api/v1/epochs/current
```

**Response** `200 OK`:
```json
{
  "id": "a1b2c3d4",
  "name": "Post June 2025 Dataslate",
  "start_date": "2025-06-15",
  "end_date": null,
  "is_current": true,
  "start_event": {
    "id": "evt123",
    "type": "balance_update",
    "title": "Balance Dataslate June 2025",
    "source_url": "https://www.warhammer-community.com/...",
    "pdf_url": "https://assets.warhammer-community.com/...",
    "summary": "Major points changes to Aeldari and Space Marines..."
  },
  "stats": {
    "event_count": 45,
    "placement_count": 1234,
    "faction_breakdown": {
      "Aeldari": 156,
      "Space Marines": 234,
      "Chaos Space Marines": 189
    }
  }
}
```

---

### Events (Tournaments)

#### List Events

```
GET /api/v1/events
```

**Query Parameters**:
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `epoch_id` | string | No | Filter by epoch (default: current) |
| `from` | date | No | Start date (ISO 8601) |
| `to` | date | No | End date (ISO 8601) |
| `page` | integer | No | Page number |
| `page_size` | integer | No | Items per page |

**Response** `200 OK`:
```json
{
  "data": [
    {
      "id": "evt456",
      "name": "London GT 2025",
      "date": "2025-07-12",
      "location": "London, UK",
      "player_count": 120,
      "round_count": 6,
      "source_url": "https://www.goonhammer.com/...",
      "epoch_id": "a1b2c3d4",
      "top_factions": ["Aeldari", "Space Marines", "Tyranids"]
    }
  ],
  "pagination": {
    "page": 1,
    "page_size": 50,
    "total_items": 45,
    "total_pages": 1,
    "has_next": false,
    "has_prev": false
  }
}
```

#### Get Single Event

```
GET /api/v1/events/{event_id}
```

**Response** `200 OK`:
```json
{
  "id": "evt456",
  "name": "London GT 2025",
  "date": "2025-07-12",
  "location": "London, UK",
  "player_count": 120,
  "round_count": 6,
  "source_url": "https://www.goonhammer.com/...",
  "epoch_id": "a1b2c3d4",
  "extraction_confidence": "high",
  "placements": [
    {
      "rank": 1,
      "player_name": "John Smith",
      "faction": "Aeldari",
      "subfaction": "Ynnari",
      "detachment": "Seer Council",
      "record": {"wins": 5, "losses": 1, "draws": 0},
      "list_id": "list789"
    },
    {
      "rank": 2,
      "player_name": "Jane Doe",
      "faction": "Space Marines",
      "subfaction": "Black Templars",
      "detachment": "Righteous Crusaders",
      "record": {"wins": 5, "losses": 1, "draws": 0},
      "list_id": "list790"
    }
  ]
}
```

**Response** `404 Not Found`:
```json
{
  "error": {
    "code": "NOT_FOUND",
    "message": "Event with ID 'evt456' not found"
  }
}
```

---

### Factions

#### Faction Summary

```
GET /api/v1/factions/summary
```

**Query Parameters**:
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `epoch_id` | string | No | Filter by epoch (default: current) |
| `from` | date | No | Start date |
| `to` | date | No | End date |

**Response** `200 OK`:
```json
{
  "epoch_id": "a1b2c3d4",
  "epoch_name": "Post June 2025 Dataslate",
  "date_range": {
    "from": "2025-06-15",
    "to": "2025-07-14"
  },
  "computed_at": "2025-07-14T10:00:00Z",
  "total_events": 45,
  "total_players": 1856,
  "total_games": 5568,
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
      "placements": {
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
    },
    {
      "name": "Space Marines",
      "tier": "B",
      "player_count": 312,
      "games_played": 936,
      "event_appearances": 45,
      "wins": 449,
      "losses": 468,
      "draws": 19,
      "win_rate": 0.48,
      "win_rate_delta": -0.02,
      "placements": {
        "first": 8,
        "top_4": 29,
        "top_10": 52,
        "top_half": 167
      },
      "podium_rate": 0.093,
      "meta_share": 0.168,
      "over_representation": 0.55,
      "average_placement_percentile": 0.45,
      "four_zero_starts": 12,
      "five_zero_starts": 4,
      "top_detachments": [
        {"name": "Gladius Task Force", "count": 145, "win_rate": 0.49},
        {"name": "Ironstorm Spearhead", "count": 78, "win_rate": 0.52}
      ]
    }
  ]
}
```

#### Top Lists for Faction

```
GET /api/v1/factions/{faction}/toplists
```

**Query Parameters**:
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `epoch_id` | string | No | Filter by epoch (default: current) |
| `limit` | integer | No | Max results (default: 10, max: 50) |

**Response** `200 OK`:
```json
{
  "faction": "Aeldari",
  "epoch_id": "a1b2c3d4",
  "lists": [
    {
      "list_id": "list789",
      "placement": {
        "rank": 1,
        "event_name": "London GT 2025",
        "event_date": "2025-07-12",
        "player_name": "John Smith"
      },
      "detachment": "Seer Council",
      "total_points": 2000,
      "key_units": [
        {"name": "Yvraine", "count": 1},
        {"name": "Wraithguard", "count": 10},
        {"name": "Wave Serpent", "count": 2}
      ]
    }
  ]
}
```

---

### Army Lists

#### Get Army List

```
GET /api/v1/lists/{list_id}
```

**Response** `200 OK`:
```json
{
  "id": "list789",
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
  "raw_text": "Original list text...",
  "extraction_confidence": "high",
  "placement": {
    "event_id": "evt456",
    "event_name": "London GT 2025",
    "rank": 1,
    "player_name": "John Smith"
  }
}
```

---

### Derived Data

#### Themes

```
GET /api/v1/derived/themes
```

**Query Parameters**:
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `epoch_id` | string | No | Epoch (default: current) |
| `dt` | date | No | Specific computation date |

**Response** `200 OK`:
```json
{
  "epoch_id": "a1b2c3d4",
  "computed_at": "2025-07-14T10:00:00Z",
  "themes": [
    {
      "name": "Wraith-heavy Aeldari",
      "description": "Lists featuring multiple Wraith units with Wave Serpent transports",
      "evidence_count": 28,
      "factions": ["Aeldari"],
      "key_units": ["Wraithguard", "Wraithblades", "Wave Serpent"],
      "sample_list_ids": ["list789", "list801", "list823"],
      "confidence": "high"
    },
    {
      "name": "Monster Mash Tyranids",
      "description": "Heavy monster lists with minimal infantry",
      "evidence_count": 15,
      "factions": ["Tyranids"],
      "key_units": ["Hierophant", "Haruspex", "Tyrannofex"],
      "sample_list_ids": ["list456", "list478"],
      "confidence": "medium"
    }
  ]
}
```

#### Top Combos

```
GET /api/v1/derived/top-combos
```

**Query Parameters**:
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `epoch_id` | string | No | Epoch (default: current) |
| `faction` | string | No | Filter by faction |
| `dt` | date | No | Specific computation date |

**Response** `200 OK`:
```json
{
  "epoch_id": "a1b2c3d4",
  "faction": "Aeldari",
  "computed_at": "2025-07-14T10:00:00Z",
  "combos": [
    {
      "units": ["Yvraine", "Wraithguard", "Wave Serpent"],
      "count": 28,
      "percentage_of_lists": 0.72,
      "average_placement_percentile": 0.15,
      "sample_list_ids": ["list789", "list801"],
      "confidence": "high"
    },
    {
      "units": ["Farseer", "Warp Spiders", "Fire Prism"],
      "count": 19,
      "percentage_of_lists": 0.49,
      "average_placement_percentile": 0.28,
      "sample_list_ids": ["list802", "list815"],
      "confidence": "high"
    }
  ]
}
```

---

### Review Queue

#### List Review Items

```
GET /api/v1/review
```

**Query Parameters**:
| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `status` | string | No | `pending` or `resolved` (default: pending) |
| `entity_type` | string | No | Filter by entity type |
| `page` | integer | No | Page number |
| `page_size` | integer | No | Items per page |

**Response** `200 OK`:
```json
{
  "data": [
    {
      "id": "review-123",
      "entity_type": "placement",
      "entity_id": "abc123",
      "reason": "low_confidence",
      "details": "Faction 'Craftworlds' not recognized",
      "created_at": "2025-07-14T10:00:00Z",
      "source_path": "raw/goonhammer/2025/07/14/abc123.html"
    }
  ],
  "pagination": {...}
}
```

---

## CORS Configuration

For local development, CORS is permissive:

```
Access-Control-Allow-Origin: *
Access-Control-Allow-Methods: GET, OPTIONS
Access-Control-Allow-Headers: Content-Type, Accept
```

---

## Rate Limiting

No rate limiting for local use. Future versions may add:

```
X-RateLimit-Limit: 100
X-RateLimit-Remaining: 99
X-RateLimit-Reset: 1626300000
```

---

## UX Integration Notes

> **Note**: Full UX specification will be defined after backend is complete. Below are known visualization requirements.

### Required Visualizations (Phase 2)

#### 1. Faction Tier Chart
Visual ranking of all factions by performance within current epoch.

**Inspired by**: [Stat Check](https://www.stat-check.com/the-meta), tier list visualizations

**Data source**: `GET /factions/summary`

**Display elements**:
- Faction icons/names ranked by win rate or tier
- Color coding: S-tier (gold), A-tier (silver), B-tier (bronze), C/D-tier (grey)
- Visual bar/graph showing relative performance
- Win rate percentage displayed on each faction

#### 2. Companion Stats Table
Detailed metrics table displayed alongside the tier chart.

**Columns**:
| Column | Description |
|--------|-------------|
| Faction | Name + icon |
| Tier | S/A/B/C/D ranking |
| Win Rate | Percentage with delta from previous epoch |
| Wins / Losses / Draws | Raw counts |
| Games Played | Total games |
| Podium Rate | % of players reaching top 4 |
| Meta Share | % of total players |
| Over-Rep | Ratio of top-4 representation to player population |
| 4-0 Starts | Players starting 4-0 |
| 5-0 Starts | Players starting 5-0 |

#### 3. Win Rate Over Time (Future)
Line chart showing faction win rates across multiple epochs.

**Data source**: Compare `/factions/summary` across epochs

**Display**:
- X-axis: Time / Epoch
- Y-axis: Win rate (40%-60% typical range)
- One line per faction (toggleable)
- Vertical markers at epoch boundaries (balance updates)

### API Endpoint Mapping

| Visualization | Endpoint(s) |
|---------------|-------------|
| Faction Tier Chart | `/factions/summary` |
| Stats Table | `/factions/summary` |
| Win Rate Over Time | `/epochs` + `/factions/summary?epoch_id=X` per epoch |
| Faction Deep Dive | `/factions/{faction}/toplists` + `/derived/top-combos` |
| Event Browser | `/events` with pagination |
| List Viewer | `/lists/{list_id}` |
| Trend Analysis | `/derived/themes` + `/derived/top-combos` |
