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
