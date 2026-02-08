# AI Agents

This document defines the AI agents used for content discovery and extraction.

## Design Principles

1. **AI Discovers, Never Invents**: Agents extract facts from sources, never generate fictional data
2. **Multi-Agent Validation**: At least two agents review data before final storage
3. **Explicit Confidence**: Every extraction includes a confidence score
4. **Audit Trail**: Raw sources archived for re-extraction
5. **Graceful Degradation**: Flag for review when uncertain, don't fail silently

---

## AI Backend Configuration

### Default: Local (Ollama)

```toml
[ai]
backend = "ollama"
base_url = "http://localhost:11434"
model = "llama3.2"  # or mistral, mixtral
timeout_seconds = 120
max_retries = 3
```

### Optional: Remote API

```toml
[ai]
backend = "openai"  # or "anthropic"
api_key_env = "OPENAI_API_KEY"
model = "gpt-4o"
timeout_seconds = 60
max_retries = 3
```

Feature flag: `--features remote-ai`

---

## Agent Trait Definition

All agents implement a common trait:

```rust
#[async_trait]
pub trait Agent {
    type Input;
    type Output;
    type Error;

    /// Agent identifier for logging and metrics
    fn name(&self) -> &'static str;

    /// Execute the agent's task
    async fn execute(&self, input: Self::Input) -> Result<Self::Output, Self::Error>;

    /// Retry policy for this agent
    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy::default()
    }
}

pub struct AgentOutput<T> {
    pub data: T,
    pub confidence: Confidence,
    pub extraction_notes: Vec<String>,
    pub raw_source_path: PathBuf,
}

pub enum Confidence {
    High,
    Medium,
    Low,
}
```

---

## Discovery Agents

### Balance Watcher Agent

Monitors Warhammer Community for new balance updates and edition releases.

**Input**:
```rust
pub struct BalanceWatcherInput {
    pub known_events: Vec<SignificantEventId>,
    pub warhammer_community_url: Url,
}
```

**Output**:
```rust
pub struct BalanceWatcherOutput {
    pub new_events: Vec<AgentOutput<SignificantEvent>>,
    pub pdf_paths: Vec<PathBuf>,  // Downloaded PDFs
}
```

**Prompt Strategy**:
```
You are analyzing the Warhammer Community website for balance updates.

Look for:
1. "Balance Dataslate" announcements with PDF links
2. Edition release announcements (e.g., "10th Edition")
3. Major FAQ updates that affect competitive play

For each found, extract:
- Title (exact as shown)
- Date (publication date)
- PDF URL (if available)
- Brief summary of key changes

Return JSON format. If uncertain about any field, set confidence to "low".
Do NOT invent information not present on the page.
```

---

### Event Scout Agent

Discovers tournament events from Goonhammer Competitive Innovations.

**Input**:
```rust
pub struct EventScoutInput {
    pub article_html: String,
    pub article_url: Url,
    pub article_date: NaiveDate,
}
```

**Output**:
```rust
pub struct EventScoutOutput {
    pub events: Vec<AgentOutput<EventStub>>,
}

pub struct EventStub {
    pub name: String,
    pub date: Option<NaiveDate>,
    pub location: Option<String>,
    pub player_count: Option<u32>,
    pub round_count: Option<u32>,
}
```

**Prompt Strategy**:
```
You are extracting tournament information from a Goonhammer Competitive Innovations article.

For each tournament mentioned, extract:
- Event name (exact as written)
- Date (if mentioned)
- Location (city, country if available)
- Player count (number)
- Round count (number)

Tournaments are typically introduced with phrases like:
- "X-player, Y-round Major/GT in [Location]"
- "The [Event Name] was held..."

Return a JSON array. Use null for fields not found.
Set confidence based on how clearly the information was stated.
Do NOT guess or invent information.
```

---

### Result Harvester Agent

Extracts placement results and army lists from event coverage.

**Input**:
```rust
pub struct ResultHarvesterInput {
    pub article_html: String,
    pub event_stub: EventStub,
}
```

**Output**:
```rust
pub struct ResultHarvesterOutput {
    pub placements: Vec<AgentOutput<PlacementStub>>,
    pub raw_lists: Vec<RawListText>,
}

pub struct PlacementStub {
    pub rank: u32,
    pub player_name: String,
    pub faction: String,
    pub subfaction: Option<String>,
    pub detachment: Option<String>,
    pub record: Option<WinLossRecord>,
}

pub struct RawListText {
    pub placement_rank: u32,
    pub text: String,
}
```

**Prompt Strategy**:
```
You are extracting tournament results from a Goonhammer article section about [Event Name].

For each placing player, extract:
- Rank (1st, 2nd, 3rd, etc.)
- Player name
- Faction (e.g., "Aeldari", "Space Marines", "Death Guard")
- Subfaction (e.g., "Ynnari", "Black Templars") if mentioned
- Detachment name if mentioned
- Win/Loss/Draw record if shown

Also extract the full army list text for each player if present.
Lists typically appear in expandable sections or formatted blocks.

Return JSON. Use null for missing fields. Set confidence per placement.
Do NOT invent player names, factions, or results.
```

