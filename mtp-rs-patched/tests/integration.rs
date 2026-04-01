//! Integration tests for mtp-rs.
//!
//! Requires a real MTP device (e.g., Android phone) connected via USB.
//! MTP only supports one operation at a time, so use `--test-threads=1`.
//!
//! ```sh
//! # Read-only tests (safe):
//! cargo test --test integration readonly -- --ignored --nocapture --test-threads=1
//!
//! # Destructive tests (writes to device):
//! cargo test --test integration destructive -- --ignored --nocapture --test-threads=1
//!
//! # All tests (skip slow ones):
//! cargo test --test integration -- --ignored --nocapture --test-threads=1 --skip slow
//! ```

use mtp_rs::mtp::Storage;
use mtp_rs::ptp::ObjectHandle;
use serial_test::serial;
use std::time::Instant;

/// Global test start time - initialized lazily on first use
static TEST_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Get the elapsed time since tests started, formatted as [HH:MM:SS.mmm]
fn elapsed_timestamp() -> String {
    let start = TEST_START.get_or_init(Instant::now);
    let elapsed = start.elapsed();
    let total_secs = elapsed.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    let millis = elapsed.subsec_millis();
    format!("[{:02}:{:02}:{:02}.{:03}]", hours, minutes, seconds, millis)
}

/// Timestamped logging macro
macro_rules! tlog {
    ($($arg:tt)*) => {{
        // Initialize start time on first log
        let _ = TEST_START.get_or_init(Instant::now);
        println!("{} {}", $crate::elapsed_timestamp(), format_args!($($arg)*));
    }};
}

/// Handle device errors gracefully - skip test on hardware issues, panic on others.
macro_rules! try_device {
    ($expr:expr, $context:expr) => {
        match $expr {
            Ok(v) => v,
            Err(e) => {
                if is_hardware_error(&e) {
                    tlog!("SKIPPING: {} - {:?}", $context, e);
                    print_device_help(&e);
                    return;
                } else {
                    panic!("{} failed: {:?}", $context, e);
                }
            }
        }
    };
}

fn is_hardware_error(e: &mtp_rs::Error) -> bool {
    use mtp_rs::Error;
    matches!(e, Error::Timeout | Error::NoDevice | Error::Disconnected) || e.is_exclusive_access()
}

fn print_device_help(e: &mtp_rs::Error) {
    use mtp_rs::Error;
    match e {
        Error::Timeout => {
            tlog!("  Check: phone unlocked? USB authorized? Cable connected?");
        }
        Error::NoDevice => {
            tlog!("  Check: phone connected? Set to MTP/File Transfer mode?");
        }
        Error::Disconnected => {
            tlog!("  Check: cable secure? Phone didn't sleep?");
        }
        _ if e.is_exclusive_access() => {
            tlog!("  Close other apps (file managers, Photos, Android File Transfer)");
        }
        _ => {}
    }
}

/// Search common Android folders for a file in the given size range.
/// Returns (handle, size, filename) if found.
async fn find_file_in_common_folders(
    storage: &Storage,
    min_size: u64,
    max_size: u64,
) -> Option<(ObjectHandle, u64, String)> {
    let root_objects = storage.list_objects(None).await.ok()?;

    let common_folders = [
        "Download",
        "Downloads",
        "DCIM",
        "Pictures",
        "Music",
        "Documents",
    ];

    for folder_name in &common_folders {
        let Some(folder) = root_objects
            .iter()
            .find(|o| o.is_folder() && o.filename == *folder_name)
        else {
            continue;
        };

        let objects = storage
            .list_objects(Some(folder.handle))
            .await
            .unwrap_or_default();

        // For DCIM, also check Camera subfolder
        let objects_to_check = if *folder_name == "DCIM" {
            if let Some(camera) = objects
                .iter()
                .find(|o| o.is_folder() && o.filename == "Camera")
            {
                storage
                    .list_objects(Some(camera.handle))
                    .await
                    .unwrap_or_default()
            } else {
                objects
            }
        } else {
            objects
        };

        if let Some(f) = objects_to_check
            .iter()
            .find(|o| o.is_file() && o.size > min_size && o.size < max_size)
        {
            return Some((f.handle, f.size, f.filename.clone()));
        }
    }
    None
}

/// Find a suitable file, falling back to recursive listing if needed.
async fn find_suitable_file(
    storage: &Storage,
    min_size: u64,
    max_size: u64,
) -> Option<(ObjectHandle, u64, String)> {
    // Try common folders first (fast)
    if let Some(result) = find_file_in_common_folders(storage, min_size, max_size).await {
        return Some(result);
    }

    // Fall back to recursive listing (slow)
    tlog!("No file in common folders, trying recursive listing...");
    let objects = storage.list_objects_recursive(None).await.ok()?;
    objects
        .iter()
        .find(|o| o.is_file() && o.size > min_size && o.size < max_size)
        .map(|f| (f.handle, f.size, f.filename.clone()))
}

