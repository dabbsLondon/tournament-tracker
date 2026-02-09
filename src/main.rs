use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use meta_agent::agents::backend::{AiBackend, OllamaBackend};
use meta_agent::ingest::{self, TestMockBackend};

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
        Commands::Sync { once, .. } => {
            if once {
                tracing::info!("Running one-time sync...");
            }
            // TODO: Implement sync command
            tracing::warn!("Sync command not yet implemented");
        }
        Commands::Serve { host, port, .. } => {
            tracing::info!("Starting server on {}:{}", host, port);
            // TODO: Implement serve command
            tracing::warn!("Serve command not yet implemented");
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
        Commands::Debug { action } => {
            match action {
                DebugAction::ParseFixture { path } => {
                    tracing::info!("Parsing fixture: {}", path);
                }
                DebugAction::ValidateStorage => {
                    tracing::info!("Validating storage...");
                }
                DebugAction::Epochs => {
                    tracing::info!("Showing epoch timeline...");
                    // TODO: Implement epochs display
                    tracing::warn!("Epochs display not yet implemented");
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
    }

    Ok(())
}
