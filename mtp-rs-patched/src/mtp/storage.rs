//! Storage operations.

use crate::mtp::object::NewObjectInfo;
use crate::mtp::stream::{FileDownload, Progress};
use crate::ptp::{ObjectHandle, ObjectInfo, StorageId, StorageInfo};
use crate::Error;
use bytes::Bytes;
use futures::Stream;
use std::ops::ControlFlow;
use std::sync::Arc;

use super::device::MtpDeviceInner;

/// An in-progress directory listing that yields [`ObjectInfo`] items one at a time.
///
/// Created by [`Storage::list_objects_stream()`]. After `GetObjectHandles` completes,
/// the total count is known immediately. Each call to [`next()`](Self::next) fetches
/// one `GetObjectInfo` from USB, so the consumer can report progress (e.g.,
/// "Loading files (42 of 500)...") as items arrive.
///
/// # Important
///
/// The MTP session is busy while this listing is active. You must consume
/// all items (or drop the listing) before calling other storage methods.
///
/// # Example
///
/// ```rust,ignore
/// let mut listing = storage.list_objects_stream(None).await?;
/// println!("Loading {} files...", listing.total());
///
/// while let Some(result) = listing.next().await {
///     let info = result?;
///     println!("[{}/{}] {}", listing.fetched(), listing.total(), info.filename);
/// }
/// ```
pub struct ObjectListing {
    inner: Arc<MtpDeviceInner>,
    handles: Vec<ObjectHandle>,
    /// Index of the next handle to fetch.
    cursor: usize,
    /// Parent filter: if Some, only items matching this parent are yielded.
    parent_filter: Option<ParentFilter>,
}

/// Describes how to filter objects by parent handle.
enum ParentFilter {
    /// Accept objects whose parent matches exactly.
    Exact(ObjectHandle),
    /// Android root: accept parent 0 or 0xFFFFFFFF.
    AndroidRoot,
}

impl ObjectListing {
    /// Total number of object handles returned by the device.
    ///
    /// When a parent filter is active (e.g., Fuji devices that return all objects
    /// for root), some items may be skipped, so the actual yielded count can be lower.
    #[must_use]
    pub fn total(&self) -> usize {
        self.handles.len()
    }

    /// Number of handles processed so far (including filtered-out items).
    #[must_use]
    pub fn fetched(&self) -> usize {
        self.cursor
    }

    /// Fetch the next object from the device.
    ///
    /// Returns `None` when all handles have been processed.
    /// Items that don't match the parent filter are silently skipped.
    pub async fn next(&mut self) -> Option<Result<ObjectInfo, Error>> {
        loop {
            if self.cursor >= self.handles.len() {
                return None;
            }

            let handle = self.handles[self.cursor];
            self.cursor += 1;

            let mut info = match self.inner.session.get_object_info(handle).await {
                Ok(info) => info,
                Err(e) => return Some(Err(e)),
            };
            info.handle = handle;

            // Apply parent filter if present
            if let Some(filter) = &self.parent_filter {
                let matches = match filter {
                    ParentFilter::Exact(expected) => info.parent == *expected,
                    ParentFilter::AndroidRoot => info.parent.0 == 0 || info.parent.0 == 0xFFFFFFFF,
                };
                if !matches {
                    continue;
                }
            }

            return Some(Ok(info));
        }
    }
}

/// A storage location on an MTP device.
///
/// `Storage` holds an `Arc<MtpDeviceInner>` so it can outlive the original
/// `MtpDevice` and be used from multiple tasks.
pub struct Storage {
    inner: Arc<MtpDeviceInner>,
    id: StorageId,
    info: StorageInfo,
}

impl Storage {
    /// Create a new Storage (internal).
    pub(crate) fn new(inner: Arc<MtpDeviceInner>, id: StorageId, info: StorageInfo) -> Self {
        Self { inner, id, info }
    }

    #[must_use]
    pub fn id(&self) -> StorageId {
        self.id
    }

    /// Storage information (cached, call refresh() to update).
    #[must_use]
    pub fn info(&self) -> &StorageInfo {
        &self.info
    }

    /// Refresh storage info from device (updates free space, etc.).
    pub async fn refresh(&mut self) -> Result<(), Error> {
        self.info = self.inner.session.get_storage_info(self.id).await?;
        Ok(())
    }

