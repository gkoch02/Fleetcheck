# fleetcheck

A single-binary Rust CLI that reports the health of a fleet of Linux hosts
over SSH. Built for a small home Raspberry Pi fleet plus one Ubuntu box;
designed to run cleanly from cron.

Per run it concurrently checks each host for:

- SSH reachability
- uptime
- root-partition disk usage
- CPU temperature (where `/sys/class/thermal/thermal_zone0/temp` exists)
- 1-minute load average
- memory usage %

Results render as a colored table, or as JSON with `--json`. The process
exits non-zero when any host is unreachable or any configured threshold is
crossed.

## Setup (fresh Mac)

The steps below take a just-out-of-the-box Mac to a working `fleetcheck` on
your `$PATH`. They assume Apple Silicon or Intel macOS 12+; earlier macOS
versions work too but aren't tested.

### 1. Install Xcode Command Line Tools

This brings in `git`, `cc`, and the linker that `cargo` needs.

```sh
xcode-select --install
```

Accept the GUI prompt and wait for it to finish.

### 2. Install Rust

Use the official installer. No Homebrew needed — `rustup` manages its own
toolchains and keeps them current.

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Pick the default (stable) toolchain at the prompt, then reload your shell
so `~/.cargo/bin` is on `$PATH`:

```sh
source "$HOME/.cargo/env"
rustc --version   # expect 1.85 or newer
```

If `rustup` is already installed but the toolchain is older than 1.85,
update it before continuing — `openssh`'s transitive deps require the
2024 edition:

```sh
rustup update stable
```

### 3. Confirm `ssh` is present

macOS ships with OpenSSH. Nothing to install, but double-check:

```sh
ssh -V
```

### 4. Set up passwordless SSH to the fleet

