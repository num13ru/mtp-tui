use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

fn main() -> Result<()> {
    let terminal = ratatui::init();
    let result = App::new()?.run(terminal);
    ratatui::restore();
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    Host,
    Device,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceEntryKind {
    Directory,
    File,
}

#[derive(Debug, Clone)]
struct HostEntry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    size: Option<u64>,
}

#[derive(Debug, Clone)]
struct DeviceEntry {
    id: String,
    name: String,
    kind: DeviceEntryKind,
    size: Option<u64>,
}

trait DeviceBackend {
    fn device_name(&self) -> &str;
    fn current_path(&self) -> &str;
    fn list_current_dir(&self) -> Result<Vec<DeviceEntry>>;
    fn enter_dir(&mut self, entry_id: &str) -> Result<()>;
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

struct MockDeviceBackend {
    device_name: String,
    path_stack: Vec<String>,
}

impl MockDeviceBackend {
    fn new() -> Self {
        Self {
            device_name: "Mock Kindle".to_string(),
            path_stack: vec!["/".to_string()],
        }
    }

    fn current_level(&self) -> usize {
        self.path_stack.len().saturating_sub(1)
    }
}

impl DeviceBackend for MockDeviceBackend {
    fn device_name(&self) -> &str {
        &self.device_name
    }

    fn current_path(&self) -> &str {
        self.path_stack.last().map(String::as_str).unwrap_or("/")
    }

    fn list_current_dir(&self) -> Result<Vec<DeviceEntry>> {
        let level = self.current_level();
        let entries = match level {
            0 => vec![
                DeviceEntry {
                    id: "documents".into(),
                    name: "documents".into(),
                    kind: DeviceEntryKind::Directory,
                    size: None,
                },
                DeviceEntry {
                    id: "downloads".into(),
                    name: "downloads".into(),
                    kind: DeviceEntryKind::Directory,
                    size: None,
                },
                DeviceEntry {
                    id: "system".into(),
                    name: "system".into(),
                    kind: DeviceEntryKind::Directory,
                    size: None,
                },
            ],
            1 => vec![
                DeviceEntry {
                    id: "book-1".into(),
                    name: "The Left Hand of Darkness.epub".into(),
                    kind: DeviceEntryKind::File,
                    size: Some(815_204),
                },
                DeviceEntry {
                    id: "book-2".into(),
                    name: "Roadside Picnic.epub".into(),
                    kind: DeviceEntryKind::File,
                    size: Some(1_245_009),
                },
            ],
            _ => vec![],
        };

        Ok(entries)
    }

    fn enter_dir(&mut self, entry_id: &str) -> Result<()> {
        self.path_stack.push(format!(
            "{}/{}",
            self.current_path().trim_end_matches('/'),
            entry_id
        ));
        Ok(())
    }

    fn go_up(&mut self) -> Result<()> {
        if self.path_stack.len() > 1 {
            self.path_stack.pop();
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }
}

struct PaneState<T> {
    entries: Vec<T>,
    selected: usize,
}

impl<T> PaneState<T> {
    fn new(entries: Vec<T>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    fn select_next(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
        } else {
            self.selected = (self.selected + 1).min(self.entries.len() - 1);
        }
    }

    fn select_prev(&mut self) {
        if self.entries.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.saturating_sub(1);
        }
    }

    fn selected(&self) -> Option<&T> {
        self.entries.get(self.selected)
    }
}

struct App {
    host_cwd: PathBuf,
    host: PaneState<HostEntry>,
    device: PaneState<DeviceEntry>,
    focus: FocusPane,
    backend: Box<dyn DeviceBackend>,
    status: String,
    show_help: bool,
    last_tick: Instant,
}

impl App {
    fn new() -> Result<Self> {
        let host_cwd = std::env::current_dir().context("failed to get current directory")?;
        let mut backend: Box<dyn DeviceBackend> = Box::new(MockDeviceBackend::new());
        let host = PaneState::new(Self::read_host_dir(&host_cwd)?);
        let device = PaneState::new(backend.list_current_dir()?);

        Ok(Self {
            host_cwd,
            host,
            device,
            focus: FocusPane::Host,
            backend,
            status: "Ready".into(),
            show_help: false,
            last_tick: Instant::now(),
        })
    }

    fn run(mut self, mut terminal: DefaultTerminal) -> Result<()> {
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

            if self.last_tick.elapsed() >= Duration::from_secs(5) {
                self.last_tick = Instant::now();
            }
        }

