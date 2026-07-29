pub mod commands;
pub mod db;
pub mod parser;

use clap::{Parser, Subcommand, ValueEnum};
use std::error::Error;
use std::path::PathBuf;

fn parse_unresolved_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_string())?;
    if (1..=200).contains(&limit) {
        Ok(limit)
    } else {
        Err("limit must be between 1 and 200".to_string())
    }
}

fn parse_tests_for_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_string())?;
    if (1..=100).contains(&limit) {
        Ok(limit)
    } else {
        Err("limit must be between 1 and 100".to_string())
    }
}

fn parse_diff_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_string())?;
    if (1..=500).contains(&limit) {
        Ok(limit)
    } else {
        Err("limit must be between 1 and 500".to_string())
    }
}

fn parse_impact_limit(value: &str) -> Result<usize, String> {
    let limit = value
        .parse::<usize>()
        .map_err(|_| "limit must be a positive integer".to_string())?;
    if (1..=200).contains(&limit) {
        Ok(limit)
    } else {
        Err("limit must be between 1 and 200".to_string())
    }
}

fn parse_impact_depth(value: &str) -> Result<usize, String> {
    let depth = value
        .parse::<usize>()
        .map_err(|_| "depth must be a positive integer".to_string())?;
    if (1..=5).contains(&depth) {
        Ok(depth)
    } else {
        Err("depth must be between 1 and 5".to_string())
    }
}

