use std::path::PathBuf;

use clap::Parser;

/// Raspberry Pi fleet health check.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    /// Path to config file. Defaults to ~/.config/fleetcheck/hosts.toml.
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Emit machine-readable JSON instead of a colored table.
    #[arg(long)]
    pub json: bool,

    /// Per-host SSH connect + command timeout, in seconds.
    #[arg(long, default_value_t = 5)]
    pub timeout_secs: u64,
}
