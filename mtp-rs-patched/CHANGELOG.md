# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.1] - 2026-03-24

### Fixed

- Detect vendor-specific MTP devices (e.g. Amazon Kindle) that use USB class 0xFF with non-standard subclass/protocol ([#1](https://github.com/vdavid/mtp-rs/issues/1))

## [0.4.0] - 2026-03-20

### Changed

- Replaced platform-specific IOKit/location_id code with nusb's cross-platform `port_chain()` + `bus_id()`
- **Breaking:** `location_id` values will differ from previous versions (now derived from USB topology instead of macOS IOKit)
- Fixed timeout race condition: `receive_bulk` now leaves USB transfers pending on timeout instead of cancelling them, preventing data loss on retry
- `receive_interrupt()` now awaits indefinitely for events (no timeout); callers should use async cancellation
- Switched from `std::sync::Mutex` to `futures::lock::Mutex` for async-safe locking across `.await` points
- Re-added `futures-timer` dependency for async timeout support

### Removed

- Removed `io-kit-sys` and `core-foundation` macOS dependencies (location info now provided by nusb)
- **Breaking:** Removed `event_timeout`, `DEFAULT_EVENT_TIMEOUT`, `set_event_timeout()`, `event_timeout()`, and `open_with_timeouts()` from `NusbTransport`
- **Breaking:** Removed `event_timeout()` from `MtpDeviceBuilder`

## [0.3.0] - 2026-03-20

### Removed

- Removed `futures-timer` dependency (timeouts now handled by nusb internally)

### Changed

- **Breaking:** Upgraded `nusb` dependency from 0.1 to 0.2
- **Breaking:** MSRV raised from 1.75 to 1.79
- **Breaking:** `UsbDeviceInfo::open()` now returns `Result<nusb::Device, nusb::Error>` instead of `Result<nusb::Device, std::io::Error>`
- **Breaking:** Removed `NusbTransport::bulk_in_endpoint()`, `bulk_out_endpoint()`, `interrupt_in_endpoint()` accessors
- Improved MTP device detection: can now detect composite MTP devices without opening them (nusb 0.2 exposes interface info on `DeviceInfo`)
- Transport internals now use nusb 0.2's `Endpoint` pattern with `transfer_blocking` instead of single-shot methods

## [0.2.0] - 2026-03-17

### Added

- `Storage::list_objects_stream()` — streaming object listing that yields `ObjectInfo` items one at a time from USB, with `total()` and `fetched()` for progress reporting
- `ObjectListing` struct for iterating over streamed results
- Reproducible benchmark suite (`mtp-bench` crate at `benchmarks/mtp-rs-vs-libmtp/`) comparing mtp-rs against libmtp
- Benchmark results in README: mtp-rs is 1.06x–4.04x faster across all operations
- Release process documentation (`docs/releasing.md`)

### Changed

- `list_objects()` refactored to use `list_objects_stream()` internally — no behavior change

## [0.1.0] - 2026-02-20

Initial release targeting modern Android devices.

### Added

- Connect to Android phones/tablets over USB
- List, download, upload, delete, move, and copy files
- Create and delete folders
- Stream large file downloads with progress tracking
- Listen for device events (file added, storage removed, etc.)
- Two-layer API: high-level `mtp::` and low-level `ptp::`
- Runtime-agnostic async design (works with tokio, async-std, etc.)
- Pure Rust implementation using `nusb` for USB access
- Smart recursive listing that auto-detects Android and uses manual traversal
- `Storage::list_objects_recursive_manual()` for explicit manual traversal
- `Storage::list_objects_recursive_native()` for explicit native MTP recursive listing
- Android device detection via `"android.com"` vendor extension
- Integration tests organized into `readonly` and `destructive` categories
- Serial test execution to avoid USB device conflicts
- Diagnostic example (`examples/diagnose.rs`)

### Fixed

- MTP device detection for composite USB devices (class 0)
  - Most Android phones are composite devices with MTP as one interface
  - Now properly inspects interface descriptors to find MTP
- Large MTP data containers (>64KB) now handled correctly
  - Data spanning multiple USB transfers is reassembled before parsing
- Recursive listing now works on Android devices
  - Android ignores `ObjectHandle::ALL`; we detect this and use manual traversal
- Integration tests now use `Download/` folder instead of root
  - Android doesn't allow creating files/folders in storage root

### Changed

- `list_objects_recursive()` now automatically chooses the best strategy:
  - Android devices: manual folder-by-folder traversal
  - Other devices: native recursive, with fallback to manual if results look incomplete

### Not included (by design)

- MTPZ (DRM extension for old devices)
- Playlist and metadata syncing
- Vendor-specific extensions
- Legacy device quirks database
