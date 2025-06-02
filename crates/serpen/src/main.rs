use clap::{Parser, Subcommand};
use env_logger::Env;
use log::{debug, info};
use std::fs;
use std::path::PathBuf;

use serpen::bundler::Bundler;
use serpen::config::Config;
use serpen::unused_import_trimmer::{TrimConfig, UnusedImportTrimmer};

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Configuration file path
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Bundle Python modules into a single file
    Bundle {
        /// Entry point Python script
        #[arg(short, long)]
        entry: PathBuf,

        /// Output bundled Python file
        #[arg(short, long)]
        output: PathBuf,

        /// Emit requirements.txt with third-party dependencies
        #[arg(long)]
        emit_requirements: bool,
    },
    /// Trim unused imports from Python files
    Trim {
        /// Python file to analyze and trim
        file: PathBuf,

        /// Output file (if not specified, overwrites input file)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Preview mode - show what would be trimmed without making changes
        #[arg(long)]
        dry_run: bool,

        /// Preserve __future__ imports even if unused
        #[arg(long)]
        preserve_future: bool,

        /// Preserve imports matching these patterns (comma-separated)
        #[arg(long)]
        preserve_patterns: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(Env::default().default_filter_or(log_level)).init();

    info!("Starting Serpen Python bundler");

    match cli.command {
        Commands::Bundle {
            entry,
            output,
            emit_requirements,
        } => {
            debug!("Entry point: {:?}", entry);
            debug!("Output: {:?}", output);

            // Load configuration
            let config = Config::load(cli.config.as_deref())?;
            debug!("Configuration: {:?}", config);

            // Create bundler and run
            let mut bundler = Bundler::new(config);
            bundler.bundle(&entry, &output, emit_requirements)?;

            info!("Bundle created successfully at {:?}", output);
        }
        Commands::Trim {
            file,
            output,
            dry_run,
            preserve_future,
            preserve_patterns,
        } => {
            debug!("Trimming unused imports from: {:?}", file);

            // Read the source file
            let source = fs::read_to_string(&file)?;

            // Configure trimming behavior
            let preserve_patterns_vec = preserve_patterns
                .map(|patterns| patterns.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default();

            let trim_config = TrimConfig {
                preserve_future_imports: preserve_future,
                preserve_patterns: preserve_patterns_vec,
                ..Default::default()
            };

            let mut trimmer = UnusedImportTrimmer::new();

            if dry_run {
                // Preview mode - just analyze without changing
                let unused_imports = trimmer.analyze_only(&source, &trim_config)?;

                if unused_imports.is_empty() {
                    info!("No unused imports found in {:?}", file);
                } else {
                    info!(
                        "Found {} unused imports in {:?}:",
                        unused_imports.len(),
                        file
                    );
                    for import in &unused_imports {
                        info!("  - {} ({})", import.name, import.qualified_name);
                    }
                }
            } else {
                // Actually trim the imports
                let result = trimmer.trim_unused_imports(&source, &trim_config)?;

                if result.has_changes {
                    let output_path = output.as_ref().unwrap_or(&file);
                    fs::write(output_path, &result.code)?;

                    info!(
                        "Trimmed {} unused imports from {:?}, saved to {:?}",
                        result.removed_imports.len(),
                        file,
                        output_path
                    );

                    for import in &result.removed_imports {
                        info!("  - Removed {} ({})", import.name, import.qualified_name);
                    }
                } else {
                    info!("No unused imports found in {:?}", file);
                }
            }
        }
    }

    Ok(())
}
