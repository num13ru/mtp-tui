# Roadmap

Planned improvements for mtp-tui, organized by milestone.

Items marked with **(done)** are implemented and shipped.

---

## v0.2.0 — Reliability

The next release focuses on predictable behavior under real-world MTP
conditions: mid-transfer failures, device disconnects, stale state.

### Transfer cancellation (P0)

Allow the user to cancel an in-progress push or pull (e.g. `Esc` during
transfer). The backend should use the USB SIC cancel mechanism
(`NusbTransport::cancel_transfer()` in mtp-rs) to cleanly abort the transfer
instead of forcing `Ctrl+C`.

Without this, a large transfer is effectively uninterruptible.

### Disconnect recovery (P0)

Detect when the device disappears during a transfer or listing and transition
to a clear error state: "Device disconnected — press `r` to reconnect."

This is a stepping stone toward full hot-plug detection. It does not require
`nusb` hotplug events — detecting a transport error and cleaning up is enough
for now.

### Safer overwrite semantics (P1)

Currently overwrite is delete-then-upload (MTP has no in-place overwrite).
If the upload fails after the delete, the original file is lost.

Safer strategy where possible:
1. Upload under a temporary name.
2. Delete the original.
3. Rename the new file to the original name.
4. If rename is not supported by the device, warn the user explicitly before
   overwriting that the operation is not atomic.

Not all devices support this workflow, but the confirmation dialog should
state the risk clearly regardless.

### Directory cache with generation invalidation (P1)

Cache device directory listings. Invalidate after any mutation (push, delete,
rename, mkdir) and after reconnect.

Cache key should include enough context to avoid stale hits:

```
device_identity + storage_id + parent_handle + generation
```

Where `generation` increments on reconnect or manual full refresh (`r` key).

Risks to handle:
- Device contents changed outside mtp-tui (external app, camera, etc.)
- Object handles may be reused after reconnect
- Different storages may have overlapping handle values
- Failed listings must not be cached — a transient USB error should not
  stick as an empty directory until manual refresh

Impact: eliminates redundant USB round-trips on back-navigation.

### GitHub Release binary (P1)

Add a GitHub Actions workflow that builds a universal macOS binary on tagged
releases and attaches it to a GitHub Release. Users should not need
`cargo run` to try the tool.

---

## v0.3.0 — Navigation & Polish

### Prefetch highlighted directory

When the cursor sits on a directory for a short delay, start fetching its
contents in the background. By the time the user presses Enter the listing
may already be ready.

### Filter / search (P2)

Incremental search within the current directory (`/` to start typing). Filter
the visible list as the user types.

### Sort modes (P2)

Allow sorting by name, size, or date (toggle with `s`). Persist the choice
per pane.

### Multi-storage support (P2)

Some devices expose multiple storages (internal + SD card). Show a storage
picker or multiple tabs.

### File size column alignment

Right-align the size column so sizes are easy to scan visually.

### Config warning on fallback

Currently config errors fall back silently to defaults. Show a one-line
status bar warning when a config value is ignored (e.g.
`default_device_dir = "/Documents"` not found on device).

### App decomposition

`App` (app.rs, ~900 lines) currently owns UI state, input handling, file
operations, device lifecycle, transfer lifecycle, dialogs, and navigation.
This is manageable today but will become a bottleneck as features grow.

Candidate modules to extract:
- `actions.rs` — user commands (push, pull, delete, rename, mkdir)
- `device_tasks.rs` — background listing and transfer jobs
- `host_fs.rs` — local filesystem operations
- `dialogs.rs` — confirm / text input / transfer / inspector dialogs
- `state_machine.rs` — `DeviceState` transitions

Should be done before bulk operations or operation queuing.

---

## v0.4.0 — Power Features

### Bulk operations (P3)

Select multiple files (toggle with `Space`, select range with `Shift`), then
push/pull/delete in batch with a progress indicator.

Prerequisite: transfer cancellation and disconnect recovery must be solid
before batch operations are safe to ship.

### Configurable keybindings (P3)

Read keybindings from `~/.config/mtp-tui/config.toml` so users can remap
keys.

---

## Later / Exploratory

### Hot-plug detection

Watch for USB connect/disconnect events (`nusb` device hotplug API).
Auto-reconnect when a device appears, show a message when it disconnects.

### Multi-device support

List all connected MTP devices and let the user choose which one to browse.

### Batch property fetch via GetObjectPropList (0x9805)

The MTP spec defines `GetObjectPropList` to return properties for multiple
objects in a single USB transaction. This would collapse the current N+1
round-trips per directory (1 `GetObjectHandles` + N `GetObjectInfo`) down
to ~2 calls.

This requires contributing the operation to mtp-rs or sending raw PTP
commands through the session layer.

Impact: can significantly reduce USB round-trips for large directories.
Actual speedup depends on device firmware, object count, and USB latency.

---

## Shipped

### Streaming directory listing with progress **(done)**

Uses `Storage::list_objects_stream()` instead of `list_objects()`. The stream
yields items one at a time after a single `GetObjectHandles` call, so the UI
shows "Loading (42/500)..." while the remaining `GetObjectInfo` calls complete
in a background thread.

### Async directory loading **(done)**

Device directory listing runs on a background thread via `std::thread::spawn`.
The backend is moved into the thread for the duration of the listing and
returned via `mpsc` channel. The main thread stays responsive — a braille
spinner animates in the pane title and navigation keys are blocked until the
listing finishes.

### Push file (host to device) **(done)**

Uses `Storage::upload_with_progress()` to stream a host file to the device in
256 KB chunks. If a file with the same name already exists in the current
device directory, a modal confirmation dialog asks whether to overwrite
(delete-then-upload, since MTP has no in-place overwrite). The device listing
refreshes automatically after a successful push.

### Pull file (device to host) **(done)**

Uses `Storage::download_stream()` to stream device files to disk in chunks
via `FileDownload::next_chunk()`, avoiding full in-memory buffering. If a file
with the same name already exists on the host, a modal confirmation dialog
asks whether to overwrite. The host listing refreshes automatically after a
successful pull.

### Create directory **(done)**

Uses `Storage::create_folder()` via the `m` key. A modal text input dialog
prompts for the directory name. The device listing refreshes automatically
after creation.

### Delete file/directory **(done)**

Uses `Storage::delete()` via the `d` key. A confirmation dialog (Y/N) is
shown before executing. The device listing refreshes automatically after
deletion, preserving the current selection position.

### Rename file/directory **(done)**

Uses `Storage::rename()` (SetObjectPropValue 0x9804) via the `R` key. A
modal text input dialog is pre-filled with the current name. Checks
`MtpDevice::supports_rename()` and bails with a clear error if the device
doesn't support it.

### Confirmation dialogs **(done)**

Reusable modal dialog (`ConfirmDialog` / `ConfirmAction`) for destructive
operations. Y/Enter to confirm, N/Esc to cancel. Used for overwrite-on-push,
overwrite-on-pull, delete, and quit. A separate `TextInputDialog` provides
free-text input for mkdir and rename.

### Configurable default directories **(done)**

TOML config file at `~/.config/mtp-tui/config.toml`, auto-created on first
run with a commented-out template. Supports `default_host_dir` (with `~`
expansion) and `default_device_dir`. `$XDG_CONFIG_HOME` is respected.
