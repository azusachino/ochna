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
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Initialize the code graph database and scan the project
    Init,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    let current_dir = std::env::current_dir()?;

    match cli.command {
        Commands::Init => {
            commands::run_init(&current_dir)?;
        }
        Commands::Status => {
            commands::run_status(&current_dir)?;
        }
        Commands::Files => {
            commands::run_files(&current_dir)?;
        }
        Commands::Search { query } => {
            commands::run_search(&current_dir, &query)?;
        }
        Commands::Callers { symbol } => {
            commands::run_callers(&current_dir, &symbol)?;
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
            )?;
        }
        Commands::Explore { query } => {
            commands::run_explore(&current_dir, &query)?;
        }
    }

    Ok(())
}
