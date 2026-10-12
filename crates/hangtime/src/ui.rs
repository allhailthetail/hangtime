//! Full-screen terminal UI: a fixed layout redrawn in place, like htop.
//!
//! Any thread can update the screen through the functions here; each
//! update redraws at once. There is no redraw loop, so an idle radio
//! costs nothing. Before [`start`] and after [`stop`], updates go to
//! plain stdout so nothing is lost.

use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::sync::Mutex;
use std::time::Instant;

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::crossterm::{cursor, execute};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph};
use ratatui::{Frame, Terminal};

/// Log lines kept for scrolling.
const LOG_LINES: usize = 200;

/// What the radio is doing, which sets the status colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Transmitting,
    Receiving,
    Busy,
    Hang,
}

/// A talkgroup row.
#[derive(Debug, Clone)]
pub struct GroupRow {
    pub name: String,
    /// "open", "keyed", or empty for All Call.
    pub kind: &'static str,
    pub id: Option<u32>,
}

/// A connected peer row.
#[derive(Debug, Clone)]
pub struct PeerRow {
    pub radio_id: u32,
    pub name: String,
    pub address: String,
    pub fingerprint: String,
}

struct State {
    title: String,
    activity: Activity,
    status: String,
    vox: Option<u8>,
    hold_to_talk: bool,
    groups: Vec<GroupRow>,
    selected: usize,
    peers: Vec<PeerRow>,
    log: VecDeque<(Instant, String)>,
    started: Instant,
}

struct Ui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    state: State,
}

static UI: Mutex<Option<Ui>> = Mutex::new(None);

/// Takes over the terminal. `title` heads the screen.
pub fn start(title: String, hold_to_talk: bool) -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
    // Put the terminal back even if something panics.
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
    let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let mut ui = Ui {
        terminal,
        state: State {
            title,
            activity: Activity::Idle,
            status: String::new(),
            vox: None,
            hold_to_talk,
            groups: Vec::new(),
            selected: 0,
            peers: Vec::new(),
            log: VecDeque::new(),
            started: Instant::now(),
        },
    };
    ui.draw();
    *UI.lock().unwrap() = Some(ui);
    Ok(())
}

/// Gives the terminal back.
pub fn stop() {
    UI.lock().unwrap().take();
    restore();
}

fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, cursor::Show);
}

/// Adds a line to the log.
pub fn notice(message: &str) {
    update(
        |state| {
            if state.log.len() == LOG_LINES {
                state.log.pop_front();
            }
            state.log.push_back((Instant::now(), message.to_string()));
        },
        || println!("{message}"),
    );
}

pub fn set_status(activity: Activity, text: &str) {
    update(
        |state| {
            state.activity = activity;
            text.clone_into(&mut state.status);
        },
        || {},
    );
}

/// Whether SPACE is held to talk (true) or toggles transmit.
pub fn set_hold_to_talk(hold: bool) {
    update(|state| state.hold_to_talk = hold, || {});
}

/// `Some(level)` while VOX is on.
pub fn set_vox(vox: Option<u8>) {
    update(|state| state.vox = vox, || {});
}

pub fn set_groups(groups: Vec<GroupRow>, selected: usize) {
    update(
        |state| {
            state.groups = groups;
            state.selected = selected;
        },
        || {},
    );
}

pub fn set_selected(selected: usize) {
    update(|state| state.selected = selected, || {});
}

pub fn add_peer(peer: PeerRow) {
    update(
        |state| {
            state.peers.retain(|p| p.radio_id != peer.radio_id);
            state.peers.push(peer);
        },
        || {},
    );
}

pub fn remove_peer(radio_id: u32) {
    update(
        |state| state.peers.retain(|p| p.radio_id != radio_id),
        || {},
    );
}

/// Redraws, for example after the window is resized.
pub fn redraw() {
    update(|_| {}, || {});
}

/// Applies `change` and redraws, or runs `fallback` if the UI isn't up.
fn update(change: impl FnOnce(&mut State), fallback: impl FnOnce()) {
    let mut guard = UI.lock().unwrap();
    match guard.as_mut() {
        Some(ui) => {
            change(&mut ui.state);
            ui.draw();
        }
        None => fallback(),
    }
}

impl Ui {
    fn draw(&mut self) {
        let state = &self.state;
        let _ = self.terminal.draw(|frame| render(frame, state));
    }
}

const BORDER: Color = Color::DarkGray;
const DIM: Color = Color::DarkGray;
const ACCENT: Color = Color::Cyan;

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).bold(),
        ))
}

