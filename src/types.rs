use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Host,
    Device,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceEntryKind {
    Directory,
    File,
}

#[derive(Debug, Clone)]
pub struct HostEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct DeviceEntry {
    pub id: String,
    pub name: String,
    pub kind: DeviceEntryKind,
    pub size: Option<u64>,
}

pub struct PaneState<T> {
    pub entries: Vec<T>,
    pub selected: usize,
}

impl<T> PaneState<T> {
    pub fn new(entries: Vec<T>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    pub fn select_next(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
        } else {
            self.selected = (self.selected + 1).min(self.entries.len() - 1);
        }
    }

    pub fn select_prev(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    pub fn selected(&self) -> Option<&T> {
        self.entries.get(self.selected)
    }
}
