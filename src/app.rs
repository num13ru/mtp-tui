use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::backend::{DeviceBackend, MtpBackend};
use crate::types::{
    ConfirmAction, ConfirmDialog, DeviceEntry, DeviceEntryKind, FocusPane, HostEntry, PaneState,
    TextInputAction, TextInputDialog,
};

enum ListingMsg {
    Progress { fetched: usize, total: usize },
    Done {
        backend: Box<dyn DeviceBackend>,
        result: Result<Vec<DeviceEntry>>,
    },
    InitFailed(String),
}

pub struct App {
    pub host_cwd: PathBuf,
    pub host: PaneState<HostEntry>,
    pub device: PaneState<DeviceEntry>,
    pub focus: FocusPane,
    pub backend: Option<Box<dyn DeviceBackend>>,
    pub device_error: Option<String>,
    pub device_name_cached: String,
    pub device_path_cached: String,
    pub status: String,
    pub show_help: bool,
    pub confirm_dialog: Option<ConfirmDialog>,
    pub text_input_dialog: Option<TextInputDialog>,
    pub device_loading: bool,
    pub device_connecting: bool,
    pub loading_progress: Option<(usize, usize)>,
    pub spinner_tick: usize,
    should_quit: bool,
    last_tick: Instant,
    dir_rx: Option<mpsc::Receiver<ListingMsg>>,
    device_selected_name: Option<String>,
}

