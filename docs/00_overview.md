# Warhammer Meta Agent — Overview

## Project Vision

The Warhammer Meta Agent is a **local, read-only meta tracker** for Warhammer 40,000 competitive play. It automatically discovers tournament results and balance updates from public sources, stores them locally, and calculates meta statistics — all without inventing or guessing data.

### Core Philosophy

1. **AI Discovers, Never Invents**: AI agents extract facts from public sources (HTML, PDFs). They never generate fictional data.
2. **Rust Calculates, AI Doesn't**: All statistics, trends, and meta analysis are computed deterministically by Rust code — not AI inference.
3. **Multi-Agent Validation**: Multiple AI agents cross-check extracted data to catch hallucinations before storage.
4. **Conservative Outputs**: When uncertain, flag for manual review rather than guess.
5. **Local-First**: Runs entirely on your machine. No paid SaaS dependencies. No database server.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        PUBLIC SOURCES                               │
│  Goonhammer Competitive Innovations | Warhammer Community PDFs      │
└─────────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────────┐
│                     DISCOVERY AGENTS (AI-Powered)                   │
│  Balance Watcher | Event Scout | Result Harvester | List Normalizer │
│                                                                     │
│  → Extract structured data from unstructured content                │
│  → No brittle parsers — AI handles format variations                │
└─────────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────────┐
│                     VALIDATION AGENTS (AI Cross-Check)              │
│  Fact Checker | Duplicate Detector                                  │
│                                                                     │
│  → Verify extracted data against source                             │
│  → Flag inconsistencies for manual review                           │
└─────────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────────┐
│                     GROUND TRUTH STORAGE                            │
│  Raw HTML/PDFs | Normalized JSONL | Parquet Analytics               │
│                                                                     │
│  → Append-only diary of events and results                          │
│  → Review queue for flagged items                                   │
└─────────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────────┐
│                     RUST CALCULATION ENGINE                         │
│  Stats Engine | Trend Analyzer | Combo Finder | Epoch Aggregator    │
│                                                                     │
│  → Deterministic calculations from stored facts                     │
│  → Win rates, faction popularity, unit frequency                    │
└─────────────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────────────┐
│                     REST API → RICH UX                              │
│  Epoch-aware endpoints | Paginated queries | Derived artifacts      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Why AI-First (No Parsers)

Traditional web scrapers break when sites change their HTML structure. This project takes a different approach:

| Traditional Approach | Our Approach |
|---------------------|--------------|
| Regex/XPath parsers for each site | AI agents interpret content semantically |
| Breaks on layout changes | Robust to format variations |
| Requires constant maintenance | Self-adapting extraction |
| Hardcoded faction lists | AI identifies factions from context |

The AI reads the content like a human would, extracting structured data without relying on specific HTML tags or CSS classes.

---

## Meta Epochs

The competitive meta is segmented into **epochs** — time periods between significant events:

| Significant Event Type | What It Means |
|------------------------|---------------|
| **Balance Update** | Points changes, rules errata, datasheet updates from Warhammer Community |
| **Edition Release** | New edition launch (e.g., 10th Edition) — hard meta reset |

All tournament results are tagged with their epoch. Analysis runs within epochs to provide meaningful comparisons.

---

## Data Sources

### Primary (MVP)

| Source | Content | Access |
|--------|---------|--------|
| [Goonhammer Competitive Innovations](https://www.goonhammer.com/category/columns/40k-competitive-innovations/) | Weekly tournament roundups, placements, army lists | Public HTML |
| [Warhammer Community](https://www.warhammer-community.com/) | Balance Dataslate PDFs, edition announcements | Public PDF download |

### Future Expansion

| Source | Content | Notes |
|--------|---------|-------|
| [40kstats.goonhammer.com](https://40kstats.goonhammer.com/) | Pre-computed win rates | Enrichment/validation |
| [Stat-Check](https://www.stat-check.com/the-meta) | Meta dashboard | Cross-reference |
| [Best Coast Pairings](https://www.bestcoastpairings.com/) | Raw tournament data | May require subscription |

---

## What This Project Is NOT

- **Not a scraper farm**: We respect source sites and rate-limit requests
- **Not a prediction engine**: We report facts, not forecasts
- **Not a paid service**: Runs locally, no SaaS dependencies
- **Not inventing data**: AI extracts, never fabricates
- **Not gated content**: Only processes clearly public pages

---

## Technology Stack

| Component | Technology |
|-----------|------------|
| Language | Rust 2021 |
| AI Backend | Ollama (local, default) or OpenAI/Anthropic API (optional) |
| Storage | Filesystem data lake (JSONL + Parquet) |
| API Framework | Axum |
| Async Runtime | Tokio |
| Error Handling | anyhow + thiserror |
| Observability | tracing |

---

## Development Phases

| Phase | Focus | Status |
|-------|-------|--------|
| **Phase 1** | Backend (Rust) — Ingestion, storage, API, calculations | Current |
| **Phase 2** | Frontend (TBD) — Rich UX for visualizing meta data | After backend complete |

**Frontend Note**: The frontend specification will be defined once the backend is fully functional. The API is designed to be frontend-agnostic and will support any modern web framework.

---

## Quick Start (After Build)

```bash
# Sync tournament data (one-time)
meta-agent sync --once

# Start API server
meta-agent serve --port 8080

# View current epoch stats
curl http://localhost:8080/api/v1/epochs/current
```

See [docs/06_ops_and_scheduling.md](./06_ops_and_scheduling.md) for full CLI reference.
