//! Test file operations: create folder, upload, download, delete.
//!
//! Run with: cargo run --example test_file_ops

use bytes::Bytes;
use mtp_rs::mtp::{MtpDevice, NewObjectInfo};
use std::time::{SystemTime, UNIX_EPOCH};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== File operations test ===\n");

    let device = MtpDevice::open_first().await?;
    println!(
        "Connected to: {} {}",
        device.device_info().manufacturer,
        device.device_info().model
    );

    let storages = device.storages().await?;
    println!("Found {} storage(s):", storages.len());
    for s in &storages {
        println!(
            "  {} (ID: {:08X}) - {:?}",
            s.info().description,
            s.id().0,
            s.info().storage_type
        );
    }

    // Use the first storage
    let storage = &storages[0];
    println!("\nUsing storage: {}\n", storage.info().description);

    // Generate unique names using timestamp
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let folder_name = format!("mtp_rs_test_{}", timestamp);
    let file_name = format!("test_{}.txt", timestamp);
    let test_content = format!("Hello from mtp-rs! Timestamp: {}", timestamp);

    // Step 1: List root before
    println!("=== Step 1: List root (before) ===");
    let root_objects = storage.list_objects(None).await?;
    println!("Root contains {} objects:", root_objects.len());
    for obj in &root_objects {
        let kind = if obj.is_folder() { "DIR " } else { "FILE" };
        println!("  {} {}", kind, obj.filename);
    }
    println!();

    // Step 2: Create a test folder in root
    println!("=== Step 2: Create folder '{}' ===", folder_name);
    let folder_handle = match storage.create_folder(None, &folder_name).await {
        Ok(h) => {
            println!("✓ Created folder with handle {:?}", h);
            h
        }
        Err(e) => {
            println!("✗ Failed to create folder: {}", e);
            println!("\nThis might be expected if the storage is read-only or virtual.");
            println!("Try switching the camera to USB storage mode instead of tether mode.");
            return Ok(());
        }
    };
    println!();

    // Step 3: Verify folder appears in root listing
    println!("=== Step 3: Verify folder in root listing ===");
    let root_objects = storage.list_objects(None).await?;
    let folder_found = root_objects.iter().any(|o| o.filename == folder_name);
    if folder_found {
        println!("✓ Folder '{}' found in root", folder_name);
    } else {
        println!("✗ Folder '{}' NOT found in root!", folder_name);
    }
    println!();

    // Step 4: Upload a test file into the folder
    println!("=== Step 4: Upload file '{}' into folder ===", file_name);
    let content_bytes = test_content.as_bytes();
    let file_info = NewObjectInfo::file(&file_name, content_bytes.len() as u64);
    let data_stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(
        content_bytes.to_vec(),
    ))]);

    let file_handle = match storage
        .upload(Some(folder_handle), file_info, Box::pin(data_stream))
        .await
    {
        Ok(h) => {
            println!(
                "✓ Uploaded file with handle {:?} ({} bytes)",
                h,
                content_bytes.len()
            );
            h
        }
        Err(e) => {
            println!("✗ Failed to upload file: {}", e);
            // Try to clean up folder
            println!("\nCleaning up folder...");
            let _ = storage.delete(folder_handle).await;
            return Ok(());
        }
    };
    println!();

    // Step 5: List folder contents
    println!("=== Step 5: List folder contents ===");
    let folder_objects = storage.list_objects(Some(folder_handle)).await?;
    println!("Folder contains {} objects:", folder_objects.len());
    for obj in &folder_objects {
        let kind = if obj.is_folder() { "DIR " } else { "FILE" };
        println!("  {} {} ({} bytes)", kind, obj.filename, obj.size);
    }
    let file_found = folder_objects.iter().any(|o| o.filename == file_name);
    if file_found {
        println!("✓ File '{}' found in folder", file_name);
    } else {
        println!("✗ File '{}' NOT found in folder!", file_name);
    }
    println!();

    // Step 6: Download the file and verify content
    println!("=== Step 6: Download and verify file ===");
    let downloaded = storage.download(file_handle).await?;
    let downloaded_str = String::from_utf8_lossy(&downloaded);
    if downloaded_str == test_content {
        println!("✓ Downloaded content matches! ({} bytes)", downloaded.len());
    } else {
        println!("✗ Content mismatch!");
        println!("  Expected: {}", test_content);
        println!("  Got: {}", downloaded_str);
    }
    println!();

    // Step 7: Delete the file
    println!("=== Step 7: Delete file ===");
    match storage.delete(file_handle).await {
        Ok(()) => println!("✓ Deleted file"),
        Err(e) => println!("✗ Failed to delete file: {}", e),
    }
    println!();

    // Step 8: Verify file is gone
    println!("=== Step 8: Verify file deleted ===");
    let folder_objects = storage.list_objects(Some(folder_handle)).await?;
    let file_still_exists = folder_objects.iter().any(|o| o.filename == file_name);
    if !file_still_exists {
        println!("✓ File no longer in folder");
    } else {
        println!("✗ File still exists!");
    }
    println!();

    // Step 9: Delete the folder
    println!("=== Step 9: Delete folder ===");
    match storage.delete(folder_handle).await {
        Ok(()) => println!("✓ Deleted folder"),
        Err(e) => println!("✗ Failed to delete folder: {}", e),
    }
    println!();

    // Step 10: Verify folder is gone
    println!("=== Step 10: Verify folder deleted ===");
    let root_objects = storage.list_objects(None).await?;
    let folder_still_exists = root_objects.iter().any(|o| o.filename == folder_name);
    if !folder_still_exists {
        println!("✓ Folder no longer in root");
    } else {
        println!("✗ Folder still exists!");
    }
    println!();

    println!("=== All file operations completed successfully! ===");
    Ok(())
}
