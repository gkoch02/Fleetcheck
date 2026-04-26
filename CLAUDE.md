# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`fleetcheck` is a single-binary Rust CLI that SSHes into a list of Linux hosts in
parallel, runs an embedded shell script to collect five metrics (uptime, root
disk %, CPU temp, 1m load, mem %), and reports either a colored table or JSON.
Designed to be run from cron, so its exit code is the primary signal.

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

1. `cli::Cli::parse` → `config::load` reads `~/.config/fleetcheck/hosts.toml`
   (XDG path on every OS, including macOS — see `config::default_path`).
2. For each host, build a future that calls `check::check_host`. All futures
   run concurrently under `futures::future::join_all` — we deliberately want
   partial results, not short-circuit-on-first-error.
3. Each `check_host` does: `ssh::connect` (opens an `openssh` native-mux
   ControlMaster) → `ssh::run_script` (pipes the embedded script over stdin to
   `sh -s`) → `metrics::parse` → `check::evaluate` against thresholds.
4. Results are sorted by name and rendered through `report::render_table` +
   `report::summary_line`, or `report::render_json`.
5. `main` maps the result to an exit code: `0` healthy, `1` any host
   unreachable or any threshold tripped (the case cron cares about), `2` hard
   error (bad config, etc.).

Module map:

- `main.rs` — tokio runtime, fan-out, exit-code mapping.
- `cli.rs` — clap derive struct.
- `config.rs` — `Config`, `Thresholds`, `PartialThresholds`, `HostConfig`,
  `Thresholds::merged` (per-host overlay), `load`.
- `metrics.rs` — `Metrics` struct, `parse` for `key=value` script output,
  `format_uptime`.
- `ssh.rs` — `connect` + `run_script`. The bundled script is loaded via
  `include_str!("script.sh")`, so the binary has no runtime asset dependency.
- `script.sh` — POSIX `sh` + `awk` + `df` + `free`. Emits one `key=value` per
  line; an empty value (e.g. `temp_millic=`) means "metric unavailable".
- `check.rs` — `HostReport` / `HostOutcome` (serde-tagged enum: `status: "ok"`
  or `"unreachable"` is flattened into each row), `Violation`, `Metric`,
  `evaluate`, `check_host`.
- `report.rs` — comfy-table renderer, JSON envelope (`{ thresholds, hosts }`),
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
this; don't reshape it casually.

## Adding a new metric

The README documents this and the parse layer is built for it:

1. Add a `key=value` line to `src/script.sh`.
2. Add the field to `Metrics` in `src/metrics.rs` and a match arm in `parse()`.
3. Optional: add a `Thresholds` field in `src/config.rs`, a `Metric` variant
   and an `evaluate` branch in `src/check.rs`.
4. Add a column to `src/report.rs` (both table row builders and, implicitly,
   JSON via the `Serialize` derive).
