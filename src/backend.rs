use std::cmp::Ordering;
use std::path::Path;

use anyhow::{Context, Result};
use mtp_rs::mtp::{MtpDevice, Storage};
use mtp_rs::ptp::ObjectHandle;

use crate::types::{DeviceEntry, DeviceEntryKind};

#[allow(dead_code)]
pub trait DeviceBackend: Send {
    fn device_name(&self) -> &str;
    fn current_path(&self) -> &str;
    fn list_current_dir(&self) -> Result<Vec<DeviceEntry>>;
    fn enter_dir(&mut self, entry_id: &str, name: &str) -> Result<()>;
    fn go_up(&mut self) -> Result<()>;
    fn refresh(&mut self) -> Result<()>;
    fn pull_file(&mut self, _entry_id: &str, _target_dir: &Path) -> Result<()> {
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
    _device: MtpDevice,
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
                anyhow::anyhow!(
                    "Another process holds the USB device.\n\
                     On macOS, ptpcamerad or Android File Transfer may auto-claim MTP devices.\n\
                     Try: sudo killall ptpcamerad\n\
                     Original error: {e}"
                )
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
            _device: device,
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

    fn list_current_dir(&self) -> Result<Vec<DeviceEntry>> {
        let parent = self.current_handle();
        let objects = self
            .rt
            .block_on(self.storage.list_objects(parent))
            .context("failed to list device directory")?;

        let mut entries: Vec<DeviceEntry> = objects
            .into_iter()
            .map(|obj| {
                let is_dir = obj.is_folder();
                DeviceEntry {
                    id: obj.handle.0.to_string(),
                    size: if is_dir { None } else { Some(obj.size) },
                    kind: if is_dir {
                        DeviceEntryKind::Directory
                    } else {
                        DeviceEntryKind::File
                    },
                    name: obj.filename,
                }
            })
            .collect();

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

    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
}
