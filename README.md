# mtp-tui

![Rust](https://img.shields.io/badge/Rust-000000?style=flat&logo=rust&logoColor=white)
[![CI](https://github.com/num13ru/mtp-tui/actions/workflows/CI.yml/badge.svg)](https://github.com/num13ru/mtp-tui/actions/workflows/CI.yml)

A terminal file manager for MTP devices (Android phones, Kindle, etc.).

Two-pane layout: local filesystem on the left, device storage on the right. Browse, push, pull, delete, rename, and create directories.

Built with [ratatui](https://ratatui.rs) and [mtp-rs](https://github.com/vdavid/mtp-rs). Pure Rust, no libmtp/libusb.

![mtp-tui screenshot](assets/screenshot.png)

## Features

- **Device support** — Android phones, Kindle, cameras via MTP/PTP
- **Two-pane browsing** — local filesystem + device storage side by side
- **File transfers** — push (`p`) and pull (`g`) with overwrite confirmation
- **File management** — delete (`d`), rename (`R`), create directory (`m`)
- **Object inspector** *(WIP)* — view MTP metadata and properties (`i`)
- **Async loading** — spinner and streaming progress ("Loading 42/500..."), UI never freezes
- **Storage info** — free / total space display

See [ROADMAP.md](ROADMAP.md) for planned improvements.

## Usage

Connect an MTP device via USB, then:

```sh
cargo run
```

### macOS

If macOS grabs the device first:

```sh
sudo killall ptpcamerad
```

### Keybindings

| Key | Action |
|---|---|
| `Tab` | Switch active pane |
| `j` / `k` | Move selection down / up |
| `Enter` | Enter directory |
| `Backspace` | Go to parent |
| `p` | Push selected host file to device |
| `g` | Pull selected device file to host |
| `d` | Delete selected device entry |
| `m` | Create directory on device |
| `R` | Rename selected device entry |
| `i` | Inspect object metadata |
| `r` | Refresh both panes |
| `?` | Toggle help overlay |
| `Esc` | Close dialog / help |
| `q` | Quit (confirms) |
| `Ctrl+C` | Force quit |

## Configuration

On first run, mtp-tui creates a config file at `~/.config/mtp-tui/config.toml` with all options commented out. Edit it to customize behavior:

```toml
# Host pane opens here instead of the current working directory.
# Supports ~ for home directory.
default_host_dir = "~/Downloads"

# Navigate to this device folder after connecting (default: root).
default_device_dir = "/Download"
```

`$XDG_CONFIG_HOME` is respected when set. If the config file is missing or malformed, defaults are used silently.

## Diagnostics

Dump device capabilities, supported operations, storages, and root objects:

```sh
mkdir -p log && cargo run --example mtp_capabilities > log/mtp_capabilities.log 2>&1
```

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2024, requires rustc 1.85+)
- [just](https://github.com/casey/just) — task runner

### Setup

```sh
git clone https://github.com/num13ru/mtp-tui.git
cd mtp-tui
cargo build
```

### Common tasks

| Command | Description |
|---|---|
| `just` | Run all checks (format, clippy, tests) |
| `just fix` | Auto-fix formatting and clippy warnings |
| `just fmt` | Format code with `cargo fmt` |
| `just fmt-check` | Check formatting without modifying files |
| `just clippy` | Run clippy with `-D warnings` |
| `just test` | Run tests |

### Workflow

```sh
just        # before committing — runs fmt-check, clippy, tests
just fix    # auto-fix what can be fixed
```

Clippy is configured with `-D warnings` — all warnings are treated as errors. See [`clippy.toml`](clippy.toml) for project-specific lints.

## License

MIT
