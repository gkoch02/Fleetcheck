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

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct Thresholds {
    pub disk_pct: u8,
    pub temp_c: f32,
    pub load_1m: f32,
    pub mem_pct: u8,
}

#[derive(Debug, Default, Deserialize)]
pub struct PartialThresholds {
    pub disk_pct: Option<u8>,
    pub temp_c: Option<f32>,
    pub load_1m: Option<f32>,
    pub mem_pct: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct HostConfig {
    pub addr: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub thresholds: Option<PartialThresholds>,
}

impl Thresholds {
    /// Overlay optional per-host overrides on top of the global defaults.
    /// Takes the global by value (it's `Copy`), which keeps the call site clean.
    pub fn merged(self, over: Option<&PartialThresholds>) -> Self {
        let Some(o) = over else { return self };
        Self {
            disk_pct: o.disk_pct.unwrap_or(self.disk_pct),
            temp_c: o.temp_c.unwrap_or(self.temp_c),
            load_1m: o.load_1m.unwrap_or(self.load_1m),
            mem_pct: o.mem_pct.unwrap_or(self.mem_pct),
        }
    }
}

pub fn default_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("fleetcheck").join("hosts.toml"))
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

    #[test]
    fn merge_prefers_override_when_present() {
        let global = Thresholds { disk_pct: 85, temp_c: 75.0, load_1m: 2.0, mem_pct: 90 };
        let over = PartialThresholds { disk_pct: Some(95), ..Default::default() };
        let merged = global.merged(Some(&over));
        assert_eq!(merged.disk_pct, 95);
        assert_eq!(merged.temp_c, 75.0);
        assert_eq!(merged.mem_pct, 90);
    }

    #[test]
    fn merge_returns_global_when_no_override() {
        let global = Thresholds { disk_pct: 85, temp_c: 75.0, load_1m: 2.0, mem_pct: 90 };
        let merged = global.merged(None);
        assert_eq!(merged.disk_pct, 85);
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
        "#;
        let cfg: Config = toml::from_str(raw).unwrap();
        assert_eq!(cfg.thresholds.disk_pct, 85);
        assert_eq!(cfg.hosts.len(), 3);
        assert_eq!(cfg.hosts["counterpoint"].addr.as_deref(), Some("counterpoint.lan"));
        let fc_over = cfg.hosts["fuzzyclock"].thresholds.as_ref().unwrap();
        assert_eq!(fc_over.disk_pct, Some(95));
    }
}