Skip this if you've already copied keys over. Otherwise, generate a key
(if you don't already have `~/.ssh/id_ed25519`) and push it to each host:

```sh
ssh-keygen -t ed25519 -C "$(whoami)@$(hostname)"   # press Enter for defaults
for host in homebridge pihole fuzzyclock airquality dashboard counterpoint; do
    ssh-copy-id "$host"
done
```

Store the key in the macOS keychain so `ssh-agent` serves it automatically
across reboots (on modern macOS, `ssh` picks this up through `ssh-agent`
without extra config):

```sh
ssh-add --apple-use-keychain ~/.ssh/id_ed25519
```

Verify each host answers without prompting:

```sh
for host in homebridge pihole fuzzyclock airquality dashboard counterpoint; do
    ssh -o BatchMode=yes "$host" true && echo "$host ok" || echo "$host FAILED"
done
```

If any host fails, fix that before moving on — fleetcheck will treat it
as `UNREACHABLE`.

### 5. Install fleetcheck

```sh
git clone <this repo> ~/src/fleetcheck
cd ~/src/fleetcheck
cargo install --locked --path .
```

`cargo install` drops a release-mode `fleetcheck` in `~/.cargo/bin/`
(already on `$PATH` from step 2). Verify:

```sh
fleetcheck --version
```

To work in-tree without installing, use `cargo run --release -- <args>`
instead.

### 6. Create the config

```sh
mkdir -p ~/.config/fleetcheck
$EDITOR ~/.config/fleetcheck/hosts.toml
```

See the [Configuration](#configuration) section below for the schema, then
run `fleetcheck` — you should see a colored table of the fleet.

### Remote requirements

Each host needs POSIX `sh` plus `awk`, `df`, and `free`. These are present
by default on Raspberry Pi OS and Ubuntu, so no remote setup is required.

## Configuration

fleetcheck reads a TOML file at `~/.config/fleetcheck/hosts.toml` by default
(override with `--config <path>`).

```toml
# Default thresholds; tripped values render red and make the run exit 1.
[thresholds]
disk_pct = 85      # root partition used %
temp_c   = 75.0    # CPU °C
load_1m  = 2.0     # 1-minute load average
mem_pct  = 90      # used memory %

# Minimal host entry: the table key is both the label and the SSH destination.
[hosts.homebridge]
[hosts.pihole]
[hosts.airquality]
[hosts.dashboard]

# Override per host when defaults don't fit — e.g. a tiny SD card that runs
# near full on purpose.
[hosts.fuzzyclock]
  [hosts.fuzzyclock.thresholds]
  disk_pct = 95

# Full form: custom address, user, port.
[hosts.counterpoint]
addr = "counterpoint.lan"
user = "gkoch"
port = 22
```

**Field reference (host table):**

| Field        | Default                | Notes                                   |
|--------------|------------------------|-----------------------------------------|
| `addr`       | the table key          | Hostname or IP passed to `ssh`.         |
| `user`       | `$USER` / `~/.ssh/config` | Override when the remote user differs. |
| `port`       | 22 / `~/.ssh/config`   | Override for non-standard ports.        |
| `thresholds` | global `[thresholds]`  | Any subset of the four threshold keys.  |

Anything configurable through `~/.ssh/config` (jump hosts, key files, aliases)
is honored because fleetcheck shells out through the system `ssh`.

## Usage

```sh
fleetcheck                            # colored table
fleetcheck --json                     # machine-readable output
fleetcheck --config ./other.toml      # alternate config path
fleetcheck --timeout-secs 10          # per-host timeout (default 5s)
```

### Exit codes

| Code | Meaning                                                             |
|------|---------------------------------------------------------------------|
| 0    | All hosts reachable, no thresholds exceeded.                        |
| 1    | At least one host unreachable, or at least one threshold exceeded.  |
| 2    | Config missing/invalid, or an unrecoverable internal error.         |

### Cron example (macOS)

```cron
*/5 * * * * /Users/gkoch/.cargo/bin/fleetcheck --json > /tmp/fleetcheck.json 2>&1 || /usr/bin/osascript -e 'display notification "fleet unhealthy" with title "fleetcheck"'
```

cron on macOS needs Full Disk Access granted to `/usr/sbin/cron` in
System Settings → Privacy & Security → Full Disk Access if you want it
to read files outside `/tmp`. `launchd` is the more native alternative
if cron gives you trouble.

## Verification / smoke tests

After setup, run through these to confirm the install end-to-end:

1. **Happy path**
   ```sh
   fleetcheck
   echo "exit=$?"
   ```
   Expect: a colored table, a `✓ all N hosts healthy` summary line, and
   `exit=0`.

2. **JSON output parses and matches the table**
   ```sh
   fleetcheck --json | jq .
   ```
   Expect: a top-level `{ "thresholds": {...}, "hosts": [...] }` object,
   one entry per host, sorted by name, same numbers as the table.

3. **Threshold violation triggers a non-zero exit**
   Temporarily edit `~/.config/fleetcheck/hosts.toml` to set an impossibly
   tight threshold, e.g. `disk_pct = 1`:
   ```sh
   fleetcheck; echo "exit=$?"
   ```
   Expect: offending cells render red and bold, the status column reads
   `WARN`, summary line starts with a red `✗`, `exit=1`.

4. **Unreachable host**
   Power off one host (or add a fake entry pointing at an unreachable
   address) and rerun:
   ```sh
   fleetcheck; echo "exit=$?"
   ```
   Expect: that host's row shows `UNREACHABLE (...)` in red with em-dashes
   in the metric columns, `exit=1`.

5. **Missing config — hard error path**
   ```sh
   fleetcheck --config /does/not/exist; echo "exit=$?"
   ```
   Expect: a single-line error like `fleetcheck: reading config at
   /does/not/exist: No such file or directory (os error 2)`, `exit=2`.

6. **Non-TTY output is plain**
   ```sh
   fleetcheck | cat
   ```
   Expect: no ANSI color escapes in the piped output (comfy-table and
   owo-colors both detect the non-TTY sink).

## Development

```sh
cargo check                               # fast type-check
cargo clippy --all-targets -- -D warnings # lints, warnings as errors
cargo test                                # unit tests (parsing, merging, eval)
cargo build --release                     # optimized binary
```

### Layout

```
src/
  main.rs     # entry, exit codes, tokio runtime
  cli.rs      # clap derive struct
  config.rs   # Config / Thresholds / PartialThresholds + load()
  metrics.rs  # Metrics struct, key=value parser, uptime formatter
  ssh.rs      # openssh Session + run_script (native-mux)
  script.sh   # bundled metric collector, included via include_str!
  check.rs    # per-host orchestration + threshold evaluation
  report.rs   # table (comfy-table) + JSON + summary line (owo-colors)
```

### Extending with a new metric

1. Add one `echo "key=value"` line to `src/script.sh`.
2. Add a field to `Metrics` in `src/metrics.rs` and handle the new key in
   `parse()`.
3. (Optional) Add a threshold field in `src/config.rs` and a `Violation`
   check in `src/check.rs::evaluate`.
4. Add a column in `src/report.rs`.

The parser ignores unknown keys, so an updated script can roll out ahead
of the binary without breaking existing deployments.
