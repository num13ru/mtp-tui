use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::backend::MtpBackend;
use crate::config::Config;
use crate::types::{
    ActiveDialog, ConfirmAction, ConfirmDialog, DeviceCache, DeviceEntry, DeviceEntryKind,
    DeviceState, FocusPane, HostEntry, InfoDialog, ListingMsg, LoadingState, PaneState,
    TextInputAction, TextInputDialog, TextInputResult, TransferDialog, TransferKind, TransferMsg,
};
use crate::ui::truncate_middle;

const DIALOG_FILENAME_MAX: usize = 40;

const MAX_MSGS_PER_TICK: usize = 1_000;

pub struct App {
    pub host_cwd: PathBuf,
    pub host: PaneState<HostEntry>,
    pub device_pane: PaneState<DeviceEntry>,
    pub focus: FocusPane,
    pub device_state: DeviceState,
    pub status: String,
    pub show_help: bool,
    pub dialog: ActiveDialog,
    pending_warning: Option<InfoDialog>,
    should_quit: bool,
}

impl App {
    pub fn new() -> Result<Self> {
        let config = Config::load();

        let host_dir_warning = match (config.host_dir(), config.default_host_dir.as_deref()) {
            (Some(_), _) => None,
            (None, Some(raw)) => Some(format!(
                "Can't access default_host_dir = \"{raw}\"\n\n\
                 Using current directory instead."
            )),
            (None, None) => None,
        };

        let host_cwd = match config.host_dir() {
            Some(dir) => dir,
            None => std::env::current_dir().context("failed to get current directory")?,
        };
        let host = PaneState::new(read_host_dir(&host_cwd)?);

        let default_device_dir = config.device_dir().map(String::from);

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = MtpBackend::new().and_then(|b| {
                let mut backend: Box<dyn crate::backend::DeviceBackend> = Box::new(b);
                let device_dir_warning = match default_device_dir {
                    Some(ref dir) => navigate_to_device_dir(&mut *backend, dir),
                    None => None,
                };
                let entries = backend.list_current_dir()?;
                let storage_info = backend.storage_info();
                Ok((backend, entries, storage_info, device_dir_warning))
            });
            match result {
                Ok((backend, entries, storage_info, warning)) => {
                    tx.send(ListingMsg::Done {
                        backend,
                        result: Ok(entries),
                        storage_info,
                        warning,
                    })
                    .ok();
                }
                Err(e) => {
                    tx.send(ListingMsg::InitFailed(format!("{e:#}"))).ok();
                }
            }
        });

        let dialog = match host_dir_warning {
            Some(message) => ActiveDialog::Info(InfoDialog {
                title: "Warning".into(),
                message,
            }),
            None => ActiveDialog::None,
        };

        Ok(Self {
            host_cwd,
            host,
            device_pane: PaneState::new(vec![]),
            focus: FocusPane::Host,
            device_state: DeviceState::Connecting {
                rx,
                spinner_tick: 0,
            },
            status: "Connecting to device…".into(),
            show_help: false,
            dialog,
            pending_warning: None,
            should_quit: false,
        })
    }

    pub fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        loop {
            terminal.draw(|frame| crate::ui::draw(&self, frame))?;

            if event::poll(Duration::from_millis(200))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.handle_key(key)?;
            }

            if self.should_quit {
                break;
            }

            self.poll_device_listing();
            self.poll_transfer();
            self.device_state.tick_spinner();
        }

        Ok(())
    }

    fn poll_device_listing(&mut self) {
        for _ in 0..MAX_MSGS_PER_TICK {
            let rx = match &self.device_state {
                DeviceState::Connecting { rx, .. } => rx,
                DeviceState::Loading(state) => &state.rx,
                _ => return,
            };

            let msg = match rx.try_recv() {
                Ok(m) => m,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.device_state = DeviceState::Disconnected { error: None };
                    self.status = "Error: device listing thread crashed".into();
                    return;
                }
            };

            match msg {
                ListingMsg::Progress { fetched, total } => {
                    if let DeviceState::Loading(state) = &mut self.device_state {
                        state.progress = Some((fetched, total));
                    }
                }
                ListingMsg::Done {
                    backend,
                    result,
                    storage_info,
                    warning,
                } => {
                    let was_connecting =
                        matches!(self.device_state, DeviceState::Connecting { .. });
                    let selected_name = match &self.device_state {
                        DeviceState::Loading(state) => state.selected_name.clone(),
                        _ => None,
                    };

                    let cache = DeviceCache {
                        name: backend.device_name().to_string(),
                        path: backend.current_path().to_string(),
                        storage_info,
                    };
                    if was_connecting {
                        self.status = format!("Connected to {}", cache.name);
                    }

                    self.device_state = DeviceState::Connected { backend, cache };
                    match result {
                        Ok(entries) => {
                            self.device_pane.entries = entries;
                            self.device_pane
                                .restore_selection_by_name(selected_name.as_deref(), |e| &e.name);
                        }
                        Err(e) => self.status = format!("Error: {e:#}"),
                    }

                    if was_connecting
                        && let Some(message) = warning
                    {
                        let info = InfoDialog {
                            title: "Warning".into(),
                            message,
                        };
                        if matches!(self.dialog, ActiveDialog::None) {
                            self.dialog = ActiveDialog::Info(info);
                        } else {
                            self.pending_warning = Some(info);
                        }
                    }
                }
                ListingMsg::InitFailed(msg) => {
                    self.device_state = DeviceState::Disconnected { error: Some(msg) };
                    self.status = "No device connected".into();
                }
            }
        }
    }

    fn spawn_device_listing(&mut self, selected_name: Option<String>) {
        debug_assert!(
            matches!(self.device_state, DeviceState::Connected { .. }),
            "spawn_device_listing called in non-Connected state"
        );
        if !matches!(self.device_state, DeviceState::Connected { .. }) {
            return;
        }
        let prev = std::mem::replace(
            &mut self.device_state,
            DeviceState::Disconnected { error: None },
        );
        let DeviceState::Connected { backend, cache } = prev else {
            unreachable!();
        };
        self.start_listing_thread(backend, cache, selected_name);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        match &self.dialog {
            ActiveDialog::Inspector(_) => {
                self.handle_inspector_key(key);
                return Ok(());
            }
            ActiveDialog::Info(_) => {
                self.dialog = match self.pending_warning.take() {
                    Some(next) => ActiveDialog::Info(next),
                    None => ActiveDialog::None,
                };
                return Ok(());
            }
            ActiveDialog::TextInput(_) => {
                self.handle_text_input_key(key);
                return Ok(());
            }
            ActiveDialog::Confirm(_) => {
                self.handle_confirm_key(key);
                return Ok(());
            }
            ActiveDialog::Transfer(_) => {
                if key.code == KeyCode::Char('c') && key.modifiers == KeyModifiers::CONTROL {
                    self.should_quit = true;
                }
                return Ok(());
            }
            ActiveDialog::None => {}
        }

        if self.device_state.is_loading() && self.focus == FocusPane::Device {
            self.handle_loading_key(key);
            return Ok(());
        }

        self.handle_normal_key(key)
    }

    fn handle_inspector_key(&mut self, key: KeyEvent) {
        let ActiveDialog::Inspector(ref mut data) = self.dialog else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('i') | KeyCode::Char('q') => {
                self.dialog = ActiveDialog::None;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                data.scroll_offset = data.scroll_offset.saturating_add(1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                data.scroll_offset = data.scroll_offset.saturating_sub(1);
            }
            _ => {}
        }
    }

    fn handle_text_input_key(&mut self, key: KeyEvent) {
        let ActiveDialog::TextInput(ref mut dialog) = self.dialog else {
            return;
        };
        match dialog.handle_key(key) {
            TextInputResult::Consumed => {}
            TextInputResult::Cancel => {
                self.status = "Cancelled".into();
                self.dialog = ActiveDialog::None;
            }
            TextInputResult::Submit(input) => {
                let action = std::mem::replace(&mut dialog.on_submit, TextInputAction::Mkdir);
                self.dialog = ActiveDialog::None;
                self.submit_text_input(action, &input);
            }
        }
    }

    fn handle_confirm_key(&mut self, key: KeyEvent) {
        let dialog = std::mem::replace(&mut self.dialog, ActiveDialog::None);
        let ActiveDialog::Confirm(dialog) = dialog else {
            return;
        };
        match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                self.execute_confirm(dialog.on_confirm);
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.status = "Cancelled".into();
            }
            _ => {
                self.dialog = ActiveDialog::Confirm(dialog);
            }
        }
    }

    fn execute_confirm(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::OverwritePush { source, delete_id } => {
                self.spawn_transfer(TransferKind::Push {
                    source,
                    delete_id: Some(delete_id),
                });
            }
            ConfirmAction::OverwritePull { entry_id, filename } => {
                self.spawn_transfer(TransferKind::Pull {
                    entry_id,
                    filename,
                    target_dir: self.host_cwd.clone(),
                });
            }
            ConfirmAction::Delete { entry_id, name } => {
                let DeviceState::Connected { backend, cache } = &mut self.device_state else {
                    self.status = "No device connected".into();
                    return;
                };
                match backend.delete(&entry_id) {
                    Ok(()) => {
                        cache.storage_info = backend.storage_info();
                        self.status = format!("Deleted {name}");
                        let sel = self.device_pane.selected().map(|e| e.name.clone());
                        self.spawn_device_listing(sel);
                    }
                    Err(e) => self.status = format!("Error: {e:#}"),
                }
            }
            ConfirmAction::Quit => {
                self.should_quit = true;
            }
        }
    }

    fn handle_loading_key(&mut self, key: KeyEvent) {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => self.confirm_quit(),
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.should_quit = true,
            (KeyCode::Tab, _) => self.toggle_focus(),
            (KeyCode::Char('?'), _) => self.show_help = !self.show_help,
            _ => {}
        }
    }

    fn handle_normal_key(&mut self, key: KeyEvent) -> Result<()> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => self.confirm_quit(),
            (KeyCode::Esc, _) if self.show_help => self.show_help = false,
            (KeyCode::Tab, _) => self.toggle_focus(),
            (KeyCode::Char('?'), _) => self.show_help = !self.show_help,
            (KeyCode::Char('r'), _) => {
                if let Err(e) = self.refresh() {
                    self.status = format!("Error: {e:#}");
                }
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => self.move_up(),
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => self.move_down(),
            (KeyCode::Enter, _) | (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
                if let Err(e) = self.enter_selected() {
                    self.status = format!("Error: {e:#}");
                }
            }
            (KeyCode::Backspace, _) | (KeyCode::Left, _) | (KeyCode::Char('h'), _) => {
                if let Err(e) = self.go_up() {
                    self.status = format!("Error: {e:#}");
                }
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.should_quit = true,
            (KeyCode::Char('p'), _) => {
                if let Err(e) = self.copy_host_to_device() {
                    self.status = format!("Error: {e:#}");
                }
            }
            (KeyCode::Char('g'), _) => {
                if let Err(e) = self.copy_device_to_host() {
                    self.status = format!("Error: {e:#}");
                }
            }
            (KeyCode::Char('d'), _) => self.delete_selected(),
            (KeyCode::Char('m'), _) => self.mkdir_prompt(),
            (KeyCode::Char('R'), _) => self.rename_prompt(),
            (KeyCode::Char('i'), _) => self.open_inspector(),
            _ => {}
        }

        Ok(())
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            FocusPane::Host => FocusPane::Device,
            FocusPane::Device => FocusPane::Host,
        };
    }

    fn move_up(&mut self) {
        match self.focus {
            FocusPane::Host => self.host.select_prev(),
            FocusPane::Device => self.device_pane.select_prev(),
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            FocusPane::Host => self.host.select_next(),
            FocusPane::Device => self.device_pane.select_next(),
        }
    }

    fn enter_selected(&mut self) -> Result<()> {
        match self.focus {
            FocusPane::Host => {
                let Some(entry) = self.host.selected().cloned() else {
                    return Ok(());
                };
                if entry.is_dir {
                    self.host.push_cursor(entry.name.clone());
                    self.host_cwd = entry.path;
                    self.host.entries = read_host_dir(&self.host_cwd)?;
                    self.host.selected = 0;
                    self.status = format!("Host: {}", self.host_cwd.display());
                }
            }
            FocusPane::Device => {
                let DeviceState::Connected { backend, cache } = &mut self.device_state else {
                    self.status = "No device connected".into();
                    return Ok(());
                };
                let Some(entry) = self.device_pane.selected().cloned() else {
                    return Ok(());
                };
                if entry.kind == DeviceEntryKind::Directory {
                    self.device_pane.push_cursor(entry.name.clone());
                    backend.enter_dir(&entry.id, &entry.name)?;
                    cache.path = backend.current_path().to_string();
                    self.status = format!("Device: {}", cache.path);
                    self.device_pane.selected = 0;
                    self.spawn_device_listing(None);
                }
            }
        }
        Ok(())
    }

    fn go_up(&mut self) -> Result<()> {
        match self.focus {
            FocusPane::Host => {
                if let Some(parent) = self.host_cwd.parent() {
                    self.host_cwd = parent.to_path_buf();
                    self.host.entries = read_host_dir(&self.host_cwd)?;
                    self.host.pop_cursor(|e| &e.name);
                    self.status = format!("Host: {}", self.host_cwd.display());
                }
            }
            FocusPane::Device => {
                let DeviceState::Connected { backend, cache } = &mut self.device_state else {
                    self.status = "No device connected".into();
                    return Ok(());
                };
                let pop_name = self.device_pane.pop_cursor_name();
                backend.go_up()?;
                cache.path = backend.current_path().to_string();
                self.status = format!("Device: {}", cache.path);
                self.spawn_device_listing(pop_name);
            }
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<()> {
        self.host
            .update_entries(read_host_dir(&self.host_cwd)?, |e| &e.name);
        if matches!(self.device_state, DeviceState::Connected { .. }) {
            let name = self.device_pane.selected().map(|e| e.name.clone());
            self.spawn_device_listing(name);
        }
        self.status = "Refreshed".into();
        Ok(())
    }

    fn copy_host_to_device(&mut self) -> Result<()> {
        if !matches!(self.device_state, DeviceState::Connected { .. }) {
            self.status = "No device connected".into();
            return Ok(());
        }
        let Some(entry) = self.host.selected() else {
            return Ok(());
        };
        if entry.is_dir {
            self.status = "Skipping directory push for now".into();
            return Ok(());
        }

        let filename = &entry.name;
        let existing = self
            .device_pane
            .entries
            .iter()
            .find(|d| d.name == *filename && d.kind == DeviceEntryKind::File);

        if let Some(existing) = existing {
            self.dialog = ActiveDialog::Confirm(ConfirmDialog {
                title: "Overwrite?".into(),
                message: format!(
                    "\"{}\" already exists on device. Overwrite?",
                    truncate_middle(filename, DIALOG_FILENAME_MAX),
                ),
                on_confirm: ConfirmAction::OverwritePush {
                    source: entry.path.clone(),
                    delete_id: existing.id.clone(),
                },
            });
            return Ok(());
        }

        let path = entry.path.clone();
        self.spawn_transfer(TransferKind::Push {
            source: path,
            delete_id: None,
        });
        Ok(())
    }

    fn copy_device_to_host(&mut self) -> Result<()> {
        if !matches!(self.device_state, DeviceState::Connected { .. }) {
            self.status = "No device connected".into();
            return Ok(());
        }
        let Some(entry) = self.device_pane.selected() else {
            return Ok(());
        };
        if entry.kind == DeviceEntryKind::Directory {
            self.status = "Skipping directory pull for now".into();
            return Ok(());
        }

        let entry_id = entry.id.clone();
        let filename = entry.name.clone();

        if self.host_cwd.join(&filename).exists() {
            self.dialog = ActiveDialog::Confirm(ConfirmDialog {
                title: "Overwrite?".into(),
                message: format!(
                    "\"{}\" already exists on host. Overwrite?",
                    truncate_middle(&filename, DIALOG_FILENAME_MAX),
                ),
                on_confirm: ConfirmAction::OverwritePull { entry_id, filename },
            });
            return Ok(());
        }

        self.spawn_transfer(TransferKind::Pull {
            entry_id,
            filename,
            target_dir: self.host_cwd.clone(),
        });
        Ok(())
    }

    fn submit_text_input(&mut self, action: TextInputAction, input: &str) {
        match action {
            TextInputAction::Mkdir => {
                let DeviceState::Connected { backend, cache } = &mut self.device_state else {
                    self.status = "No device connected".into();
                    return;
                };
                match backend.mkdir(input) {
                    Ok(()) => {
                        cache.storage_info = backend.storage_info();
                        self.status = format!("Created directory {input}");
                        let sel = self.device_pane.selected().map(|e| e.name.clone());
                        self.spawn_device_listing(sel);
                    }
                    Err(e) => self.status = format!("Error: {e:#}"),
                }
            }
            TextInputAction::Rename { entry_id } => {
                let DeviceState::Connected { backend, .. } = &mut self.device_state else {
                    self.status = "No device connected".into();
                    return;
                };
                match backend.rename(&entry_id, input) {
                    Ok(()) => {
                        self.status = format!("Renamed to {input}");
                        let sel = self.device_pane.selected().map(|e| e.name.clone());
                        self.spawn_device_listing(sel);
                    }
                    Err(e) => self.status = format!("Error: {e:#}"),
                }
            }
        }
    }

    fn confirm_quit(&mut self) {
        self.dialog = ActiveDialog::Confirm(ConfirmDialog {
            title: "Quit?".into(),
            message: "Are you sure you want to quit?".into(),
            on_confirm: ConfirmAction::Quit,
        });
    }

    fn rename_prompt(&mut self) {
        if self.focus != FocusPane::Device {
            return;
        }
        if !matches!(self.device_state, DeviceState::Connected { .. }) {
            self.status = "No device connected".into();
            return;
        }
        let Some(entry) = self.device_pane.selected() else {
            return;
        };
        let cursor_pos = entry.name.len();
        self.dialog = ActiveDialog::TextInput(TextInputDialog {
            title: "Rename".into(),
            prompt: format!(
                "Rename \"{}\" to:",
                truncate_middle(&entry.name, DIALOG_FILENAME_MAX),
            ),
            input: entry.name.clone(),
            cursor_pos,
            on_submit: TextInputAction::Rename {
                entry_id: entry.id.clone(),
            },
        });
    }

    fn mkdir_prompt(&mut self) {
        if self.focus != FocusPane::Device {
            return;
        }
        if !matches!(self.device_state, DeviceState::Connected { .. }) {
            self.status = "No device connected".into();
            return;
        }
        self.dialog = ActiveDialog::TextInput(TextInputDialog {
            title: "Create Directory".into(),
            prompt: "Directory name:".into(),
            input: String::new(),
            cursor_pos: 0,
            on_submit: TextInputAction::Mkdir,
        });
    }

    fn delete_selected(&mut self) {
        if self.focus != FocusPane::Device {
            return;
        }
        if !matches!(self.device_state, DeviceState::Connected { .. }) {
            self.status = "No device connected".into();
            return;
        }
        let Some(entry) = self.device_pane.selected() else {
            return;
        };
        let kind = match entry.kind {
            DeviceEntryKind::Directory => "directory",
            DeviceEntryKind::File => "file",
        };
        self.dialog = ActiveDialog::Confirm(ConfirmDialog {
            title: "Delete?".into(),
            message: format!(
                "Delete {kind} \"{}\"?",
                truncate_middle(&entry.name, DIALOG_FILENAME_MAX),
            ),
            on_confirm: ConfirmAction::Delete {
                entry_id: entry.id.clone(),
                name: entry.name.clone(),
            },
        });
    }

    fn open_inspector(&mut self) {
        if self.focus != FocusPane::Device {
            self.dialog = ActiveDialog::Info(InfoDialog {
                title: "Inspector".into(),
                message: "Inspector is only available for device files (MTP objects).\n\
                          Switch to the device pane with Tab first."
                    .into(),
            });
            return;
        }
        let DeviceState::Connected { backend, .. } = &self.device_state else {
            self.status = "No device connected".into();
            return;
        };
        let Some(entry) = self.device_pane.selected() else {
            return;
        };
        let entry_id = entry.id.clone();
        let entry_name = entry.name.clone();
        self.status = format!("Inspecting {entry_name}...");
        match backend.inspect_object(&entry_id) {
            Ok(data) => {
                self.status = format!("Inspector: {entry_name}");
                self.dialog = ActiveDialog::Inspector(Box::new(data));
            }
            Err(e) => {
                self.status = format!("Error: {e:#}");
            }
        }
    }

    fn spawn_transfer(&mut self, kind: TransferKind) {
        if !matches!(self.device_state, DeviceState::Connected { .. }) {
            self.status = "No device connected".into();
            return;
        }
        let prev = std::mem::replace(
            &mut self.device_state,
            DeviceState::Disconnected { error: None },
        );
        let DeviceState::Connected { backend, cache } = prev else {
            unreachable!();
        };

        let (filename, direction) = match &kind {
            TransferKind::Push { source, .. } => (
                source
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "Pushing",
            ),
            TransferKind::Pull { filename, .. } => (filename.clone(), "Pulling"),
        };

        let (tx, rx) = mpsc::channel();
        self.device_state = DeviceState::Transferring { cache };
        self.dialog = ActiveDialog::Transfer(TransferDialog {
            rx,
            filename: filename.clone(),
            direction,
            spinner_tick: 0,
        });
        self.status = format!("{direction} {filename}...");

        thread::spawn(move || {
            let mut backend = backend;
            let result = match kind {
                TransferKind::Push { source, delete_id } => {
                    let mut r = Ok(());
                    if let Some(ref id) = delete_id {
                        r = backend.delete(id);
                    }
                    if r.is_ok() {
                        r = backend.push_file(&source);
                    }
                    r
                }
                TransferKind::Pull {
                    entry_id,
                    filename,
                    target_dir,
                } => backend.pull_file(&entry_id, &filename, &target_dir),
            };
            let storage_info = backend.storage_info();
            tx.send(TransferMsg::Done {
                backend,
                result,
                storage_info,
            })
            .ok();
        });
    }

    fn poll_transfer(&mut self) {
        let ActiveDialog::Transfer(ref mut dialog) = self.dialog else {
            return;
        };
        dialog.spinner_tick = dialog.spinner_tick.wrapping_add(1);

        let msg = match dialog.rx.try_recv() {
            Ok(m) => m,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.device_state = DeviceState::Disconnected { error: None };
                self.dialog = ActiveDialog::None;
                self.status = "Error: transfer thread crashed".into();
                return;
            }
        };

        let TransferMsg::Done {
            backend,
            result,
            storage_info,
        } = msg;

        let is_pull = dialog.direction == "Pulling";
        let filename = dialog.filename.clone();

        let mut cache = match std::mem::replace(
            &mut self.device_state,
            DeviceState::Disconnected { error: None },
        ) {
            DeviceState::Transferring { cache } => cache,
            other => {
                self.device_state = other;
                self.dialog = ActiveDialog::None;
                self.status = "Error: unexpected state after transfer".into();
                return;
            }
        };
        cache.storage_info = storage_info;

        self.device_state = DeviceState::Connected { backend, cache };
        self.dialog = ActiveDialog::None;

        match result {
            Ok(()) => {
                self.status = format!("{} {filename}", if is_pull { "Pulled" } else { "Pushed" });
                if is_pull {
                    if let Ok(entries) = read_host_dir(&self.host_cwd) {
                        self.host.update_entries(entries, |e| &e.name);
                    }
                } else {
                    let sel = self.device_pane.selected().map(|e| e.name.clone());
                    self.spawn_device_listing(sel);
                }
            }
            Err(e) => self.status = format!("Error: {e:#}"),
        }
    }

    fn start_listing_thread(
        &mut self,
        backend: Box<dyn crate::backend::DeviceBackend>,
        cache: DeviceCache,
        selected_name: Option<String>,
    ) {
        let (tx, rx) = mpsc::channel();
        self.device_state = DeviceState::Loading(Box::new(LoadingState {
            rx,
            progress: None,
            spinner_tick: 0,
            cache,
            selected_name,
        }));

        thread::spawn(move || {
            let mut backend = backend;
            let progress_tx = tx.clone();
            let result = backend.list_current_dir_with_progress(&|fetched, total| {
                progress_tx
                    .send(ListingMsg::Progress { fetched, total })
                    .ok();
            });
            let storage_info = if result.is_ok() {
                backend.refresh_storage_info()
            } else {
                backend.storage_info()
            };
            tx.send(ListingMsg::Done {
                backend,
                result,
                storage_info,
                warning: None,
            })
            .ok();
        });
    }
}