fn parse_confidence(value: &str) -> Result<i64, String> {
    let confidence = value
        .parse::<i64>()
        .map_err(|_| "min-confidence must be an integer".to_string())?;
    if (0..=100).contains(&confidence) {
        Ok(confidence)
    } else {
        Err("min-confidence must be between 0 and 100".to_string())
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ImpactDirection {
    Callers,
    Callees,
    Both,
}

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
    /// Target the workspace at this path instead of the current directory,
    /// so its `.ochna/ochna.db` is reachable from any cwd
    #[arg(long = "workspace", short = 'C', global = true)]
    workspace: Option<PathBuf>,
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
    /// Print the recommended ochna workflow for humans or agents
    Howto,
    /// Display index statistics
    Status,
    /// Inspect index health and graph-quality diagnostics for structural review
    Doctor,
    /// List call sites whose targets could not be resolved
    Unresolved {
        /// Only include source files whose path starts with this prefix
        #[arg(long = "in")]
        in_path: Option<String>,
        /// Maximum references to return (default 50, maximum 200)
        #[arg(long, default_value_t = 50, value_parser = parse_unresolved_limit)]
        limit: usize,
    },
    /// Find test relationships for a production symbol; evidence is structural, not coverage proof
    TestsFor {
        /// The name, qualified name, or ID of the production symbol to query
        symbol: String,
        /// Only resolve target symbols whose file path starts with this prefix
        #[arg(long = "in")]
        in_path: Option<String>,
        /// Maximum test relationships to return (default 30, maximum 100)
        #[arg(long, default_value_t = 30, value_parser = parse_tests_for_limit)]
        limit: usize,
    },
    /// Traverse bounded, confidence-labelled structural impact from one symbol
    Impact {
        /// The ID or qualified name of exactly one indexed symbol
        symbol: String,
        /// Maximum traversal depth (default 2, maximum 5)
        #[arg(long, default_value_t = 2, value_parser = parse_impact_depth)]
        depth: usize,
        /// Traverse incoming callers, outgoing callees, or both
        #[arg(long, value_enum, default_value_t = ImpactDirection::Both)]
        direction: ImpactDirection,
        /// Only traverse relationships at or above this confidence (default 80)
        #[arg(long, default_value_t = 80, value_parser = parse_confidence)]
        min_confidence: i64,
        /// Maximum reported nodes (default 50, maximum 200)
        #[arg(long, default_value_t = 50, value_parser = parse_impact_limit)]
        limit: usize,
    },
    /// Map a Git change or explicit paths onto current indexed symbols
    Diff {
        /// Git revision to compare from (optionally with --head)
        #[arg(long, conflicts_with = "files")]
        base: Option<String>,
        /// Git revision to compare to; requires --base
        #[arg(long, requires = "base")]
        head: Option<String>,
        /// Current workspace-relative paths to map without asking Git for a range
        #[arg(long, num_args = 1.., conflicts_with = "base")]
        files: Vec<String>,
        /// Maximum changed symbols to return (default 100, maximum 500)
        #[arg(long, default_value_t = 100, value_parser = parse_diff_limit)]
        limit: usize,
    },
    /// List indexed files with metadata
    Files,
    /// Search for nodes/symbols matching a query string
    Search {
        /// The search query
        query: String,
        /// Maximum number of results to display
        #[arg(long = "limit", default_value_t = 30)]
        limit: usize,
    },
    /// Find callers of a given symbol
    Callers {
        /// The name or ID of the symbol to query
        symbol: String,
        /// Minimum confidence level to include in callers results (e.g. 80)
        #[arg(long = "min-confidence")]
        min_confidence: Option<i64>,
        /// Display resolution kind and confidence metrics alongside symbols
        #[arg(long = "show-resolution")]
        show_resolution: bool,
        /// Only resolve target symbols whose file path starts with this prefix
        #[arg(long = "in")]
        in_path: Option<String>,
    },
    /// Find callees of a given symbol
    Callees {
        /// The name or ID of the symbol to query
        symbol: String,
        /// Minimum confidence level to include in callees results (e.g. 80)
        #[arg(long = "min-confidence")]
        min_confidence: Option<i64>,
        /// Display resolution kind and confidence metrics alongside symbols
        #[arg(long = "show-resolution")]
        show_resolution: bool,
        /// Only resolve target symbols whose file path starts with this prefix
        #[arg(long = "in")]
        in_path: Option<String>,
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
        /// Display resolution kind and confidence metrics alongside symbols
        #[arg(long = "show-resolution")]
        show_resolution: bool,
    },
    /// Explore the codebase using FTS and show relationships
    Explore {
        /// Query terms to search for nodes
        query: String,
        /// Display resolution kind and confidence metrics alongside symbols
        #[arg(long = "show-resolution")]
        show_resolution: bool,
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

    let current_dir = match cli.workspace {
        Some(path) => path,
        None => std::env::current_dir()?,
    };
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
        Commands::Howto => {
            commands::run_howto(json)?;
        }
        Commands::Status => {
            commands::run_status(&current_dir, json)?;
        }
        Commands::Doctor => {
            commands::run_doctor(&current_dir, json)?;
        }
        Commands::Unresolved { in_path, limit } => {
            commands::run_unresolved(&current_dir, in_path.as_deref(), limit, json)?;
        }
        Commands::TestsFor {
            symbol,
            in_path,
            limit,
        } => {
            commands::run_tests_for(
                &current_dir,
                &symbol,
                in_path.as_deref(),
                limit,
                json,
                no_tests,
            )?;
        }
        Commands::Impact {
            symbol,
            depth,
            direction,
            min_confidence,
            limit,
        } => {
            commands::run_impact(
                &current_dir,
                &symbol,
                depth,
                direction,
                min_confidence,
                limit,
                json,
                no_tests,
            )?;
        }
        Commands::Diff {
            base,
            head,
            files,
            limit,
        } => {
            if base.is_none() && files.is_empty() {
                return Err(
                    "diff requires exactly one selector: --base <revision> or --files <path>..."
                        .into(),
                );
            }
            commands::run_diff(
                &current_dir,
                base.as_deref(),
                head.as_deref(),
                &files,
                limit,
                json,
            )?;
        }
        Commands::Files => {
            commands::run_files(&current_dir, json)?;
        }
        Commands::Search { query, limit } => {
            commands::run_search(&current_dir, &query, json, no_tests, limit)?;
        }
        Commands::Callers {
            symbol,
            min_confidence,
            show_resolution,
            in_path,
        } => {
            commands::run_callers(
                &current_dir,
                &symbol,
                json,
                no_tests,
                min_confidence,
                show_resolution,
                in_path.as_deref(),
            )?;
        }
        Commands::Callees {
            symbol,
            min_confidence,
            show_resolution,
            in_path,
        } => {
            commands::run_callees(
                &current_dir,
                &symbol,
                json,
                no_tests,
                min_confidence,
                show_resolution,
                in_path.as_deref(),
            )?;
        }
        Commands::Node {
            file,
            offset,
            limit,
            symbols_only,
            symbol,
            include_code,
            line,
            show_resolution,
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
                show_resolution,
            )?;
        }
        Commands::Explore {
            query,
            show_resolution,
        } => {
            commands::run_explore(&current_dir, &query, json, no_tests, show_resolution)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_flag_parses_globally_after_subcommand() {
        // `--workspace`/`-C` is global, so it must parse whether placed before or
        // after the subcommand; the value overrides cwd-based DB resolution.
        let long = Cli::try_parse_from(["ochna", "search", "Runtime", "--workspace", "/tmp/ws"])
            .expect("long form should parse after subcommand");
        assert_eq!(long.workspace, Some(PathBuf::from("/tmp/ws")));

        let short = Cli::try_parse_from(["ochna", "-C", "/tmp/ws", "status"])
            .expect("short form should parse before subcommand");
        assert_eq!(short.workspace, Some(PathBuf::from("/tmp/ws")));

        let absent = Cli::try_parse_from(["ochna", "status"]).expect("flag is optional");
        assert_eq!(absent.workspace, None);
    }

    #[test]
    fn tests_for_parses_scope_and_enforces_its_limit() {
        let cli = Cli::try_parse_from([
            "ochna",
            "tests-for",
            "render",
            "--in",
            "src",
            "--limit",
            "100",
        ])
        .expect("valid tests-for arguments should parse");
        match cli.command {
            Commands::TestsFor {
                symbol,
                in_path,
                limit,
            } => {
                assert_eq!(symbol, "render");
                assert_eq!(in_path.as_deref(), Some("src"));
                assert_eq!(limit, 100);
            }
            _ => panic!("expected tests-for command"),
        }
        assert!(Cli::try_parse_from(["ochna", "tests-for", "render", "--limit", "101"]).is_err());
    }

    #[test]
    fn diff_requires_one_selector_and_enforces_its_limit() {
        assert!(Cli::try_parse_from(["ochna", "diff", "--base", "HEAD", "--limit", "500"]).is_ok());
        assert!(Cli::try_parse_from(["ochna", "diff", "--files", "src/lib.rs"]).is_ok());
        assert!(
            Cli::try_parse_from(["ochna", "diff", "--base", "HEAD", "--files", "src/lib.rs"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from(["ochna", "diff", "--base", "HEAD", "--limit", "501"]).is_err()
        );
    }

    #[test]
    fn impact_parses_contract_defaults_and_bounds() {
        let cli = Cli::try_parse_from(["ochna", "impact", "render"])
            .expect("impact defaults should parse");
        match cli.command {
            Commands::Impact {
                depth,
                direction,
                min_confidence,
                limit,
                ..
            } => {
                assert_eq!(depth, 2);
                assert!(matches!(direction, ImpactDirection::Both));
                assert_eq!(min_confidence, 80);
                assert_eq!(limit, 50);
            }
            _ => panic!("expected impact command"),
        }
        assert!(Cli::try_parse_from(["ochna", "impact", "render", "--depth", "6"]).is_err());
        assert!(Cli::try_parse_from(["ochna", "impact", "render", "--limit", "201"]).is_err());
        assert!(
            Cli::try_parse_from(["ochna", "impact", "render", "--min-confidence", "101"]).is_err()
        );
    }
}
