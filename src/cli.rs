use std::path::PathBuf;

use clap::Parser;

pub const DEFAULT_CONNECT_SECS: u64 = 5;
pub const DEFAULT_SCRIPT_SECS: u64 = 10;
pub const DEFAULT_MAX_CONCURRENT: usize = 32;

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

    /// Per-host SSH connect timeout, in seconds.
    #[arg(long)]
    pub connect_timeout: Option<u64>,

    /// Per-host remote script execution timeout, in seconds.
    #[arg(long)]
    pub script_timeout: Option<u64>,

    /// Deprecated: sets both --connect-timeout and --script-timeout to this value.
    /// Kept so existing cron lines keep working; prefer the split flags.
    #[arg(long, conflicts_with_all = ["connect_timeout", "script_timeout"])]
    pub timeout_secs: Option<u64>,

    /// Maximum number of hosts to check in parallel. 0 means unbounded.
    #[arg(long, default_value_t = DEFAULT_MAX_CONCURRENT)]
    pub max_concurrent: usize,

    /// How many times to retry a failed SSH connect (with exponential backoff
    /// and jitter) before giving up. 0 disables retries (v1 behavior).
    /// Per-host overrides in TOML take precedence when this is left at 0.
    #[arg(long, default_value_t = 0)]
    pub retries: u32,
}

impl Cli {
    /// Resolve timeouts from flags. The legacy `--timeout-secs` populates both
    /// new flags and emits a deprecation notice on stderr. Otherwise the split
    /// flags fall back to their defaults.
    pub fn resolve_timeout_secs(&self) -> (u64, u64) {
        if let Some(t) = self.timeout_secs {
            eprintln!(
                "fleetcheck: --timeout-secs is deprecated; \
                 use --connect-timeout and --script-timeout"
            );
            return (t, t);
        }
        (
            self.connect_timeout.unwrap_or(DEFAULT_CONNECT_SECS),
            self.script_timeout.unwrap_or(DEFAULT_SCRIPT_SECS),
        )
    }
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
        assert!(cli.timeout_secs.is_none());
        assert!(cli.connect_timeout.is_none());
        assert!(cli.script_timeout.is_none());
        assert_eq!(cli.max_concurrent, DEFAULT_MAX_CONCURRENT);
        assert_eq!(cli.retries, 0);
        assert_eq!(cli.resolve_timeout_secs(), (DEFAULT_CONNECT_SECS, DEFAULT_SCRIPT_SECS));
    }

    #[test]
    fn parses_all_flags() {
        let cli = Cli::parse_from([
            "fleetcheck",
            "--json",
            "--config",
            "/tmp/x.toml",
            "--connect-timeout",
            "3",
            "--script-timeout",
            "12",
        ]);
        assert!(cli.json);
        assert_eq!(cli.config.as_deref(), Some(std::path::Path::new("/tmp/x.toml")));
        assert_eq!(cli.connect_timeout, Some(3));
        assert_eq!(cli.script_timeout, Some(12));
        assert_eq!(cli.resolve_timeout_secs(), (3, 12));
    }

    #[test]
    fn legacy_timeout_secs_aliases_both_phases() {
        // Existing cron lines using --timeout-secs N must keep working,
        // setting both connect and script timeouts to the same value.
        let cli = Cli::parse_from(["fleetcheck", "--timeout-secs", "7"]);
        assert_eq!(cli.timeout_secs, Some(7));
        assert_eq!(cli.resolve_timeout_secs(), (7, 7));
    }

    #[test]
    fn parses_max_concurrent_zero_for_unbounded() {
        let cli = Cli::parse_from(["fleetcheck", "--max-concurrent", "0"]);
        assert_eq!(cli.max_concurrent, 0);
    }

    #[test]
    fn legacy_flag_conflicts_with_new_flags() {
        let res = Cli::try_parse_from([
            "fleetcheck",
            "--timeout-secs",
            "5",
            "--connect-timeout",
            "3",
        ]);
        assert!(res.is_err(), "expected conflict error, got {res:?}");
    }
}
