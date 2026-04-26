use std::path::Path;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::app::App;
use crate::types::{ActiveDialog, DeviceEntryKind, DeviceState, FocusPane, HostEntry, PaneState};

const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

pub fn draw(app: &App, frame: &mut Frame) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(frame.area());

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(vertical[0]);

    draw_host_pane(
        &app.host,
        &app.host_cwd,
        app.focus == FocusPane::Host,
        frame,
        panes[0],
    );
    draw_device_pane(app, frame, panes[1]);
    draw_status_bar(&app.status, frame, vertical[1]);

    match &app.dialog {
        ActiveDialog::None => {}
        ActiveDialog::Confirm(_) => draw_confirm_dialog(app, frame),
        ActiveDialog::TextInput(_) => draw_text_input_dialog(app, frame),
        ActiveDialog::Info(_) => draw_info_dialog(app, frame),
        ActiveDialog::Inspector(_) => draw_inspector(app, frame),
    }

    if app.show_help {
        draw_help(frame);
    }
}

fn draw_host_pane(
    pane: &PaneState<HostEntry>,
    cwd: &Path,
    focused: bool,
    frame: &mut Frame,
    area: Rect,
) {
    let title = format!(" Host {} ", cwd.display());
    let block = pane_block(title, focused);
    let items: Vec<ListItem> = pane
        .entries
        .iter()
        .map(|entry| {
            let icon = if entry.is_dir { "📁" } else { "📄" };
            let size = entry
                .size
                .map(format_size)
                .unwrap_or_else(|| "<DIR>".into());
            ListItem::new(Line::from(vec![
                Span::raw(format!("{icon} {}", entry.name)),
                Span::raw(format!("  {size}")),
            ]))
        })
        .collect();

    render_file_list(items, block, pane.selected, frame, area);
}

fn draw_device_pane(app: &App, frame: &mut Frame, area: Rect) {
    let focused = app.focus == FocusPane::Device;

    match &app.device_state {
        DeviceState::Disconnected { error } => {
            let block = pane_block(" Device (not connected) ".into(), focused);
            let msg = error.as_deref().unwrap_or("No MTP device found");
            let paragraph = Paragraph::new(msg).block(block).wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
        }
        DeviceState::Connecting { spinner_tick, .. } => {
            let spinner = SPINNER_FRAMES[spinner_tick % SPINNER_FRAMES.len()];
            let title = format!(" Device (connecting…) {spinner} ");
            let block = pane_block(title, focused);
            let paragraph = Paragraph::new("Connecting to device…")
                .block(block)
                .wrap(Wrap { trim: false });
            frame.render_widget(paragraph, area);
        }
        DeviceState::Loading(state) => {
            let spinner = SPINNER_FRAMES[state.spinner_tick % SPINNER_FRAMES.len()];
            let title = format!(" {} {} {spinner} ", state.cache.name, state.cache.path);
            let mut block = pane_block(title, focused);
            if let Some((free, total)) = state.cache.storage_info {
                block = block.title_bottom(
                    Line::from(format!(
                        " {} free / {} ",
                        format_size(free),
                        format_size(total)
                    ))
                    .alignment(Alignment::Right),
                );
            }

            if app.device_pane.entries.is_empty() {
                let msg = match state.progress {
                    Some((fetched, total)) if total > 0 => {
                        format!("Loading ({fetched}/{total})…")
                    }
                    _ => "Loading…".into(),
                };
                let paragraph = Paragraph::new(msg).block(block).wrap(Wrap { trim: false });
                frame.render_widget(paragraph, area);
            } else {
                draw_device_entries(&app.device_pane, block, frame, area);
            }
        }
        DeviceState::Connected { cache, .. } => {
            let title = format!(" {} {} ", cache.name, cache.path);
            let mut block = pane_block(title, focused);
            if let Some((free, total)) = cache.storage_info {
                block = block.title_bottom(
                    Line::from(format!(
                        " {} free / {} ",
                        format_size(free),
                        format_size(total)
                    ))
                    .alignment(Alignment::Right),
                );
            }
            draw_device_entries(&app.device_pane, block, frame, area);
        }
    }
}

fn draw_device_entries(
    pane: &PaneState<crate::types::DeviceEntry>,
    block: Block<'_>,
    frame: &mut Frame,
    area: Rect,
) {
    let items: Vec<ListItem> = pane
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
                Span::raw(format!("{icon} {}", entry.name)),
                Span::raw(format!("  {size}")),
            ]))
        })
        .collect();

    render_file_list(items, block, pane.selected, frame, area);
}

