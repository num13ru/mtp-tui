# mtp-tui
![Rust](https://img.shields.io/badge/Rust-000000?style=flat&logo=rust&logoColor=white)
[![CI](https://github.com/num13ru/mtp-tui/actions/workflows/CI.yml/badge.svg)](https://github.com/num13ru/mtp-tui/actions/workflows/CI.yml)

A terminal file manager for MTP devices (Android phones, Kindle, etc.).

Two-pane layout: local filesystem on the left, device storage on the right. Browse, push, pull, delete, rename, and create directories.

Built with [ratatui](https://ratatui.rs) and [mtp-rs](https://github.com/vdavid/mtp-rs). Pure Rust, no libmtp/libusb.

## Usage

Connect an MTP device via USB, then:

```
cargo run
```

### macOS

If macOS grabs the device first:

```
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
| `r` | Refresh both panes |
| `?` | Toggle help overlay |
| `Esc` | Close dialog / help |
| `q` | Quit (confirms) |
| `Ctrl+C` | Force quit |

## Diagnostics

Dump device capabilities, supported operations, storages, and root objects:

```
cargo run --example mtp_capabilities
```

## Status

Early stage. Working:

- Device detection and connection (including Kindle)
- Directory browsing on both panes
- File size display
- Async directory loading with spinner (UI never freezes)
- Streaming progress counter ("Loading 42/500...")

- Push file to device (`p`) with overwrite confirmation
- Pull file from device (`g`) with overwrite confirmation
- Delete file/directory on device (`d`) with confirmation
- Create directory on device (`m`)
- Rename file/directory on device (`R`)
- Quit confirmation dialog

See [ROADMAP.md](ROADMAP.md) for planned improvements.

## License

MIT
