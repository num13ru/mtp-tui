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
use crate::types::{DeviceEntry, DeviceEntryKind, FocusPane, HostEntry, PaneState};

type ListingPayload = (Box<dyn DeviceBackend>, Result<Vec<DeviceEntry>>);

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
    pub device_loading: bool,
    pub spinner_tick: usize,
    last_tick: Instant,
    dir_rx: Option<mpsc::Receiver<ListingPayload>>,
}

impl App {
    pub fn new() -> Result<Self> {
        let host_cwd = std::env::current_dir().context("failed to get current directory")?;
        let host = PaneState::new(Self::read_host_dir(&host_cwd)?);

        let (backend, device, device_error, device_name, device_path, status) =
            match MtpBackend::new() {
                Ok(b) => {
                    let backend: Box<dyn DeviceBackend> = Box::new(b);
                    let entries = backend.list_current_dir()?;
                    let name = backend.device_name().to_string();
                    let path = backend.current_path().to_string();
                    let status = format!("Connected to {}", name);
                    (
                        Some(backend),
                        PaneState::new(entries),
                        None,
                        name,
                        path,
                        status,
                    )
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    (
                        None,
                        PaneState::new(vec![]),
                        Some(msg),
                        String::new(),
                        String::new(),
                        "No device connected".into(),
                    )
                }
            };

        Ok(Self {
            host_cwd,
            host,
            device,
            focus: FocusPane::Host,
            backend,
            device_error,
            device_name_cached: device_name,
            device_path_cached: device_path,
            status,
            show_help: false,
            device_loading: false,
            spinner_tick: 0,
            last_tick: Instant::now(),
            dir_rx: None,
        })
    }

    pub fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;

            let timeout = Duration::from_millis(200);
            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if self.handle_key(key)? {
                            break;
                        }
                    }
                    _ => {}
                }
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
        let payload = match rx.try_recv() {
            Ok(p) => p,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.device_loading = false;
                self.dir_rx = None;
                self.status = "Error: device listing thread crashed".into();
                return;
            }
        };

        let (backend, result) = payload;
        self.device_name_cached = backend.device_name().to_string();
        self.device_path_cached = backend.current_path().to_string();
        self.backend = Some(backend);
        self.device_loading = false;
        self.dir_rx = None;
        match result {
            Ok(entries) => {
                self.device.entries = entries;
                self.device.selected = 0;
            }
            Err(e) => self.status = format!("Error: {e:#}"),
        }
    }

    fn spawn_device_listing(&mut self) {
        let Some(backend) = self.backend.take() else {
            return;
        };

        self.device_loading = true;
        self.spinner_tick = 0;
        self.device.entries.clear();

        let (tx, rx) = mpsc::channel();
        self.dir_rx = Some(rx);

        thread::spawn(move || {
            let result = backend.list_current_dir();
            tx.send((backend, result)).ok();
        });
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if self.device_loading && self.focus == FocusPane::Device {
            return match (key.code, key.modifiers) {
                (KeyCode::Char('q'), _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => Ok(true),
                (KeyCode::Tab, _) => {
                    self.toggle_focus();
                    Ok(false)
                }
                (KeyCode::Char('?'), _) => {
                    self.show_help = !self.show_help;
                    Ok(false)
                }
                _ => Ok(false),
            };
        }

        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => return Ok(true),
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
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(true),
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
            _ => {}
        }

        Ok(false)
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
        self.host.entries = Self::read_host_dir(&self.host_cwd)?;
        if self.backend.is_some() {
            self.spawn_device_listing();
        }
        self.status = "Refreshed".into();
        Ok(())
    }

    fn copy_host_to_device(&mut self) -> Result<()> {
        let Some(backend) = &mut self.backend else {
            self.status = "No device connected".into();
            return Ok(());
        };
        let Some(entry) = self.host.selected() else {
            return Ok(());
        };
        if entry.is_dir {
            self.status = "Skipping directory push for now".into();
            return Ok(());
        }
        backend.push_file(&entry.path)?;
        self.status = format!("Pushed {}", entry.name);
        Ok(())
    }

    fn copy_device_to_host(&mut self) -> Result<()> {
        let Some(backend) = &mut self.backend else {
            self.status = "No device connected".into();
            return Ok(());
        };
        let Some(entry) = self.device.selected() else {
            return Ok(());
        };
        if entry.kind == DeviceEntryKind::Directory {
            self.status = "Skipping directory pull for now".into();
            return Ok(());
        }
        backend.pull_file(&entry.id, &self.host_cwd)?;
        self.status = format!("Pulled {}", entry.name);
        Ok(())
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
