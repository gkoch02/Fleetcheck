# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`fleetcheck` is a single-binary Rust CLI (edition 2021, MSRV 1.75) that concurrently checks the
health of a small fleet of Linux hosts over SSH and renders a colored table or JSON. It is designed
to be cron-friendly: exit codes drive alerting, not parsing.

## Common commands

```sh
cargo check                                # fast type-check
cargo clippy --all-targets -- -D warnings  # lint, warnings-as-errors
cargo test                                 # unit tests live inline as `#[cfg(test)] mod tests`
cargo test <name>                          # single test by substring, e.g. `cargo test parses_healthy_pi_output`
cargo test -- --nocapture                  # show println! from tests
cargo build --release                      # optimized binary in target/release/fleetcheck
cargo run -- --json                        # run against the default config
cargo run -- --config ./hosts.toml         # run against a specific config
cargo install --path .                     # install to ~/.cargo/bin
```

There is no integration test suite, no CI config, and no `tests/` directory — all tests are unit
tests colocated with the module they cover (see `config.rs`, `metrics.rs`, `check.rs`).

## Exit-code contract

`main.rs` encodes three distinct exit paths and callers (cron, scripts) depend on them:

| Code | Meaning                                                            |
|------|--------------------------------------------------------------------|
| 0    | All hosts reachable, no thresholds exceeded.                       |
| 1    | Soft failure — any host unreachable, or any threshold crossed.     |
| 2    | Hard failure — config missing/invalid or unrecoverable error.      |

Preserve this split when touching `main::run`: return `Ok(true)` for soft failures and `Err(...)`
only for things a human needs to go fix. The `HostReport::is_bad()` method is the single source of
truth for "does this host contribute to exit=1".

## Architecture

The data flow is a straight pipeline, one module per stage:

```
cli.rs ─▶ config.rs ─▶ check.rs ──▶ ssh.rs ──▶ script.sh (remote)
                         │             │
                         │             └─▶ stdout text
                         ▼
                   metrics.rs (parse key=value)
                         │
                         ▼
                  check.rs::evaluate (Vec<Violation>)
                         │
                         ▼
                    report.rs (table | JSON)
```

Key cross-cutting decisions that are easy to miss:

- **Fan-out, never short-circuit.** `main::run` uses `futures::join_all` over all hosts so a single
  failure never hides results for the rest. `check_host` is infallible at the type level — any
  error becomes `HostOutcome::Unreachable { error }` so the row still renders.
- **Embedded remote script.** `src/script.sh` is pulled in with `include_str!` and piped to
  `sh -s` over stdin. The binary therefore has no runtime dependency beyond the system `ssh` and
  POSIX `sh`/`awk`/`df`/`free` on the remote.
- **Forgiving metric parser.** `metrics::parse` ignores unknown `key=value` lines on purpose, so a
  newer `script.sh` can deploy to a fleet before the binary is rebuilt. Add new keys rather than
  reshaping existing ones.
- **Thermal zone is optional.** `temp_millic=` (empty value) is a first-class case meaning "this
  host has no thermal zone" and must not be an error; it surfaces as `temp_c: None` and an em-dash
  cell in the table.
- **Thresholds merge, not replace.** `Thresholds::merged(Option<&PartialThresholds>)` overlays
  per-host overrides on the global `[thresholds]` table. Any new threshold field needs three
  coordinated edits: `Thresholds`, `PartialThresholds`, and the `merged` body.
- **SSH reuse.** `ssh::connect` opens a `native-mux` ControlMaster; subsequent commands reuse the
  socket. The `connect_timeout` bounds only the handshake — command execution is bounded
  separately by `tokio::time::timeout` inside `run_script`. Keep those two timeouts distinct.
- **Sorted output is a property of the Config type.** `Config::hosts` is a `BTreeMap`, so iteration
  is alphabetical without an explicit sort. `main::run` additionally sorts reports by name after
  `join_all` because completion order is nondeterministic.
- **Color detection.** `report.rs` uses `owo_colors`'s `if_supports_color(Stream::Stdout, ...)` and
  `comfy-table`'s built-in TTY detection. Do not hardcode ANSI escapes — piped output must stay
  plain so users can grep/jq it.

## Adding a new metric (codified convention)

Changes must land in this order to preserve the "script rolls out ahead of binary" property:

1. `src/script.sh` — add one `echo "key=value"` line (empty value when unavailable).
2. `src/metrics.rs` — add the field to `Metrics` and a match arm in `parse()`.
3. `src/config.rs` — if it has a threshold, add to both `Thresholds` and `PartialThresholds`, and
   extend `Thresholds::merged`.
4. `src/check.rs::evaluate` — add a `Violation` branch and a variant to the `Metric` enum.
5. `src/report.rs` — add a column in both `ok_row` and `unreachable_row` (keep widths aligned).

## Branch policy

Work on the branch specified by the active task (currently
`claude/add-claude-documentation-GY7dR`). Push with `git push -u origin <branch>`; do not open PRs
unless the user explicitly asks.