pub fn read_host_dir(path: &Path) -> Result<Vec<HostEntry>> {
    let mut entries = fs::read_dir(path)
        .with_context(|| format!("failed to read directory: {}", path.display()))?
        .filter_map(|result| result.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            let is_dir = metadata.is_dir();
            let size = if metadata.is_file() {
                Some(metadata.len())
            } else {
                None
            };
            Some(HostEntry {
                name: entry.file_name().to_string_lossy().to_string(),
                path,
                is_dir,
                size,
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    Ok(entries)
}

/// Walk into `device_dir` segment by segment (e.g. "/Download/Books").
/// Returns a warning message if the full path couldn't be reached.
fn navigate_to_device_dir(
    backend: &mut dyn crate::backend::DeviceBackend,
    device_dir: &str,
) -> Option<String> {
    let segments: Vec<&str> = device_dir.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return None;
    }

    for (i, segment) in segments.iter().enumerate() {
        let entries = match backend.list_current_dir() {
            Ok(e) => e,
            Err(_) => return Some(device_dir_warning(device_dir, &segments[..i])),
        };
        let Some(entry) = entries
            .iter()
            .find(|e| e.kind == DeviceEntryKind::Directory && e.name == *segment)
        else {
            return Some(device_dir_warning(device_dir, &segments[..i]));
        };
        if backend.enter_dir(&entry.id, &entry.name).is_err() {
            return Some(device_dir_warning(device_dir, &segments[..i]));
        }
    }
    None
}

fn device_dir_warning(configured: &str, reached_segments: &[&str]) -> String {
    let reached = if reached_segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", reached_segments.join("/"))
    };
    format!(
        "Can't access default_device_dir = \"{configured}\"\n\n\
         Opened \"{reached}\" instead."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_dir_warning_no_segments_reached() {
        let msg = device_dir_warning("/Download/Books", &[]);
        assert!(msg.contains("default_device_dir = \"/Download/Books\""));
        assert!(msg.contains("Opened \"/\" instead."));
    }

    #[test]
    fn device_dir_warning_partial_path_reached() {
        let msg = device_dir_warning("/Download/Books/Fiction", &["Download"]);
        assert!(msg.contains("default_device_dir = \"/Download/Books/Fiction\""));
        assert!(msg.contains("Opened \"/Download\" instead."));
    }

    #[test]
    fn device_dir_warning_two_segments_reached() {
        let msg = device_dir_warning("/A/B/C", &["A", "B"]);
        assert!(msg.contains("Opened \"/A/B\" instead."));
    }
}
