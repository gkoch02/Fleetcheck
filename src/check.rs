use std::time::Duration;

use serde::Serialize;

use crate::config::{HostConfig, Thresholds};
use crate::metrics::Metrics;
use crate::ssh;

/// One of the four metrics whose threshold we can trip.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Metric {
    Disk,
    Temp,
    Load,
    Mem,
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

/// Compare collected metrics against thresholds. Temp is `Option` — a host
/// without a thermal zone simply can't violate the temp threshold.
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
    out
}

/// Run the full check for one host: connect, fetch metrics, evaluate
/// thresholds. Infallible by design — any failure becomes
/// `HostOutcome::Unreachable` so partial results still render.
pub async fn check_host(
    name: String,
    host: &HostConfig,
    thresholds: Thresholds,
    timeout: Duration,
) -> HostReport {
    let outcome = match run(name.as_str(), host, timeout).await {
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

async fn run(name: &str, host: &HostConfig, t: Duration) -> anyhow::Result<Metrics> {
    let session = ssh::connect(name, host, t).await?;
    let output = ssh::run_script(&session, t).await?;
    // Best-effort close; ignore errors — we already have our data.
    let _ = session.close().await;
    crate::metrics::parse(&output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn th() -> Thresholds {
        Thresholds { disk_pct: 85, temp_c: 75.0, load_1m: 2.0, mem_pct: 90 }
    }

    #[test]
    fn flags_high_disk_only() {
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 90,
            temp_c: Some(50.0),
            load_1m: 0.5,
            mem_pct: 40,
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
        };
        assert!(evaluate(&m, &th()).is_empty());
    }

    #[test]
    fn flags_all_four_metrics_when_all_exceed() {
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 99,
            temp_c: Some(99.0),
            load_1m: 9.9,
            mem_pct: 99,
        };
        let v = evaluate(&m, &th());
        assert_eq!(v.len(), 4);
    }

    #[test]
    fn equal_to_threshold_is_not_a_violation() {
        // Comparison is strictly greater than, so a value sitting exactly
        // on the threshold is still considered healthy.
        let m = Metrics {
            uptime: Duration::from_secs(1),
            disk_pct: 85,
            temp_c: Some(75.0),
            load_1m: 2.0,
            mem_pct: 90,
        };
        assert!(evaluate(&m, &th()).is_empty());
    }

    fn healthy_metrics() -> Metrics {
        Metrics {
            uptime: Duration::from_secs(0),
            disk_pct: 0,
            temp_c: None,
            load_1m: 0.0,
            mem_pct: 0,
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
}
