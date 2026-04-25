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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        // clap's debug_assert catches misconfigured derive attributes
        // (duplicate short flags, bad arg shapes, etc).
        Cli::command().debug_assert();
    }

    #[test]
    fn defaults_match_documented_values() {
        let cli = Cli::parse_from(["fleetcheck"]);
        assert!(cli.config.is_none());
        assert!(!cli.json);
        assert_eq!(cli.timeout_secs, 5);
    }

    #[test]
    fn parses_all_flags() {
        let cli = Cli::parse_from([
            "fleetcheck",
            "--json",
            "--config",
            "/tmp/x.toml",
            "--timeout-secs",
            "10",
        ]);
        assert!(cli.json);
        assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/tmp/x.toml")));
        assert_eq!(cli.timeout_secs, 10);
    }
}
