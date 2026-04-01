//! Debug script to diagnose slow MTP operations on Google Pixel devices.
//!
//! This script times each MTP operation to identify bottlenecks.
//! Run with: cargo run --example debug_pixel_slow
//!
//! Expected output will show timing for:
//! - Device open/transport initialization
//! - Session open
//! - GetStorageIds
//! - GetStorageInfo
//! - GetObjectHandles (root) - likely the slow operation
//! - GetObjectInfo for first few handles

use mtp_rs::ptp::{ObjectHandle, PtpSession};
use mtp_rs::transport::NusbTransport;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Helper to time an operation and print the result
async fn time_op<T, F, Fut>(name: &str, f: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let start = Instant::now();
    print!("{:<40} ", format!("{}...", name));
    let result = f().await;
    let elapsed = start.elapsed();
    println!("{:>10.3}s", elapsed.as_secs_f64());
    result
}

/// Sync version for non-async operations
fn time_sync<T, F>(name: &str, f: F) -> T
where
    F: FnOnce() -> T,
{
    let start = Instant::now();
    print!("{:<40} ", format!("{}...", name));
    let result = f();
    let elapsed = start.elapsed();
    println!("{:>10.3}s", elapsed.as_secs_f64());
    result
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== MTP Performance Diagnostic for Google Pixel ===\n");
    println!("This script times each MTP operation to identify bottlenecks.\n");

    let total_start = Instant::now();

    // Step 1: List MTP devices (sync)
    let devices = time_sync("Listing MTP devices", NusbTransport::list_mtp_devices)?;

    if devices.is_empty() {
        println!("\nNo MTP devices found!");
        println!("Make sure your device is:");
        println!("  1. Connected via USB");
        println!("  2. Unlocked");
        println!("  3. Set to 'File Transfer' mode (not charging only)");
        return Ok(());
    }

    println!("\nFound {} device(s)", devices.len());
    for (i, d) in devices.iter().enumerate() {
        println!(
            "  {}. {} {} ({:04x}:{:04x}) serial={:?} location={:08x}",
            i + 1,
            d.manufacturer.as_deref().unwrap_or("Unknown"),
            d.product.as_deref().unwrap_or("Unknown"),
            d.vendor_id,
            d.product_id,
            d.serial_number,
            d.location_id
        );
    }
    println!();

    // Step 2: Open USB device
    let device_info = devices.into_iter().next().unwrap();
    let usb_device = time_sync("Opening USB device", || device_info.open())?;

    // Step 3: Open transport (claim interface, find endpoints)
    let transport = time_op("Opening MTP transport", || async {
        NusbTransport::open_with_timeout(usb_device, Duration::from_secs(120)).await
    })
    .await?;

    let transport: Arc<dyn mtp_rs::transport::Transport> = Arc::new(transport);

    // Step 4: Open PTP session
    let session = time_op("Opening PTP session", || async {
        PtpSession::open(transport.clone(), 1).await
    })
    .await?;

    // Step 5: Get device info
    let device_info = time_op("Getting device info", || async {
        session.get_device_info().await
    })
    .await?;

    println!("\n--- Device Information ---");
    println!("Manufacturer: {}", device_info.manufacturer);
    println!("Model: {}", device_info.model);
    println!("Serial: {}", device_info.serial_number);
    println!("Vendor Extension: {}", device_info.vendor_extension_desc);
    let is_android = device_info
        .vendor_extension_desc
        .to_lowercase()
        .contains("android.com");
    println!("Is Android: {}", is_android);
    println!();

    // Step 6: Get storage IDs
    let storage_ids = time_op("Getting storage IDs", || async {
        session.get_storage_ids().await
    })
    .await?;

    println!("\n--- Storages ({}) ---", storage_ids.len());
    for id in &storage_ids {
        println!("  Storage ID: 0x{:08X}", id.0);
    }
    println!();

    // Step 7: Get storage info for each storage
    for storage_id in &storage_ids {
        let info = time_op(
            &format!("Getting storage info (0x{:08X})", storage_id.0),
            || async { session.get_storage_info(*storage_id).await },
        )
        .await?;

        println!("  Description: {}", info.description);
        println!("  Volume ID: {}", info.volume_identifier);
        println!("  Capacity: {} MB", info.max_capacity / (1024 * 1024));
        println!("  Free: {} MB", info.free_space_bytes / (1024 * 1024));
        println!();
    }

    // Step 8: THE CRITICAL TEST - GetObjectHandles for root
    println!("=== Critical Test: GetObjectHandles ===\n");

    let storage_id = storage_ids[0];

    // Test 8a: Root level only (parent = None, which means handle 0)
    println!("--- Test A: Root level only (parent=0) ---");
    let root_handles = time_op("GetObjectHandles (root, parent=0)", || async {
        session.get_object_handles(storage_id, None, None).await
    })
    .await?;
    println!("  Returned {} handles\n", root_handles.len());

    // Test 8b: Root level with explicit handle 0
    println!("--- Test B: Root level (parent=0xFFFFFFFF for root children) ---");
    // In MTP, parent=0 means "root only", parent=0xFFFFFFFF means "all objects"
    let all_handles = time_op("GetObjectHandles (parent=0xFFFFFFFF)", || async {
        session
            .get_object_handles(storage_id, None, Some(ObjectHandle::ALL))
            .await
    })
    .await?;
    println!("  Returned {} handles\n", all_handles.len());

    // Step 9: Get object info for first few handles
    if !root_handles.is_empty() {
        println!("=== GetObjectInfo timing for first 10 root objects ===\n");

        let info_start = Instant::now();
        for handle in root_handles.iter().take(10) {
            let info = time_op(&format!("GetObjectInfo (handle {})", handle.0), || async {
                session.get_object_info(*handle).await
            })
            .await?;

            let kind = if info.is_folder() { "DIR" } else { "FILE" };
            println!("  {} {} ({} bytes)\n", kind, info.filename, info.size);
        }

        let info_total = info_start.elapsed();
        println!(
            "Total time for {} GetObjectInfo calls: {:.3}s",
            std::cmp::min(10, root_handles.len()),
            info_total.as_secs_f64()
        );
        println!(
            "Average per call: {:.3}s\n",
            info_total.as_secs_f64() / std::cmp::min(10, root_handles.len()) as f64
        );
    }

    // Step 10: Analyze the problem
    println!("=== Analysis of Results ===\n");

    println!("Key Finding:");
    println!(
        "  GetObjectHandles (root, parent=0) returned {} handles",
        root_handles.len()
    );
    println!(
        "  GetObjectHandles (parent=0xFFFFFFFF) returned {} handles",
        all_handles.len()
    );
    println!();

    if root_handles.len() > all_handles.len() * 10 {
        println!("PROBLEM IDENTIFIED:");
        println!("  When parent=0 (meaning 'root level only'), Android/Pixel returns");
        println!(
            "  ALL {} objects on the device, not just root-level objects!",
            root_handles.len()
        );
        println!();
        println!("  This is a known Android MTP bug: parent=0 is interpreted as 'no filter'");
        println!("  instead of 'root objects only'.");
        println!();

        println!("SOLUTION:");
        println!("  Use parent=0xFFFFFFFF (ObjectHandle::ALL) to get root-level objects.");
        println!(
            "  Counter-intuitively, 0xFFFFFFFF gives us the {} actual root items!",
            all_handles.len()
        );
        println!();

        // Calculate impact on list_objects
        let handles_to_process = root_handles.len();
        let avg_info_time = 0.001; // seconds per GetObjectInfo call
        let estimated_list_time = handles_to_process as f64 * avg_info_time;

        println!("Impact on Storage::list_objects(None):");
        println!("  Old behavior: calls GetObjectHandles(parent=0)");
        println!(
            "  Returns {} handles instead of {}",
            handles_to_process,
            all_handles.len()
        );
        println!("  Then calls GetObjectInfo for EACH handle");
        println!(
            "  Estimated time: {} handles x {:.3}s/call = {:.1}s",
            handles_to_process, avg_info_time, estimated_list_time
        );
        println!();

        println!("FIX APPLIED in src/mtp/storage.rs:");
        println!("  For Android devices, when listing root (parent=None):");
        println!("  1. Use parent=0xFFFFFFFF to get actual root objects");
        println!("  2. Filter results by parent_handle == 0 or 0xFFFFFFFF");
    } else {
        println!("Results look normal - parent=0 and parent=0xFFFFFFFF return similar counts.");
    }

    // Test the fix using low-level API (simulating what Storage::list_objects now does)
    println!("\n=== Testing Fixed Approach (Low-Level) ===\n");

    // Simulate what the fixed Storage::list_objects does for Android root listing
    println!("Using ObjectHandle::ALL for Android root listing (the fix)...");
    let fixed_start = Instant::now();

    // Step 1: Get handles using the fixed approach
    let handles = session
        .get_object_handles(storage_id, None, Some(ObjectHandle::ALL))
        .await?;
    let handles_time = fixed_start.elapsed();
    println!(
        "  GetObjectHandles time: {:.3}s ({} handles)",
        handles_time.as_secs_f64(),
        handles.len()
    );

    // Step 2: Get object info for each handle (filter will be applied)
    let mut root_objects = Vec::new();
    for handle in &handles {
        let info = session.get_object_info(*handle).await?;
        // Filter: root items have parent 0 or 0xFFFFFFFF
        if info.parent.0 == 0 || info.parent.0 == 0xFFFFFFFF {
            root_objects.push((handle, info));
        }
    }
    let total_time = fixed_start.elapsed();
    println!(
        "  Total time with fix: {:.3}s ({} root objects)",
        total_time.as_secs_f64(),
        root_objects.len()
    );

    println!("\nRoot objects (with fix):");
    for (i, (handle, obj)) in root_objects.iter().take(20).enumerate() {
        let kind = if obj.is_folder() { "DIR" } else { "FILE" };
        println!(
            "  {}. {} {} (handle={}, {} bytes)",
            i + 1,
            kind,
            obj.filename,
            handle.0,
            obj.size
        );
    }
    if root_objects.len() > 20 {
        println!("  ... and {} more", root_objects.len() - 20);
    }

    // Close session
    println!("\nClosing session...");
    session.close().await?;

    // Summary
    let total_elapsed = total_start.elapsed();
    println!("\n=== Summary ===");
    println!("Total diagnostic time: {:.3}s", total_elapsed.as_secs_f64());

    println!("\n=== Analysis ===");
    println!("The slowness is likely caused by one of:");
    println!("1. GetObjectHandles returning many handles (Android recursion issue)");
    println!("2. GetObjectInfo being slow per call");
    println!("3. Combination of both: list_objects calls GetObjectInfo for EVERY handle");
    println!();
    println!("For Android devices with thousands of files, list_objects(None) can be slow");
    println!("because it needs to call GetObjectInfo for each handle returned.");

    println!("\n=== Diagnostic complete ===");
    Ok(())
}