    /// List objects in a folder (None = root), returning all results at once.
    ///
    /// For progress reporting during large listings, use
    /// [`list_objects_stream()`](Self::list_objects_stream) instead.
    ///
    /// This method handles various device quirks:
    /// - Android devices: parent=0 returns ALL objects, so we use parent=0xFFFFFFFF instead
    /// - Samsung devices: return InvalidObjectHandle for parent=0, so we fall back to recursive
    /// - Fuji devices: return all objects for root, so we filter by parent handle
    pub async fn list_objects(
        &self,
        parent: Option<ObjectHandle>,
    ) -> Result<Vec<ObjectInfo>, Error> {
        let mut listing = self.list_objects_stream(parent).await?;
        let mut objects = Vec::with_capacity(listing.total());
        while let Some(result) = listing.next().await {
            objects.push(result?);
        }
        Ok(objects)
    }

    /// List objects in a folder as a streaming [`ObjectListing`].
    ///
    /// Returns immediately after `GetObjectHandles` completes (one USB round-trip).
    /// The total count is then known via [`ObjectListing::total()`], and each call
    /// to [`ObjectListing::next()`] fetches one object's metadata from USB.
    ///
    /// This enables progress reporting (e.g., "Loading 42 of 500...") during
    /// what would otherwise be a single blocking `list_objects()` call.
    ///
    /// Handles the same device quirks as [`list_objects()`](Self::list_objects).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut listing = storage.list_objects_stream(None).await?;
    /// println!("Found {} items", listing.total());
    ///
    /// while let Some(result) = listing.next().await {
    ///     let info = result?;
    ///     println!("[{}/{}] {}", listing.fetched(), listing.total(), info.filename);
    /// }
    /// ```
    pub async fn list_objects_stream(
        &self,
        parent: Option<ObjectHandle>,
    ) -> Result<ObjectListing, Error> {
        // Android quirk: When listing root (parent=None/0), Android returns ALL objects
        // on the device instead of just root-level objects. This makes listing extremely slow.
        // Counter-intuitively, using parent=0xFFFFFFFF (ObjectHandle::ALL) returns the
        // actual root-level objects on Android devices.
        let effective_parent = if parent.is_none() && self.inner.is_android() {
            Some(ObjectHandle::ALL)
        } else {
            parent
        };

        let result = self
            .inner
            .session
            .get_object_handles(self.id, None, effective_parent)
            .await;

        let handles = match result {
            Ok(h) => h,
            Err(Error::Protocol {
                code: crate::ptp::ResponseCode::InvalidObjectHandle,
                ..
            }) if parent.is_none() => {
                // Samsung fallback: use recursive listing and filter to root items
                return self.list_objects_stream_samsung_fallback().await;
            }
            Err(e) => return Err(e),
        };

        // Build parent filter for devices that return more objects than requested
        let parent_filter = if parent.is_none() && self.inner.is_android() {
            Some(ParentFilter::AndroidRoot)
        } else {
            // Filter by exact parent (catches Fuji devices that return all objects for root)
            Some(ParentFilter::Exact(parent.unwrap_or(ObjectHandle::ROOT)))
        };

        Ok(ObjectListing {
            inner: Arc::clone(&self.inner),
            handles,
            cursor: 0,
            parent_filter,
        })
    }

    /// Samsung fallback returning a streaming [`ObjectListing`].
    async fn list_objects_stream_samsung_fallback(&self) -> Result<ObjectListing, Error> {
        let handles = self
            .inner
            .session
            .get_object_handles(self.id, None, Some(ObjectHandle::ALL))
            .await?;

        Ok(ObjectListing {
            inner: Arc::clone(&self.inner),
            handles,
            cursor: 0,
            // Root items have parent 0 or 0xFFFFFFFF (depending on device)
            parent_filter: Some(ParentFilter::AndroidRoot),
        })
    }

    /// List objects recursively.
    ///
    /// This method automatically detects Android devices and uses manual traversal
    /// for them, since Android's MTP implementation doesn't support the native
    /// `ObjectHandle::ALL` recursive listing.
    ///
    /// For non-Android devices, it tries native recursive listing first and falls
    /// back to manual traversal if the results look incomplete.
    pub async fn list_objects_recursive(
        &self,
        parent: Option<ObjectHandle>,
    ) -> Result<Vec<ObjectInfo>, Error> {
        if self.inner.is_android() {
            return self.list_objects_recursive_manual(parent).await;
        }

        let native_result = self.list_objects_recursive_native(parent).await?;

        // Heuristic: if we only got folders and no files, native listing
        // probably didn't work - fall back to manual traversal
        let has_files = native_result.iter().any(|o| o.is_file());
        if !native_result.is_empty() && !has_files {
            return self.list_objects_recursive_manual(parent).await;
        }

        Ok(native_result)
    }

