//! Diagnostic script to investigate MTP issues.
//!
//! Run with: cargo run --example diagnose

use bytes::Bytes;
use mtp_rs::mtp::{MtpDevice, NewObjectInfo};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== MTP Diagnostic Tool ===\n");

    // Connect to device
    let device = MtpDevice::open_first().await?;
    println!(
        "Connected to: {} {}",
        device.device_info().manufacturer,
        device.device_info().model
    );

    let storages = device.storages().await?;
    let storage = &storages[0];
    println!("Storage: {}\n", storage.info().description);

    // Test 1: List root objects (non-recursive)
    println!("=== Test 1: Root folder listing (non-recursive) ===");
    let root_objects = storage.list_objects(None).await?;
    let root_folders = root_objects.iter().filter(|o| o.is_folder()).count();
    let root_files = root_objects.iter().filter(|o| o.is_file()).count();
    println!(
        "Root contains: {} folders, {} files, {} total\n",
        root_folders,
        root_files,
        root_objects.len()
    );

    // Test 2: List recursive (smart - auto-detects Android)
    println!("=== Test 2: Recursive listing (smart) ===");
    println!(
        "Device is Android: {}",
        device
            .device_info()
            .vendor_extension_desc
            .contains("android.com")
    );
    let start = std::time::Instant::now();
    let recursive_objects = storage.list_objects_recursive(None).await?;
    let elapsed = start.elapsed();
    let rec_folders = recursive_objects.iter().filter(|o| o.is_folder()).count();
    let rec_files = recursive_objects.iter().filter(|o| o.is_file()).count();
    println!(
        "Recursive contains: {} folders, {} files, {} total",
        rec_folders,
        rec_files,
        recursive_objects.len()
    );
    println!("Time taken: {:.2}s\n", elapsed.as_secs_f64());

    // Test 3: Manual recursive listing of first folder
    if let Some(first_folder) = root_objects.iter().find(|o| o.is_folder()) {
        println!(
            "=== Test 3: Listing contents of '{}' folder ===",
            first_folder.filename
        );
        let folder_contents = storage.list_objects(Some(first_folder.handle)).await?;
        let sub_folders = folder_contents.iter().filter(|o| o.is_folder()).count();
        let sub_files = folder_contents.iter().filter(|o| o.is_file()).count();
        println!(
            "'{}' contains: {} folders, {} files, {} total\n",
            first_folder.filename,
            sub_folders,
            sub_files,
            folder_contents.len()
        );

        // Show first few items
        for (i, obj) in folder_contents.iter().take(5).enumerate() {
            let kind = if obj.is_folder() { "DIR" } else { "FILE" };
            println!(
                "  {}. {} {} ({} bytes)",
                i + 1,
                kind,
                obj.filename,
                obj.size
            );
        }
        if folder_contents.len() > 5 {
            println!("  ... and {} more", folder_contents.len() - 5);
        }
        println!();
    }

    // Test 4: Find and download a small file
    println!("=== Test 4: Download test ===");
    let small_file = root_objects
        .iter()
        .find(|o| o.is_file() && o.size > 1000 && o.size < 100_000);

    match small_file {
        Some(file) => {
            println!("Downloading: {} ({} bytes)", file.filename, file.size);
            let data = storage.download(file.handle).await?;
            println!("Downloaded {} bytes successfully!", data.len());

            // Verify size matches
            if data.len() as u64 == file.size {
                println!("✓ Size matches expected");
            } else {
                println!(
                    "✗ Size mismatch: expected {}, got {}",
                    file.size,
                    data.len()
                );
            }
        }
        None => {
            println!("No suitable small file found in root, checking subfolders...");

            // Try to find a file in a subfolder
            for folder in root_objects.iter().filter(|o| o.is_folder()).take(5) {
                let contents = storage.list_objects(Some(folder.handle)).await?;
                if let Some(file) = contents
                    .iter()
                    .find(|o| o.is_file() && o.size > 1000 && o.size < 100_000)
                {
                    println!(
                        "Found file in '{}': {} ({} bytes)",
                        folder.filename, file.filename, file.size
                    );
                    let data = storage.download(file.handle).await?;
                    println!("Downloaded {} bytes successfully!", data.len());

                    if data.len() as u64 == file.size {
                        println!("✓ Size matches expected");
                    } else {
                        println!(
                            "✗ Size mismatch: expected {}, got {}",
                            file.size,
                            data.len()
                        );
                    }
                    break;
                }
            }
        }
    }

    // Test 5: Upload test
    println!("\n=== Test 5: Upload test ===");

    // Try uploading to the Download folder (more likely to work)
    let download_folder = root_objects.iter().find(|o| o.filename == "Download");

    match download_folder {
        Some(folder) => {
            println!("Uploading to Download folder (handle: {:?})", folder.handle);

            let test_content = b"Test file from mtp-rs diagnostic";
            let info = NewObjectInfo::file("mtp-rs-diag-test.txt", test_content.len() as u64);
            let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(
                test_content.to_vec(),
            ))]);

            match storage
                .upload(Some(folder.handle), info, Box::pin(stream))
                .await
            {
                Ok(handle) => {
                    println!("✓ Upload succeeded! Handle: {:?}", handle);

                    // Clean up - delete the file
                    println!("Cleaning up...");
                    match storage.delete(handle).await {
                        Ok(_) => println!("✓ Cleanup successful"),
                        Err(e) => println!("✗ Cleanup failed: {}", e),
                    }
                }
                Err(e) => {
                    println!("✗ Upload to Download folder failed: {}", e);

                    // Try uploading to root
                    println!("\nTrying upload to root...");
                    let info2 =
                        NewObjectInfo::file("mtp-rs-diag-test.txt", test_content.len() as u64);
                    let stream2 = futures::stream::iter(vec![Ok::<_, std::io::Error>(
                        Bytes::from(test_content.to_vec()),
                    )]);

                    match storage.upload(None, info2, Box::pin(stream2)).await {
                        Ok(handle) => {
                            println!("✓ Upload to root succeeded! Handle: {:?}", handle);
                            let _ = storage.delete(handle).await;
                        }
                        Err(e2) => println!("✗ Upload to root also failed: {}", e2),
                    }
                }
            }
        }
        None => {
            println!("Download folder not found, trying root...");
            let test_content = b"Test file from mtp-rs diagnostic";
            let info = NewObjectInfo::file("mtp-rs-diag-test.txt", test_content.len() as u64);
            let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(Bytes::from(
                test_content.to_vec(),
            ))]);

            match storage.upload(None, info, Box::pin(stream)).await {
                Ok(handle) => {
                    println!("✓ Upload succeeded! Handle: {:?}", handle);
                    let _ = storage.delete(handle).await;
                }
                Err(e) => println!("✗ Upload failed: {}", e),
            }
        }
    }

    println!("\n=== Diagnostics complete ===");
    Ok(())
}
