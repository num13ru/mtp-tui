# Roadmap

Planned improvements for mtp-tui, roughly ordered by priority.

Items marked with **(done)** are implemented and shipped.

## Performance

### Streaming directory listing with progress **(done)**

Uses `Storage::list_objects_stream()` instead of `list_objects()`. The stream
yields items one at a time after a single `GetObjectHandles` call, so the UI
shows "Loading (42/500)..." while the remaining `GetObjectInfo` calls complete
in a background thread.

### Async directory loading **(done)**

Device directory listing runs on a background thread via `std::thread::spawn`.
The backend is moved into the thread for the duration of the listing and
returned via `mpsc` channel. The main thread stays responsive -- a braille
spinner animates in the pane title and navigation keys are blocked until the
listing finishes.

### Directory cache

Cache device directory listings keyed by `ObjectHandle`. Navigating back to a
parent folder becomes instant. Invalidate the cache after any mutation (push,
delete, rename, mkdir).

Impact: eliminates redundant USB round-trips on back-navigation and refresh.

### Prefetch highlighted directory

When the selection cursor sits on a directory for a short delay, start fetching
its contents in the background. By the time the user presses Enter the listing
may already be ready.

Impact: eliminates perceived wait on directory enter for common navigation
patterns.

### Batch property fetch via GetObjectPropList (0x9805)

The MTP spec defines `GetObjectPropList` to return properties for multiple
objects in a single USB transaction. The Kindle (and most Android devices)
supports it. This would collapse the current N+1 round-trips per directory
(1 `GetObjectHandles` + N `GetObjectInfo`) down to ~2 calls.

This requires either contributing the operation to `mtp-rs` upstream or sending
raw PTP commands through the session layer.

Impact: 100-1000x actual speedup for large directories (e.g., 2000+ items in
root).

## File operations

### Push file (host to device) **(done)**

Uses `Storage::upload_with_progress()` to stream a host file to the device in
256 KB chunks. If a file with the same name already exists in the current device
directory, a modal confirmation dialog asks whether to overwrite (delete-then-
upload, since MTP has no in-place overwrite). The device listing refreshes
automatically after a successful push.

### Pull file (device to host) **(done)**

Uses `Storage::download_stream()` to stream device files to disk in chunks
via `FileDownload::next_chunk()`, avoiding full in-memory buffering. If a file
with the same name already exists on the host, a modal confirmation dialog asks
whether to overwrite. The host listing refreshes automatically after a
successful pull.

### Create directory **(done)**

Uses `Storage::create_folder()` via the `m` key. A modal text input dialog
prompts for the directory name. The device listing refreshes automatically
after creation.

### Delete file/directory **(done)**

Uses `Storage::delete()` via the `d` key. A confirmation dialog (Y/N) is shown
before executing. The device listing refreshes automatically after deletion,
preserving the current selection position.

### Rename file/directory **(done)**

Uses `Storage::rename()` (SetObjectPropValue 0x9804) via the `R` key. A modal
text input dialog is pre-filled with the current name. Checks
`MtpDevice::supports_rename()` and bails with a clear error if the device
doesn't support it.

### Bulk operations

Select multiple files (toggle with Space, select range with Shift), then
push/pull/delete in batch with a progress indicator.

## UX

### File size column alignment

Right-align the size column so sizes are easy to scan visually.

### Sort modes

Allow sorting by name, size, or date (toggle with `s`). Persist the choice per
pane.

### Filter / search

Incremental search within the current directory (`/` to start typing). Filter
the visible list as the user types.

### Confirmation dialogs **(done)**

Reusable modal dialog (`ConfirmDialog` / `ConfirmAction`) for destructive
operations. Y/Enter to confirm, N/Esc to cancel. Used for overwrite-on-push,
overwrite-on-pull, delete, and quit. A separate `TextInputDialog` provides
free-text input for mkdir and rename.

### Configurable keybindings

Read keybindings from a config file (`~/.config/mtp-tui/config.toml`) so
users can remap keys.

## Device handling

### Multi-storage support

Some devices expose multiple storages (internal + SD card). Show a storage
picker or multiple tabs.

### Hot-plug detection

Watch for USB connect/disconnect events (`nusb` device hotplug API). Auto-
reconnect when a device appears, show a message when it disconnects.

### Multi-device support

List all connected MTP devices and let the user choose which one to browse.

## Build and distribution

### Release binary

Add a GitHub Actions workflow that builds a universal macOS binary on tagged
releases and attaches it to a GitHub Release.

### Homebrew formula

Publish a Homebrew tap so users can install with `brew install mtp-tui`.