    /// List objects recursively using native MTP recursive listing.
    pub async fn list_objects_recursive_native(
        &self,
        parent: Option<ObjectHandle>,
    ) -> Result<Vec<ObjectInfo>, Error> {
        let recursive_parent = if parent.is_none() {
            Some(ObjectHandle::ALL)
        } else {
            parent
        };

        let handles = self
            .inner
            .session
            .get_object_handles(self.id, None, recursive_parent)
            .await?;

        let mut objects = Vec::with_capacity(handles.len());
        for handle in handles {
            let mut info = self.inner.session.get_object_info(handle).await?;
            info.handle = handle;
            objects.push(info);
        }
        Ok(objects)
    }

    /// List objects recursively using manual folder traversal.
    pub async fn list_objects_recursive_manual(
        &self,
        parent: Option<ObjectHandle>,
    ) -> Result<Vec<ObjectInfo>, Error> {
        let mut result = Vec::new();
        let mut folders_to_visit = vec![parent];

        while let Some(current_parent) = folders_to_visit.pop() {
            let objects = self.list_objects(current_parent).await?;

            for obj in objects {
                if obj.is_folder() {
                    folders_to_visit.push(Some(obj.handle));
                }
                result.push(obj);
            }
        }

        Ok(result)
    }

    /// Get object metadata by handle.
    pub async fn get_object_info(&self, handle: ObjectHandle) -> Result<ObjectInfo, Error> {
        let mut info = self.inner.session.get_object_info(handle).await?;
        info.handle = handle;
        Ok(info)
    }

    // =========================================================================
    // Download operations
    // =========================================================================

    /// Download a file and return all bytes.
    ///
    /// For small to medium files where you want all the data in memory.
    /// For large files or streaming to disk, use [`download_stream()`](Self::download_stream).
    pub async fn download(&self, handle: ObjectHandle) -> Result<Vec<u8>, Error> {
        self.inner.session.get_object(handle).await
    }