        Ok(())
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        match (key.code, key.modifiers) {
            (KeyCode::Char('q'), _) => return Ok(true),
            (KeyCode::Tab, _) => self.toggle_focus(),
            (KeyCode::Char('?'), _) => self.show_help = !self.show_help,
            (KeyCode::Char('r'), _) => self.refresh()?,
            (KeyCode::Up, _) | (KeyCode::Char('k'), _) => self.move_up(),
            (KeyCode::Down, _) | (KeyCode::Char('j'), _) => self.move_down(),
            (KeyCode::Enter, _) | (KeyCode::Right, _) | (KeyCode::Char('l'), _) => {
                self.enter_selected()?
            }
            (KeyCode::Backspace, _) | (KeyCode::Left, _) | (KeyCode::Char('h'), _) => {
                self.go_up()?
            }
            (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(true),
            (KeyCode::Char('p'), _) => self.copy_host_to_device()?,
            (KeyCode::Char('g'), _) => self.copy_device_to_host()?,
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
                let Some(entry) = self.device.selected().cloned() else {
                    return Ok(());
                };
                if entry.kind == DeviceEntryKind::Directory {
                    self.backend.enter_dir(&entry.id)?;
                    self.device.entries = self.backend.list_current_dir()?;
                    self.device.selected = 0;
                    self.status = format!("Device: {}", self.backend.current_path());
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
                self.backend.go_up()?;
                self.device.entries = self.backend.list_current_dir()?;
                self.device.selected = 0;
                self.status = format!("Device: {}", self.backend.current_path());
            }
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<()> {
        self.host.entries = Self::read_host_dir(&self.host_cwd)?;
        self.backend.refresh()?;
        self.device.entries = self.backend.list_current_dir()?;
        self.status = "Refreshed".into();
        Ok(())
    }

    fn copy_host_to_device(&mut self) -> Result<()> {
        let Some(entry) = self.host.selected() else {
            return Ok(());
        };
        if entry.is_dir {
            self.status = "Skipping directory push for now".into();
            return Ok(());
        }
        self.backend.push_file(&entry.path)?;
        self.status = format!("Pushed {}", entry.name);
        Ok(())
    }

    fn copy_device_to_host(&mut self) -> Result<()> {
        let Some(entry) = self.device.selected() else {
            return Ok(());
        };
        if entry.kind == DeviceEntryKind::Directory {
            self.status = "Skipping directory pull for now".into();
            return Ok(());
        }
        self.backend.pull_file(&entry.id, &self.host_cwd)?;
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

    fn draw(&self, frame: &mut Frame) {
        let vertical = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .split(frame.area());

        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(vertical[0]);

        self.draw_host_pane(frame, panes[0]);
        self.draw_device_pane(frame, panes[1]);
        self.draw_status_bar(frame, vertical[1]);

        if self.show_help {
            self.draw_help(frame);
        }
    }

    fn draw_host_pane(&self, frame: &mut Frame, area: Rect) {
        let title = format!(" Host {} ", self.host_cwd.display());
        let block = pane_block(title, self.focus == FocusPane::Host);
        let items = self
            .host
            .entries
            .iter()
            .map(|entry| {
                let icon = if entry.is_dir { "📁" } else { "📄" };
                let size = entry
                    .size
                    .map(format_size)
                    .unwrap_or_else(|| "<DIR>".into());
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{} {}", icon, entry.name)),
                    Span::raw(format!("  {}", size)),
                ]))
            })
            .collect::<Vec<_>>();

        let mut state = ListState::default().with_selected(Some(self.host.selected));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_device_pane(&self, frame: &mut Frame, area: Rect) {
        let title = format!(
            " {} {} ",
            self.backend.device_name(),
            self.backend.current_path()
        );
        let block = pane_block(title, self.focus == FocusPane::Device);
        let items = self
            .device
            .entries
            .iter()
            .map(|entry| {
                let icon = if entry.kind == DeviceEntryKind::Directory {
                    "📁"
                } else {
                    "📚"
                };
                let size = entry
                    .size
                    .map(format_size)
                    .unwrap_or_else(|| "<DIR>".into());
                ListItem::new(Line::from(vec![
                    Span::raw(format!("{} {}", icon, entry.name)),
                    Span::raw(format!("  {}", size)),
                ]))
            })
            .collect::<Vec<_>>();

        let mut state = ListState::default().with_selected(Some(self.device.selected));
        let list = List::new(items)
            .block(block)
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
        frame.render_stateful_widget(list, area, &mut state);
    }

    fn draw_status_bar(&self, frame: &mut Frame, area: Rect) {
        let text = format!(
            "Tab switch pane • Enter open • Backspace up • p push • g pull • r refresh • ? help • q quit    {}",
            self.status
        );
        frame.render_widget(Paragraph::new(text), area);
    }

    fn draw_help(&self, frame: &mut Frame) {
        let area = centered_rect(frame.area(), 72, 55);
        frame.render_widget(Clear, area);

        let lines = vec![
            Line::from("mac-mtp-tui"),
            Line::from(""),
            Line::from("Navigation:"),
            Line::from("  Tab         switch active pane"),
            Line::from("  j / k       move selection"),
            Line::from("  Enter       enter directory"),
            Line::from("  Backspace   go to parent"),
            Line::from(""),
            Line::from("File actions:"),
            Line::from("  p           push selected host file to device"),
            Line::from("  g           pull selected device file to host"),
            Line::from("  r           refresh both panes"),
            Line::from(""),
            Line::from("App:"),
            Line::from("  ?           toggle this help"),
            Line::from("  q           quit"),
            Line::from(""),
            Line::from("Current build uses MockDeviceBackend."),
            Line::from("Next step: replace it with an mtp-rs adapter."),
        ];

        let help = Paragraph::new(lines)
            .block(Block::default().title(" Help ").borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        frame.render_widget(help, area);
    }
}

fn pane_block(title: String, active: bool) -> Block<'static> {
    let title = if active { format!(">{}", title) } else { title };

    Block::default().title(title).borders(Borders::ALL)
}

fn format_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f >= GB {
        format!("{:.1} GB", bytes_f / GB)
    } else if bytes_f >= MB {
        format!("{:.1} MB", bytes_f / MB)
    } else if bytes_f >= KB {
        format!("{:.1} KB", bytes_f / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn centered_rect(area: Rect, width_percent: u16, height_percent: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - height_percent) / 2),
            Constraint::Percentage(height_percent),
            Constraint::Percentage((100 - height_percent) / 2),
        ])
        .split(area);

    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - width_percent) / 2),
            Constraint::Percentage(width_percent),
            Constraint::Percentage((100 - width_percent) / 2),
        ])
        .split(vertical[1]);

    horizontal[1]
}