/// Read-only tests that don't modify the device.
mod readonly {
    use super::*;
    use mtp_rs::mtp::MtpDevice;
    use mtp_rs::ptp::PtpDevice;
    use std::time::Duration;

    #[test]
    #[serial]
    fn test_list_devices() {
        let devices = MtpDevice::list_devices().expect("USB subsystem error");
        tlog!("Found {} MTP device(s)", devices.len());
        for dev in &devices {
            tlog!(
                "  {} {} ({:04x}:{:04x}) location={:08x}",
                dev.manufacturer.as_deref().unwrap_or("?"),
                dev.product.as_deref().unwrap_or("?"),
                dev.vendor_id,
                dev.product_id,
                dev.location_id
            );
        }
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_device_connection() {
        let device = try_device!(MtpDevice::open_first().await, "open device");
        let info = device.device_info();
        tlog!(
            "Connected: {} {} ({})",
            info.manufacturer,
            info.model,
            info.serial_number
        );
        assert!(!info.manufacturer.is_empty());
        assert!(!info.model.is_empty());
        device.close().await.expect("close failed");
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_list_storages() {
        let device = try_device!(MtpDevice::open_first().await, "open device");
        let storages = try_device!(device.storages().await, "get storages");
        tlog!("Found {} storage(s)", storages.len());
        assert!(!storages.is_empty());

        for storage in &storages {
            let info = storage.info();
            tlog!(
                "  {} - {:.2} GB free / {:.2} GB total",
                info.description,
                info.free_space_bytes as f64 / 1e9,
                info.max_capacity as f64 / 1e9
            );
        }
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_list_root_folder() {
        let device = try_device!(MtpDevice::open_first().await, "open device");
        let storages = try_device!(device.storages().await, "get storages");
        let storage = &storages[0];

        let objects = try_device!(storage.list_objects(None).await, "list root");
        tlog!("Root contains {} objects", objects.len());

        for obj in objects.iter().take(20) {
            let kind = if obj.is_folder() { "DIR " } else { "FILE" };
            let size = if obj.is_folder() {
                "-".to_string()
            } else {
                format!("{}", obj.size)
            };
            tlog!("  {} {:>12} {}", kind, size, obj.filename);
        }
        if objects.len() > 20 {
            tlog!("  ... and {} more", objects.len() - 20);
        }

        assert!(objects.iter().any(|o| o.is_folder()));
    }

    /// SLOW: Lists ALL objects recursively. Set MTP_RUN_SLOW_TESTS=1 to run.
    #[tokio::test]
    #[ignore]
    #[serial]
    async fn slow_test_list_recursive() {
        if std::env::var("MTP_RUN_SLOW_TESTS").is_err() {
            tlog!("SKIPPING slow_test_list_recursive (set MTP_RUN_SLOW_TESTS=1 to run)");
            return;
        }

        let device = try_device!(MtpDevice::open_first().await, "open device");
        let storages = try_device!(device.storages().await, "get storages");
        let storage = &storages[0];

        tlog!("Starting recursive listing (may take several minutes)...");
        let objects = try_device!(storage.list_objects_recursive(None).await, "recursive list");

        let folders = objects.iter().filter(|o| o.is_folder()).count();
        let files = objects.iter().filter(|o| o.is_file()).count();
        tlog!(
            "Total: {} objects ({} folders, {} files)",
            objects.len(),
            folders,
            files
        );
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_download_with_progress() {
        let device = try_device!(MtpDevice::open_first().await, "open device");
        let storages = try_device!(device.storages().await, "get storages");
        let storage = &storages[0];

        tlog!("Searching for file (100KB-10MB)...");
        let Some((handle, file_size, file_name)) =
            find_suitable_file(storage, 100_000, 10_000_000).await
        else {
            tlog!("No suitable file found, skipping");
            return;
        };
        tlog!("Downloading {} ({} bytes)", file_name, file_size);

        let mut download = try_device!(storage.download_stream(handle).await, "start download");
        let total = download.size();
        let mut last_percent = 0u64;

        while let Some(result) = download.next_chunk().await {
            result.expect("download error");
            let percent = download.bytes_received() * 100 / total;
            if percent >= last_percent + 10 {
                tlog!("  {}%", percent);
                last_percent = percent;
            }
        }
        tlog!("Download complete");
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_custom_timeout() {
        let device = try_device!(
            MtpDevice::builder()
                .timeout(Duration::from_secs(60))
                .open_first()
                .await,
            "open with timeout"
        );
        tlog!("Opened with 60s timeout: {}", device.device_info().model);
        device.close().await.expect("close failed");
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_ptp_device() {
        let device = try_device!(PtpDevice::open_first().await, "open PTP device");
        let info = try_device!(device.get_device_info().await, "get device info");
        tlog!("PTP Device: {} {}", info.manufacturer, info.model);

        let session = try_device!(device.open_session().await, "open session");
        let storage_ids = try_device!(session.get_storage_ids().await, "get storage IDs");
        tlog!("Storage IDs: {:?}", storage_ids);
        session.close().await.expect("close failed");
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_refresh_storage() {
        let device = try_device!(MtpDevice::open_first().await, "open device");
        let mut storages = try_device!(device.storages().await, "get storages");
        let storage = &mut storages[0];

        let before = storage.info().free_space_bytes;
        try_device!(storage.refresh().await, "refresh storage");
        let after = storage.info().free_space_bytes;
        tlog!("Free space: {} -> {} bytes", before, after);
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_streaming_download() {
        let device = try_device!(MtpDevice::open_first().await, "open device");
        let storages = try_device!(device.storages().await, "get storages");
        let storage = &storages[0];

        tlog!("Searching for file (100KB-5MB)...");
        let Some((handle, file_size, file_name)) =
            find_suitable_file(storage, 100_000, 5_000_000).await
        else {
            tlog!("No suitable file found, skipping");
            return;
        };
        tlog!("Streaming {} ({} bytes)", file_name, file_size);

        let mut download = try_device!(storage.download_stream(handle).await, "start download");
        assert_eq!(download.size(), file_size);

        let mut total_received = 0u64;
        let mut chunk_count = 0u64;

        while let Some(result) = download.next_chunk().await {
            let chunk = result.expect("download error");
            total_received += chunk.len() as u64;
            chunk_count += 1;
        }

        tlog!(
            "Received {} bytes in {} chunks",
            total_received,
            chunk_count
        );
        assert_eq!(total_received, file_size);
    }
}

// Camera control tests disabled - need PtpSession device property methods.

/// Destructive tests - these write to the device.
mod destructive {
    use super::*;
    use bytes::Bytes;
    use mtp_rs::mtp::{MtpDevice, NewObjectInfo};
    use mtp_rs::Error;

    /// Helper to get device, storage, and Download folder handle
    async fn setup_with_download_folder() -> Option<(MtpDevice, mtp_rs::mtp::Storage, ObjectHandle)>
    {
        let device = MtpDevice::open_first().await.ok()?;
        let storages = device.storages().await.ok()?;
        let storage = storages.into_iter().next()?;
        let root = storage.list_objects(None).await.ok()?;
        let download = root.iter().find(|o| o.filename == "Download")?;
        Some((device, storage, download.handle))
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_upload_download_delete() {
        let Some((_device, storage, download_handle)) = setup_with_download_folder().await else {
            tlog!("Setup failed (no device or Download folder)");
            return;
        };

        let content = format!("Test file at {:?}", std::time::SystemTime::now());
        let content_bytes = content.as_bytes();

        tlog!("Uploading {} bytes...", content_bytes.len());
        let info = NewObjectInfo::file("mtp-rs-test.txt", content_bytes.len() as u64);
        let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(
            content_bytes.to_vec(),
        ))]);
        let handle = storage
            .upload(Some(download_handle), info, Box::pin(stream))
            .await
            .expect("upload failed");

        // Verify
        let obj_info = storage
            .get_object_info(handle)
            .await
            .expect("get info failed");
        assert_eq!(obj_info.filename, "mtp-rs-test.txt");
        assert_eq!(obj_info.size, content_bytes.len() as u64);

        // Download and verify content
        let downloaded = storage.download(handle).await.expect("download failed");
        assert_eq!(downloaded, content_bytes);
        tlog!("Content verified");

        // Delete and verify
        storage.delete(handle).await.expect("delete failed");
        let result = storage.get_object_info(handle).await;
        assert!(matches!(
            result,
            Err(Error::Protocol {
                code: mtp_rs::ptp::ResponseCode::InvalidObjectHandle,
                ..
            })
        ));
        tlog!("Upload/download/delete PASSED");
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_create_delete_folder() {
        let Some((_device, storage, download_handle)) = setup_with_download_folder().await else {
            tlog!("Setup failed");
            return;
        };

        let folder_name = format!("mtp-rs-test-{}", std::process::id());
        tlog!("Creating folder: {}", folder_name);

        let handle = storage
            .create_folder(Some(download_handle), &folder_name)
            .await
            .expect("create failed");

        let info = storage
            .get_object_info(handle)
            .await
            .expect("get info failed");
        assert!(info.is_folder());
        assert_eq!(info.filename, folder_name);

        storage.delete(handle).await.expect("delete failed");
        tlog!("Create/delete folder PASSED");
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_rename_file() {
        let device = try_device!(MtpDevice::open_first().await, "open device");

        if !device.supports_rename() {
            tlog!("Device doesn't support rename, skipping");
            return;
        }

        let storages = try_device!(device.storages().await, "get storages");
        let storage = &storages[0];
        let root = try_device!(storage.list_objects(None).await, "list root");
        let download = root
            .iter()
            .find(|o| o.filename == "Download")
            .expect("no Download folder");

        let original = format!("mtp-rs-rename-{}.txt", std::process::id());
        let renamed = format!("mtp-rs-renamed-{}.txt", std::process::id());
        let content = b"rename test";

        let info = NewObjectInfo::file(&original, content.len() as u64);
        let stream =
            futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(content.to_vec()))]);
        let handle = storage
            .upload(Some(download.handle), info, Box::pin(stream))
            .await
            .expect("upload failed");

        tlog!("Renaming {} -> {}", original, renamed);
        match storage.rename(handle, &renamed).await {
            Ok(()) => {
                let info = storage
                    .get_object_info(handle)
                    .await
                    .expect("get info failed");
                assert_eq!(info.filename, renamed);
                tlog!("Rename verified");
            }
            Err(Error::Protocol {
                code: mtp_rs::ptp::ResponseCode::OperationNotSupported,
                ..
            }) => {
                tlog!("Rename not actually supported (device lied)");
            }
            Err(e) => {
                storage.delete(handle).await.ok();
                panic!("Rename failed: {:?}", e);
            }
        }

        storage.delete(handle).await.expect("cleanup failed");
        tlog!("Rename test PASSED");
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_streaming_upload() {
        let Some((_device, storage, download_handle)) = setup_with_download_folder().await else {
            tlog!("Setup failed");
            return;
        };

        let chunk_size = 64 * 1024;
        let num_chunks = 10;
        let total_size = chunk_size * num_chunks;

        tlog!("Uploading {} bytes in {} chunks", total_size, num_chunks);

        let chunks: Vec<Result<Bytes, std::io::Error>> = (0..num_chunks)
            .map(|i| Ok(Bytes::from(vec![i as u8; chunk_size])))
            .collect();

        let filename = format!("mtp-rs-stream-{}.bin", std::process::id());
        let info = NewObjectInfo::file(&filename, total_size as u64);
        let handle = storage
            .upload(Some(download_handle), info, futures::stream::iter(chunks))
            .await
            .expect("upload failed");

        // Verify
        let obj_info = storage
            .get_object_info(handle)
            .await
            .expect("get info failed");
        assert_eq!(obj_info.size, total_size as u64);

        let downloaded = storage.download(handle).await.expect("download failed");
        for i in 0..num_chunks {
            let start = i * chunk_size;
            assert!(downloaded[start..start + chunk_size]
                .iter()
                .all(|&b| b == i as u8));
        }

        storage.delete(handle).await.expect("cleanup failed");
        tlog!("Streaming upload PASSED");
    }

    #[tokio::test]
    #[ignore]
    #[serial]
    async fn test_streaming_copy() {
        let Some((_device, storage, download_handle)) = setup_with_download_folder().await else {
            tlog!("Setup failed");
            return;
        };

        // Find a file to copy
        let objects = storage
            .list_objects(Some(download_handle))
            .await
            .unwrap_or_default();
        let Some(source) = objects
            .iter()
            .find(|o| o.is_file() && o.size > 50_000 && o.size < 500_000)
        else {
            tlog!("No suitable source file (50KB-500KB), skipping");
            return;
        };

        let source_handle = source.handle;
        let source_size = source.size;
        tlog!("Copying {} ({} bytes)", source.filename, source_size);

        // Download
        let download = storage
            .download_stream(source_handle)
            .await
            .expect("download failed");
        let data = download.collect().await.expect("collect failed");

        // Upload copy
        let dest_name = format!("mtp-rs-copy-{}.bin", std::process::id());
        let info = NewObjectInfo::file(&dest_name, source_size);
        let stream =
            futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(data.clone()))]);
        let dest_handle = storage
            .upload(Some(download_handle), info, stream)
            .await
            .expect("upload failed");

        // Verify
        let copy_data = storage
            .download(dest_handle)
            .await
            .expect("download copy failed");
        assert_eq!(copy_data, data);

        storage.delete(dest_handle).await.expect("cleanup failed");
        tlog!("Streaming copy PASSED");
    }
}
