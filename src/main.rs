pub mod commands;
pub mod db;
pub mod parser;

use clap::{Parser, Subcommand};
use std::error::Error;

#[derive(Parser, Debug)]
#[command(name = "ochna")]
#[command(author, version, about = "Code graph indexing and analysis tool", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    /// Emit machine-readable JSON results on stdout instead of human text
    #[arg(long, global = true)]
    json: bool,
    /// Exclude symbols classified as test code from query results
    #[arg(long = "no-tests", global = true)]
    no_tests: bool,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize the code graph database and scan the project
    Init {
        /// Include vendored/build/library directories such as target, node_modules, .venv, vendor, build, and dist
        #[arg(long = "include-library")]
        include_library: bool,
    },
    /// Sync the code graph database with incremental updates for modified files
    Sync {
        /// Include vendored/build/library directories such as target, node_modules, .venv, vendor, build, and dist
        #[arg(long = "include-library")]
        include_library: bool,
    },
    /// Display index statistics
    Status,
    /// List indexed files with metadata
    Files,
    /// Search for nodes/symbols matching a query string
    Search {
        /// The search query
        query: String,
    },
    /// Find callers of a given symbol
    Callers {
        /// The name or ID of the symbol to query
        symbol: String,
    },
    /// Inspect details of a file or a symbol
    Node {
        /// The relative path of the file to inspect
        #[arg(long)]
        file: Option<String>,
        /// 1-based start line number for file mode
        #[arg(long)]
        offset: Option<i64>,
        /// Number of lines to read in file mode
        #[arg(long)]
        limit: Option<i64>,
        /// If true, only list the symbols in the file
        #[arg(long = "symbols-only")]
        symbols_only: bool,
        /// The name or ID of the symbol to query
        #[arg(long)]
        symbol: Option<String>,
        /// If true, include the source code of the symbol
        #[arg(long = "include-code")]
        include_code: bool,
        /// Specific line number to filter by (symbol mode only)
        #[arg(long)]
        line: Option<i64>,
    },
    /// Explore the codebase using FTS and show relationships
    Explore {
        /// Query terms to search for nodes
        query: String,
    },
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    // Diagnostics (progress, warnings, errors) go to stderr via tracing so that
    // stdout carries only command results — keeping `--json` output clean for agents.
    // Verbosity is controlled by RUST_LOG (defaults to `info`).
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .without_time()
        .init();

    let current_dir = std::env::current_dir()?;
    let json = cli.json;
    let no_tests = cli.no_tests;

    match cli.command {
        Commands::Init { include_library } => {
            commands::run_init(&current_dir, include_library)?;
        }
        Commands::Sync { include_library } => {
            let ochna_dir = current_dir.join(".ochna");
            if !ochna_dir.exists() {
                return Err("Database not initialized. Please run 'ochna init' first.".into());
            }
            commands::run_init(&current_dir, include_library)?;
        }
        Commands::Status => {
            commands::run_status(&current_dir, json)?;
        }
        Commands::Files => {
            commands::run_files(&current_dir, json)?;
        }
        Commands::Search { query } => {
            commands::run_search(&current_dir, &query, json, no_tests)?;
        }
        Commands::Callers { symbol } => {
            commands::run_callers(&current_dir, &symbol, json, no_tests)?;
        }
        Commands::Node {
            file,
            offset,
            limit,
            symbols_only,
            symbol,
            include_code,
            line,
        } => {
            commands::run_node(
                &current_dir,
                file,
                offset,
                limit,
                symbols_only,
                symbol,
                include_code,
                line,
                json,
                no_tests,
            )?;
        }
        Commands::Explore { query } => {
            commands::run_explore(&current_dir, &query, json, no_tests)?;
        }
    }

    Ok(())
}
