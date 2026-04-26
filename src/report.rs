use anyhow::Result;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, Color, Table};
use owo_colors::{OwoColorize, Stream};

use crate::check::{HostOutcome, HostReport, Metric, Violation};
use crate::config::Thresholds;
use crate::metrics::{format_uptime, Metrics};

const MISSING: &str = "—";

/// One-line summary suitable for cron log scraping. Colored when stdout is
/// a TTY, plain text otherwise — `if_supports_color` handles the detection.
pub fn summary_line(reports: &[HostReport]) -> String {
    let total = reports.len();
    let unreachable = reports
        .iter()
        .filter(|r| matches!(r.outcome, HostOutcome::Unreachable { .. }))
        .count();
    let warn = reports
        .iter()
        .filter(|r| matches!(&r.outcome, HostOutcome::Ok { violations, .. } if !violations.is_empty()))
        .count();

    if unreachable == 0 && warn == 0 {
        format!(
            "{} all {} hosts healthy",
            "✓".if_supports_color(Stream::Stdout, |t| t.green()),
            total
        )
    } else {
        format!(
            "{} {} unreachable, {} with warnings (of {} hosts)",
            "✗".if_supports_color(Stream::Stdout, |t| t.red()),
            unreachable,
            warn,
            total
        )
    }
}

/// Render reports as a colored human-readable table.
pub fn render_table(reports: &[HostReport]) -> String {
    let mut table = Table::new();
    table.load_preset(UTF8_FULL);
    table.set_header([
        Cell::new("host").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("status").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("uptime").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("disk %").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("temp °C").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("load 1m").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("mem %").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("swap %").add_attribute(comfy_table::Attribute::Bold),
        Cell::new("procs").add_attribute(comfy_table::Attribute::Bold),
    ]);

    for r in reports {
        match &r.outcome {
            HostOutcome::Ok { metrics, violations } => {
                table.add_row(ok_row(&r.name, metrics, violations));
            }
            HostOutcome::Unreachable { error } => {
                table.add_row(unreachable_row(&r.name, error));
            }
        }
    }

    table.to_string()
}

fn ok_row(name: &str, m: &Metrics, violations: &[Violation]) -> Vec<Cell> {
    let disk_bad = violations.iter().any(|v| matches!(v.metric, Metric::Disk));
    let temp_bad = violations.iter().any(|v| matches!(v.metric, Metric::Temp));
    let load_bad = violations.iter().any(|v| matches!(v.metric, Metric::Load));
    let mem_bad = violations.iter().any(|v| matches!(v.metric, Metric::Mem));
    let swap_bad = violations.iter().any(|v| matches!(v.metric, Metric::Swap));
    let proc_bad = violations.iter().any(|v| matches!(v.metric, Metric::Proc));

    let status_cell = if violations.is_empty() {
        Cell::new("OK").fg(Color::Green)
    } else {
        Cell::new("WARN").fg(Color::Yellow)
    };

    vec![
        Cell::new(name),
        status_cell,
        Cell::new(format_uptime(m.uptime)),
        metric_cell(format!("{}", m.disk_pct), disk_bad),
        match m.temp_c {
            Some(c) => metric_cell(format!("{c:.1}"), temp_bad),
            None => Cell::new(MISSING).fg(Color::DarkGrey),
        },
        metric_cell(format!("{:.2}", m.load_1m), load_bad),
        metric_cell(format!("{}", m.mem_pct), mem_bad),
        match m.swap_pct {
            Some(v) => metric_cell(format!("{v}"), swap_bad),
            None => Cell::new(MISSING).fg(Color::DarkGrey),
        },
        match m.proc_count {
            Some(v) => metric_cell(format!("{v}"), proc_bad),
            None => Cell::new(MISSING).fg(Color::DarkGrey),
        },
    ]
}

fn unreachable_row(name: &str, error: &str) -> Vec<Cell> {
    vec![
        Cell::new(name),
        Cell::new(format!("UNREACHABLE ({error})")).fg(Color::Red),
        Cell::new(MISSING).fg(Color::DarkGrey),
        Cell::new(MISSING).fg(Color::DarkGrey),
        Cell::new(MISSING).fg(Color::DarkGrey),
        Cell::new(MISSING).fg(Color::DarkGrey),
        Cell::new(MISSING).fg(Color::DarkGrey),
        Cell::new(MISSING).fg(Color::DarkGrey),
        Cell::new(MISSING).fg(Color::DarkGrey),
    ]
}

fn metric_cell(text: String, bad: bool) -> Cell {
    let c = Cell::new(text);
    if bad {
        c.fg(Color::Red).add_attribute(comfy_table::Attribute::Bold)
    } else {
        c.fg(Color::Green)
    }
}

