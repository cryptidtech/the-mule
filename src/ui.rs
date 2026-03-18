use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use crate::config::{PeerAssignment, PeerName, TestCommand};

/// Batch of commands sharing the same timestamp.
pub struct CommandBatch {
    pub time: u64,
    pub delta: u64,
    pub commands: Vec<BatchCommand>,
    pub sent: bool,
    pub sent_at: Option<Instant>,
}

pub struct BatchCommand {
    pub peer: PeerName,
    pub host: String,
    pub command: String,
}

/// Pre-compute command batches from the config.
pub fn build_batches(
    commands: &[TestCommand],
    assignments: &[PeerAssignment],
) -> Vec<CommandBatch> {
    let host_map: BTreeMap<PeerName, String> = assignments
        .iter()
        .map(|a| (a.peer_name.clone(), a.host.address.clone()))
        .collect();

    let mut batches: Vec<CommandBatch> = Vec::new();
    let mut prev_time: u64 = 0;

    for cmd in commands {
        let host = host_map
            .get(&cmd.peer)
            .cloned()
            .unwrap_or_else(|| "?".to_string());

        if batches.is_empty() || batches.last().unwrap().time != cmd.time {
            let delta = cmd.time.saturating_sub(prev_time);
            prev_time = cmd.time;
            batches.push(CommandBatch {
                time: cmd.time,
                delta,
                commands: Vec::new(),
                sent: false,
                sent_at: None,
            });
        }

        batches.last_mut().unwrap().commands.push(BatchCommand {
            peer: cmd.peer.clone(),
            host,
            command: cmd.command.clone(),
        });
    }

    batches
}

/// Format seconds as HH:MM:SS.
pub fn format_elapsed(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

/// Render the TUI frame.
pub fn render(
    frame: &mut Frame,
    name: &str,
    elapsed: Duration,
    statuses: &BTreeMap<PeerName, String>,
    assignments: &[PeerAssignment],
    batches: &[CommandBatch],
    current_batch_idx: usize,
) {
    let area = frame.area();

    // Split into header, peers, commands
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // header
            Constraint::Length(peer_pane_height(assignments.len())), // peers
            Constraint::Min(5),    // commands
        ])
        .split(area);

    render_header(frame, chunks[0], name, elapsed);
    render_peers(frame, chunks[1], statuses, assignments);
    render_commands(frame, chunks[2], batches, current_batch_idx, elapsed);
}

fn peer_pane_height(peer_count: usize) -> u16 {
    // 2 peers per row + 2 for border
    let rows = (peer_count + 1) / 2;
    (rows as u16) + 2
}

