//! Debug recursive listing to find duplicate bug.
//!
//! Run with: cargo run --example debug_listing

use mtp_rs::ptp::{ObjectHandle, PtpDevice};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Debug listing ===\n");

    let device = PtpDevice::open_first().await?;
    let session = device.open_session().await?;

    let storage_ids = session.get_storage_ids().await?;
    let storage_id = storage_ids[0];
    println!("Storage ID: {:?}\n", storage_id);

    // Test 1: Native recursive listing with ALL handle
    println!("=== Native recursive (ObjectHandle::ALL) ===");
    let handles = session
        .get_object_handles(storage_id, None, Some(ObjectHandle::ALL))
        .await?;
    println!("Got {} handles: {:?}\n", handles.len(), handles);

    // Check for duplicates
    let mut seen = std::collections::HashSet::new();
    let mut duplicates = Vec::new();
    for h in &handles {
        if !seen.insert(h.0) {
            duplicates.push(h);
        }
    }
    if !duplicates.is_empty() {
        println!("⚠️  DUPLICATES in native listing: {:?}\n", duplicates);
    } else {
        println!("✓ No duplicates in native listing\n");
    }

    // Test 2: Manual listing - root
    println!("=== Manual listing (root) ===");
    let root_handles = session
        .get_object_handles(storage_id, None, Some(ObjectHandle::ROOT))
        .await?;
    println!("Root handles: {:?}", root_handles);
    for h in &root_handles {
        let info = session.get_object_info(*h).await?;
        println!("  {:?}: {} (parent: {:?})", h, info.filename, info.parent);
    }
    println!();

    // Test 3: Traverse manually
    println!("=== Manual traversal ===");
    let mut all_objects = Vec::new();
    let mut to_visit = vec![(None, root_handles.clone())];

    while let Some((parent, handles)) = to_visit.pop() {
        println!(
            "Visiting parent {:?} with {} handles",
            parent,
            handles.len()
        );
        for h in handles {
            let info = session.get_object_info(h).await?;
            println!(
                "  {:?}: {} (folder: {})",
                h,
                info.filename,
                info.is_folder()
            );
            all_objects.push((h, info.filename.clone()));

            if info.is_folder() {
                let children = session
                    .get_object_handles(storage_id, None, Some(h))
                    .await?;
                if !children.is_empty() {
                    to_visit.push((Some(h), children));
                }
            }
        }
    }

    println!(
        "\nTotal objects via manual traversal: {}",
        all_objects.len()
    );

    // Check for duplicates in manual traversal
    let mut seen = std::collections::HashSet::new();
    for (h, name) in &all_objects {
        if !seen.insert(h.0) {
            println!("⚠️  Duplicate in manual: {:?} ({})", h, name);
        }
    }

    println!("\n=== Done ===");
    Ok(())
}
