use std::cmp::Ordering;
use std::fs;
use std::io::{Read, Write};
use std::ops::ControlFlow;
use std::path::Path;

use anyhow::{Context, Result};
use bytes::Bytes;
use mtp_rs::mtp::{MtpDevice, NewObjectInfo, Storage};
use mtp_rs::ptp::ObjectHandle;

use crate::types::{DeviceEntry, DeviceEntryKind};

pub trait DeviceBackend: Send {
    fn device_name(&self) -> &str;
    fn current_path(&self) -> &str;
    fn list_current_dir_with_progress(
        &self,
        on_progress: &dyn Fn(usize, usize),
    ) -> Result<Vec<DeviceEntry>>;
    fn list_current_dir(&self) -> Result<Vec<DeviceEntry>> {
        self.list_current_dir_with_progress(&|_, _| {})
    }
    fn enter_dir(&mut self, entry_id: &str, name: &str) -> Result<()>;
    fn go_up(&mut self) -> Result<()>;
    fn pull_file(&mut self, _entry_id: &str, _filename: &str, _target_dir: &Path) -> Result<()> {
        anyhow::bail!("pull_file is not implemented yet")
    }
    fn push_file(&mut self, _source: &Path) -> Result<()> {
        anyhow::bail!("push_file is not implemented yet")
    }
    fn mkdir(&mut self, _name: &str) -> Result<()> {
        anyhow::bail!("mkdir is not implemented yet")
    }
    fn delete(&mut self, _entry_id: &str) -> Result<()> {
        anyhow::bail!("delete is not implemented yet")
    }
    fn rename(&mut self, _entry_id: &str, _new_name: &str) -> Result<()> {
        anyhow::bail!("rename is not implemented yet")
    }
}

pub struct MtpBackend {
    rt: tokio::runtime::Runtime,
    device: MtpDevice,
    storage: Storage,
    device_name_cached: String,
    current_path_cached: String,
    path_stack: Vec<(Option<ObjectHandle>, String)>,
}

impl MtpBackend {
    pub fn new() -> Result<Self> {
        let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;

        let device = rt.block_on(MtpDevice::open_first()).map_err(|e| {
            if e.is_exclusive_access() {
                #[cfg(target_os = "macos")]
                let hint = "\nOn macOS, ptpcamerad or Android File Transfer may \
                            auto-claim MTP devices.\nTry: sudo killall ptpcamerad";
                #[cfg(not(target_os = "macos"))]
                let hint = "";

                anyhow::anyhow!("Another process holds the USB device.{hint}\nOriginal error: {e}")
            } else {
                anyhow::anyhow!("Failed to open MTP device: {e}")
            }
        })?;

        let info = device.device_info();
        let device_name = format!("{} {}", info.manufacturer, info.model);

        let storages = rt
            .block_on(device.storages())
            .context("failed to list device storages")?;
        let storage = storages
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no storage found on device"))?;

        Ok(Self {
            rt,
            device,
            storage,
            device_name_cached: device_name,
            current_path_cached: "/".into(),
            path_stack: vec![(None, "/".into())],
        })
    }

    fn current_handle(&self) -> Option<ObjectHandle> {
        self.path_stack.last().and_then(|(h, _)| *h)
    }

    fn rebuild_path(&mut self) {
        if self.path_stack.len() <= 1 {
            self.current_path_cached = "/".into();
        } else {
            let mut path = String::new();
            for (_, name) in &self.path_stack[1..] {
                path.push('/');
                path.push_str(name);
            }
            self.current_path_cached = path;
        }
    }
}

pub fn sort_device_entries(entries: &mut Vec<DeviceEntry>) {
    entries.sort_by(|a, b| match (a.kind, b.kind) {
        (DeviceEntryKind::Directory, DeviceEntryKind::File) => Ordering::Less,
        (DeviceEntryKind::File, DeviceEntryKind::Directory) => Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });
}

impl DeviceBackend for MtpBackend {
    fn device_name(&self) -> &str {
        &self.device_name_cached
    }

    fn current_path(&self) -> &str {
        &self.current_path_cached
    }