fn render_file_list(
    items: Vec<ListItem<'_>>,
    block: Block<'_>,
    selected: usize,
    frame: &mut Frame,
    area: Rect,
) {
    let mut state = ListState::default().with_selected(Some(selected));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_status_bar(status: &str, frame: &mut Frame, area: Rect) {
    let text = format!(
        "Tab pane • i inspect • p push • g pull • d del • m mkdir • R rename • r refresh • ? help • q quit    {status}",
    );
    frame.render_widget(Paragraph::new(text), area);
}

fn draw_help(frame: &mut Frame) {
    let area = centered_rect(frame.area(), 72, 55);
    frame.render_widget(Clear, area);

    let lines = vec![
        Line::from(""),
        Line::from("Navigation:"),
        Line::from("  Tab         switch active pane"),
        Line::from("  j / k       move selection"),
        Line::from("  Enter       enter directory"),
        Line::from("  Backspace   go to parent"),
        Line::from(""),
        Line::from("File actions (device pane):"),
        Line::from("  i           inspect object metadata"),
        Line::from("  p           push host file to device"),
        Line::from("  g           pull device file to host"),
        Line::from("  d           delete (confirms)"),
        Line::from("  m           create directory"),
        Line::from("  R           rename"),
        Line::from(""),
        Line::from("App:"),
        Line::from("  r           refresh both panes"),
        Line::from("  ?           toggle this help"),
        Line::from("  Esc         close dialog / help"),
        Line::from("  q           quit"),
    ];

    let help = Paragraph::new(lines)
        .block(Block::default().title(" Help ").borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(help, area);
}

fn draw_inspector(app: &App, frame: &mut Frame) {
    let ActiveDialog::Inspector(data) = &app.dialog else {
        return;
    };

    let area = centered_rect(frame.area(), 75, 85);
    frame.render_widget(Clear, area);

    let lines = build_inspector_lines(data);

    let inner_height = area.height.saturating_sub(2) as usize;
    let max_offset = lines.len().saturating_sub(inner_height);
    let offset = data.scroll_offset.min(max_offset);
    let visible: Vec<Line> = lines.into_iter().skip(offset).take(inner_height).collect();

    let title = format!(" Inspector: {} ", data.filename);
    let paragraph =
        Paragraph::new(visible).block(Block::default().title(title).borders(Borders::ALL));
    frame.render_widget(paragraph, area);
}

fn build_inspector_lines<'a>(data: &'a crate::types::InspectorData) -> Vec<Line<'a>> {
    let dim = Style::default().add_modifier(Modifier::DIM);
    let bold = Style::default().add_modifier(Modifier::BOLD);

    let field = |label: &'a str, value: &'a str| -> Line<'a> {
        Line::from(vec![Span::styled(label, dim), Span::raw(value)])
    };

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled("--- ObjectInfo ---", bold)),
        Line::from(""),
        field("  Handle:      ", &data.object_handle),
        field("  Filename:    ", &data.filename),
        field("  Format:      ", &data.format),
        field("  Size:        ", &data.size),
        field("  Storage:     ", &data.storage_id),
        field("  Parent:      ", &data.parent_id),
        field("  Protection:  ", &data.protection),
        field(
            "  Created:     ",
            data.created.as_deref().unwrap_or("(none)"),
        ),
        field(
            "  Modified:    ",
            data.modified.as_deref().unwrap_or("(none)"),
        ),
    ];
    if !data.keywords.is_empty() {
        lines.push(field("  Keywords:    ", &data.keywords));
    }
    if let Some(ref dims) = data.image_dimensions {
        lines.push(field("  Image dims:  ", dims));
    }
    if let Some(ref thumb) = data.thumb_dimensions {
        lines.push(field("  Thumbnail:   ", thumb));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "--- MTP Properties (GetObjectPropValue) ---",
        bold,
    )));
    lines.push(Line::from(""));

    for prop in &data.properties {
        let label = format!("  0x{:04X} {:<20} ", prop.code, prop.name);
        let style = if prop.is_error { dim } else { Style::default() };
        let prefix = if prop.is_error { "ERR " } else { "" };
        lines.push(Line::from(vec![
            Span::styled(label, dim),
            Span::styled(format!("{prefix}{}", prop.value), style),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  j/k scroll • Esc/i/q close",
        dim,
    )));

    lines
}