fn render_header(frame: &mut Frame, area: Rect, name: &str, elapsed: Duration) {
    let elapsed_str = format_elapsed(elapsed.as_secs());
    let text = Line::from(vec![
        Span::styled(
            format!(" The Mule: {name}"),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "{:>width$}",
            elapsed_str,
            width = area.width as usize - name.len() - 13
        )),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("The Mule");
    let paragraph = Paragraph::new(text).block(block);
    frame.render_widget(paragraph, area);
}

fn render_peers(
    frame: &mut Frame,
    area: Rect,
    statuses: &BTreeMap<PeerName, String>,
    assignments: &[PeerAssignment],
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("PEERS");

    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Build 2-column layout of peers
    let mut lines: Vec<Line> = Vec::new();
    let half_width = inner.width as usize / 2;

    for pair in assignments.chunks(2) {
        let mut spans = Vec::new();

        for (i, assignment) in pair.iter().enumerate() {
            let status = statuses
                .get(&assignment.peer_name)
                .map(|s| s.as_str())
                .unwrap_or("pending");
            let color = status_color(status);

            let entry = format!(
                " {:<10} {:<16} {}",
                assignment.peer_name.as_str(), assignment.host.address, status
            );

            if i == 1 {
                // Pad first column
                let pad = half_width.saturating_sub(spans.iter().map(|s: &Span| s.width()).sum());
                spans.push(Span::raw(" ".repeat(pad)));
            }

            spans.push(Span::styled(entry, Style::default().fg(color)));
        }

        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}

fn render_commands(
    frame: &mut Frame,
    area: Rect,
    batches: &[CommandBatch],
    current_batch_idx: usize,
    elapsed: Duration,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("COMMANDS");

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut rows: Vec<Line> = Vec::new();
    let mut first_focused_row: Option<usize> = None;

    for (batch_idx, batch) in batches.iter().enumerate() {
        let is_focused = batch_idx == current_batch_idx && !batch.sent;
        let is_sent = batch.sent;

        for (cmd_idx, cmd) in batch.commands.iter().enumerate() {
            let row_idx = rows.len();

            if is_focused && first_focused_row.is_none() {
                first_focused_row = Some(row_idx);
            }

            // Sent time column
            let sent_time = if is_sent {
                if let Some(sent_at) = batch.sent_at {
                    format_elapsed(sent_at.duration_since(Instant::now() - elapsed).as_secs())
                } else {
                    format_elapsed(batch.time)
                }
            } else {
                "        ".to_string()
            };

            // Delta column (only show on first command of batch)
            let delta = if cmd_idx == 0 {
                format!("+{}s", batch.delta)
            } else {
                "    ".to_string()
            };

            // Status column
            let status = if is_sent {
                "sent".to_string()
            } else if is_focused {
                let remaining = batch
                    .time
                    .saturating_sub(elapsed.as_secs());
                format!("{}s", remaining)
            } else {
                String::new()
            };

            // Style
            let style = if is_focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if is_sent {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default().fg(Color::White)
            };

            let marker = if is_focused { ">" } else { " " };

            let truncated_cmd = if cmd.command.len() > 20 {
                format!("{}..", &cmd.command[..18])
            } else {
                cmd.command.clone()
            };

            let line = Line::from(vec![
                Span::styled(
                    format!(
                        " {marker} {sent_time}  {delta:<6} {peer:<10} {host:<16} {cmd:<20} {status}",
                        peer = cmd.peer.as_str(),
                        host = cmd.host,
                        cmd = truncated_cmd,
                    ),
                    style,
                ),
            ]);

            rows.push(line);
        }
    }

    // Auto-scroll to keep focused batch centered
    let visible_height = inner.height as usize;
    let scroll_offset = if let Some(focused) = first_focused_row {
        focused.saturating_sub(visible_height / 2)
    } else {
        0
    };

    let paragraph = Paragraph::new(rows).scroll((scroll_offset as u16, 0));
    frame.render_widget(paragraph, inner);
}

/// Map a status string to a terminal color.
pub fn status_color(status: &str) -> Color {
    match status {
        "connected" => Color::Green,
        "connecting" | "disconnecting" => Color::Yellow,
        "disconnected" | "stopped" => Color::Red,
        "started" => Color::White,
        _ => Color::DarkGray,
    }
}

/// Check for user input (non-blocking with short timeout).
pub fn poll_input(timeout: Duration) -> anyhow::Result<Option<InputEvent>> {
    if event::poll(timeout)? {
        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') => return Ok(Some(InputEvent::Quit)),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(Some(InputEvent::Quit))
                }
                _ => {}
            }
        }
    }
    Ok(None)
}

pub enum InputEvent {
    Quit,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use crate::config::PeerAssignment;

    #[test]
    fn format_elapsed_zero() {
        assert_eq!(format_elapsed(0), "00:00:00");
    }

    #[test]
    fn format_elapsed_hours_and_mins() {
        // 1 hour, 23 minutes, 45 seconds = 5025 seconds
        assert_eq!(format_elapsed(5025), "01:23:45");
    }

    #[test]
    fn build_batches_grouping() {
        let commands = vec![
            crate::config::TestCommand {
                time: 0,
                peer: PeerName::new("alice"),
                command: "connect".to_string(),
            },
            crate::config::TestCommand {
                time: 0,
                peer: PeerName::new("bob"),
                command: "connect".to_string(),
            },
            crate::config::TestCommand {
                time: 10,
                peer: PeerName::new("alice"),
                command: "push".to_string(),
            },
        ];
        let assignments = vec![PeerAssignment {
            peer_name: PeerName::new("alice"),
            host: crate::config::HostConfig {
                address: "host0".to_string(),
                name: None,
                ssh_user: "user".to_string(),
                ssh_auth: "agent".to_string(),
                base_port: 10000,
                tags: Vec::new(),
            },
            port: 10000,
            listen_addr: "/ip4/0.0.0.0/udp/10000/quic-v1".to_string(),
            extra_env: HashMap::new(),
            docker_image: "test:latest".to_string(),
        }];
        let batches = build_batches(&commands, &assignments);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].time, 0);
        assert_eq!(batches[0].commands.len(), 2);
        assert_eq!(batches[0].delta, 0);
        assert_eq!(batches[1].time, 10);
        assert_eq!(batches[1].commands.len(), 1);
        assert_eq!(batches[1].delta, 10);
    }

    #[test]
    fn status_color_mapping() {
        assert_eq!(status_color("connected"), Color::Green);
        assert_eq!(status_color("connecting"), Color::Yellow);
        assert_eq!(status_color("disconnecting"), Color::Yellow);
        assert_eq!(status_color("disconnected"), Color::Red);
        assert_eq!(status_color("stopped"), Color::Red);
        assert_eq!(status_color("started"), Color::White);
        assert_eq!(status_color("unknown"), Color::DarkGray);
    }
}