    fn list_current_dir_with_progress(
        &self,
        on_progress: &dyn Fn(usize, usize),
    ) -> Result<Vec<DeviceEntry>> {
        let parent = self.current_handle();
        let mut listing = self
            .rt
            .block_on(self.storage.list_objects_stream(parent))
            .context("failed to list device directory")?;

        let total = listing.total();
        on_progress(0, total);

        let mut entries = Vec::with_capacity(total);
        while let Some(result) = self.rt.block_on(listing.next()) {
            let obj = result.context("failed to get object info")?;
            let is_dir = obj.is_folder();
            entries.push(DeviceEntry {
                id: obj.handle.0.to_string(),
                size: if is_dir { None } else { Some(obj.size) },
                kind: if is_dir {
                    DeviceEntryKind::Directory
                } else {
                    DeviceEntryKind::File
                },
                name: obj.filename,
            });
            on_progress(listing.fetched(), total);
        }

        sort_device_entries(&mut entries);

        Ok(entries)
    }

    fn enter_dir(&mut self, entry_id: &str, name: &str) -> Result<()> {
        let handle_raw: u32 = entry_id
            .parse()
            .with_context(|| format!("invalid object handle: {entry_id}"))?;

        self.path_stack
            .push((Some(ObjectHandle(handle_raw)), name.to_string()));
        self.rebuild_path();
        Ok(())
    }

    fn go_up(&mut self) -> Result<()> {
        if self.path_stack.len() > 1 {
            self.path_stack.pop();
            self.rebuild_path();
        }
        Ok(())
    }

    fn push_file(&mut self, source: &Path) -> Result<()> {
        let filename = source
            .file_name()
            .context("source path has no filename")?
            .to_string_lossy()
            .into_owned();
        let metadata = fs::metadata(source)
            .with_context(|| format!("failed to read metadata: {}", source.display()))?;
        let file_size = metadata.len();

        const CHUNK_SIZE: usize = 256 * 1024;
        let mut file = std::io::BufReader::new(
            fs::File::open(source)
                .with_context(|| format!("failed to open: {}", source.display()))?,
        );

        let chunks: Vec<Result<Bytes, std::io::Error>> = std::iter::from_fn(move || {
            let mut buf = vec![0u8; CHUNK_SIZE];
            match file.read(&mut buf) {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    Some(Ok(Bytes::from(buf)))
                }
                Err(e) => Some(Err(e)),
            }
        })
        .collect();

        let stream = futures::stream::iter(chunks);
        let info = NewObjectInfo::file(&filename, file_size);
        let parent = self.current_handle();

        self.rt
            .block_on(self.storage.upload_with_progress(
                parent,
                info,
                stream,
                |_progress| ControlFlow::Continue(()),
            ))
            .with_context(|| format!("failed to upload {filename}"))?;

        Ok(())
    }

    fn pull_file(&mut self, entry_id: &str, filename: &str, target_dir: &Path) -> Result<()> {
        let handle_raw: u32 = entry_id
            .parse()
            .with_context(|| format!("invalid object handle: {entry_id}"))?;
        let handle = ObjectHandle(handle_raw);
        let target_path = target_dir.join(filename);

        let mut download = self
            .rt
            .block_on(self.storage.download_stream(handle))
            .with_context(|| format!("failed to start download of {filename}"))?;

        let file = fs::File::create(&target_path)
            .with_context(|| format!("failed to create: {}", target_path.display()))?;
        let mut writer = std::io::BufWriter::new(file);

        while let Some(result) = self.rt.block_on(download.next_chunk()) {
            let chunk = result.with_context(|| format!("error downloading {filename}"))?;
            writer
                .write_all(&chunk)
                .with_context(|| format!("failed to write to {}", target_path.display()))?;
        }

        writer
            .flush()
            .with_context(|| format!("failed to flush {}", target_path.display()))?;

        Ok(())
    }

    fn mkdir(&mut self, name: &str) -> Result<()> {
        let parent = self.current_handle();
        self.rt
            .block_on(self.storage.create_folder(parent, name))
            .with_context(|| format!("failed to create directory {name}"))?;
        Ok(())
    }

    fn delete(&mut self, entry_id: &str) -> Result<()> {
        let handle_raw: u32 = entry_id
            .parse()
            .with_context(|| format!("invalid object handle: {entry_id}"))?;
        self.rt
            .block_on(self.storage.delete(ObjectHandle(handle_raw)))
            .with_context(|| format!("failed to delete object {entry_id}"))?;
        Ok(())
    }

    fn rename(&mut self, entry_id: &str, new_name: &str) -> Result<()> {
        if !self.device.supports_rename() {
            anyhow::bail!("device does not support renaming");
        }
        let handle_raw: u32 = entry_id
            .parse()
            .with_context(|| format!("invalid object handle: {entry_id}"))?;
        self.rt
            .block_on(self.storage.rename(ObjectHandle(handle_raw), new_name))
            .with_context(|| format!("failed to rename object {entry_id}"))?;
        Ok(())
    }
}
