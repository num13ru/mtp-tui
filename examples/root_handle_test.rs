//! Diagnostic: compare GetObjectHandles with parent=0 vs parent=0xFFFFFFFF.
//!
//! Determines whether the device returns only root-level objects or dumps everything
//! for each parent value. The result tells us which fix path is viable for
//! non-Android devices (Kindle, Fuji) that return all objects for parent=0.
//!
//! Run with: cargo run --example root_handle_test

use mtp_rs::ptp::{ObjectHandle, PtpDevice, StorageId};

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Root Handle Diagnostic ===\n");

    let device = PtpDevice::open_first().await?;
    let session = device.open_session().await?;
    let info = session.get_device_info().await?;

    println!("Device   : {} {}", info.manufacturer, info.model);
    println!("Vendor   : {:?}", info.vendor_extension_desc);
    println!();

    let storage_ids = session.get_storage_ids().await?;
    let target = storage_ids.first().copied().unwrap_or(StorageId::ALL);
    println!("Storage  : 0x{:08X}\n", target.0);

    let cases: &[(Option<ObjectHandle>, &str)] = &[
        (None, "parent=0x00000000 (None)"),
        (Some(ObjectHandle::ALL), "parent=0xFFFFFFFF (ALL)"),
    ];

    for (parent, label) in cases {
        println!("--- GetObjectHandles({label}) ---");
        match session.get_object_handles(target, None, *parent).await {
            Ok(handles) => {
                println!("  Returned {} handle(s)", handles.len());

                let preview = 25;
                for handle in handles.iter().take(preview) {
                    match session.get_object_info(*handle).await {
                        Ok(oi) => {
                            let kind = if oi.is_folder() { "DIR " } else { "FILE" };
                            let size = if oi.is_folder() {
                                String::new()
                            } else {
                                format!(" ({})", format_bytes(oi.size))
                            };
                            println!(
                                "  {kind}  {} [handle={}, parent=0x{:08X}]{size}",
                                oi.filename, handle.0, oi.parent.0,
                            );
                        }
                        Err(e) => println!("  handle {}: error {e}", handle.0),
                    }
                }
                if handles.len() > preview {
                    println!("  ... and {} more", handles.len() - preview);
                }
            }
            Err(e) => println!("  Error: {e}"),
        }
        println!();
    }

    println!("=== Diagnostic complete ===");
    Ok(())
}
