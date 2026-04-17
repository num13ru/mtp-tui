# Roadmap

Planned improvements for mac-mtp-tui, roughly ordered by priority.

## Performance

### Streaming directory listing with progress

Use `Storage::list_objects_stream()` instead of `list_objects()`. The stream
yields items one at a time after a single `GetObjectHandles` call, so the UI
can show "Loading (42/500)..." while the remaining `GetObjectInfo` calls
complete in the background.

Impact: UI stays responsive during large listings. No change in total wall-clock time.

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

### Push file (host to device)

Implement `push_file` using `Storage::upload()` /
`Storage::upload_with_progress()`. Show a progress bar in the status line.

### Pull file (device to host)

Implement `pull_file` using `Storage::download_stream()` for large-file
support. Stream to disk instead of buffering in memory.

### Create directory

Implement `mkdir` using `Storage::create_folder()`. Prompt for directory name
via an inline text input widget.

### Delete file/directory

Implement `delete` using `Storage::delete()`. Require confirmation before
executing.

### Rename file/directory

Implement `rename` using `Storage::rename()`. The device supports
`SetObjectPropValue` (0x9804). Use an inline text input widget pre-filled with
the current name.

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

### Confirmation dialogs

Modal confirmation for destructive operations (delete, overwrite). Escape to
cancel, Enter to confirm.

### Configurable keybindings

Read keybindings from a config file (`~/.config/mac-mtp-tui/config.toml`) so
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

Publish a Homebrew tap so users can install with `brew install mac-mtp-tui`.