---

### List Normalizer Agent

Converts raw army list text to canonical format.

**Input**:
```rust
pub struct ListNormalizerInput {
    pub raw_text: String,
    pub faction_hint: Option<String>,
}
```

**Output**:
```rust
pub struct ListNormalizerOutput {
    pub list: AgentOutput<ArmyList>,
}
```

**Prompt Strategy**:
```
You are normalizing a Warhammer 40,000 army list into a structured format.

Given this raw list text, extract:
- Faction and subfaction
- Detachment name
- Total points
- Each unit with:
  - Unit name (canonical Games Workshop name)
  - Model count
  - Points cost
  - Wargear/upgrades selected
  - Keywords (if identifiable)

Handle various list formats:
- App exports (Battlescribe, New Recruit, official app)
- Plain text lists
- Abbreviated/shorthand notation

Return JSON. Preserve the original text in raw_text field.
If a unit name is unclear, include it as-is with confidence "low".
Do NOT add units not mentioned in the source text.
```

---

## Validation Agents

### Fact Checker Agent

Verifies extracted data against the original source.

**Input**:
```rust
pub struct FactCheckerInput {
    pub source_content: String,  // Original HTML/text
    pub extracted_data: serde_json::Value,  // What was extracted
    pub entity_type: EntityType,
}
```

**Output**:
```rust
pub struct FactCheckerOutput {
    pub verified: bool,
    pub discrepancies: Vec<Discrepancy>,
    pub suggested_corrections: Vec<Correction>,
    pub overall_confidence: Confidence,
}

pub struct Discrepancy {
    pub field: String,
    pub extracted_value: String,
    pub source_evidence: Option<String>,
    pub severity: Severity,  // Minor, Major, Critical
}
```

**Prompt Strategy**:
```
You are fact-checking extracted data against the original source.

Compare the extracted JSON against the source content.
For each field, verify it matches what's in the source.

Report:
- Fields that match exactly ✓
- Fields with minor discrepancies (typos, formatting)
- Fields with major discrepancies (wrong values)
- Fields that appear fabricated (not in source at all)

Return JSON with verification results.
Be strict: if you can't find evidence for a claim, flag it.
```

---

### Duplicate Detector Agent

Identifies potential duplicate entries.

**Input**:
```rust
pub struct DuplicateDetectorInput {
    pub candidate: Entity,
    pub existing_entities: Vec<EntitySummary>,
}
```

**Output**:
```rust
pub struct DuplicateDetectorOutput {
    pub is_duplicate: bool,
    pub matching_entity_id: Option<EntityId>,
    pub similarity_score: f32,
    pub match_reasons: Vec<String>,
}
```

**Prompt Strategy**:
```
You are checking if a new entity is a duplicate of existing entries.

New entity: [JSON]
Existing entities: [JSON array of summaries]

Consider:
- Name similarity (exact match, typos, abbreviations)
- Date match (same day or adjacent days)
- Location match (same city, country)
- Player count similarity

Return JSON indicating if this is likely a duplicate.
Err on the side of flagging potential duplicates for human review.
```

---

## Agent Pipeline

```
Source Content
      │
      ▼
┌─────────────┐
│ Event Scout │ ──────► EventStubs
└─────────────┘
      │
      ▼
┌─────────────────┐
│ Result Harvester│ ──────► PlacementStubs + RawLists
└─────────────────┘
      │
      ▼
┌────────────────┐
│ List Normalizer│ ──────► ArmyLists
└────────────────┘
      │
      ▼
┌──────────────┐     ┌───────────────────┐
│ Fact Checker │ ◄───│ Duplicate Detector│
└──────────────┘     └───────────────────┘
      │
      ▼
┌─────────────────────────────────────────┐
│ Decision Gate                           │
│ - High confidence + verified → Store    │
│ - Medium confidence → Store + flag      │
│ - Low/failed verification → Review queue│
└─────────────────────────────────────────┘
```

---

## Error Handling

Agents return structured errors, never panic:

```rust
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("AI backend unavailable: {0}")]
    BackendUnavailable(String),

    #[error("AI response unparseable: {0}")]
    ResponseParseError(String),

    #[error("AI refused to extract (content unclear): {0}")]
    ExtractionRefused(String),

    #[error("Timeout after {0} seconds")]
    Timeout(u64),

    #[error("Rate limited, retry after {0} seconds")]
    RateLimited(u64),
}
```

---

## Testing Strategy

1. **Fixture-Based Tests**: Pre-saved HTML/PDF content, no network calls
2. **Golden Output Tests**: Known-good extractions compared against agent output
3. **Prompt Regression Tests**: Ensure prompt changes don't degrade extraction quality
4. **Confidence Calibration**: Verify confidence scores correlate with actual accuracy

```rust
#[test]
fn test_event_scout_extracts_london_gt() {
    let html = include_str!("fixtures/competitive-innovations-2025-07.html");
    let output = event_scout.execute(input).await.unwrap();

    assert!(output.events.iter().any(|e| e.data.name == "London GT 2025"));
    assert!(output.events[0].confidence == Confidence::High);
}
```
