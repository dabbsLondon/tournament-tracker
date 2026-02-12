use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::NaiveDate;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use meta_agent::agents::backend::{AiBackend, OllamaBackend};
use meta_agent::agents::list_normalizer::{ListNormalizerAgent, ListNormalizerInput};
use meta_agent::agents::Agent;
use meta_agent::api::dedup_by_id;
use meta_agent::fetch::{Fetcher, FetcherConfig};
use meta_agent::ingest::{self, TestMockBackend};
use meta_agent::models::{
    ArmyList, Confidence, EpochMapper, SignificantEvent, SignificantEventType,
};
use meta_agent::storage::{
    read_significant_events, write_significant_events, EntityType, JsonlReader, JsonlWriter,
    StorageConfig,
};
use meta_agent::sync::{SyncConfig, SyncOrchestrator, SyncSource};

#[derive(Parser)]
#[command(name = "meta-agent")]
#[command(about = "Warhammer 40k meta tracker with AI-powered extraction")]
#[command(version)]
struct Cli {
    /// Path to configuration file
    #[arg(long, default_value = "./config.toml")]
    config: String,

    /// Data directory path
    #[arg(long, default_value = "./data")]
    data_dir: String,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Output logs as JSON
    #[arg(long)]
    json_logs: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Sync tournament data from sources
    Sync {
        /// Run sync once and exit
        #[arg(long)]
        once: bool,

        /// Run continuously at interval
        #[arg(long)]
        watch: bool,

        /// Sync interval (e.g., "6h", "30m")
        #[arg(long, default_value = "6h")]
        interval: String,

        /// Start date for sync range
        #[arg(long)]
        from: Option<String>,

        /// End date for sync range
        #[arg(long)]
        to: Option<String>,

        /// Only sync from this source
        #[arg(long)]
        source: Option<String>,

        /// Fetch and parse but don't store
        #[arg(long)]
        dry_run: bool,

        /// Process a single article URL directly (bypasses discovery)
        #[arg(long)]
        url: Option<String>,
    },

    /// Start the API server
    Serve {
        /// Bind address
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port number
        #[arg(long, default_value = "8080")]
        port: u16,

        /// Log all HTTP requests
        #[arg(long)]
        access_log: bool,
    },

    /// Rebuild Parquet files from JSONL
    BuildParquet {
        /// Epoch to rebuild
        #[arg(long)]
        epoch: Option<String>,

        /// Entity type to rebuild
        #[arg(long)]
        entity: Option<String>,

        /// Rebuild all epochs
        #[arg(long)]
        all: bool,
    },

    /// Compute derived analytics
    Derive {
        /// Epoch to analyze
        #[arg(long)]
        epoch: Option<String>,

        /// Derivations to run (comma-separated)
        #[arg(long)]
        run: Option<String>,

        /// Force recompute
        #[arg(long)]
        force: bool,
    },

    /// Manage review queue
    Review {
        #[command(subcommand)]
        action: ReviewAction,
    },

    /// Debug utilities
    Debug {
        #[command(subcommand)]
        action: DebugAction,
    },

    /// Normalize army lists using AI
    NormalizeLists {
        /// Only process lists that have empty units
        #[arg(long)]
        only_empty: bool,

        /// Dry run (don't write results)
        #[arg(long)]
        dry_run: bool,

        /// Max lists to process (for testing)
        #[arg(long)]
        limit: Option<usize>,

        /// Epoch to normalize (default: current epoch)
        #[arg(long)]
        epoch: Option<String>,

        /// Only process lists matching this faction (e.g. "Space Marines")
        #[arg(long)]
        faction: Option<String>,
    },

    /// Register a balance pass / significant event
    AddBalancePass {
        /// Date of the balance pass (YYYY-MM-DD)
        #[arg(long)]
        date: String,

        /// Title (e.g. "Balance Dataslate Q4 2025")
        #[arg(long)]
        title: String,

        /// Source URL
        #[arg(long, default_value = "")]
        source_url: String,

        /// PDF URL
        #[arg(long)]
        pdf_url: Option<String>,

        /// Event type: "balance" or "edition"
        #[arg(long, default_value = "balance")]
        event_type: String,
    },

    /// Discover balance passes from Warhammer Community
    DiscoverBalancePasses {
        /// Print what would be found without writing
        #[arg(long)]
        dry_run: bool,

        /// Override URL to fetch
        #[arg(long)]
        url: Option<String>,
    },

    /// Weekly update: fetch new results, check for balance passes, update epochs
    WeeklyUpdate {
        /// Print what would happen without writing
        #[arg(long)]
        dry_run: bool,

        /// How many days back to look for new articles (default 7)
        #[arg(long, default_value = "7")]
        days: u32,
    },

    /// Reclassify factions using the canonical taxonomy
    ReclassifyFactions {
        /// Epoch to reclassify (default: current). Use --all to reclassify every epoch.
        #[arg(long, default_value = "current")]
        epoch: String,

        /// Reclassify all epochs found in the normalized directory
        #[arg(long)]
        all: bool,

        /// Show what would change without writing
        #[arg(long)]
        dry_run: bool,
    },

    /// Repartition data by epoch
    Repartition {
        /// Show what would happen without writing
        #[arg(long)]
        dry_run: bool,

        /// Source epoch directory to repartition from
        #[arg(long, default_value = "current")]
        source: String,

        /// Keep original files after repartitioning
        #[arg(long)]
        keep_originals: bool,
    },
}