/// Render reports as JSON. Serialization is driven by the `Serialize` derives
/// on `HostReport` and friends — see the `#[serde(tag = "status", ...)]` on
/// `HostOutcome` for why "ok"/"unreachable" are flattened into each row.
pub fn render_json(reports: &[HostReport], thresholds: &Thresholds) -> Result<String> {
    // A small wrapper so the JSON has a stable top-level shape.
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        thresholds: &'a Thresholds,
        hosts: &'a [HostReport],
    }
    let env = Envelope { thresholds, hosts: reports };
    Ok(serde_json::to_string_pretty(&env)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::check::{HostOutcome, HostReport, Metric, Violation};
    use crate::metrics::Metrics;
    use std::time::Duration;

    fn defaults() -> Thresholds {
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

    fn ok_report(name: &str, violations: Vec<Violation>) -> HostReport {
        HostReport {
            name: name.into(),
            outcome: HostOutcome::Ok {
                metrics: Metrics {
                    uptime: Duration::from_secs(3_600),
                    disk_pct: 50,
                    temp_c: Some(45.0),
                    load_1m: 0.5,
                    mem_pct: 30,
                    swap_pct: Some(5),
                    proc_count: Some(150),
                },
                violations,
            },
        }
    }

    fn unreachable_report(name: &str, error: &str) -> HostReport {
        HostReport {
            name: name.into(),
            outcome: HostOutcome::Unreachable { error: error.into() },
        }
    }

    #[test]
    fn summary_line_all_healthy() {
        let reports = vec![ok_report("a", vec![]), ok_report("b", vec![])];
        let s = summary_line(&reports);
        assert!(s.contains("all 2 hosts healthy"), "got: {s}");
    }

    #[test]
    fn summary_line_counts_failures() {
        let reports = vec![
            ok_report("a", vec![]),
            ok_report(
                "b",
                vec![Violation { metric: Metric::Disk, value: 99.0, limit: 85.0 }],
            ),
            unreachable_report("c", "connection refused"),
        ];
        let s = summary_line(&reports);
        assert!(s.contains("1 unreachable"), "got: {s}");
        assert!(s.contains("1 with warnings"), "got: {s}");
        assert!(s.contains("of 3 hosts"), "got: {s}");
    }

    #[test]
    fn render_json_envelope_shape() {
        let reports = vec![
            ok_report("alpha", vec![]),
            unreachable_report("beta", "boom"),
        ];
        let json = render_json(&reports, &defaults()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(v.get("thresholds").is_some());
        let hosts = v["hosts"].as_array().expect("hosts is an array");
        assert_eq!(hosts.len(), 2);

        assert_eq!(hosts[0]["name"], "alpha");
        assert_eq!(hosts[0]["status"], "ok");
        // Uptime is serialized as a flat integer, not the default {secs, nanos}.
        assert!(hosts[0]["metrics"]["uptime_secs"].is_u64());

        assert_eq!(hosts[1]["name"], "beta");
        assert_eq!(hosts[1]["status"], "unreachable");
        assert_eq!(hosts[1]["error"], "boom");
    }

    #[test]
    fn render_table_includes_host_names_and_status() {
        let reports = vec![
            ok_report("alpha", vec![]),
            unreachable_report("beta", "no route to host"),
        ];
        let t = render_table(&reports);
        assert!(t.contains("alpha"));
        assert!(t.contains("beta"));
        assert!(t.contains("OK"));
        assert!(t.contains("UNREACHABLE"));
    }

    #[test]
    fn render_table_marks_warn_on_violations() {
        let reports = vec![ok_report(
            "alpha",
            vec![Violation { metric: Metric::Disk, value: 99.0, limit: 85.0 }],
        )];
        let t = render_table(&reports);
        assert!(t.contains("WARN"), "got: {t}");
    }

    #[test]
    fn render_json_includes_new_metric_fields() {
        let reports = vec![ok_report("alpha", vec![])];
        let json = render_json(&reports, &defaults()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(v["hosts"][0]["metrics"]["swap_pct"].is_u64());
        assert!(v["hosts"][0]["metrics"]["proc_count"].is_u64());
    }

    #[test]
    fn render_json_includes_custom_threshold_map() {
        let mut t = defaults();
        t.custom.insert("proc_count".into(), 500.0);
        let reports = vec![ok_report("alpha", vec![])];
        let json = render_json(&reports, &t).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["thresholds"]["custom"]["proc_count"], 500.0);
    }

    #[test]
    fn render_json_thresholds_keys_are_stable() {
        // The JSON contract pins which keys appear under `thresholds`. v2
        // additions are: `custom` (always present), `swap_pct`/`proc_count`
        // (only when set, via skip_serializing_if).
        let json = render_json(&[], &defaults()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let t = v["thresholds"].as_object().expect("thresholds is object");
        let mut keys: Vec<_> = t.keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["custom", "disk_pct", "load_1m", "mem_pct", "temp_c"],
        );
    }

    #[test]
    fn render_json_custom_violation_carries_metric_name() {
        // Custom-threshold violations should round-trip through JSON
        // carrying the metric name so cron pipelines can act on them.
        let reports = vec![ok_report(
            "alpha",
            vec![Violation {
                metric: Metric::Custom("proc_count".into()),
                value: 600.0,
                limit: 500.0,
            }],
        )];
        let json = render_json(&reports, &defaults()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        let violations = v["hosts"][0]["violations"].as_array().unwrap();
        assert_eq!(violations[0]["metric"]["custom"], "proc_count");
    }
}
