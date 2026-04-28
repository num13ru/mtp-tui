use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::backend::DeviceBackend;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusPane {
    Host,
    Device,
}

pub struct DeviceCache {
    pub name: String,
    pub path: String,
    pub storage_info: Option<(u64, u64)>,
}

pub enum ListingMsg {
    Progress {
        fetched: usize,
        total: usize,
    },
    Done {
        backend: Box<dyn DeviceBackend>,
        result: Result<Vec<DeviceEntry>>,
        storage_info: Option<(u64, u64)>,
        warning: Option<String>,
    },
    InitFailed(String),
}

pub struct LoadingState {
    pub rx: mpsc::Receiver<ListingMsg>,
    pub progress: Option<(usize, usize)>,
    pub spinner_tick: usize,
    pub cache: DeviceCache,
    pub selected_name: Option<String>,
}

pub enum DeviceState {
    Connecting {
        rx: mpsc::Receiver<ListingMsg>,
        spinner_tick: usize,
    },
    Loading(Box<LoadingState>),
    Connected {
        backend: Box<dyn DeviceBackend>,
        cache: DeviceCache,
    },
    Transferring {
        cache: DeviceCache,
    },
    Disconnected {
        error: Option<String>,
    },
}

impl DeviceState {
    pub fn is_loading(&self) -> bool {
        matches!(
            self,
            Self::Connecting { .. } | Self::Loading(_) | Self::Transferring { .. }
        )
    }

    pub fn tick_spinner(&mut self) {
        match self {
            Self::Connecting { spinner_tick, .. } => {
                *spinner_tick = spinner_tick.wrapping_add(1);
            }
            Self::Loading(state) => {
                state.spinner_tick = state.spinner_tick.wrapping_add(1);
            }
            _ => {}
        }
    }
}

pub enum TransferKind {
    Push {
        source: PathBuf,
        delete_id: Option<String>,
    },
    Pull {
        entry_id: String,
        filename: String,
        target_dir: PathBuf,
    },
}

pub enum TransferMsg {
    Done {
        backend: Box<dyn DeviceBackend>,
        result: Result<()>,
        storage_info: Option<(u64, u64)>,
    },
}

pub struct TransferDialog {
    pub rx: mpsc::Receiver<TransferMsg>,
    pub filename: String,
    pub direction: &'static str,
    pub spinner_tick: usize,
}

pub enum ActiveDialog {
    None,
    Confirm(ConfirmDialog),
    TextInput(TextInputDialog),
    Info(InfoDialog),
    Inspector(Box<InspectorData>),
    Transfer(TransferDialog),
}

pub struct InfoDialog {
    pub title: String,
    pub message: String,
}

pub struct ConfirmDialog {
    pub title: String,
    pub message: String,
    pub on_confirm: ConfirmAction,
}

pub enum ConfirmAction {
    OverwritePush { source: PathBuf, delete_id: String },
    OverwritePull { entry_id: String, filename: String },
    Delete { entry_id: String, name: String },
    Quit,
}

pub enum TextInputResult {
    Consumed,
    Submit(String),
    Cancel,
}

pub struct TextInputDialog {
    pub title: String,
    pub prompt: String,
    pub input: String,
    pub cursor_pos: usize,
    pub on_submit: TextInputAction,
}

impl TextInputDialog {
    pub fn handle_key(&mut self, key: KeyEvent) -> TextInputResult {
        match key.code {
            KeyCode::Esc => TextInputResult::Cancel,
            KeyCode::Enter => {
                let input = self.input.trim().to_string();
                if input.is_empty() {
                    TextInputResult::Cancel
                } else {
                    TextInputResult::Submit(input)
                }
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += c.len_utf8();
                TextInputResult::Consumed
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    let prev = self.input[..self.cursor_pos]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    self.cursor_pos -= prev;
                    self.input.remove(self.cursor_pos);
                }
                TextInputResult::Consumed
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
                TextInputResult::Consumed
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    let prev = self.input[..self.cursor_pos]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    self.cursor_pos -= prev;
                }
                TextInputResult::Consumed
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    let next = self.input[self.cursor_pos..]
                        .chars()
                        .next()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    self.cursor_pos += next;
                }
                TextInputResult::Consumed
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
                TextInputResult::Consumed
            }
            KeyCode::End => {
                self.cursor_pos = self.input.len();
                TextInputResult::Consumed
            }
            _ => TextInputResult::Consumed,
        }
    }
}

pub enum TextInputAction {
    Mkdir,
    Rename { entry_id: String },
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

pub struct InspectorProperty {
    pub code: u16,
    pub name: String,
    pub value: String,
    pub is_error: bool,
}

pub struct InspectorData {
    pub object_handle: String,
    pub filename: String,
    pub format: String,
    pub size: String,
    pub storage_id: String,
    pub parent_id: String,
    pub protection: String,
    pub created: Option<String>,
    pub modified: Option<String>,
    pub keywords: String,
    pub image_dimensions: Option<String>,
    pub thumb_dimensions: Option<String>,
    pub properties: Vec<InspectorProperty>,
    pub scroll_offset: usize,
}

pub struct PaneState<T> {
    pub entries: Vec<T>,
    pub selected: usize,
    cursor_name_stack: Vec<String>,
}

impl<T> PaneState<T> {
    pub fn new(entries: Vec<T>) -> Self {
        Self {
            entries,
            selected: 0,
            cursor_name_stack: Vec::new(),
        }
    }

    pub fn push_cursor(&mut self, name: String) {
        self.cursor_name_stack.push(name);
    }

    pub fn pop_cursor<F>(&mut self, name_of: F)
    where
        F: Fn(&T) -> &str,
    {
        if let Some(name) = self.cursor_name_stack.pop() {
            self.restore_selection_by_name(Some(&name), name_of);
        } else {
            self.clamp_selected();
        }
    }

    pub fn pop_cursor_name(&mut self) -> Option<String> {
        self.cursor_name_stack.pop()
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

    pub fn update_entries<F>(&mut self, new_entries: Vec<T>, name_of: F)
    where
        F: Fn(&T) -> &str,
    {
        let prev_name = self.selected().map(|e| name_of(e).to_owned());
        self.entries = new_entries;
        self.restore_selection_by_name(prev_name.as_deref(), name_of);
    }

    pub fn restore_selection_by_name<F>(&mut self, name: Option<&str>, name_of: F)
    where
        F: Fn(&T) -> &str,
    {
        if let Some(name) = name
            && let Some(pos) = self.entries.iter().position(|e| name_of(e) == name)
        {
            self.selected = pos;
            return;
        }
        self.clamp_selected();
    }

    pub fn clamp_selected(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.entries.len() - 1);
        }
    }
}