impl App {
    pub fn new() -> Result<Self> {
        let host_cwd = std::env::current_dir().context("failed to get current directory")?;
        let host = PaneState::new(Self::read_host_dir(&host_cwd)?);

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let result = MtpBackend::new().and_then(|b| {
                let backend: Box<dyn DeviceBackend> = Box::new(b);
                let entries = backend.list_current_dir()?;
                Ok((backend, entries))
            });
            match result {
                Ok((backend, entries)) => {
                    tx.send(ListingMsg::Done {
                        backend,
                        result: Ok(entries),
                    })
                    .ok();
                }
                Err(e) => {
                    tx.send(ListingMsg::InitFailed(format!("{e:#}"))).ok();
                }
            }
        });

        Ok(Self {
            host_cwd,
            host,
            device: PaneState::new(vec![]),
            focus: FocusPane::Host,
            backend: None,
            device_error: None,
            device_name_cached: String::new(),
            device_path_cached: String::new(),
            status: "Connecting to device…".into(),
            show_help: false,
            confirm_dialog: None,
            text_input_dialog: None,
            device_loading: true,
            device_connecting: true,
            loading_progress: None,
            spinner_tick: 0,
            should_quit: false,
            last_tick: Instant::now(),
            dir_rx: Some(rx),
            device_selected_name: None,
        })
    }

    pub fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;

            let timeout = Duration::from_millis(200);
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        self.handle_key(key)?;
                    }
                    _ => {}
                }
            }

            if self.should_quit {
                break;
            }

            self.poll_device_listing();

            if self.device_loading {
                self.spinner_tick = self.spinner_tick.wrapping_add(1);
            }

            if self.last_tick.elapsed() >= Duration::from_secs(5) {
                self.last_tick = Instant::now();
            }
        }

        Ok(())
    }

    fn poll_device_listing(&mut self) {
        let Some(rx) = &self.dir_rx else { return };

        loop {
            let msg = match rx.try_recv() {
                Ok(m) => m,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.device_loading = false;
                    self.loading_progress = None;
                    self.dir_rx = None;
                    self.status = "Error: device listing thread crashed".into();
                    return;
                }
            };

            match msg {
                ListingMsg::Progress { fetched, total } => {
                    self.loading_progress = Some((fetched, total));
                }
                ListingMsg::Done { backend, result } => {
                    let was_connecting = self.device_connecting;
                    self.device_connecting = false;
                    self.device_name_cached = backend.device_name().to_string();
                    self.device_path_cached = backend.current_path().to_string();
                    if was_connecting {
                        self.status = format!("Connected to {}", self.device_name_cached);
                    }
                    self.backend = Some(backend);
                    self.device_loading = false;
                    self.loading_progress = None;
                    self.dir_rx = None;
                    match result {
                        Ok(entries) => {
                            self.device.entries = entries;
                            self.device.restore_selection_by_name(
                                self.device_selected_name.as_deref(),
                                |e| &e.name,
                            );
                            self.device_selected_name = None;
                        }
                        Err(e) => self.status = format!("Error: {e:#}"),
                    }
                    return;
                }
                ListingMsg::InitFailed(msg) => {
                    self.device_connecting = false;
                    self.device_loading = false;
                    self.loading_progress = None;
                    self.dir_rx = None;
                    self.device_error = Some(msg);
                    self.status = "No device connected".into();
                    return;
                }
            }
        }
    }

    fn spawn_device_listing(&mut self) {
        self.spawn_device_listing_inner(true);
    }

    fn spawn_device_listing_preserving_selection(&mut self) {
        self.spawn_device_listing_inner(false);
    }

    fn spawn_device_listing_inner(&mut self, reset_selection: bool) {
        let Some(backend) = self.backend.take() else {
            return;
        };

        if reset_selection {
            self.device_selected_name = None;
        } else {
            self.device_selected_name =
                self.device.selected().map(|e| e.name.clone());
        }

        self.device_loading = true;
        self.loading_progress = None;
        self.spinner_tick = 0;
        self.device.entries.clear();

        let (tx, rx) = mpsc::channel();
        self.dir_rx = Some(rx);

        thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = backend.list_current_dir_with_progress(&|fetched, total| {
                progress_tx
                    .send(ListingMsg::Progress { fetched, total })
                    .ok();
            });
            tx.send(ListingMsg::Done { backend, result }).ok();
        });
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(mut dialog) = self.text_input_dialog.take() {
            match key.code {
                KeyCode::Esc => {
                    self.status = "Cancelled".into();
                }
                KeyCode::Enter => {
                    let input = dialog.input.trim().to_string();
                    if input.is_empty() {
                        self.status = "Empty name, cancelled".into();
                    } else {
                        self.submit_text_input(dialog.on_submit, &input);
                    }
                }
                KeyCode::Char(c) => {
                    dialog.input.insert(dialog.cursor_pos, c);
                    dialog.cursor_pos += c.len_utf8();
                    self.text_input_dialog = Some(dialog);
                }
                KeyCode::Backspace => {
                    if dialog.cursor_pos > 0 {
                        let prev = dialog.input[..dialog.cursor_pos]
                            .chars()
                            .last()
                            .map(|c| c.len_utf8())
                            .unwrap_or(0);
                        dialog.cursor_pos -= prev;
                        dialog.input.remove(dialog.cursor_pos);
                    }
                    self.text_input_dialog = Some(dialog);
                }
                KeyCode::Delete => {
                    if dialog.cursor_pos < dialog.input.len() {
                        dialog.input.remove(dialog.cursor_pos);
                    }
                    self.text_input_dialog = Some(dialog);
                }
                KeyCode::Left => {
                    if dialog.cursor_pos > 0 {
                        let prev = dialog.input[..dialog.cursor_pos]
                            .chars()
                            .last()
                            .map(|c| c.len_utf8())
                            .unwrap_or(0);
                        dialog.cursor_pos -= prev;
                    }
                    self.text_input_dialog = Some(dialog);
                }
                KeyCode::Right => {
                    if dialog.cursor_pos < dialog.input.len() {
                        let next = dialog.input[dialog.cursor_pos..]
                            .chars()
                            .next()
                            .map(|c| c.len_utf8())
                            .unwrap_or(0);
                        dialog.cursor_pos += next;
                    }
                    self.text_input_dialog = Some(dialog);
                }
                KeyCode::Home => {
                    dialog.cursor_pos = 0;
                    self.text_input_dialog = Some(dialog);
                }
                KeyCode::End => {
                    dialog.cursor_pos = dialog.input.len();
                    self.text_input_dialog = Some(dialog);
                }
                _ => {
                    self.text_input_dialog = Some(dialog);
                }
            }
            return Ok(());
        }

        if let Some(dialog) = self.confirm_dialog.take() {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                    match dialog.on_confirm {
                        ConfirmAction::OverwritePush { source, delete_id } => {
                            if let Err(e) = self.do_push_file(&source, Some(&delete_id)) {
                                self.status = format!("Error: {e:#}");
                            }
                        }
                        ConfirmAction::OverwritePull { entry_id, filename } => {
                            if let Err(e) = self.do_pull_file(&entry_id, &filename) {
                                self.status = format!("Error: {e:#}");
                            }
                        }
                        ConfirmAction::Delete { entry_id, name } => {
                            match self.backend.as_mut() {
                                Some(backend) => match backend.delete(&entry_id) {
                                    Ok(()) => {
                                        self.status = format!("Deleted {name}");
                                        self.spawn_device_listing_preserving_selection();
                                    }
                                    Err(e) => self.status = format!("Error: {e:#}"),
                                },
                                None => self.status = "No device connected".into(),
                            }
                        }
                        ConfirmAction::Quit => {
                            self.should_quit = true;
                        }
                    }
                }
                KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.status = "Cancelled".into();
                }
                _ => {
                    self.confirm_dialog = Some(dialog);
                }
            }
            return Ok(());
        }

        if self.device_loading && self.focus == FocusPane::Device {
            match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _) => self.confirm_quit(),
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => self.should_quit = true,
                (KeyCode::Tab, _) => self.toggle_focus(),
                (KeyCode::Char('?'), _) => self.show_help = !self.show_help,
                _ => {}
            }
            return Ok(());
        }

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
            FocusPane::Device => self.device.select_prev(),
        }
    }

    fn move_down(&mut self) {
        match self.focus {
            FocusPane::Host => self.host.select_next(),
            FocusPane::Device => self.device.select_next(),
        }
    }

    fn enter_selected(&mut self) -> Result<()> {
        match self.focus {
            FocusPane::Host => {
                let Some(entry) = self.host.selected().cloned() else {
                    return Ok(());
                };
                if entry.is_dir {
                    self.host_cwd = entry.path;
                    self.host.entries = Self::read_host_dir(&self.host_cwd)?;
                    self.host.selected = 0;
                    self.status = format!("Host: {}", self.host_cwd.display());
                }
            }
            FocusPane::Device => {
                let Some(backend) = &mut self.backend else {
                    self.status = "No device connected".into();
                    return Ok(());
                };
                let Some(entry) = self.device.selected().cloned() else {
                    return Ok(());
                };
                if entry.kind == DeviceEntryKind::Directory {
                    backend.enter_dir(&entry.id, &entry.name)?;
                    self.device_path_cached = backend.current_path().to_string();
                    self.status = format!("Device: {}", self.device_path_cached);
                    self.spawn_device_listing();
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
                    self.host.entries = Self::read_host_dir(&self.host_cwd)?;
                    self.host.selected = 0;
                    self.status = format!("Host: {}", self.host_cwd.display());
                }
            }
            FocusPane::Device => {
                let Some(backend) = &mut self.backend else {
                    self.status = "No device connected".into();
                    return Ok(());
                };
                backend.go_up()?;
                self.device_path_cached = backend.current_path().to_string();
                self.status = format!("Device: {}", self.device_path_cached);
                self.spawn_device_listing();
            }
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<()> {
        self.host
            .update_entries(Self::read_host_dir(&self.host_cwd)?, |e| &e.name);
        if self.backend.is_some() {
            self.spawn_device_listing_preserving_selection();
        }
        self.status = "Refreshed".into();
        Ok(())
    }

    fn copy_host_to_device(&mut self) -> Result<()> {
        if self.backend.is_none() {
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
            .device
            .entries
            .iter()
            .find(|d| d.name == *filename && d.kind == DeviceEntryKind::File);

        if let Some(existing) = existing {
            self.confirm_dialog = Some(ConfirmDialog {
                title: "Overwrite?".into(),
                message: format!("\"{filename}\" already exists on device. Overwrite?"),
                on_confirm: ConfirmAction::OverwritePush {
                    source: entry.path.clone(),
                    delete_id: existing.id.clone(),
                },
            });
            return Ok(());
        }

        let path = entry.path.clone();
        self.do_push_file(&path, None)
    }

    fn do_push_file(&mut self, source: &Path, delete_id: Option<&str>) -> Result<()> {
        let backend = self.backend.as_mut().context("no device connected")?;
        let filename = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        if let Some(id) = delete_id {
            self.status = format!("Deleting old {filename}...");
            backend.delete(id)?;
        }

        self.status = format!("Pushing {filename}...");
        backend.push_file(source)?;
        self.status = format!("Pushed {filename}");
        self.spawn_device_listing_preserving_selection();
        Ok(())
    }

    fn copy_device_to_host(&mut self) -> Result<()> {
        if self.backend.is_none() {
            self.status = "No device connected".into();
            return Ok(());
        }
        let Some(entry) = self.device.selected() else {
            return Ok(());
        };
        if entry.kind == DeviceEntryKind::Directory {
            self.status = "Skipping directory pull for now".into();
            return Ok(());
        }

        let entry_id = entry.id.clone();
        let filename = entry.name.clone();

        if self.host_cwd.join(&filename).exists() {
            self.confirm_dialog = Some(ConfirmDialog {
                title: "Overwrite?".into(),
                message: format!("\"{filename}\" already exists on host. Overwrite?"),
                on_confirm: ConfirmAction::OverwritePull { entry_id, filename },
            });
            return Ok(());
        }

        self.do_pull_file(&entry_id, &filename)
    }

    fn do_pull_file(&mut self, entry_id: &str, filename: &str) -> Result<()> {
        let backend = self.backend.as_mut().context("no device connected")?;

        self.status = format!("Pulling {filename}...");
        backend.pull_file(entry_id, filename, &self.host_cwd)?;
        self.status = format!("Pulled {filename}");
        self.host
            .update_entries(Self::read_host_dir(&self.host_cwd)?, |e| &e.name);
        Ok(())
    }

    fn submit_text_input(&mut self, action: TextInputAction, input: &str) {
        match action {
            TextInputAction::Mkdir => match self.backend.as_mut() {
                Some(backend) => match backend.mkdir(input) {
                    Ok(()) => {
                        self.status = format!("Created directory {input}");
                        self.spawn_device_listing_preserving_selection();
                    }
                    Err(e) => self.status = format!("Error: {e:#}"),
                },
                None => self.status = "No device connected".into(),
            },
            TextInputAction::Rename { entry_id } => match self.backend.as_mut() {
                Some(backend) => match backend.rename(&entry_id, input) {
                    Ok(()) => {
                        self.status = format!("Renamed to {input}");
                        self.spawn_device_listing_preserving_selection();
                    }
                    Err(e) => self.status = format!("Error: {e:#}"),
                },
                None => self.status = "No device connected".into(),
            },
        }
    }

    fn confirm_quit(&mut self) {
        self.confirm_dialog = Some(ConfirmDialog {
            title: "Quit?".into(),
            message: "Are you sure you want to quit?".into(),
            on_confirm: ConfirmAction::Quit,
        });
    }

    fn rename_prompt(&mut self) {
        if self.focus != FocusPane::Device {
            return;
        }
        if self.backend.is_none() {
            self.status = "No device connected".into();
            return;
        }
        let Some(entry) = self.device.selected() else {
            return;
        };
        let cursor_pos = entry.name.len();
        self.text_input_dialog = Some(TextInputDialog {
            title: "Rename".into(),
            prompt: format!("Rename \"{}\" to:", entry.name),
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
        if self.backend.is_none() {
            self.status = "No device connected".into();
            return;
        }
        self.text_input_dialog = Some(TextInputDialog {
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
        if self.backend.is_none() {
            self.status = "No device connected".into();
            return;
        }
        let Some(entry) = self.device.selected() else {
            return;
        };
        let kind = match entry.kind {
            DeviceEntryKind::Directory => "directory",
            DeviceEntryKind::File => "file",
        };
        self.confirm_dialog = Some(ConfirmDialog {
            title: "Delete?".into(),
            message: format!("Delete {kind} \"{}\"?", entry.name),
            on_confirm: ConfirmAction::Delete {
                entry_id: entry.id.clone(),
                name: entry.name.clone(),
            },
        });
    }

    fn read_host_dir(path: &Path) -> Result<Vec<HostEntry>> {
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
}
