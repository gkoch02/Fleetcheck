use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use futures::future::join_all;
use futures::stream::{self, StreamExt};

mod check;
mod cli;
mod config;
mod metrics;
mod report;
mod ssh;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(false) => ExitCode::from(0),
        // Soft failure: at least one host unreachable or over a threshold.
        // This is the case cron cares about.
        Ok(true) => ExitCode::from(1),
        // Hard failure: config missing, invalid, etc. Print the chain and bail.
        Err(e) => {
            eprintln!("fleetcheck: {e:#}");
            ExitCode::from(2)
        }
    }
}

/// Returns `true` if the run should exit non-zero (any host unreachable or
/// over threshold), `false` if everything's green.
async fn run() -> Result<bool> {
    let args = cli::Cli::parse();
    let (connect_secs, script_secs) = args.resolve_timeout_secs();
    let timeouts = check::Timeouts {
        connect: Duration::from_secs(connect_secs),
        script: Duration::from_secs(script_secs),
    };
    let cfg_path = match args.config {
        Some(p) => p,
        None => config::default_path()
            .context("cannot determine ~/.config directory")?,
    };
    let cfg = config::load(&cfg_path)?;

    // Fan out across hosts. We deliberately want partial results, not
    // short-circuiting on the first failure. With `--max-concurrent 0` we
    // fan out unbounded (v1 behavior); otherwise cap parallelism with
    // `buffer_unordered` so larger fleets don't open hundreds of SSH sessions
    // at once.
    let futs = cfg.hosts.iter().map(|(name, hc)| {
        let t = cfg.thresholds.merged(hc.thresholds.as_ref());
        // CLI flag wins when set non-zero, otherwise fall back to the per-host
        // override, otherwise no retries (v1 behavior).
        let retries = if args.retries > 0 {
            args.retries
        } else {
            hc.retries.unwrap_or(0)
        };
        let name = name.clone();
        async move { check::check_host(name, hc, t, timeouts, retries).await }
    });
    let mut reports: Vec<_> = if args.max_concurrent == 0 {
        join_all(futs).await
    } else {
        stream::iter(futs).buffer_unordered(args.max_concurrent).collect().await
    };
    reports.sort_by(|a, b| a.name.cmp(&b.name));

    if args.json {
        println!("{}", report::render_json(&reports, &cfg.thresholds)?);
    } else {
        println!("{}", report::render_table(&reports));
        println!("{}", report::summary_line(&reports));
    }

    Ok(reports.iter().any(|r| r.is_bad()))
}
