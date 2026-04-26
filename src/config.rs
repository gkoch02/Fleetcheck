use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Config {
    pub thresholds: Thresholds,
    // BTreeMap instead of HashMap so iteration is already sorted by hostname.
    pub hosts: BTreeMap<String, HostConfig>,
}

/// `Copy` was dropped to make room for the `custom` map (BTreeMap isn't Copy).
/// `merged` now takes `&self`, which is cheap thanks to a small core plus a
/// usually-empty map.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Thresholds {
    pub disk_pct: u8,
    pub temp_c: f32,
    pub load_1m: f32,
    pub mem_pct: u8,
    /// New typed thresholds added in v2. `Option` so old configs without
    /// these keys keep parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_pct: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proc_count: Option<u32>,
    /// Escape hatch for thresholding any metric the script emits, including
    /// future ones the binary doesn't have a typed field for. Keys are
    /// metric names (e.g. `"swap_pct"`, `"disk_pct"`, `"proc_count"`),
    /// values are the upper bound (strictly `>` is a violation, matching
    /// the typed thresholds). Sorted for stable JSON output.
    #[serde(default)]
    pub custom: BTreeMap<String, f64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct PartialThresholds {
    pub disk_pct: Option<u8>,
    pub temp_c: Option<f32>,
    pub load_1m: Option<f32>,
    pub mem_pct: Option<u8>,
    pub swap_pct: Option<u8>,
    pub proc_count: Option<u32>,
    /// Per-host entries are merged on top of the global map per-key, not as
    /// a whole-map replacement.
    #[serde(default)]
    pub custom: BTreeMap<String, f64>,
}

#[derive(Debug, Deserialize)]
pub struct HostConfig {
    pub addr: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub thresholds: Option<PartialThresholds>,
    /// Per-host override for connect-retry count. Used only when the global
    /// `--retries` flag is left at its default of 0; otherwise the CLI wins.
    pub retries: Option<u32>,
}

impl Thresholds {
    /// Overlay optional per-host overrides on top of the global defaults.
    /// Custom-map entries are merged per-key, so a host can shadow one
    /// custom threshold without redeclaring the whole map.
    pub fn merged(&self, over: Option<&PartialThresholds>) -> Self {
        let Some(o) = over else { return self.clone() };
        let mut custom = self.custom.clone();
        for (k, v) in &o.custom {
            custom.insert(k.clone(), *v);
        }
        Self {
            disk_pct: o.disk_pct.unwrap_or(self.disk_pct),
            temp_c: o.temp_c.unwrap_or(self.temp_c),
            load_1m: o.load_1m.unwrap_or(self.load_1m),
            mem_pct: o.mem_pct.unwrap_or(self.mem_pct),
            swap_pct: o.swap_pct.or(self.swap_pct),
            proc_count: o.proc_count.or(self.proc_count),
            custom,
        }
    }
}

pub fn default_path() -> Option<PathBuf> {
    // Use ~/.config/ on every platform, including macOS where dirs::config_dir()
    // points at ~/Library/Application Support/. fleetcheck is a Unix-y CLI and
    // the README documents the XDG path.
    dirs::home_dir().map(|h| h.join(".config").join("fleetcheck").join("hosts.toml"))
}

pub fn load(path: &Path) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    let cfg: Config = toml::from_str(&raw)
        .with_context(|| format!("parsing config at {}", path.display()))?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Thresholds {
        Thresholds {
            disk_pct: 85,
            temp_c: 75.0,
            load_1m: 2.0,
            mem_pct: 90,
            swap_pct: None,
            proc_count: None,
            custom: BTreeMap::new(),
        }
    }

    #[test]
    fn merge_prefers_override_when_present() {
        let over = PartialThresholds { disk_pct: Some(95), ..Default::default() };
        let merged = defaults().merged(Some(&over));
        assert_eq!(merged.disk_pct, 95);
        assert_eq!(merged.temp_c, 75.0);
        assert_eq!(merged.mem_pct, 90);
    }

    #[test]
    fn merge_returns_global_when_no_override() {
        let merged = defaults().merged(None);
        assert_eq!(merged.disk_pct, 85);
    }

    #[test]
    fn merge_overlays_custom_map_per_key() {
        // Per-host custom-threshold overrides should shadow the global entry
        // for the same key without dropping unrelated global entries.
        let mut global = defaults();
        global.custom.insert("disk_pct".into(), 90.0);
        global.custom.insert("proc_count".into(), 500.0);

        let mut over = PartialThresholds::default();
        over.custom.insert("proc_count".into(), 1000.0);
        over.custom.insert("swap_pct".into(), 50.0);

        let merged = global.merged(Some(&over));
        assert_eq!(merged.custom.get("disk_pct"), Some(&90.0)); // global preserved
        assert_eq!(merged.custom.get("proc_count"), Some(&1000.0)); // host wins
        assert_eq!(merged.custom.get("swap_pct"), Some(&50.0)); // host adds
    }

    #[test]
    fn old_config_without_custom_still_loads() {
        // Forward compat: configs written for v1 (no swap_pct, proc_count,
        // or custom keys) must keep parsing cleanly.
        let raw = r#"
            [thresholds]
            disk_pct = 85
            temp_c = 75.0
            load_1m = 2.0
            mem_pct = 90
            [hosts.x]
        "#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert!(cfg.thresholds.swap_pct.is_none());
        assert!(cfg.thresholds.proc_count.is_none());
        assert!(cfg.thresholds.custom.is_empty());
    }

    #[test]
    fn parses_custom_thresholds_table() {
        let raw = r#"
            [thresholds]
            disk_pct = 85
            temp_c = 75.0
            load_1m = 2.0
            mem_pct = 90
            swap_pct = 50
            proc_count = 500

            [thresholds.custom]
            something_new = 12.5
            disk_pct = 95.0

            [hosts.x]
        "#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.thresholds.swap_pct, Some(50));
        assert_eq!(cfg.thresholds.proc_count, Some(500));
        assert_eq!(cfg.thresholds.custom.get("something_new"), Some(&12.5));
        assert_eq!(cfg.thresholds.custom.get("disk_pct"), Some(&95.0));
    }

    #[test]
    fn parses_full_config() {
        let raw = r#"
            [thresholds]
            disk_pct = 85
            temp_c = 75.0
            load_1m = 2.0
            mem_pct = 90

            [hosts.homebridge]

            [hosts.fuzzyclock]
            [hosts.fuzzyclock.thresholds]
            disk_pct = 95

            [hosts.counterpoint]
            addr = "counterpoint.lan"
            user = "gkoch"
            retries = 2
        "#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.thresholds.disk_pct, 85);
        assert_eq!(cfg.hosts.len(), 3);
        assert_eq!(cfg.hosts["counterpoint"].addr.as_deref(), Some("counterpoint.lan"));
        assert_eq!(cfg.hosts["counterpoint"].retries, Some(2));
        assert!(cfg.hosts["homebridge"].retries.is_none());
        let fc_over = cfg.hosts["fuzzyclock"].thresholds.as_ref().unwrap();
        assert_eq!(fc_over.disk_pct, Some(95));
    }
}
