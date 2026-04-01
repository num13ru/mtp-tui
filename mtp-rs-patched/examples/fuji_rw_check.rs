//! Check camera/device read/write capabilities.
//!
//! Run with: cargo run --example fuji_rw_check
//! Or with a specific device: cargo run --example fuji_rw_check -- --location <LOCATION_ID>
//!
//! This script shows:
//! 1. What operations the device advertises (especially write operations)
//! 2. What access_capability each storage reports

use mtp_rs::ptp::{OperationCode, PtpDevice};
use mtp_rs::transport::NusbTransport;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Device R/W Capability Check ===\n");

    // List all available devices first
    let devices = NusbTransport::list_mtp_devices()?;
    println!("=== Available MTP/PTP Devices ({}) ===", devices.len());
    for dev in &devices {
        println!(
            "  Location 0x{:X}: {} {} (VID:PID = {:04X}:{:04X})",
            dev.location_id,
            dev.manufacturer.as_deref().unwrap_or("?"),
            dev.product.as_deref().unwrap_or("?"),
            dev.vendor_id,
            dev.product_id
        );
    }
    println!();

    // Check for --location argument
    let args: Vec<String> = std::env::args().collect();
    let location_id = if let Some(pos) = args.iter().position(|a| a == "--location") {
        let loc_str = args
            .get(pos + 1)
            .ok_or("Missing location ID after --location")?;
        if loc_str.starts_with("0x") || loc_str.starts_with("0X") {
            u64::from_str_radix(&loc_str[2..], 16)?
        } else {
            loc_str.parse()?
        }
    } else {
        0 // Will use first device
    };

    let device = if location_id != 0 {
        println!("Opening device at location 0x{:X}...\n", location_id);
        PtpDevice::open_by_location(location_id).await?
    } else {
        println!("Opening first available device...\n");
        PtpDevice::open_first().await?
    };
    let session = device.open_session().await?;

    // Get device info
    let info = session.get_device_info().await?;
    println!("Camera: {} {}", info.manufacturer, info.model);
    println!("Version: {}", info.device_version);
    println!("Serial: {}", info.serial_number);
    println!();

    // Check write-related operations
    println!("=== Write Operation Support ===");
    let write_ops = [
        (
            OperationCode::SendObjectInfo,
            "SendObjectInfo (upload metadata)",
        ),
        (OperationCode::SendObject, "SendObject (upload data)"),
        (OperationCode::DeleteObject, "DeleteObject"),
        (OperationCode::MoveObject, "MoveObject"),
        (OperationCode::CopyObject, "CopyObject"),
        (
            OperationCode::SetObjectPropValue,
            "SetObjectPropValue (rename)",
        ),
    ];

    let mut has_any_write = false;
    for (op, desc) in write_ops {
        let supported = info.supports_operation(op);
        let status = if supported {
            has_any_write = true;
            "✓ SUPPORTED"
        } else {
            "✗ NOT SUPPORTED"
        };
        println!(
            "  {:?} (0x{:04X}): {} - {}",
            op,
            u16::from(op),
            status,
            desc
        );
    }
    println!();

    if !has_any_write {
        println!("  → Camera does NOT advertise any write operations!");
        println!("    This is typical for PTP-mode cameras.");
    } else {
        println!("  → Camera DOES advertise write operations.");
        println!("    Note: Advertised ≠ actually works.");
    }
    println!();

    // Check storage info
    println!("=== Storage Access Capabilities ===");
    let storage_ids = session.get_storage_ids().await?;

    if storage_ids.is_empty() {
        println!("  No storage found (is SD card inserted?)");
    }

    for storage_id in storage_ids {
        let storage_info = session.get_storage_info(storage_id).await?;
        println!("  Storage 0x{:08X}:", storage_id.0);
        println!("    Description: {}", storage_info.description);
        println!("    Type: {:?}", storage_info.storage_type);
        println!("    Filesystem: {:?}", storage_info.filesystem_type);
        println!(
            "    Access: {:?} (code: {})",
            storage_info.access_capability,
            u16::from(storage_info.access_capability)
        );
        println!(
            "    Capacity: {:.2} GB",
            storage_info.max_capacity as f64 / 1e9
        );
        println!(
            "    Free: {:.2} GB",
            storage_info.free_space_bytes as f64 / 1e9
        );
        println!();

        // Explain what the access capability means
        match storage_info.access_capability {
            mtp_rs::ptp::AccessCapability::ReadWrite => {
                println!("    → Storage REPORTS as Read-Write (code 0)");
                println!("      But this doesn't mean writes actually work!");
            }
            mtp_rs::ptp::AccessCapability::ReadOnlyWithoutDeletion => {
                println!("    → Storage REPORTS as Read-Only, no deletion (code 1)");
            }
            mtp_rs::ptp::AccessCapability::ReadOnlyWithDeletion => {
                println!("    → Storage REPORTS as Read-Only, deletion allowed (code 2)");
            }
            mtp_rs::ptp::AccessCapability::Unknown(code) => {
                println!("    → Storage REPORTS unknown access code: {}", code);
            }
        }
        println!();
    }

    // Show all supported operations for reference
    println!(
        "=== All Supported Operations ({}) ===",
        info.operations_supported.len()
    );
    for op in &info.operations_supported {
        println!("  0x{:04X} {:?}", u16::from(*op), op);
    }

    // Test actual write capability if --test-write is passed
    if args.iter().any(|a| a == "--test-write") {
        println!("\n=== TESTING ACTUAL WRITE CAPABILITY ===");
        println!("Attempting to create a test file (will be deleted)...\n");

        let storage_ids = session.get_storage_ids().await?;
        if let Some(&storage_id) = storage_ids.first() {
            // Create a minimal ObjectInfo for a test file
            let test_obj = mtp_rs::ptp::ObjectInfo {
                storage_id,
                format: mtp_rs::ptp::ObjectFormatCode::Text,
                size: 4,
                filename: "_mtp_test_.txt".to_string(),
                parent: mtp_rs::ptp::ObjectHandle::ROOT,
                ..Default::default()
            };

            match session
                .send_object_info(storage_id, mtp_rs::ptp::ObjectHandle::ROOT, &test_obj)
                .await
            {
                Ok((_, _, handle)) => {
                    println!("  SendObjectInfo: SUCCESS (handle: 0x{:08X})", handle.0);

                    // Try to send the actual data
                    match session.send_object(b"test").await {
                        Ok(()) => {
                            println!("  SendObject: SUCCESS");
                            println!("\n  ✓ Camera ACTUALLY SUPPORTS WRITES!");

                            // Clean up - delete the test file
                            println!("  Deleting test file...");
                            match session.delete_object(handle).await {
                                Ok(()) => println!("  DeleteObject: SUCCESS"),
                                Err(e) => println!("  DeleteObject: FAILED - {}", e),
                            }
                        }
                        Err(e) => {
                            println!("  SendObject: FAILED - {}", e);
                            println!("\n  ✗ Camera advertises SendObject but it FAILS!");
                        }
                    }
                }
                Err(e) => {
                    println!("  SendObjectInfo: FAILED - {}", e);
                    println!("\n  ✗ Camera advertises SendObjectInfo but it FAILS!");
                    println!("    This confirms writes are NOT actually supported.");
                }
            }
        }
    } else {
        println!("\n(Use --test-write to actually attempt a write operation)");
    }

    session.close().await?;
    println!("\n=== Done ===");
    Ok(())
}