    /// Download a partial file (byte range).
    pub async fn download_partial(
        &self,
        handle: ObjectHandle,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, Error> {
        self.inner
            .session
            .get_partial_object(handle, offset, size)
            .await
    }

    pub async fn download_thumbnail(&self, handle: ObjectHandle) -> Result<Vec<u8>, Error> {
        self.inner.session.get_thumb(handle).await
    }

    /// Download a file as a stream (true USB streaming).
    ///
    /// Unlike [`download()`](Self::download), this method yields data chunks
    /// directly from USB as they arrive, without buffering the entire file
    /// in memory. Ideal for large files or when piping data to disk.
    ///
    /// # Important
    ///
    /// The MTP session is locked while the download is active. You must consume
    /// the entire download (or drop it) before calling other storage methods.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut download = storage.download_stream(handle).await?;
    /// println!("Downloading {} bytes...", download.size());
    ///
    /// let mut file = tokio::fs::File::create("output.bin").await?;
    /// while let Some(chunk) = download.next_chunk().await {
    ///     let bytes = chunk?;
    ///     file.write_all(&bytes).await?;
    ///     println!("Progress: {:.1}%", download.progress() * 100.0);
    /// }
    /// ```
    pub async fn download_stream(&self, handle: ObjectHandle) -> Result<FileDownload, Error> {
        let info = self.get_object_info(handle).await?;
        let size = info.size;

        let stream = self
            .inner
            .session
            .execute_with_receive_stream(crate::ptp::OperationCode::GetObject, &[handle.0])
            .await?;

        Ok(FileDownload::new(size, stream))
    }

    // =========================================================================
    // Upload operations
    // =========================================================================

    /// Upload a file from a stream.
    ///
    /// The stream is consumed and all data is buffered before sending
    /// (MTP protocol requires knowing the total size upfront).
    ///
    /// # Arguments
    ///
    /// * `parent` - Parent folder handle (None for root)
    /// * `info` - Object metadata including filename and size
    /// * `data` - Stream of data chunks to upload
    pub async fn upload<S>(
        &self,
        parent: Option<ObjectHandle>,
        info: NewObjectInfo,
        data: S,
    ) -> Result<ObjectHandle, Error>
    where
        S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
    {
        self.upload_with_progress(parent, info, data, |_| ControlFlow::Continue(()))
            .await
    }

    /// Upload a file with progress callback.
    ///
    /// Progress is reported as data is read from the stream. Return
    /// `ControlFlow::Break(())` from the callback to cancel the upload.
    pub async fn upload_with_progress<S, F>(
        &self,
        parent: Option<ObjectHandle>,
        info: NewObjectInfo,
        mut data: S,
        mut on_progress: F,
    ) -> Result<ObjectHandle, Error>
    where
        S: Stream<Item = Result<Bytes, std::io::Error>> + Unpin,
        F: FnMut(Progress) -> ControlFlow<()>,
    {
        use futures::StreamExt;

        let total_size = info.size;
        let mut buffer = Vec::with_capacity(total_size as usize);
        let mut bytes_received = 0u64;

        while let Some(chunk) = data.next().await {
            let chunk = chunk.map_err(Error::Io)?;
            bytes_received += chunk.len() as u64;
            buffer.extend_from_slice(&chunk);

            let progress = Progress {
                bytes_transferred: bytes_received,
                total_bytes: Some(total_size),
            };

            if let ControlFlow::Break(()) = on_progress(progress) {
                return Err(Error::Cancelled);
            }
        }

        let object_info = info.to_object_info();
        let parent_handle = parent.unwrap_or(ObjectHandle::ROOT);
        let (_, _, handle) = self
            .inner
            .session
            .send_object_info(self.id, parent_handle, &object_info)
            .await?;

        self.inner.session.send_object(&buffer).await?;

        Ok(handle)
    }

    // =========================================================================
    // Folder and object management
    // =========================================================================

    pub async fn create_folder(
        &self,
        parent: Option<ObjectHandle>,
        name: &str,
    ) -> Result<ObjectHandle, Error> {
        let info = NewObjectInfo::folder(name);
        let object_info = info.to_object_info();
        let parent_handle = parent.unwrap_or(ObjectHandle::ROOT);

        let (_, _, handle) = self
            .inner
            .session
            .send_object_info(self.id, parent_handle, &object_info)
            .await?;

        Ok(handle)
    }

    pub async fn delete(&self, handle: ObjectHandle) -> Result<(), Error> {
        self.inner.session.delete_object(handle).await
    }

    /// Move an object to a different folder.
    pub async fn move_object(
        &self,
        handle: ObjectHandle,
        new_parent: ObjectHandle,
        new_storage: Option<StorageId>,
    ) -> Result<(), Error> {
        let storage = new_storage.unwrap_or(self.id);
        self.inner
            .session
            .move_object(handle, storage, new_parent)
            .await
    }

    pub async fn copy_object(
        &self,
        handle: ObjectHandle,
        new_parent: ObjectHandle,
        new_storage: Option<StorageId>,
    ) -> Result<ObjectHandle, Error> {
        let storage = new_storage.unwrap_or(self.id);
        self.inner
            .session
            .copy_object(handle, storage, new_parent)
            .await
    }

    /// Rename an object (file or folder).
    ///
    /// Not all devices support renaming. Use `MtpDevice::supports_rename()`
    /// to check if this operation is available.
    pub async fn rename(&self, handle: ObjectHandle, new_name: &str) -> Result<(), Error> {
        self.inner.session.rename_object(handle, new_name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ptp::{
        pack_u16, pack_u32, pack_u32_array, ContainerType, DateTime, DeviceInfo, ObjectFormatCode,
        OperationCode, PtpSession, ResponseCode, StorageInfo,
    };
    use crate::transport::mock::MockTransport;

    // -- Test helpers (same protocol-level helpers as session tests) -----------

    fn mock_transport() -> (Arc<dyn crate::transport::Transport>, Arc<MockTransport>) {
        let mock = Arc::new(MockTransport::new());
        let transport: Arc<dyn crate::transport::Transport> = Arc::clone(&mock) as _;
        (transport, mock)
    }

    fn ok_response(tx_id: u32) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12);
        buf.extend_from_slice(&pack_u32(12));
        buf.extend_from_slice(&pack_u16(ContainerType::Response.to_code()));
        buf.extend_from_slice(&pack_u16(ResponseCode::Ok.into()));
        buf.extend_from_slice(&pack_u32(tx_id));
        buf
    }

    fn error_response(tx_id: u32, code: ResponseCode) -> Vec<u8> {
        let mut buf = Vec::with_capacity(12);
        buf.extend_from_slice(&pack_u32(12));
        buf.extend_from_slice(&pack_u16(ContainerType::Response.to_code()));
        buf.extend_from_slice(&pack_u16(code.into()));
        buf.extend_from_slice(&pack_u32(tx_id));
        buf
    }

    fn data_container(tx_id: u32, code: OperationCode, payload: &[u8]) -> Vec<u8> {
        let len = 12 + payload.len();
        let mut buf = Vec::with_capacity(len);
        buf.extend_from_slice(&pack_u32(len as u32));
        buf.extend_from_slice(&pack_u16(ContainerType::Data.to_code()));
        buf.extend_from_slice(&pack_u16(code.into()));
        buf.extend_from_slice(&pack_u32(tx_id));
        buf.extend_from_slice(payload);
        buf
    }

    // -- Storage-level helpers ------------------------------------------------

    /// Build a Storage backed by a mock transport for testing.
    ///
    /// Queues the OpenSession response automatically. The caller must queue
    /// further responses before calling list methods.
    async fn mock_storage(
        transport: Arc<dyn crate::transport::Transport>,
        vendor_extension_desc: &str,
    ) -> Storage {
        let session = Arc::new(PtpSession::open(transport, 1).await.unwrap());
        let inner = Arc::new(MtpDeviceInner {
            session,
            device_info: DeviceInfo {
                vendor_extension_desc: vendor_extension_desc.to_string(),
                ..DeviceInfo::default()
            },
        });
        Storage::new(inner, StorageId(1), StorageInfo::default())
    }

    /// Build a minimal ObjectInfo binary payload with a given filename and parent.
    fn object_info_bytes(filename: &str, parent: u32) -> Vec<u8> {
        let info = ObjectInfo {
            storage_id: StorageId(1),
            format: ObjectFormatCode::Jpeg,
            parent: ObjectHandle(parent),
            filename: filename.to_string(),
            created: Some(DateTime {
                year: 2024,
                month: 1,
                day: 1,
                hour: 0,
                minute: 0,
                second: 0,
            }),
            ..ObjectInfo::default()
        };
        info.to_bytes().unwrap()
    }

    /// Queue GetObjectHandles response (data + ok) for a given transaction ID.
    fn queue_handles(mock: &MockTransport, tx_id: u32, handles: &[u32]) {
        let data = pack_u32_array(handles);
        mock.queue_response(data_container(
            tx_id,
            OperationCode::GetObjectHandles,
            &data,
        ));
        mock.queue_response(ok_response(tx_id));
    }

    /// Queue GetObjectInfo response (data + ok) for a given transaction ID.
    fn queue_object_info(mock: &MockTransport, tx_id: u32, filename: &str, parent: u32) {
        let data = object_info_bytes(filename, parent);
        mock.queue_response(data_container(tx_id, OperationCode::GetObjectInfo, &data));
        mock.queue_response(ok_response(tx_id));
    }

    // -- Tests ----------------------------------------------------------------

    #[tokio::test]
    async fn stream_returns_objects_with_correct_counters() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        queue_handles(&mock, 2, &[10, 20, 30]);
        queue_object_info(&mock, 3, "photo.jpg", 0);
        queue_object_info(&mock, 4, "video.mp4", 0);
        queue_object_info(&mock, 5, "notes.txt", 0);

        let storage = mock_storage(transport, "").await;
        let mut listing = storage.list_objects_stream(None).await.unwrap();

        assert_eq!(listing.total(), 3);
        assert_eq!(listing.fetched(), 0);

        let first = listing.next().await.unwrap().unwrap();
        assert_eq!(first.filename, "photo.jpg");
        assert_eq!(first.handle, ObjectHandle(10));
        assert_eq!(listing.fetched(), 1);

        let second = listing.next().await.unwrap().unwrap();
        assert_eq!(second.filename, "video.mp4");
        assert_eq!(second.handle, ObjectHandle(20));
        assert_eq!(listing.fetched(), 2);

        let third = listing.next().await.unwrap().unwrap();
        assert_eq!(third.filename, "notes.txt");
        assert_eq!(third.handle, ObjectHandle(30));
        assert_eq!(listing.fetched(), 3);

        assert!(listing.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_empty_directory() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession
        queue_handles(&mock, 2, &[]);

        let storage = mock_storage(transport, "").await;
        let mut listing = storage.list_objects_stream(None).await.unwrap();

        assert_eq!(listing.total(), 0);
        assert!(listing.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_filters_by_parent() {
        // Simulates Fuji quirk: device returns objects with wrong parent handles
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        queue_handles(&mock, 2, &[10, 20, 30]);
        queue_object_info(&mock, 3, "root_file.jpg", 0); // parent=ROOT, included
        queue_object_info(&mock, 4, "nested.jpg", 99); // parent=99, filtered out
        queue_object_info(&mock, 5, "another_root.txt", 0); // parent=ROOT, included

        let storage = mock_storage(transport, "").await;
        let mut listing = storage.list_objects_stream(None).await.unwrap();

        assert_eq!(listing.total(), 3); // Total handles from device

        let first = listing.next().await.unwrap().unwrap();
        assert_eq!(first.filename, "root_file.jpg");
        assert_eq!(listing.fetched(), 1);

        let second = listing.next().await.unwrap().unwrap();
        assert_eq!(second.filename, "another_root.txt");
        assert_eq!(listing.fetched(), 3); // Processed all 3 (including filtered one)

        assert!(listing.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_android_root_accepts_both_parents() {
        // Android quirk: root items may have parent 0 or 0xFFFFFFFF
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        queue_handles(&mock, 2, &[10, 20, 30]);
        queue_object_info(&mock, 3, "dcim", 0); // parent=0, root
        queue_object_info(&mock, 4, "download", 0xFFFFFFFF); // parent=ALL, also root on Android
        queue_object_info(&mock, 5, "nested", 42); // not root

        let storage = mock_storage(transport, "android.com").await;
        let mut listing = storage.list_objects_stream(None).await.unwrap();

        let first = listing.next().await.unwrap().unwrap();
        assert_eq!(first.filename, "dcim");

        let second = listing.next().await.unwrap().unwrap();
        assert_eq!(second.filename, "download");

        assert!(listing.next().await.is_none()); // "nested" filtered out
    }

    #[tokio::test]
    async fn stream_subfolder_listing() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        let parent_handle = 42u32;
        queue_handles(&mock, 2, &[100, 101]);
        queue_object_info(&mock, 3, "IMG_001.jpg", parent_handle);
        queue_object_info(&mock, 4, "IMG_002.jpg", parent_handle);

        let storage = mock_storage(transport, "").await;
        let mut listing = storage
            .list_objects_stream(Some(ObjectHandle(parent_handle)))
            .await
            .unwrap();

        assert_eq!(listing.total(), 2);
        let first = listing.next().await.unwrap().unwrap();
        assert_eq!(first.filename, "IMG_001.jpg");
        let second = listing.next().await.unwrap().unwrap();
        assert_eq!(second.filename, "IMG_002.jpg");
        assert!(listing.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_propagates_mid_listing_error() {
        let (transport, mock) = mock_transport();
        mock.queue_response(ok_response(1)); // OpenSession

        queue_handles(&mock, 2, &[10, 20]);
        queue_object_info(&mock, 3, "good.jpg", 0);
        // Handle 20: device returns error instead of object info
        mock.queue_response(error_response(4, ResponseCode::InvalidObjectHandle));

        let storage = mock_storage(transport, "").await;
        let mut listing = storage.list_objects_stream(None).await.unwrap();

        let first = listing.next().await.unwrap().unwrap();
        assert_eq!(first.filename, "good.jpg");

        let second = listing.next().await.unwrap();
        assert!(second.is_err());
    }

    #[tokio::test]
    async fn list_objects_matches_stream_collect() {
        // Verify list_objects() returns identical results to collecting the stream
        let (transport1, mock1) = mock_transport();
        let (transport2, mock2) = mock_transport();

        for mock in [&mock1, &mock2] {
            mock.queue_response(ok_response(1)); // OpenSession
            queue_handles(mock, 2, &[10, 20]);
            queue_object_info(mock, 3, "a.jpg", 0);
            queue_object_info(mock, 4, "b.txt", 0);
        }

        let storage1 = mock_storage(transport1, "").await;
        let storage2 = mock_storage(transport2, "").await;

        let all_at_once = storage1.list_objects(None).await.unwrap();

        let mut listing = storage2.list_objects_stream(None).await.unwrap();
        let mut streamed = Vec::new();
        while let Some(result) = listing.next().await {
            streamed.push(result.unwrap());
        }

        assert_eq!(all_at_once.len(), streamed.len());
        for (a, b) in all_at_once.iter().zip(streamed.iter()) {
            assert_eq!(a.filename, b.filename);
            assert_eq!(a.handle, b.handle);
        }
    }
}
