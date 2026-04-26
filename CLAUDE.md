# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`fleetcheck` is a single-binary Rust CLI that SSHes into a list of Linux hosts in
parallel, runs an embedded shell script to collect seven metrics (uptime, root
disk %, CPU temp, 1m load, mem %, swap %, proc count), and reports either a
colored table or JSON. Designed to be run from cron, so its exit code is the
primary signal.

Requires Rust 1.85+ (the dependency tree pulls in 2024-edition crates).

## Common commands

```sh
cargo check                                 # fast type-check
cargo clippy --all-targets -- -D warnings   # CI runs this; warnings are errors
cargo test                                  # all unit tests
cargo test <name>                           # single test, e.g. `cargo test parses_healthy_pi_output`
cargo test --package fleetcheck <module>::  # all tests in a module, e.g. `metrics::`
cargo build --release                       # optimized binary in target/release/fleetcheck
cargo run --release -- --json               # run in-tree without installing
cargo install --locked --path .             # install to ~/.cargo/bin
```

CI (`.github/workflows/ci.yml`) runs clippy, tests, and a release build all with
`--locked`; mirror that locally before pushing if you've touched `Cargo.toml`.

## Architecture

Flow of one run (entry point: `src/main.rs`):

1. `cli::Cli::parse` → resolve `Timeouts { connect, script }` from the split
   `--connect-timeout` / `--script-timeout` flags (the legacy `--timeout-secs`
   sets both). `config::load` reads `~/.config/fleetcheck/hosts.toml` (XDG
   path on every OS, including macOS — see `config::default_path`).
2. For each host, build a future that calls `check::check_host`. Fan-out is
   capped by `--max-concurrent` via `futures::stream::buffer_unordered`
   (`--max-concurrent 0` falls back to unbounded `join_all`). We deliberately
   want partial results, not short-circuit-on-first-error.
3. Each `check_host` does: `ssh::connect` (wrapped in a retry-with-jitter
   loop bounded by `--retries` / per-host `retries`; opens an `openssh`
   native-mux ControlMaster) → `ssh::run_script` (pipes the embedded script
   over stdin to `sh -s`; not retried) → `metrics::parse` → `check::evaluate`
   against thresholds (typed metrics + the `[thresholds.custom]` map).
4. Results are sorted by name and rendered through `report::render_table` +
   `report::summary_line`, or `report::render_json`.
5. `main` maps the result to an exit code: `0` healthy, `1` any host
   unreachable or any threshold tripped (the case cron cares about), `2` hard
   error (bad config, etc.).

Module map:

- `main.rs` — tokio runtime, fan-out, exit-code mapping.
- `cli.rs` — clap derive struct.
- `config.rs` — `Config`, `Thresholds` (typed core + `custom: BTreeMap<String, f64>`
  escape hatch; `Clone` not `Copy`), `PartialThresholds`, `HostConfig` (with
  optional `retries` override), `Thresholds::merged` (per-host overlay,
  custom-map merged per-key), `load`.
- `metrics.rs` — `Metrics` struct, `parse` for `key=value` script output,
  `format_uptime`. New v2 fields (`swap_pct`, `proc_count`) are `Option`.
- `ssh.rs` — `connect` + `run_script` (each takes its own timeout). The
  bundled script is loaded via `include_str!("script.sh")`, so the binary has
  no runtime asset dependency.
- `script.sh` — POSIX `sh` + `awk` + `df` + `free` + `ps`. Emits one
  `key=value` per line; an empty value (e.g. `temp_millic=` or `swap_pct=`)
  means "metric unavailable".
- `check.rs` — `Timeouts`, `HostReport` / `HostOutcome` (serde-tagged enum:
  `status: "ok"` or `"unreachable"` is flattened into each row), `Violation`,
  `Metric` (typed variants plus `Custom(String)` for `[thresholds.custom]`
  hits), `evaluate`, `metric_value_by_name`, `check_host`, `retry_async` +
  `backoff_with_jitter` (used only around `ssh::connect`).
