use chrono::Local;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::app::{App, FocusPanel};
use crate::message::MessageType;

pub fn draw(f: &mut Frame, app: &mut App) {
    // Main layout: top area + bottom status bar
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(f.area());

    // Top area: left panel + right chat panel
    let h_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(main_chunks[0]);

    // Left panel: session list + session info
    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(h_chunks[0]);

    // Store rects for mouse hit testing
    app.session_list_area = left_chunks[0];
    app.chat_area = h_chunks[1];

    draw_session_list(f, app, left_chunks[0]);
    draw_session_info(f, app, left_chunks[1]);
    draw_chat_stream(f, app, h_chunks[1]);
    draw_status_bar(f, app, main_chunks[1]);
}

fn draw_session_list(f: &mut Frame, app: &mut App, area: Rect) {
    let sessions = &app.sorted_session_ids;
    let items: Vec<ListItem> = sessions
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let session = &app.sessions[id];
            let is_selected = app
                .selected_session
                .as_ref()
                .map(|s| s == id)
                .unwrap_or(false);
            let is_active = session.is_active();
            let prefix = if is_active {
                "● "
            } else if is_selected {
                "○ "
            } else {
                "  "
            };
            let prefix_color = if is_active {
                Color::Green
            } else {
                Color::DarkGray
            };
            let name = session.display_name();
            let time = session
                .last_activity
                .with_timezone(&Local)
                .format("%H:%M")
                .to_string();
            let msg_count = session.messages.len();

            let style = if Some(i) == app.list_state.selected() {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(Line::from(vec![
                Span::styled(prefix, Style::default().fg(prefix_color)),
                Span::styled(name, style),
                Span::styled(
                    format!(" [{}] {}", msg_count, time),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let active_label = if app.show_active_only {
        " [active] "
    } else {
        ""
    };
    let title = if let Some(ref filter) = app.filter_text {
        format!(" Sessions{} (/{}) ", active_label, filter)
    } else {
        format!(" Sessions{} ({}) ", active_label, sessions.len())
    };

    let border_color = if app.focus == FocusPanel::Sessions {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(Style::default().fg(border_color)),
        )
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    f.render_stateful_widget(list, area, &mut app.list_state);
}

fn draw_session_info(f: &mut Frame, app: &App, area: Rect) {
    let content = if let Some(ref id) = app.selected_session {
        if let Some(session) = app.sessions.get(id) {
            let branch = session.git_branch.as_deref().unwrap_or("n/a");
            let cwd = session
                .cwd
                .as_deref()
                .map(|c| {
                    // Abbreviate home dir
                    if let Some(home) = dirs::home_dir()
                        && let Some(rest) = c.strip_prefix(home.to_str().unwrap_or(""))
                    {
                        return format!("~{}", rest);
                    }
                    c.to_string()
                })
                .unwrap_or_else(|| "n/a".to_string());

            let tokens_in = format_tokens(session.total_tokens_in);
            let tokens_out = format_tokens(session.total_tokens_out);

            let mut info_lines = vec![];
            if let Some(ref title) = session.custom_title {
                info_lines.push(Line::from(vec![
                    Span::styled("Title: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        title.as_str(),
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]));
            }
            info_lines.extend([
                Line::from(vec![
                    Span::styled("ID: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(&session.id, Style::default().fg(Color::DarkGray)),
                ]),
                Line::from(vec![
                    Span::styled("Branch: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(branch, Style::default().fg(Color::Green)),
                ]),
                Line::from(vec![
                    Span::styled("CWD: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(cwd, Style::default().fg(Color::White)),
                ]),
                Line::from(vec![
                    Span::styled("Tokens: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{} in / {} out", tokens_in, tokens_out),
                        Style::default().fg(Color::Cyan),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Messages: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        session.messages.len().to_string(),
                        Style::default().fg(Color::White),
                    ),
                ]),
                Line::from(vec![
                    Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                    if session.is_active() {
                        Span::styled(
                            "active",
                            Style::default()
                                .fg(Color::Green)
                                .add_modifier(Modifier::BOLD),
                        )
                    } else {
                        Span::styled("idle", Style::default().fg(Color::DarkGray))
                    },
                ]),
            ]);
            if let Some(ref summary) = session.summary {
                info_lines.push(Line::from(vec![
                    Span::styled("Summary: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(summary.as_str(), Style::default().fg(Color::White)),
                ]));
            }
            info_lines
        } else {
            vec![Line::from("No session selected")]
        }
    } else {
        vec![Line::from("No session selected")]
    };

    let info = Paragraph::new(content).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Session Info ")
            .border_style(Style::default().fg(Color::Cyan)),
    );

    f.render_widget(info, area);
}

fn draw_chat_stream(f: &mut Frame, app: &mut App, area: Rect) {
    let messages = if let Some(ref id) = app.selected_session {
        app.sessions.get(id).map(|s| &s.messages[..]).unwrap_or(&[])
    } else {
        &[]
    };

    let mut lines: Vec<Line> = Vec::new();

    for msg in messages.iter() {
        let time = msg
            .timestamp
            .with_timezone(&Local)
            .format("%H:%M")
            .to_string();

        let (prefix, style) = match msg.msg_type {
            MessageType::User => (
                "User",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            MessageType::Assistant => (
                "Assistant",
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            ),
            MessageType::ToolUse => ("Tool", Style::default().fg(Color::Magenta)),
            MessageType::Progress => ("...", Style::default().fg(Color::DarkGray)),
            MessageType::Other => ("Other", Style::default().fg(Color::DarkGray)),
        };

        // Skip progress messages in the chat view (too noisy)
        if msg.msg_type == MessageType::Progress {
            continue;
        }

        lines.push(Line::from(vec![
            Span::styled(format!("[{}] ", time), Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}: ", prefix), style),
        ]));

        // Truncate long content for display (char-boundary safe)
        for content_line in msg.content.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {}", content_line),
                Style::default().fg(Color::White),
            )));
        }
        lines.push(Line::from("")); // blank separator
    }

    let inner_height = area.height.saturating_sub(2) as usize;

    let title = if let Some(ref id) = app.selected_session {
        if let Some(session) = app.sessions.get(id) {
            format!(" Chat - {} ", session.display_name())
        } else {
            " Chat ".to_string()
        }
    } else {
        " Chat (select a session) ".to_string()
    };

    let chat_border = if app.focus == FocusPanel::Chat {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(chat_border));

    // Use ratatui's built-in line_count for exact wrapped line calculation
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false });
    let inner_width = area.width.saturating_sub(2);
    let visual_lines = paragraph.line_count(inner_width);
    let max_scroll = visual_lines.saturating_sub(inner_height);
    let scroll_offset: u16 = if app.chat_scroll_locked_to_bottom {
        max_scroll as u16
    } else {
        app.chat_scroll = app.chat_scroll.min(max_scroll);
        app.chat_scroll as u16
    };
    app.chat_total_lines = visual_lines;

    let chat = paragraph.scroll((scroll_offset, 0));
    f.render_widget(chat, area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    let mode_text = if app.filter_mode {
        format!("FILTER: /{}", app.filter_text.as_deref().unwrap_or(""))
    } else {
        {
            let focus_label = match app.focus {
                FocusPanel::Sessions => "sessions",
                FocusPanel::Chat => "chat",
            };
            format!(
                "q:quit  Tab:focus({})  j/k:nav  Enter:select  r:refresh  /:filter  a:active({})",
                focus_label,
                if app.show_active_only { "on" } else { "off" }
            )
        }
    };

    let bar = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(mode_text, Style::default().fg(Color::DarkGray)),
    ]));

    f.render_widget(bar, area);
}

fn format_tokens(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}
