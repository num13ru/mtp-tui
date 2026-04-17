# mac-mtp-tui
![Rust](https://img.shields.io/badge/Rust-000000?style=flat&logo=rust&logoColor=white)
[![Rust](https://github.com/num13ru/mac-mtp-tui/actions/workflows/rust.yml/badge.svg)](https://github.com/num13ru/mac-mtp-tui/actions/workflows/rust.yml)

A terminal file manager for MTP devices (Android phones, Kindle, etc.) on macOS.

Two-pane layout: local filesystem on the left, device storage on the right. Browse both, push and pull files.

Built with [ratatui](https://ratatui.rs) and [mtp-rs](https://github.com/vdavid/mtp-rs). Pure Rust, no libmtp/libusb.

## Usage

Connect an MTP device via USB, then:

```
cargo run
```

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
| `r` | Refresh both panes |
| `?` | Toggle help overlay |
| `q` | Quit |

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

Not yet implemented: file push/pull, mkdir, delete, rename.

See [ROADMAP.md](ROADMAP.md) for planned improvements.

## License

MIT
