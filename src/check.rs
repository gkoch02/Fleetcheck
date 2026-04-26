use std::time::Duration;

use serde::Serialize;

use crate::config::{HostConfig, Thresholds};
use crate::metrics::Metrics;
use crate::ssh;

/// Per-host timeout budget. `connect` bounds SSH TCP+handshake; `script`
/// bounds remote script execution. They're separate because they answer
/// different questions ("is this host reachable?" vs. "did the metric
/// collection finish?").
#[derive(Debug, Clone, Copy)]
pub struct Timeouts {
    pub connect: Duration,
    pub script: Duration,
}

/// A metric whose threshold we can trip. The first six are typed; `Custom`
/// covers anything thresholded via `[thresholds.custom]` whose key matches
/// a script-emitted metric the binary doesn't have a typed field for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Disk,
    Temp,
    Load,
    Mem,
    Swap,
    Proc,
    Custom(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Violation {
    pub metric: Metric,
    pub value: f64,
    pub limit: f64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HostOutcome {
    Ok {
        metrics: Metrics,
        violations: Vec<Violation>,
    },
    Unreachable {
        error: String,
    },
}

#[derive(Debug, Serialize)]
pub struct HostReport {
    pub name: String,
    #[serde(flatten)]
    pub outcome: HostOutcome,
}

impl HostReport {
    /// Does this report count as a "check failed" case for the exit code?
    pub fn is_bad(&self) -> bool {
        match &self.outcome {
            HostOutcome::Unreachable { .. } => true,
            HostOutcome::Ok { violations, .. } => !violations.is_empty(),
        }
    }
}

/// Compare collected metrics against thresholds. Optional metrics
/// (`temp_c`, `swap_pct`, `proc_count`) are skipped when absent — a host
/// without a thermal zone or without swap simply can't violate that
/// threshold. Custom-map keys that don't match any known metric are
/// silently ignored (forward-compatible with future script keys).
fn evaluate(m: &Metrics, t: &Thresholds) -> Vec<Violation> {
    let mut out = Vec::new();
    if m.disk_pct > t.disk_pct {
        out.push(Violation {
            metric: Metric::Disk,
            value: m.disk_pct as f64,
            limit: t.disk_pct as f64,
        });
    }
    if let Some(c) = m.temp_c {
        if c > t.temp_c {
            out.push(Violation {
                metric: Metric::Temp,
                value: c as f64,
                limit: t.temp_c as f64,
            });
        }
    }
    if m.load_1m > t.load_1m {
        out.push(Violation {
            metric: Metric::Load,
            value: m.load_1m as f64,
            limit: t.load_1m as f64,
        });
    }
    if m.mem_pct > t.mem_pct {
        out.push(Violation {
            metric: Metric::Mem,
            value: m.mem_pct as f64,
            limit: t.mem_pct as f64,
        });
    }
    if let (Some(swap), Some(limit)) = (m.swap_pct, t.swap_pct) {
        if swap > limit {
            out.push(Violation {
                metric: Metric::Swap,
                value: swap as f64,
                limit: limit as f64,
            });
        }
    }
    if let (Some(proc), Some(limit)) = (m.proc_count, t.proc_count) {
        if proc > limit {
            out.push(Violation {
                metric: Metric::Proc,
                value: proc as f64,
                limit: limit as f64,
            });
        }
    }
    for (key, &limit) in &t.custom {
        if let Some(value) = metric_value_by_name(m, key) {
            if value > limit {
                out.push(Violation {
                    metric: Metric::Custom(key.clone()),
                    value,
                    limit,
                });
            }
        }
        // Unknown / not-present metrics are silently skipped: forward
        // compatibility with future script.sh keys.
    }
    out
}

/// Resolve a metric by name to a comparable f64. Names match the
/// `key=value` lines emitted by `script.sh` plus `temp_c` (the parsed,
/// not-millidegrees form). Unknown keys return `None`.
fn metric_value_by_name(m: &Metrics, key: &str) -> Option<f64> {
    match key {
        "disk_pct" => Some(m.disk_pct as f64),
        "load_1m" => Some(m.load_1m as f64),
        "mem_pct" => Some(m.mem_pct as f64),
        "temp_c" => m.temp_c.map(|v| v as f64),
        "swap_pct" => m.swap_pct.map(|v| v as f64),
        "proc_count" => m.proc_count.map(|v| v as f64),
        "uptime_secs" => Some(m.uptime.as_secs() as f64),
        _ => None,
    }
}

/// Run the full check for one host: connect, fetch metrics, evaluate
/// thresholds. Infallible by design — any failure becomes
/// `HostOutcome::Unreachable` so partial results still render.
pub async fn check_host(
    name: String,
    host: &HostConfig,
    thresholds: Thresholds,
    timeouts: Timeouts,
    retries: u32,
) -> HostReport {
    let outcome = match run(name.as_str(), host, timeouts, retries).await {
        Ok(metrics) => {
            let violations = evaluate(&metrics, &thresholds);
            HostOutcome::Ok { metrics, violations }
        }
        Err(e) => HostOutcome::Unreachable {
            // `{:#}` prints the full anyhow chain ("opening SSH mux: connection refused").
            error: format!("{e:#}"),
        },
    };
    HostReport { name, outcome }
}

async fn run(
    name: &str,
    host: &HostConfig,
    t: Timeouts,
    retries: u32,
) -> anyhow::Result<Metrics> {
    // Only the connect phase is retried. A successful connect followed by a
    // script failure or parse failure is deterministic and shouldn't be
    // re-attempted.
    let session = retry_async(retries, |_| ssh::connect(name, host, t.connect)).await?;
    let output = ssh::run_script(&session, t.script).await?;
    // Best-effort close; ignore errors — we already have our data.
    let _ = session.close().await;
    crate::metrics::parse(&output)
}

/// Retry an async operation up to `attempts` times (so up to `attempts + 1`
/// total invocations). Sleeps `backoff_with_jitter(attempt)` between tries.
/// Returns the last error if every attempt fails.
async fn retry_async<T, E, F, Fut>(attempts: u32, mut f: F) -> Result<T, E>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
{
    let mut attempt: u32 = 0;
    loop {
        match f(attempt).await {
            Ok(v) => return Ok(v),
            Err(e) if attempt >= attempts => return Err(e),
            Err(_) => {
                tokio::time::sleep(backoff_with_jitter(attempt)).await;
                attempt += 1;
            }
        }
    }
}

/// Exponential backoff with ±20% jitter. Base 200ms doubling per attempt,
/// capped at 5s. Jitter source is the system clock subsec nanos — quality
/// doesn't matter here, we just want to avoid synchronizing retries across
/// hosts.
fn backoff_with_jitter(attempt: u32) -> Duration {
    let shift = attempt.min(5);
    let base_ms = 200u64.saturating_mul(1u64 << shift).min(5_000);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(attempt) as i64;
    // Map [0, 40_000] to [-20_000, +20_000] (i.e. ±20% in 1/100_000 units).
    let frac = (nanos % 40_001) - 20_000;
    let jitter_ms = (base_ms as i64 * frac) / 100_000;
    let total = (base_ms as i64 + jitter_ms).max(0) as u64;
    Duration::from_millis(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th() -> Thresholds {
        Thresholds {
            disk_pct: 85,
            temp_c: 75.0,
            load_1m: 2.0,
            mem_pct: 90,
            swap_pct: None,
            proc_count: None,
            custom: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn flags_high_disk_only() {
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 90,
            temp_c: Some(50.0),
            load_1m: 0.5,
            mem_pct: 40,
            swap_pct: None,
            proc_count: None,
        };
        let v = evaluate(&m, &th());
        assert_eq!(v.len(), 1);
        assert!(matches!(v[0].metric, Metric::Disk));
    }

    #[test]
    fn missing_temp_is_not_a_violation() {
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 10,
            temp_c: None,
            load_1m: 0.1,
            mem_pct: 10,
            swap_pct: None,
            proc_count: None,
        };
        assert!(evaluate(&m, &th()).is_empty());
    }

    #[test]
    fn flags_all_typed_metrics_when_all_exceed() {
        let mut t = th();
        t.swap_pct = Some(50);
        t.proc_count = Some(500);
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 99,
            temp_c: Some(99.0),
            load_1m: 9.9,
            mem_pct: 99,
            swap_pct: Some(99),
            proc_count: Some(999),
        };
        let v = evaluate(&m, &t);
        assert_eq!(v.len(), 6);
    }

    #[test]
    fn equal_to_threshold_is_not_a_violation() {
        // Comparison is strictly greater than, so a value sitting exactly
        // on the threshold is still considered healthy.
        let mut t = th();
        t.swap_pct = Some(50);
        t.proc_count = Some(500);
        t.custom.insert("disk_pct".into(), 85.0);

        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 85,
            temp_c: Some(75.0),
            load_1m: 2.0,
            mem_pct: 90,
            swap_pct: Some(50),
            proc_count: Some(500),
        };
        assert!(evaluate(&m, &t).is_empty());
    }

    #[test]
    fn typed_swap_and_proc_thresholds_trip() {
        let mut t = th();
        t.swap_pct = Some(50);
        t.proc_count = Some(500);
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 0,
            temp_c: None,
            load_1m: 0.0,
            mem_pct: 0,
            swap_pct: Some(60),
            proc_count: Some(600),
        };
        let v = evaluate(&m, &t);
        assert_eq!(v.len(), 2);
        assert!(v.iter().any(|x| matches!(x.metric, Metric::Swap)));
        assert!(v.iter().any(|x| matches!(x.metric, Metric::Proc)));
    }

    #[test]
    fn typed_swap_threshold_skipped_when_metric_absent() {
        let mut t = th();
        t.swap_pct = Some(50);
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 0,
            temp_c: None,
            load_1m: 0.0,
            mem_pct: 0,
            swap_pct: None, // host has no swap
            proc_count: None,
        };
        assert!(evaluate(&m, &t).is_empty());
    }

    #[test]
    fn custom_threshold_trips_on_known_metric() {
        let mut t = th();
        t.custom.insert("proc_count".into(), 100.0);
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 0,
            temp_c: None,
            load_1m: 0.0,
            mem_pct: 0,
            swap_pct: None,
            proc_count: Some(101),
        };
        let v = evaluate(&m, &t);
        assert_eq!(v.len(), 1);
        match &v[0].metric {
            Metric::Custom(name) => assert_eq!(name, "proc_count"),
            other => panic!("expected Custom, got {other:?}"),
        }
    }

    #[test]
    fn custom_threshold_for_unknown_metric_is_silently_ignored() {
        // Forward-compat: a config can declare a threshold for a metric the
        // running binary doesn't know about (yet). Don't trip a violation
        // and don't error.
        let mut t = th();
        t.custom.insert("unknown_future_metric".into(), 0.0);
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 0,
            temp_c: None,
            load_1m: 0.0,
            mem_pct: 0,
            swap_pct: None,
            proc_count: None,
        };
        assert!(evaluate(&m, &t).is_empty());
    }

    fn healthy_metrics() -> Metrics {
        Metrics {
            uptime: Duration::from_secs(0),
            disk_pct: 0,
            temp_c: None,
            load_1m: 0.0,
            mem_pct: 0,
            swap_pct: None,
            proc_count: None,
        }
    }

    #[test]
    fn is_bad_false_for_clean_ok() {
        let r = HostReport {
            name: "a".into(),
            outcome: HostOutcome::Ok {
                metrics: healthy_metrics(),
                violations: vec![],
            },
        };
        assert!(!r.is_bad());
    }

    #[test]
    fn is_bad_true_for_violations() {
        let r = HostReport {
            name: "a".into(),
            outcome: HostOutcome::Ok {
                metrics: healthy_metrics(),
                violations: vec![Violation { metric: Metric::Disk, value: 99.0, limit: 85.0 }],
            },
        };
        assert!(r.is_bad());
    }

    #[test]
    fn is_bad_true_for_unreachable() {
        let r = HostReport {
            name: "a".into(),
            outcome: HostOutcome::Unreachable { error: "boom".into() },
        };
        assert!(r.is_bad());
    }

    #[tokio::test(start_paused = true)]
    async fn retry_async_succeeds_on_first_attempt() {
        let mut calls = 0u32;
        let r: Result<u32, &'static str> = retry_async(3, |_| {
            calls += 1;
            async move { Ok::<u32, &'static str>(42) }
        })
        .await;
        assert_eq!(r, Ok(42));
        assert_eq!(calls, 1);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_async_succeeds_after_failures() {
        let mut calls = 0u32;
        let r: Result<u32, &'static str> = retry_async(3, |attempt| {
            calls += 1;
            async move {
                if attempt < 2 {
                    Err("flaky")
                } else {
                    Ok(7)
                }
            }
        })
        .await;
        assert_eq!(r, Ok(7));
        assert_eq!(calls, 3); // attempts 0, 1, 2
    }

    #[tokio::test(start_paused = true)]
    async fn retry_async_gives_up_after_attempts() {
        let mut calls = 0u32;
        let r: Result<u32, &'static str> = retry_async(2, |_| {
            calls += 1;
            async move { Err::<u32, &'static str>("nope") }
        })
        .await;
        assert_eq!(r, Err("nope"));
        // attempts=2 means up to 3 total invocations (0, 1, 2).
        assert_eq!(calls, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_async_zero_attempts_runs_exactly_once() {
        // Default --retries=0 must preserve v1 behavior: no retry on failure.
        let mut calls = 0u32;
        let r: Result<u32, &'static str> = retry_async(0, |_| {
            calls += 1;
            async move { Err::<u32, &'static str>("once") }
        })
        .await;
        assert_eq!(r, Err("once"));
        assert_eq!(calls, 1);
    }

    #[test]
    fn backoff_doubles_and_caps_at_5s() {
        // Worst-case (max positive jitter) bound is base + 20%, capped at 6s
        // (5_000 * 1.2). Smallest is base * 0.8.
        for attempt in 0..6 {
            let d = backoff_with_jitter(attempt).as_millis();
            assert!(d <= 6_000, "attempt {attempt} produced {d}ms, expected <= 6000");
        }
        // Past the cap, still bounded.
        let d = backoff_with_jitter(20).as_millis();
        assert!(d <= 6_000, "high attempt produced {d}ms");
    }
}