- `report.rs` — comfy-table renderer (host, status, uptime, disk %, temp,
  load, mem %, swap %, procs, ip), JSON envelope (`{ thresholds, hosts }`),
  summary line. Uses `owo-colors` with `Stream::Stdout` so colors auto-disable
  when piped.

## Conventions worth respecting

**Forward-compatible script protocol.** `metrics::parse` ignores unknown keys
on purpose — a newer `script.sh` can roll out before the binary is updated. If
you add a metric, the parse step should keep accepting old script output too
(make new fields `Option` or default them).

**Threshold comparison is strictly `>`.** A value sitting exactly on the
threshold is healthy. There's a test pinning this (`equal_to_threshold_is_not_a_violation`).

**Errors flow into `HostOutcome::Unreachable`, not out of `check_host`.** Any
SSH/parse failure for one host must not abort the whole run — the table needs
partial results. `check_host` is infallible by signature.

**Sorted output.** `Config::hosts` is a `BTreeMap` so iteration is already
sorted by hostname; `main` also sorts the report vec after `join_all`. Don't
swap to `HashMap` without restoring the sort somewhere.

**Custom serde for `Duration`.** `Metrics::uptime` serializes as a flat
`uptime_secs: u64` via `ser_secs`, not serde's default `{secs, nanos}` struct.
A test pins this shape (`render_json_envelope_shape`).

**JSON shape is part of the contract.** Top-level is
`{ "thresholds": {...}, "hosts": [...] }` and each host row is flattened with
`status: "ok" | "unreachable"` plus the variant payload. Cron pipelines parse
this; don't reshape it casually. v2 additions are additive, but the rule
differs by struct:

- **`Metrics`** always serializes every field. `Option` fields appear as
  `null` when absent (`temp_c`, `swap_pct`, `proc_count`, `ip_addr` all
  follow this). This gives consumers a stable schema — they can always
  reach `metrics.foo` without checking for key existence.
- **`Thresholds`** uses `skip_serializing_if = "Option::is_none"` on the
  v2-added typed fields (`swap_pct`, `proc_count`) so a config that
  doesn't set them produces JSON byte-identical to v1 under the
  `thresholds` key. `custom` is always present (an empty `{}` when
  unused) so consumers can always do `thresholds.custom.foo`.

Tests pin the typed `thresholds` key set under default config
(`render_json_thresholds_keys_are_stable`) and the `Metric::Custom`
payload shape.

**Custom thresholds without re-typing.** Users can add a `[thresholds.custom]`
TOML map to threshold any metric the script emits — even ones the binary
doesn't have a typed field for. The lookup goes through
`check::metric_value_by_name`; new typed metrics should also get a match arm
there. Custom-map keys with no matching metric are silently skipped (forward
compat).

**Retries are connect-only.** `check::run` wraps `ssh::connect` in
`retry_async`; `run_script` and `metrics::parse` are not retried. A
successful connect followed by a script/parse failure is deterministic and
re-running it would risk masking real outages or, in the future, side
effects.

## Adding a new metric

The README documents this and the parse layer is built for it:

1. Add a `key=value` line to `src/script.sh`.
2. Add an `Option<...>` field to `Metrics` in `src/metrics.rs` and a match
   arm in `parse()`. New fields should always be `Option` so a v2 binary
   running against an older script keeps working.
3. Decide between typed and custom-map thresholding:
   - **Typed (recommended for headline metrics):** add a field to
     `Thresholds` / `PartialThresholds` in `src/config.rs`, a `Metric`
     variant and an `evaluate` branch in `src/check.rs`, and a column in
     `src/report.rs`.
   - **Custom-map only:** add an arm to `check::metric_value_by_name` so the
     metric becomes thresholdable via `[thresholds.custom]`. No `Metric`
     variant needed — it surfaces as `Metric::Custom("your_key")`.
4. Add a column to `src/report.rs` if you want it in the table (both table
   row builders and, implicitly, JSON via the `Serialize` derive).