#[derive(Subcommand)]
enum ReviewAction {
    /// List pending review items
    List {
        #[arg(long)]
        entity_type: Option<String>,

        #[arg(long, default_value = "10")]
        limit: usize,
    },

    /// Show review item details
    Show { id: String },

    /// Resolve a review item
    Resolve {
        id: String,

        #[arg(long)]
        action: String,

        #[arg(long)]
        reason: Option<String>,
    },
}

#[derive(Subcommand)]
enum DebugAction {
    /// Parse a fixture file
    ParseFixture { path: String },

    /// Validate storage integrity
    ValidateStorage,

    /// Show epoch timeline
    Epochs,

    /// Check army list matching coverage
    CheckLists {
        /// Epoch to check (default: current)
        #[arg(long)]
        epoch: Option<String>,
    },

    /// Test ingestion from a fixture file
    TestIngest {
        /// Path to HTML fixture
        path: String,

        /// Type: "events" or "balance"
        #[arg(long, default_value = "events")]
        ingest_type: String,

        /// Use real Ollama backend (requires Ollama running)
        #[arg(long)]
        use_ollama: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&cli.log_level));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting meta-agent v{}", env!("CARGO_PKG_VERSION"));

    match cli.command {
        Commands::Sync {
            once,
            watch,
            interval: interval_str,
            from,
            to,
            source,
            dry_run,
            url: direct_url,
        } => {
            // Parse date range
            let date_from = from.map(|s| {
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .unwrap_or_else(|_| panic!("Invalid --from date (expected YYYY-MM-DD): {}", s))
            });
            let date_to = to.map(|s| {
                NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .unwrap_or_else(|_| panic!("Invalid --to date (expected YYYY-MM-DD): {}", s))
            });

            // Build source list
            let sources = match source.as_deref() {
                Some("goonhammer") | None => vec![SyncSource::default()],
                Some("warhammer-community") => vec![SyncSource::WarhammerCommunity {
                    url: "https://www.warhammer-community.com/en-gb/downloads/warhammer-40000/"
                        .to_string(),
                }],
                Some(other) => {
                    eprintln!(
                        "Unknown source: {}. Use 'goonhammer' or 'warhammer-community'.",
                        other
                    );
                    return Ok(());
                }
            };

            // Select backend
            let backend: Arc<dyn AiBackend> = select_backend();

            // Storage config
            let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));

            // Create fetcher with cache dir from storage config
            let fetcher = Fetcher::new(FetcherConfig {
                cache_dir: storage.raw_dir(),
                ..Default::default()
            })
            .expect("Failed to create fetcher");

            // Parse interval
            let sync_interval =
                parse_duration(&interval_str).unwrap_or(Duration::from_secs(6 * 3600));

            let sync_config = SyncConfig {
                sources,
                interval: sync_interval,
                date_from,
                date_to,
                dry_run,
                storage,
            };

            // Direct URL mode: process a single article without discovery
            if let Some(ref article_url) = direct_url {
                tracing::info!("Processing single article: {}", article_url);
                let article_url =
                    url::Url::parse(article_url).unwrap_or_else(|e| panic!("Invalid URL: {}", e));

                let sync_config_clone = sync_config.clone();
                let orchestrator = SyncOrchestrator::new(sync_config, fetcher, backend);
                match orchestrator
                    .process_single_article(
                        &article_url,
                        chrono::Utc::now().date_naive(),
                        &sync_config_clone,
                    )
                    .await
                {
                    Ok((events, placements, lists)) => {
                        println!("\n=== Single Article Results ===");
                        println!("Events found:     {}", events);
                        println!("Placements:       {}", placements);
                        println!("Lists found:      {}", lists);
                        if dry_run {
                            println!("\n(dry run - no data written to disk)");
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to process article: {}", e);
                    }
                }
                return Ok(());
            }

            let orchestrator = SyncOrchestrator::new(sync_config, fetcher, backend);

            if once {
                tracing::info!("Running one-time sync...");
                match orchestrator.sync_once().await {
                    Ok(result) => {
                        println!("\n=== Sync Results ===");
                        println!("Events synced:    {}", result.events_synced);
                        println!("Placements:       {}", result.placements_synced);
                        println!("Lists normalized: {}", result.lists_normalized);
                        println!("Duration:         {:?}", result.duration);
                        if dry_run {
                            println!("\n(dry run - no data written to disk)");
                        }
                        if !result.errors.is_empty() {
                            println!("\nErrors:");
                            for err in &result.errors {
                                println!("  - {}", err);
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!("Sync failed: {}", e);
                    }
                }
            } else if watch {
                tracing::info!("Running periodic sync (interval: {})...", interval_str);
                let orchestrator = Arc::new(orchestrator);
                orchestrator.run_periodic().await;
            } else {
                eprintln!("Specify --once or --watch");
            }
        }
        Commands::Serve { host, port, .. } => {
            let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));
            let epoch_mapper = match read_significant_events(&storage) {
                Ok(events) if !events.is_empty() => {
                    tracing::info!(
                        "Loaded {} significant events for epoch mapping",
                        events.len()
                    );
                    EpochMapper::from_significant_events(&events)
                }
                _ => EpochMapper::new(),
            };
            let state = meta_agent::api::state::AppState {
                storage: Arc::new(storage),
                epoch_mapper: Arc::new(epoch_mapper),
            };
            let app = meta_agent::api::build_router(state);
            let addr = format!("{}:{}", host, port);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            tracing::info!("Dashboard: http://{}", addr);
            axum::serve(listener, app).await?;
        }
        Commands::BuildParquet { .. } => {
            tracing::info!("Rebuilding Parquet files...");
            // TODO: Implement build-parquet command
            tracing::warn!("BuildParquet command not yet implemented");
        }
        Commands::Derive { .. } => {
            tracing::info!("Computing derived analytics...");
            // TODO: Implement derive command
            tracing::warn!("Derive command not yet implemented");
        }
        Commands::Review { action } => {
            match action {
                ReviewAction::List { .. } => {
                    tracing::info!("Listing review items...");
                }
                ReviewAction::Show { id } => {
                    tracing::info!("Showing review item: {}", id);
                }
                ReviewAction::Resolve { id, .. } => {
                    tracing::info!("Resolving review item: {}", id);
                }
            }
            // TODO: Implement review commands
            tracing::warn!("Review commands not yet implemented");
        }
        Commands::NormalizeLists {
            only_empty,
            dry_run,
            limit,
            epoch,
            faction,
        } => {
            let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));

            // Resolve epoch: use provided, or find the current one
            let epoch_id = epoch.unwrap_or_else(|| {
                let sig = read_significant_events(&storage).unwrap_or_default();
                if sig.is_empty() {
                    "current".to_string()
                } else {
                    let mapper = EpochMapper::from_significant_events(&sig);
                    mapper
                        .current_epoch()
                        .map(|e| e.id.as_str().to_string())
                        .unwrap_or_else(|| "current".to_string())
                }
            });
            tracing::info!("Normalizing lists in epoch: {}", epoch_id);

            // Read all army lists
            let reader =
                JsonlReader::<ArmyList>::for_entity(&storage, EntityType::ArmyList, &epoch_id);
            let lists = reader.read_all().expect("Failed to read army lists");
            let mut lists = dedup_by_id(lists, |l| l.id.as_str());

            let total = lists.len();
            tracing::info!("Loaded {} army lists", total);

            // Back up the file
            let src_path = storage
                .normalized_dir()
                .join(&epoch_id)
                .join("army_lists.jsonl");
            let bak_path = src_path.with_extension("jsonl.bak");
            if src_path.exists() && !dry_run {
                std::fs::copy(&src_path, &bak_path).expect("Failed to create backup");
                tracing::info!("Backed up to {:?}", bak_path);
            }

            // Select backend
            let backend: Arc<dyn AiBackend> = select_backend();
            let agent = ListNormalizerAgent::new(backend);

            // Normalize the faction filter for comparison
            let faction_filter = faction
                .as_deref()
                .map(meta_agent::api::routes::events::normalize_faction_name);

            // Determine which lists to process
            let indices: Vec<usize> = lists
                .iter()
                .enumerate()
                .filter(|(_, l)| !only_empty || l.units.is_empty())
                .filter(|(_, l)| match &faction_filter {
                    Some(ff) => meta_agent::api::routes::events::normalize_faction_name(&l.faction)
                        .eq_ignore_ascii_case(ff),
                    None => true,
                })
                .map(|(i, _)| i)
                .take(limit.unwrap_or(usize::MAX))
                .collect();

            let to_process = indices.len();
            tracing::info!(
                "Will normalize {} of {} lists{}",
                to_process,
                total,
                if dry_run { " (dry run)" } else { "" }
            );

            let mut normalized_count = 0u32;
            let mut error_count = 0u32;

            for (progress, &idx) in indices.iter().enumerate() {
                let list = &lists[idx];

                if list.raw_text.trim().is_empty() {
                    tracing::warn!(
                        "[{}/{}] Skipping list with empty raw_text",
                        progress + 1,
                        to_process
                    );
                    continue;
                }

                let input = ListNormalizerInput {
                    raw_text: list.raw_text.clone(),
                    faction_hint: if list.faction.is_empty()
                        || list.faction.contains("presents")
                        || list.faction.contains("GT")
                    {
                        None
                    } else {
                        Some(list.faction.clone())
                    },
                    player_name: format!("list-{}", idx),
                };

                match agent.execute(input).await {
                    Ok(output) => {
                        let result = output.list;
                        let norm = &result.data;

                        println!(
                            "[{}/{}] Normalized: {} - {} ({} units, {}pts)",
                            progress + 1,
                            to_process,
                            norm.faction,
                            norm.detachment.as_deref().unwrap_or("(none)"),
                            norm.units.len(),
                            norm.total_points,
                        );

                        if !dry_run {
                            let l = &mut lists[idx];
                            l.faction = norm.faction.clone();
                            l.subfaction = norm.subfaction.clone();
                            l.detachment = norm.detachment.clone();
                            l.total_points = norm.total_points;
                            l.units = norm.units.clone();
                            l.extraction_confidence = result.confidence;

                            // Regenerate ID based on new data
                            let mut unit_names: Vec<_> =
                                l.units.iter().map(|u| u.name.as_str()).collect();
                            unit_names.sort();
                            let units_str = unit_names.join(",");
                            l.id = meta_agent::models::EntityId::generate(&[
                                &l.faction,
                                l.detachment.as_deref().unwrap_or(""),
                                &units_str,
                                &l.total_points.to_string(),
                            ]);
                        }

                        normalized_count += 1;
                    }
                    Err(e) => {
                        tracing::error!(
                            "[{}/{}] Failed to normalize: {}",
                            progress + 1,
                            to_process,
                            e
                        );
                        error_count += 1;
                    }
                }

                // Rate limiting: 500ms delay between API calls
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            // Write results
            if !dry_run {
                let writer =
                    JsonlWriter::<ArmyList>::for_entity(&storage, EntityType::ArmyList, &epoch_id);
                writer
                    .write_all(&lists)
                    .expect("Failed to write normalized lists");
            }

            println!("\n=== Normalization Results ===");
            println!("Total lists:      {}", total);
            println!("Processed:        {}", to_process);
            println!("Normalized:       {}", normalized_count);
            println!("Errors:           {}", error_count);
            if !dry_run {
                println!("Backed up to:     {:?}", bak_path);
            } else {
                println!("(dry run - no data written to disk)");
            }
        }
        Commands::Debug { action } => {
            match action {
                DebugAction::ParseFixture { path } => {
                    tracing::info!("Parsing fixture: {}", path);
                }
                DebugAction::ValidateStorage => {
                    tracing::info!("Validating storage...");
                }
                DebugAction::Epochs => {
                    let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));
                    let events = read_significant_events(&storage).unwrap_or_default();
                    if events.is_empty() {
                        println!("No significant events registered.");
                        println!(
                            "Use `add-balance-pass` or `discover-balance-passes` to register epoch boundaries."
                        );
                    } else {
                        let mapper = EpochMapper::from_significant_events(&events);
                        println!(
                            "=== Epoch Timeline ({} epochs) ===\n",
                            mapper.all_epochs().len()
                        );
                        for epoch in mapper.all_epochs() {
                            let end = epoch
                                .end_date
                                .map(|d| d.to_string())
                                .unwrap_or_else(|| "now".to_string());
                            let current = if epoch.is_current { " [CURRENT]" } else { "" };
                            println!(
                                "  {} — {} to {}{}",
                                epoch.name, epoch.start_date, end, current
                            );
                            println!("    ID: {}", epoch.id);
                        }
                    }
                }
                DebugAction::CheckLists { epoch } => {
                    use meta_agent::api::routes::events::{
                        faction_match_score, normalize_faction_name,
                    };

                    let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));
                    let sig_events = read_significant_events(&storage).unwrap_or_default();
                    let mapper = EpochMapper::from_significant_events(&sig_events);
                    let epoch_id = epoch
                        .or_else(|| mapper.current_epoch().map(|e| e.id.as_str().to_string()))
                        .unwrap_or_else(|| "current".to_string());

                    let events: Vec<meta_agent::models::Event> =
                        JsonlReader::for_entity(&storage, EntityType::Event, &epoch_id)
                            .read_all()
                            .unwrap_or_default();
                    let events = dedup_by_id(events, |e| e.id.as_str());

                    let placements: Vec<meta_agent::models::Placement> =
                        JsonlReader::for_entity(&storage, EntityType::Placement, &epoch_id)
                            .read_all()
                            .unwrap_or_default();
                    let placements = dedup_by_id(placements, |p| p.id.as_str());

                    let lists: Vec<ArmyList> =
                        JsonlReader::for_entity(&storage, EntityType::ArmyList, &epoch_id)
                            .read_all()
                            .unwrap_or_default();
                    let lists = dedup_by_id(lists, |l| l.id.as_str());

                    let event_urls: std::collections::HashMap<String, String> = events
                        .iter()
                        .map(|e| (e.id.as_str().to_string(), e.source_url.clone()))
                        .collect();

                    println!("=== List Matching Report (epoch: {}) ===\n", epoch_id);
                    println!("Events: {}", events.len());
                    println!("Placements: {}", placements.len());
                    println!(
                        "Army lists: {} ({} with units)\n",
                        lists.len(),
                        lists.iter().filter(|l| !l.units.is_empty()).count()
                    );

                    let mut matched = 0u32;
                    let mut unmatched = 0u32;
                    let mut unmatched_details: Vec<String> = Vec::new();

                    // Check which top-4 placements have a matching list
                    for p in &placements {
                        if p.rank > 4 {
                            continue;
                        }
                        let event_url = event_urls
                            .get(p.event_id.as_str())
                            .cloned()
                            .unwrap_or_default();
                        let event_lists: Vec<&ArmyList> = lists
                            .iter()
                            .filter(|l| l.source_url.as_deref() == Some(event_url.as_str()))
                            .collect();

                        // Try player name match
                        let name_match = event_lists.iter().any(|l| {
                            l.player_name.as_ref().is_some_and(|name| {
                                let norm = |s: &str| {
                                    s.split_whitespace()
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                        .to_lowercase()
                                };
                                norm(&p.player_name) == norm(name)
                            })
                        });

                        // Try faction+detachment match
                        let faction_match = event_lists.iter().any(|l| {
                            let score = faction_match_score(
                                &normalize_faction_name(&p.faction),
                                &normalize_faction_name(&l.faction),
                            );
                            let det_match = match (&p.detachment, &l.detachment) {
                                (Some(pd), Some(ld)) => pd.eq_ignore_ascii_case(ld),
                                _ => false,
                            };
                            score >= 3 || (score >= 2 && det_match)
                        });

                        if name_match || faction_match {
                            matched += 1;
                        } else {
                            unmatched += 1;
                            let event_name = events
                                .iter()
                                .find(|e| e.id == p.event_id)
                                .map(|e| e.name.as_str())
                                .unwrap_or("?");
                            unmatched_details.push(format!(
                                "  #{} {} ({}, {}) — {}",
                                p.rank,
                                p.player_name,
                                p.faction,
                                p.detachment.as_deref().unwrap_or("-"),
                                event_name
                            ));
                        }
                    }

                    let total = matched + unmatched;
                    let pct = if total > 0 {
                        (matched as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };
                    println!("Top-4 placements: {}", total);
                    println!("Matched to list:  {} ({:.1}%)", matched, pct);
                    println!("Unmatched:        {}\n", unmatched);

                    if !unmatched_details.is_empty() {
                        println!("Unmatched placements:");
                        for d in &unmatched_details {
                            println!("{}", d);
                        }
                    }

                    // Check for duplicate faction names
                    let mut faction_names: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut dupes: Vec<String> = Vec::new();
                    for p in &placements {
                        let norm = normalize_faction_name(&p.faction);
                        if norm != p.faction && !faction_names.contains(&p.faction) {
                            dupes.push(format!("  \"{}\" → \"{}\"", p.faction, norm));
                        }
                        faction_names.insert(p.faction.clone());
                    }
                    if !dupes.is_empty() {
                        println!("\nFaction name normalization needed:");
                        for d in &dupes {
                            println!("{}", d);
                        }
                    }

                    // Exit with error if match rate is below threshold
                    if pct < 50.0 {
                        println!("\nWARNING: List match rate below 50%!");
                        std::process::exit(1);
                    }
                }
                DebugAction::TestIngest {
                    path,
                    ingest_type,
                    use_ollama,
                } => {
                    let backend: Arc<dyn AiBackend> = if use_ollama {
                        tracing::info!("Using Ollama backend...");
                        let ollama = OllamaBackend::new(
                            "http://localhost:11434".to_string(),
                            "llama3.2".to_string(),
                            120,
                        );

                        if !ingest::check_backend(&ollama).await {
                            tracing::error!(
                                "Ollama not available. Start Ollama or use --use-ollama=false"
                            );
                            return Ok(());
                        }
                        Arc::new(ollama)
                    } else {
                        tracing::info!("Using mock backend (for testing without AI)...");
                        Arc::new(TestMockBackend::new())
                    };

                    let result = match ingest_type.as_str() {
                        "balance" => ingest::ingest_balance_update(&path, backend).await,
                        _ => ingest::ingest_from_fixture(&path, backend).await,
                    };

                    match result {
                        Ok(r) => {
                            println!("\n=== Ingestion Results ===");
                            println!("Events found: {}", r.events_found);
                            println!("Placements found: {}", r.placements_found);
                            println!("Lists found: {}", r.lists_found);
                            if !r.errors.is_empty() {
                                println!("\nErrors:");
                                for err in &r.errors {
                                    println!("  - {}", err);
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!("Ingestion failed: {}", e);
                        }
                    }
                }
            }
        }
        Commands::AddBalancePass {
            date,
            title,
            source_url,
            pdf_url,
            event_type,
        } => {
            let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));
            let date = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
                .unwrap_or_else(|_| panic!("Invalid --date (expected YYYY-MM-DD): {}", date));

            let evt_type = match event_type.as_str() {
                "edition" => SignificantEventType::EditionRelease,
                _ => SignificantEventType::BalanceUpdate,
            };

            let mut event = SignificantEvent::new(evt_type, date, title.clone(), source_url)
                .with_confidence(Confidence::High);
            if let Some(url) = pdf_url {
                event = event.with_pdf_url(url);
            }

            // Read existing, check for duplicates
            let mut existing = read_significant_events(&storage).unwrap_or_default();
            let dup = existing.iter().any(|e| e.id == event.id);
            if dup {
                println!("Duplicate: event with same type+date+title already exists.");
                return Ok(());
            }

            existing.push(event);
            write_significant_events(&storage, &mut existing)?;

            let mapper = EpochMapper::from_significant_events(&existing);
            println!("Registered balance pass: {} ({})", title, date);
            println!(
                "\n=== Epoch Timeline ({} epochs) ===\n",
                mapper.all_epochs().len()
            );
            for epoch in mapper.all_epochs() {
                let end = epoch
                    .end_date
                    .map(|d| d.to_string())
                    .unwrap_or_else(|| "now".to_string());
                let current = if epoch.is_current { " [CURRENT]" } else { "" };
                println!(
                    "  {} — {} to {}{}",
                    epoch.name, epoch.start_date, end, current
                );
            }
        }
        Commands::DiscoverBalancePasses { dry_run, url } => {
            let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));
            let page_url = url.unwrap_or_else(|| {
                "https://www.warhammer-community.com/en-gb/downloads/warhammer-40000/".to_string()
            });

            let backend: Arc<dyn AiBackend> = select_backend();

            let fetcher = Fetcher::new(FetcherConfig {
                cache_dir: storage.raw_dir(),
                ..Default::default()
            })
            .expect("Failed to create fetcher");

            let page_url =
                url::Url::parse(&page_url).unwrap_or_else(|e| panic!("Invalid URL: {}", e));

            // Fetch and run BalanceWatcherAgent
            let fetch_result = fetcher.fetch(&page_url).await?;
            let html = fetcher.read_cached_text(&fetch_result).await?;

            use meta_agent::agents::balance_watcher::{BalanceWatcherAgent, BalanceWatcherInput};
            let existing = read_significant_events(&storage).unwrap_or_default();
            let known_ids: Vec<_> = existing.iter().map(|e| e.id.clone()).collect();

            let watcher = BalanceWatcherAgent::new(backend);
            let input = BalanceWatcherInput {
                html_content: html,
                source_url: page_url.to_string(),
                known_event_ids: known_ids,
            };

            let output = watcher.execute(input).await?;
            println!("Discovered {} balance events", output.events.len());

            if !output.events.is_empty() {
                let mut merged = existing;
                let existing_ids: std::collections::HashSet<String> =
                    merged.iter().map(|e| e.id.as_str().to_string()).collect();

                let mut new_count = 0;
                for evt in &output.events {
                    if !existing_ids.contains(evt.data.id.as_str()) {
                        merged.push(evt.data.clone());
                        new_count += 1;
                    }
                }

                if !dry_run && new_count > 0 {
                    write_significant_events(&storage, &mut merged)?;
                    println!("Added {} new events ({} total)", new_count, merged.len());
                } else if dry_run {
                    println!("(dry run — {} new events would be added)", new_count);
                } else {
                    println!("No new events to add.");
                }

                let mapper = EpochMapper::from_significant_events(&merged);
                println!(
                    "\n=== Epoch Timeline ({} epochs) ===\n",
                    mapper.all_epochs().len()
                );
                for epoch in mapper.all_epochs() {
                    let end = epoch
                        .end_date
                        .map(|d| d.to_string())
                        .unwrap_or_else(|| "now".to_string());
                    let current = if epoch.is_current { " [CURRENT]" } else { "" };
                    println!(
                        "  {} — {} to {}{}",
                        epoch.name, epoch.start_date, end, current
                    );
                }
            }
        }
        Commands::WeeklyUpdate { dry_run, days } => {
            let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));
            let backend: Arc<dyn AiBackend> = select_backend();

            let fetcher = Fetcher::new(FetcherConfig {
                cache_dir: storage.raw_dir(),
                ..Default::default()
            })
            .expect("Failed to create fetcher");

            let today = chrono::Utc::now().date_naive();
            let from_date = today - chrono::Days::new(days as u64);

            println!("=== Weekly Update ({} to {}) ===\n", from_date, today);

            // ── Step 1: Check for balance passes ──
            println!("Step 1: Checking for balance passes...");
            let wh_url = "https://www.warhammer-community.com/en-gb/downloads/warhammer-40000/";
            let balance_page_url =
                url::Url::parse(wh_url).expect("Invalid Warhammer Community URL");

            let mut new_balance_passes = 0u32;
            match fetcher.fetch(&balance_page_url).await {
                Ok(fetch_result) => match fetcher.read_cached_text(&fetch_result).await {
                    Ok(html) => {
                        use meta_agent::agents::balance_watcher::{
                            BalanceWatcherAgent, BalanceWatcherInput,
                        };
                        let watcher = BalanceWatcherAgent::new(backend.clone());
                        let existing = read_significant_events(&storage).unwrap_or_default();
                        let known_ids: Vec<meta_agent::models::EntityId> =
                            existing.iter().map(|e| e.id.clone()).collect();
                        let input = BalanceWatcherInput {
                            html_content: html,
                            source_url: wh_url.to_string(),
                            known_event_ids: known_ids,
                        };
                        match watcher.execute(input).await {
                            Ok(output) => {
                                new_balance_passes = output.events.len() as u32;
                                if new_balance_passes > 0 {
                                    println!(
                                        "  Found {} new balance pass(es)!",
                                        new_balance_passes
                                    );
                                    if !dry_run {
                                        let mut all_events = existing;
                                        let existing_ids: std::collections::HashSet<String> =
                                            all_events
                                                .iter()
                                                .map(|e| e.id.as_str().to_string())
                                                .collect();
                                        for event_output in &output.events {
                                            println!(
                                                "    - {} ({})",
                                                event_output.data.title, event_output.data.date
                                            );
                                            if !existing_ids.contains(event_output.data.id.as_str())
                                            {
                                                all_events.push(event_output.data.clone());
                                            }
                                        }
                                        if let Err(e) =
                                            write_significant_events(&storage, &mut all_events)
                                        {
                                            tracing::error!(
                                                "Failed to write significant events: {}",
                                                e
                                            );
                                        }
                                    }
                                } else {
                                    println!("  No new balance passes found.");
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Balance watcher failed: {}", e);
                                println!("  Balance watcher failed: {}", e);
                            }
                        }
                    }
                    Err(e) => println!("  Failed to read page: {}", e),
                },
                Err(e) => println!("  Failed to fetch Warhammer Community: {}", e),
            }

            // ── Step 2: Sync new tournament results ──
            println!(
                "\nStep 2: Syncing tournament results ({} to {})...",
                from_date, today
            );

            let sync_config = SyncConfig {
                sources: vec![SyncSource::default()],
                interval: Duration::from_secs(3600),
                date_from: Some(from_date),
                date_to: Some(today),
                dry_run,
                storage: storage.clone(),
            };

            let orchestrator = SyncOrchestrator::new(sync_config, fetcher, backend);
            match orchestrator.sync_once().await {
                Ok(result) => {
                    println!("  Events:     {}", result.events_synced);
                    println!("  Placements: {}", result.placements_synced);
                    println!("  Lists:      {}", result.lists_normalized);
                    if !result.errors.is_empty() {
                        println!("  Errors:     {}", result.errors.len());
                        for err in &result.errors {
                            println!("    - {}", err);
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Sync failed: {}", e);
                    println!("  Sync failed: {}", e);
                }
            }

            // ── Step 3: Repartition if new balance pass found ──
            if new_balance_passes > 0 && !dry_run {
                println!("\nStep 3: Repartitioning data into new epochs...");
                match meta_agent::sync::repartition::repartition(&storage, "current", false, false)
                {
                    Ok(result) => {
                        let mut all_epochs: Vec<_> = result.events_by_epoch.keys().collect();
                        all_epochs.sort();
                        for epoch in &all_epochs {
                            println!(
                                "  {}: {} events, {} placements, {} lists",
                                epoch,
                                result.events_by_epoch.get(*epoch).unwrap_or(&0),
                                result.placements_by_epoch.get(*epoch).unwrap_or(&0),
                                result.lists_by_epoch.get(*epoch).unwrap_or(&0),
                            );
                        }
                    }
                    Err(e) => {
                        tracing::error!("Repartition failed: {}", e);
                        println!("  Repartition failed: {}", e);
                    }
                }
            } else if new_balance_passes > 0 {
                println!("\nStep 3: Would repartition data (dry run).");
            } else {
                println!("\nStep 3: No repartition needed (no new balance passes).");
            }

            if dry_run {
                println!("\n(dry run — no data written to disk)");
            }

            println!("\n=== Weekly update complete ===");
        }
        Commands::ReclassifyFactions {
            epoch,
            all,
            dry_run,
        } => {
            use meta_agent::api::routes::events::resolve_faction;

            let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));

            // Build list of epoch IDs to process
            let epoch_ids: Vec<String> = if all {
                // Find all epoch directories in normalized/
                let norm_dir = storage.normalized_dir();
                let mut ids = Vec::new();
                if let Ok(entries) = std::fs::read_dir(&norm_dir) {
                    for entry in entries.flatten() {
                        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                            if let Some(name) = entry.file_name().to_str() {
                                ids.push(name.to_string());
                            }
                        }
                    }
                }
                ids.sort();
                ids
            } else if epoch == "current" {
                let sig = read_significant_events(&storage).unwrap_or_default();
                let resolved = if sig.is_empty() {
                    "current".to_string()
                } else {
                    let mapper = EpochMapper::from_significant_events(&sig);
                    mapper
                        .current_epoch()
                        .map(|e| e.id.as_str().to_string())
                        .unwrap_or_else(|| "current".to_string())
                };
                // Include both the resolved epoch and literal "current" if they differ
                let norm_dir = storage.normalized_dir();
                let mut ids = vec![resolved.clone()];
                if resolved != "current" && norm_dir.join("current").is_dir() {
                    ids.push("current".to_string());
                }
                ids
            } else {
                vec![epoch]
            };

            let mut grand_p_total = 0u32;
            let mut grand_p_changed = 0u32;
            let mut grand_l_total = 0u32;
            let mut grand_l_changed = 0u32;

            for epoch_id in &epoch_ids {
                println!("=== Reclassify Factions (epoch: {}) ===\n", epoch_id);

                // ── Process placements ──
                let placement_reader = JsonlReader::<meta_agent::models::Placement>::for_entity(
                    &storage,
                    meta_agent::storage::EntityType::Placement,
                    epoch_id,
                );
                let placements = match placement_reader.read_all() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::warn!("Skipping placements for epoch {}: {}", epoch_id, e);
                        Vec::new()
                    }
                };
                let mut placements = dedup_by_id(placements, |p| p.id.as_str());

                let placement_path = storage
                    .normalized_dir()
                    .join(epoch_id)
                    .join("placements.jsonl");
                if placement_path.exists() && !dry_run && !placements.is_empty() {
                    let bak = placement_path.with_extension("jsonl.pre-reclassify.bak");
                    std::fs::copy(&placement_path, &bak).expect("Failed to backup placements");
                }

                let mut p_changed = 0u32;
                let p_total = placements.len() as u32;
                for p in &mut placements {
                    let resolved = resolve_faction(&p.faction, p.subfaction.as_deref());
                    let mut changed = false;
                    if p.faction != resolved.faction {
                        if dry_run {
                            println!(
                                "  [placement] #{} {} — faction: \"{}\" → \"{}\"",
                                p.rank, p.player_name, p.faction, resolved.faction
                            );
                        }
                        p.faction = resolved.faction.clone();
                        changed = true;
                    }
                    if p.subfaction != resolved.subfaction {
                        if dry_run && (p.subfaction.is_some() || resolved.subfaction.is_some()) {
                            println!(
                                "  [placement] #{} {} — subfaction: {:?} → {:?}",
                                p.rank, p.player_name, p.subfaction, resolved.subfaction
                            );
                        }
                        p.subfaction = resolved.subfaction.clone();
                        changed = true;
                    }
                    if p.allegiance.as_deref() != Some(&resolved.allegiance) {
                        p.allegiance = Some(resolved.allegiance.clone());
                        changed = true;
                    }
                    if changed {
                        p_changed += 1;
                    }
                }

                if !dry_run && !placements.is_empty() {
                    let writer = meta_agent::storage::JsonlWriter::<meta_agent::models::Placement>::for_entity(
                        &storage, meta_agent::storage::EntityType::Placement, epoch_id);
                    writer
                        .write_all(&placements)
                        .expect("Failed to write placements");
                }

                // ── Process army lists ──
                let list_reader = JsonlReader::<ArmyList>::for_entity(
                    &storage,
                    meta_agent::storage::EntityType::ArmyList,
                    epoch_id,
                );
                let lists = match list_reader.read_all() {
                    Ok(l) => l,
                    Err(e) => {
                        tracing::warn!("Skipping army lists for epoch {}: {}", epoch_id, e);
                        Vec::new()
                    }
                };
                let mut lists = dedup_by_id(lists, |l| l.id.as_str());

                let list_path = storage
                    .normalized_dir()
                    .join(epoch_id)
                    .join("army_lists.jsonl");
                if list_path.exists() && !dry_run && !lists.is_empty() {
                    let bak = list_path.with_extension("jsonl.pre-reclassify.bak");
                    std::fs::copy(&list_path, &bak).expect("Failed to backup army lists");
                }

                let mut l_changed = 0u32;
                let l_total = lists.len() as u32;
                for l in &mut lists {
                    let resolved = resolve_faction(&l.faction, l.subfaction.as_deref());
                    let mut changed = false;
                    if l.faction != resolved.faction {
                        if dry_run {
                            println!(
                                "  [list] {} — faction: \"{}\" → \"{}\"",
                                l.player_name.as_deref().unwrap_or("?"),
                                l.faction,
                                resolved.faction
                            );
                        }
                        l.faction = resolved.faction.clone();
                        changed = true;
                    }
                    if l.subfaction != resolved.subfaction {
                        l.subfaction = resolved.subfaction.clone();
                        changed = true;
                    }
                    if l.allegiance.as_deref() != Some(&resolved.allegiance) {
                        l.allegiance = Some(resolved.allegiance.clone());
                        changed = true;
                    }
                    if changed {
                        l_changed += 1;
                    }
                }

                if !dry_run && !lists.is_empty() {
                    let writer = meta_agent::storage::JsonlWriter::<ArmyList>::for_entity(
                        &storage,
                        meta_agent::storage::EntityType::ArmyList,
                        epoch_id,
                    );
                    writer
                        .write_all(&lists)
                        .expect("Failed to write army lists");
                }

                println!("  Placements: {} total, {} changed", p_total, p_changed);
                println!("  Army lists: {} total, {} changed\n", l_total, l_changed);

                grand_p_total += p_total;
                grand_p_changed += p_changed;
                grand_l_total += l_total;
                grand_l_changed += l_changed;
            }

            println!("=== Reclassify Results ({} epochs) ===", epoch_ids.len());
            println!(
                "Placements: {} total, {} changed",
                grand_p_total, grand_p_changed
            );
            println!(
                "Army lists: {} total, {} changed",
                grand_l_total, grand_l_changed
            );
            if dry_run {
                println!("\n(dry run — no data written to disk)");
            }
        }
        Commands::Repartition {
            dry_run,
            source,
            keep_originals,
        } => {
            let storage = StorageConfig::new(std::path::PathBuf::from(&cli.data_dir));
            match meta_agent::sync::repartition::repartition(
                &storage,
                &source,
                dry_run,
                keep_originals,
            ) {
                Ok(result) => {
                    println!("\n=== Repartition Results ===");
                    let mut all_epochs: Vec<_> = result.events_by_epoch.keys().collect();
                    all_epochs.sort();
                    for epoch in &all_epochs {
                        println!(
                            "  {}: {} events, {} placements, {} lists",
                            epoch,
                            result.events_by_epoch.get(*epoch).unwrap_or(&0),
                            result.placements_by_epoch.get(*epoch).unwrap_or(&0),
                            result.lists_by_epoch.get(*epoch).unwrap_or(&0),
                        );
                    }
                    if dry_run {
                        println!("\n(dry run — no data written to disk)");
                    }
                }
                Err(e) => {
                    tracing::error!("Repartition failed: {}", e);
                }
            }
        }
    }

    Ok(())
}

/// Select the best available AI backend.
///
/// When the `remote-ai` feature is active and `ANTHROPIC_API_KEY` is set,
/// uses AnthropicBackend. Otherwise falls back to OllamaBackend.
fn select_backend() -> Arc<dyn AiBackend> {
    #[cfg(feature = "remote-ai")]
    {
        if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
            tracing::info!("Using Anthropic backend (claude-sonnet-4-20250514)");
            return Arc::new(meta_agent::agents::backend::AnthropicBackend::new(
                api_key,
                "claude-sonnet-4-20250514".to_string(),
                120,
            ));
        }
    }

    tracing::info!("Using Ollama backend (llama3.2)");
    Arc::new(OllamaBackend::new(
        "http://localhost:11434".to_string(),
        "llama3.2".to_string(),
        120,
    ))
}

/// Parse a human-friendly duration string (e.g., "6h", "30m", "90s").
fn parse_duration(s: &str) -> Option<Duration> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let (num_str, multiplier) = if let Some(n) = s.strip_suffix('h') {
        (n, 3600)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 1)
    } else {
        // Default to seconds
        (s, 1)
    };

    let num: u64 = num_str.parse().ok()?;
    Some(Duration::from_secs(num * multiplier))
}
