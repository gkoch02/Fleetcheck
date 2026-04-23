use std::time::Duration;

use anyhow::{anyhow, Result};
use serde::{Serialize, Serializer};

#[derive(Debug, Clone, Serialize)]
pub struct Metrics {
    #[serde(serialize_with = "ser_secs", rename = "uptime_secs")]
    pub uptime: Duration,
    pub disk_pct: u8,
    pub temp_c: Option<f32>,
    pub load_1m: f32,
    pub mem_pct: u8,
}

// Flat u64 instead of serde's default {"secs": N, "nanos": N} struct shape.
fn ser_secs<S: Serializer>(d: &Duration, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_u64(d.as_secs())
}

/// Parse the `key=value` output of `script.sh` into a `Metrics`.
///
/// Missing required keys or unparseable numbers produce an error; an empty
/// `temp_millic=` value is accepted and yields `temp_c: None` (boxes without
/// `/sys/class/thermal/thermal_zone0/temp`).
pub fn parse(text: &str) -> Result<Metrics> {
    let mut uptime_secs: Option<u64> = None;
    let mut disk_pct: Option<u8> = None;
    let mut temp_c: Option<f32> = None;
    let mut load_1m: Option<f32> = None;
    let mut mem_pct: Option<u8> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow!("malformed metric line: {line:?}"))?;
        match key {
            "uptime_secs" => uptime_secs = Some(value.parse()?),
            "disk_pct" => disk_pct = Some(value.parse()?),
            "temp_millic" => {
                temp_c = if value.is_empty() {
                    None
                } else {
                    Some(value.parse::<f32>()? / 1000.0)
                };
            }
            "load_1m" => load_1m = Some(value.parse()?),
            "mem_pct" => mem_pct = Some(value.parse()?),
            _ => {} // ignore unknown keys so the script can evolve without breaking old clients
        }
    }

    Ok(Metrics {
        uptime: Duration::from_secs(
            uptime_secs.ok_or_else(|| anyhow!("missing uptime_secs"))?,
        ),
        disk_pct: disk_pct.ok_or_else(|| anyhow!("missing disk_pct"))?,
        temp_c,
        load_1m: load_1m.ok_or_else(|| anyhow!("missing load_1m"))?,
        mem_pct: mem_pct.ok_or_else(|| anyhow!("missing mem_pct"))?,
    })
}

/// Render a `Duration` as e.g. "4d 2h" / "17m" / "42s". Low-fidelity on purpose.
pub fn format_uptime(d: Duration) -> String {
    let s = d.as_secs();
    let (days, rem) = (s / 86_400, s % 86_400);
    let (hours, rem) = (rem / 3_600, rem % 3_600);
    let (mins, secs) = (rem / 60, rem % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else if mins > 0 {
        format!("{mins}m")
    } else {
        format!("{secs}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_healthy_pi_output() {
        let raw = "\
uptime_secs=359245
disk_pct=42
temp_millic=48312
load_1m=0.17
mem_pct=31
";
        let m = parse(raw).unwrap();
        assert_eq!(m.uptime, Duration::from_secs(359_245));
        assert_eq!(m.disk_pct, 42);
        assert!((m.temp_c.unwrap() - 48.312).abs() < 1e-3);
        assert!((m.load_1m - 0.17).abs() < 1e-6);
        assert_eq!(m.mem_pct, 31);
    }

    #[test]
    fn empty_temp_is_none() {
        let raw = "\
uptime_secs=1
disk_pct=1
temp_millic=
load_1m=0.0
mem_pct=1
";
        let m = parse(raw).unwrap();
        assert!(m.temp_c.is_none());
    }

    #[test]
    fn missing_required_metric_errors() {
        let raw = "uptime_secs=1\ndisk_pct=1\ntemp_millic=\nload_1m=0.0\n";
        assert!(parse(raw).is_err());
    }

    #[test]
    fn uptime_formatting() {
        assert_eq!(format_uptime(Duration::from_secs(30)), "30s");
        assert_eq!(format_uptime(Duration::from_secs(125)), "2m");
        assert_eq!(format_uptime(Duration::from_secs(3_700)), "1h 1m");
        assert_eq!(format_uptime(Duration::from_secs(90_000)), "1d 1h");
    }
}
