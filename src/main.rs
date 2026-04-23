use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use futures::future::join_all;

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
    let cfg_path = match args.config {
        Some(p) => p,
        None => config::default_path()
            .context("cannot determine ~/.config directory")?,
    };
    let cfg = config::load(&cfg_path)?;
    let timeout = Duration::from_secs(args.timeout_secs);

    // Fan out across hosts. `join_all` waits for all futures — we deliberately
    // want partial results, not short-circuiting on the first failure.
    let futs = cfg.hosts.iter().map(|(name, hc)| {
        let t = cfg.thresholds.merged(hc.thresholds.as_ref());
        let name = name.clone();
        async move { check::check_host(name, hc, t, timeout).await }
    });
    let mut reports = join_all(futs).await;
    reports.sort_by(|a, b| a.name.cmp(&b.name));

    if args.json {
        println!("{}", report::render_json(&reports, &cfg.thresholds)?);
    } else {
        println!("{}", report::render_table(&reports));
        println!("{}", report::summary_line(&reports));
    }

    Ok(reports.iter().any(|r| r.is_bad()))
}
