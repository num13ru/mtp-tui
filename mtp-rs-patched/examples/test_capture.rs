//! Test capture with different parameters.
//!
//! Run with: cargo run --example test_capture

use mtp_rs::ptp::{
    DevicePropertyCode, ObjectFormatCode, OperationCode, PropertyValue, PtpDevice, StorageId,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Capture test ===\n");

    let device = PtpDevice::open_first().await?;
    let session = device.open_session().await?;

    // Get storage IDs
    let storage_ids = session.get_storage_ids().await?;
    println!("Storage IDs: {:?}\n", storage_ids);

    // Check device info
    let info = device.get_device_info().await?;
    println!(
        "InitiateCapture supported: {}",
        info.supports_operation(OperationCode::InitiateCapture)
    );

    // Check property 0xD207 (mentioned in libgphoto2 as needed for Fuji)
    println!("\n=== Checking Fuji-specific property 0xD207 ===");
    let prop_d207 = DevicePropertyCode::Unknown(0xD207);
    match session.get_device_prop_desc(prop_d207).await {
        Ok(desc) => {
            println!("0xD207 current value: {:?}", desc.current_value);
            println!("0xD207 writable: {}", desc.writable);
        }
        Err(e) => println!("0xD207 error: {}", e),
    }

    // Try different capture parameter combinations
    println!("\n=== Trying capture with different parameters ===");

    let test_cases = [
        (
            StorageId(0),
            ObjectFormatCode::Undefined,
            "StorageId(0), Undefined",
        ),
        (
            StorageId(0xFFFFFFFF),
            ObjectFormatCode::Undefined,
            "StorageId(ALL), Undefined",
        ),
        (
            storage_ids[0],
            ObjectFormatCode::Undefined,
            "actual storage, Undefined",
        ),
        (StorageId(0), ObjectFormatCode::Jpeg, "StorageId(0), JPEG"),
        (
            storage_ids[0],
            ObjectFormatCode::Jpeg,
            "actual storage, JPEG",
        ),
    ];

    for (storage_id, format, desc) in test_cases {
        print!("  {} ... ", desc);
        match session.initiate_capture(storage_id, format).await {
            Ok(()) => {
                println!("SUCCESS! Camera should capture now.");
                // Wait a bit for the capture to complete
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                break;
            }
            Err(e) => {
                println!("Error: {:?}", e);
            }
        }
    }

    // Try setting 0xD207 to 2 (as mentioned in libgphoto2) and then capture
    println!("\n=== Trying Fuji workaround (set 0xD207=2, then capture) ===");

    // Set 0xD207 to 2
    let value = PropertyValue::Uint16(2);
    match session.set_device_prop_value_typed(prop_d207, &value).await {
        Ok(()) => println!("Set 0xD207 to 2: OK"),
        Err(e) => println!("Set 0xD207 to 2: Error - {}", e),
    }

    // Now try capture again
    print!("Capture after setting 0xD207=2 ... ");
    match session
        .initiate_capture(storage_ids[0], ObjectFormatCode::Undefined)
        .await
    {
        Ok(()) => println!("SUCCESS!"),
        Err(e) => println!("Error: {:?}", e),
    }

    // Reset 0xD207 to 1
    let value = PropertyValue::Uint16(1);
    let _ = session.set_device_prop_value_typed(prop_d207, &value).await;

    session.close().await?;
    println!("\n=== Done ===");
    Ok(())
}