fn draw_text_input_dialog(app: &App, frame: &mut Frame) {
    let ActiveDialog::TextInput(dialog) = &app.dialog else {
        return;
    };

    let max_width = (frame.area().width).min(50);
    let height: u16 = 8;

    let area = centered_fixed(frame.area(), max_width, height);
    frame.render_widget(Clear, area);

    let inner_width = max_width.saturating_sub(2) as usize;
    let input = &dialog.input;
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let char_count = chars.len();

    let cursor_char = chars
        .iter()
        .position(|&(byte_pos, _)| byte_pos == dialog.cursor_pos)
        .unwrap_or(char_count);

    let vis_start = if cursor_char < inner_width {
        0
    } else {
        cursor_char - inner_width + 1
    };
    let vis_end = (vis_start + inner_width).min(char_count);
    let cursor_in_vis = cursor_char - vis_start;

    let mut before = String::new();
    let mut after = String::new();
    let mut cursor_ch: Option<char> = None;

    for (i, &(_, ch)) in chars
        .iter()
        .enumerate()
        .skip(vis_start)
        .take(vis_end - vis_start)
    {
        let rel = i - vis_start;
        if rel < cursor_in_vis {
            before.push(ch);
        } else if rel == cursor_in_vis {
            cursor_ch = Some(ch);
        } else {
            after.push(ch);
        }
    }

    let mut input_spans: Vec<Span> = Vec::new();
    if !before.is_empty() {
        input_spans.push(Span::raw(before));
    }
    if let Some(ch) = cursor_ch {
        input_spans.push(Span::styled(
            ch.to_string(),
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    } else {
        input_spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    if !after.is_empty() {
        input_spans.push(Span::raw(after));
    }

    let lines = vec![
        Line::from(""),
        Line::from(Span::raw(&dialog.prompt)),
        Line::from(""),
        Line::from(input_spans),
        Line::from(""),
        Line::from(Span::raw("Enter confirm • Esc cancel")),
    ];

    let title = format!(" {} ", dialog.title);
    let paragraph = Paragraph::new(lines)
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_info_dialog(app: &App, frame: &mut Frame) {
    let ActiveDialog::Info(dialog) = &app.dialog else {
        return;
    };

    let max_width = (frame.area().width).min(55);
    let inner_width = max_width.saturating_sub(2);
    let msg_lines: u16 = dialog
        .message
        .lines()
        .map(|line| {
            if inner_width > 0 {
                (line.len() as u16).div_ceil(inner_width)
            } else {
                1
            }
            .max(1)
        })
        .sum();
    let height = 2 + 1 + msg_lines + 1 + 1;

    let area = centered_fixed(frame.area(), max_width, height);
    frame.render_widget(Clear, area);

    let mut lines: Vec<Line> = vec![Line::from("")];
    for text_line in dialog.message.lines() {
        lines.push(Line::from(Span::raw(text_line)));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Press any key to close",
        Style::default().add_modifier(Modifier::DIM),
    )));

    let title = format!(" {} ", dialog.title);
    let paragraph = Paragraph::new(lines)
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn draw_confirm_dialog(app: &App, frame: &mut Frame) {
    let ActiveDialog::Confirm(dialog) = &app.dialog else {
        return;
    };

    let max_width = (frame.area().width).min(60);
    let inner_width = max_width.saturating_sub(2);
    let msg_len = dialog.message.len() as u16;
    let msg_lines = if inner_width > 0 {
        msg_len.div_ceil(inner_width)
    } else {
        1
    };
    let height = 2 + 1 + msg_lines + 1 + 1;

    let area = centered_fixed(frame.area(), max_width, height);
    frame.render_widget(Clear, area);

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::raw(&dialog.message)),
        Line::from(""),
        Line::from(vec![
            Span::styled("[Y]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("es    "),
            Span::styled("[N]", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("o"),
        ]),
    ];

    let title = format!(" {} ", dialog.title);
    let paragraph = Paragraph::new(lines)
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false });
    frame.render_widget(paragraph, area);
}

fn pane_block(title: String, active: bool) -> Block<'static> {
    let title = if active { format!(">{title}") } else { title };
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
        format!("{bytes} B")
    }
}

fn centered_fixed(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
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
