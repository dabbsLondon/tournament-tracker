# Build Plan

This document outlines the staged implementation plan for the Warhammer Meta Agent.

## Stage Overview

| Stage | Focus | Deliverables |
|-------|-------|--------------|
| 1 | Documentation | All `/docs` files, architecture decisions |
| 2 | Project Scaffold + CI/CD | Rust project structure, GitHub Actions, coverage gates |
| 3 | Ingest + AI Extraction | Raw fetch, AI agents, JSONL storage, basic Parquet |
| 4 | API Server + Queries | Axum API, epoch-aware endpoints, Parquet queries |
| 5 | Derived Analysis | Theme scanner, top combos, trend calculations |

Each stage ends with a **Progress Check** and requires explicit approval before proceeding.

---

## Stage 1: Documentation First

**Goal**: Define architecture, data models, and contracts before writing code.

**Deliverables**:
- `docs/00_overview.md` — Project vision and architecture
- `docs/01_build_plan.md` — This document
- `docs/02_data_model.md` — Entity schemas and relationships
- `docs/03_agents.md` — AI agent definitions and contracts
- `docs/04_storage_layout.md` — Filesystem data lake structure
- `docs/05_api_spec.md` — REST API specification
- `docs/06_ops_and_scheduling.md` — CLI and operations guide
- `docs/07_cicd_and_quality.md` — CI/CD pipeline and quality gates

**Exit Criteria**: All docs reviewed and approved.

---

## Stage 2: Project Scaffold + CI/CD

**Goal**: Set up the Rust project structure and continuous integration.

**Deliverables**:
- Cargo workspace with module structure
- GitHub Actions workflow (PR + main push)
- Quality gates: fmt, clippy, tests, 80% coverage
- Initial tests (ID hashing, epoch mapping)
- Coverage report artifact upload

**Dependencies**: Stage 1 complete.

**Exit Criteria**: CI passes on initial commit with 80%+ coverage.

---

## Stage 3: Ingest + AI Extraction (First Source)

**Goal**: Implement the full ingestion pipeline for Goonhammer Competitive Innovations.

**Deliverables**:
- Raw HTML fetching with caching
- Balance Watcher agent (Warhammer Community PDFs)
- Event Scout agent (discover tournaments)
- Result Harvester agent (extract placements, lists)
- List Normalizer agent (canonical army list format)
- Fact Checker agent (validation layer)
- JSONL writer (normalized storage)
- Basic Parquet writer (at least events + placements)
- Fixture-based tests (no network calls in tests)

**Dependencies**: Stage 2 complete.

**Exit Criteria**: Can sync one week of Competitive Innovations data end-to-end.

---

## Stage 4: API Server + Queries

**Goal**: Expose tournament data via REST API.

**Deliverables**:
- Axum server setup
- All `/api/v1/*` endpoints implemented
- Parquet query layer (with JSONL fallback)
- Pagination and filtering
- Error response format
- Integration tests

**Dependencies**: Stage 3 complete (data exists to query).

**Exit Criteria**: All endpoints return correct data; integration tests pass.

---

## Stage 5: Derived Analysis

**Goal**: Compute meta statistics and trends.

**Deliverables**:
- Stats Engine (win rates, faction counts)
- Trend Analyzer (unit frequency over time)
- Combo Finder (common unit pairings)
- Theme Scanner (per-epoch themes)
- Derived artifact JSON files
- API endpoints for derived data

**Dependencies**: Stage 4 complete.

**Exit Criteria**: Derived endpoints return computed stats with confidence flags.

---

## Phase 2: Frontend (After Backend Complete)

Once the backend is fully functional (Stages 1-5 complete), a frontend will be developed.

**Status**: Specification TBD — detailed spec after backend is working.

**Known Requirements**:

| Visualization | Description |
|---------------|-------------|
| **Faction Tier Chart** | Visual ranking of factions by performance (S/A/B/C/D tiers), similar to [Stat Check](https://www.stat-check.com/the-meta) |
| **Companion Stats Table** | Detailed table with: wins, losses, draws, win rate, podium rate, meta share, over-representation, 4-0/5-0 starts |
| **Win Rate Over Time** | Line graph showing faction performance across epochs |
| **Event Browser** | Searchable list of tournaments with results |
| **List Viewer** | Detailed army list breakdown |
| **Trend Analysis** | Theme detection and unit combo displays |

**Design References**:
- [Stat Check Meta Dashboard](https://www.stat-check.com/the-meta) — tier visualization, heatmaps
- [40kstats.goonhammer.com](https://40kstats.goonhammer.com/) — faction breakdowns
- [Spikey Bits Tier Lists](https://spikeybits.com/best-worst-meta-armies/) — tier ranking format

**Full specification to be provided when backend is ready.**

---

## Future Enhancements (Post-MVP)

These are explicitly out of scope for MVP but documented for future consideration:

| Enhancement | Description |
|-------------|-------------|
| BCP Integration | Direct Best Coast Pairings data (subscription required) |
| Image List Parsing | AI extraction from army list screenshots |
| Multi-Game Support | Age of Sigmar, Kill Team |
| LLM Narrator | Prose generation from stats (feature-flagged) |
| Historical Backfill | Process archived Competitive Innovations articles |

---

## Risk Register

| Risk | Impact | Mitigation |
|------|--------|------------|
| Goonhammer format changes | Extraction fails | AI handles variation; store raw for re-processing |
| AI hallucinations | Bad data stored | Multi-agent validation + manual review queue |
| PDF extraction quality | Missing balance changes | Store raw PDF; re-extract when models improve |
| Local AI model quality | Poor extraction | Document tested models; allow remote API fallback |
| Coverage target (80%) | CI fails | Write tests alongside code, not after |

---

## Definition of Done (Per Stage)

Each stage must satisfy:

1. All deliverables complete
2. Tests pass with 80%+ coverage
3. No clippy warnings
4. Documentation updated if needed
5. Progress Check presented
6. Explicit approval received