fn render(frame: &mut Frame<'_>, state: &State) {
    let [header, middle, log, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(state.groups.len().max(state.peers.len()).max(1) as u16 + 2),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [groups, peers] =
        Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(middle);

    render_header(frame, header, state);
    render_groups(frame, groups, state);
    render_peers(frame, peers, state);
    render_log(frame, log, state);
    render_footer(frame, footer, state);
}

fn render_header(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let colour = match state.activity {
        Activity::Transmitting => Color::Red,
        Activity::Receiving => Color::Green,
        Activity::Busy => Color::Yellow,
        Activity::Hang => Color::Cyan,
        Activity::Idle => Color::Gray,
    };
    let mut status = Style::default().fg(colour);
    if state.activity != Activity::Idle {
        status = status.add_modifier(Modifier::BOLD);
    }
    let block = panel(&state.title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [left, right] =
        Layout::horizontal([Constraint::Min(10), Constraint::Length(12)]).areas(inner);
    frame.render_widget(
        Paragraph::new(Span::styled(state.status.clone(), status)),
        left,
    );
    if let Some(level) = state.vox {
        let vox = Line::from(vec![
            Span::styled("VOX ", Style::default().fg(Color::Magenta).bold()),
            Span::styled(format!("{level}/5"), Style::default().fg(Color::Magenta)),
        ])
        .right_aligned();
        frame.render_widget(Paragraph::new(vox), right);
    }
}

fn render_groups(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let lines: Vec<Line> = state
        .groups
        .iter()
        .enumerate()
        .map(|(index, group)| {
            let selected = index == state.selected;
            let marker = if selected { "▶ " } else { "  " };
            let name_style = if selected {
                Style::default().fg(ACCENT).bold()
            } else {
                Style::default()
            };
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(ACCENT)),
                Span::styled(format!("{index} "), Style::default().fg(DIM)),
                Span::styled(group.name.clone(), name_style),
            ];
            if let Some(id) = group.id {
                spans.push(Span::styled(format!("  {id}"), Style::default().fg(DIM)));
            }
            match group.kind {
                "keyed" => spans.push(Span::styled("  keyed", Style::default().fg(Color::Yellow))),
                "open" => spans.push(Span::styled("  open", Style::default().fg(Color::Green))),
                _ => {}
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(panel("Talkgroups")), area);
}

fn render_peers(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let lines: Vec<Line> = if state.peers.is_empty() {
        vec![Line::styled(
            "looking for peers…",
            Style::default().fg(DIM).italic(),
        )]
    } else {
        state
            .peers
            .iter()
            .map(|peer| {
                Line::from(vec![
                    Span::styled(peer.name.clone(), Style::default().bold()),
                    Span::styled(format!("  {}", peer.address), Style::default().fg(DIM)),
                    Span::styled("  UNTRUSTED", Style::default().fg(Color::Yellow)),
                    Span::styled(format!("  {}", peer.fingerprint), Style::default().fg(DIM)),
                ])
            })
            .collect()
    };
    let title = format!("Peers ({})", state.peers.len());
    frame.render_widget(Paragraph::new(lines).block(panel(&title)), area);
}

fn render_log(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let block = panel("Log");
    let height = block.inner(area).height as usize;
    let lines: Vec<Line> = state
        .log
        .iter()
        .skip(state.log.len().saturating_sub(height))
        .map(|(at, message)| {
            let secs = at.duration_since(state.started).as_secs();
            Line::from(vec![
                Span::styled(
                    format!("{:02}:{:02}:{:02} ", secs / 3600, secs / 60 % 60, secs % 60),
                    Style::default().fg(DIM),
                ),
                Span::raw(message.clone()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let talk = if state.hold_to_talk {
        "hold SPACE talk"
    } else {
        "SPACE talk on/off"
    };
    let keys = [
        (talk, ""),
        ("V", " VOX"),
        ("+/-", " sensitivity"),
        ("TAB/0-9", " group"),
        ("Q", " quit"),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (i, (key, label)) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(DIM)));
        }
        spans.push(Span::styled(*key, Style::default().fg(ACCENT).bold()));
        spans.push(Span::styled(*label, Style::default().fg(Color::Gray)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn sample() -> State {
        let started = Instant::now();
        State {
            title: "Hangtime · Desk · port 4747 · 61b0:1a7d".into(),
            activity: Activity::Receiving,
            status: "RX  parrot on Test".into(),
            vox: Some(3),
            hold_to_talk: true,
            groups: vec![
                GroupRow {
                    name: "All Call".into(),
                    kind: "",
                    id: None,
                },
                GroupRow {
                    name: "Test".into(),
                    kind: "open",
                    id: Some(100),
                },
                GroupRow {
                    name: "Secure".into(),
                    kind: "keyed",
                    id: Some(200),
                },
            ],
            selected: 1,
            peers: vec![PeerRow {
                radio_id: 1,
                name: "parrot".into(),
                address: "127.0.0.1:4747".into(),
                fingerprint: "16c7:c20e".into(),
            }],
            log: VecDeque::from([
                (
                    started,
                    "microphone Headset (WH-1000XM4) (48000 Hz, 2 ch)".into(),
                ),
                (started, "parrot joined from 127.0.0.1:4747".into()),
            ]),
            started,
        }
    }

    fn screen(state: &State, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, state)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_every_panel() {
        let text = screen(&sample(), 90, 16);
        println!("{text}");
        for expected in [
            "Hangtime · Desk",
            "RX  parrot on Test",
            "VOX 3/5",
            "Talkgroups",
            "▶ 1 Test",
            "Secure  200  keyed",
            "Peers (1)",
            "UNTRUSTED",
            "parrot joined",
            "hold SPACE talk",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
    }

    #[test]
    fn survives_a_tiny_window() {
        screen(&sample(), 20, 5);
    }
}
