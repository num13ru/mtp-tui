# mtp-tui

![Rust](https://img.shields.io/badge/Rust-000000?style=flat&logo=rust&logoColor=white)
[![CI](https://github.com/num13ru/mtp-tui/actions/workflows/CI.yml/badge.svg)](https://github.com/num13ru/mtp-tui/actions/workflows/CI.yml)

A terminal file manager for MTP devices (Android phones, Kindle, etc.).

Two-pane layout: local filesystem on the left, device storage on the right. Browse, push, pull, delete, rename, and create directories.

Unlike mount-based MTP helpers, mtp-tui talks to devices directly through [mtp-rs](https://github.com/num13ru/mtp-rs) and provides a two-pane file manager interface. No FUSE, no libmtp, no libusb — pure Rust on [nusb](https://crates.io/crates/nusb).

Built with [ratatui](https://ratatui.rs).

![mtp-tui screenshot](assets/screenshot.png)

## Features

- **Device support** — Android phones, Kindle, and other MTP/PTP-compatible devices.
- **Two-pane browsing** — local filesystem + device storage side by side
- **File transfers** — push (`p`) and pull (`g`) with overwrite confirmation
- **File management** — delete (`d`), rename (`R`), create directory (`m`)
- **Object inspector** *(WIP)* — view MTP metadata and properties (`i`)
- **Async loading** — directory listing runs in a background thread with streaming progress; entries appear incrementally
- **Storage info** — free / total space display

See [ROADMAP.md](ROADMAP.md) for planned improvements.

## Why not just mount MTP?

mtp-tui does not mount the device as a filesystem. It talks to the device through MTP operations directly. This avoids FUSE/libmtp dependencies, but also means some filesystem-like operations depend on device capabilities (e.g., rename requires `SetObjectPropValue` support).

## Safety notes

MTP does not support true atomic overwrite. When overwriting a file on the device, mtp-tui deletes the existing object before uploading the replacement. If the upload fails or the device disconnects mid-transfer, the original file is lost and manual cleanup may be required.

## Tested devices

| Device | OS / Firmware | Browse | Push | Pull | Delete | Rename | Notes |
|---|---|:---:|:---:|:---:|:---:|:---:|---|
| Kindle Paperwhite (GN433X) | — | ✅ | ✅ | ✅ | ✅ | ✅ | — |

> Rename support depends on `SetObjectPropValue (0x9804)`. Some devices advertise it but reject writes.

## Known limitations

- No transfer cancellation yet — large push/pull can only be interrupted with `Ctrl+C` (force quit)
- Overwrite is not atomic (see [Safety notes](#safety-notes))
- One device at a time, one storage at a time
- No recursive directory push/pull
- Behavior depends on device MTP capabilities — use [Diagnostics](#diagnostics) to inspect

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
