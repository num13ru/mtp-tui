use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::App;
use crate::types::{DeviceEntryKind, FocusPane};

const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

impl App {
    pub fn draw(&self, frame: &mut Frame) {
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
        if self.backend.is_none() && !self.device_loading {
            let block = pane_block(
                " Device (not connected) ".into(),
                self.focus == FocusPane::Device,
            );
            let msg = self
                .device_error
                .as_deref()
                .unwrap_or("No MTP device found");
            let paragraph = Paragraph::new(msg)
                .block(block)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
            return;
        }

        let title = if self.device_loading {
            let spinner = SPINNER_FRAMES[self.spinner_tick % SPINNER_FRAMES.len()];
            format!(
                " {} {} {} ",
                self.device_name_cached, self.device_path_cached, spinner,
            )
        } else {
            format!(
                " {} {} ",
                self.device_name_cached, self.device_path_cached
            )
        };

        let block = pane_block(title, self.focus == FocusPane::Device);

        if self.device_loading && self.device.entries.is_empty() {
            let paragraph = Paragraph::new("Loading…")
                .block(block)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
            return;
        }

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
        ];

        let help = Paragraph::new(lines)
            .block(Block::default().title(" Help ").borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        frame.render_widget(help, area);
    }
}

fn pane_block(title: String, active: bool) -> Block<'static> {
    let title = if active {
        format!(">{}", title)
    } else {
        title
    };

    Block::default().title(title).borders(Borders::ALL)
}

pub fn format_size(bytes: u64) -> String {
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
