//! Full-screen terminal UI: a fixed layout redrawn in place, like htop.
//!
//! Any thread can update the screen through the functions here; each
//! update redraws at once. Level bars decay smoothly, so while one is
//! moving a ticker redraws about 15 times a second; it stops as soon as
//! every bar is at rest, so an idle radio does no work. Before [`start`]
//! and after [`stop`], notices go to plain stdout so nothing is lost.

use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

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

/// Level bars span this range, in dBFS.
const BAR_MIN_DB: f32 = -60.0;
const BAR_MAX_DB: f32 = 0.0;

/// How fast a bar drains after the audio stops: the full range in about
/// half a second.
const DECAY_DB_PER_SEC: f32 = 120.0;

/// Redraw interval while a bar is moving.
const TICK: Duration = Duration::from_millis(66);

/// The mic bar stays live this long after the last level report.
const MIC_STALE: Duration = Duration::from_millis(500);

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
}

/// A connected peer row.
#[derive(Debug, Clone)]
pub struct PeerRow {
    pub radio_id: u32,
    pub name: String,
    pub address: String,
}

/// A level that jumps up and drains back down.
#[derive(Debug, Clone, Copy, Default)]
struct Meter {
    peak_db: f32,
    at: Option<Instant>,
}

impl Meter {
    fn shown(&self, now: Instant) -> f32 {
        match self.at {
            Some(at) => self.peak_db - DECAY_DB_PER_SEC * now.duration_since(at).as_secs_f32(),
            None => BAR_MIN_DB,
        }
    }

    fn push(&mut self, db: f32, now: Instant) {
        self.peak_db = self.shown(now).max(db);
        self.at = Some(now);
    }

    fn moving(&self, now: Instant) -> bool {
        self.shown(now) > BAR_MIN_DB
    }
}

struct State {
    title: String,
    activity: Activity,
    status: String,
    /// VOX level and its minimum voice level (dBFS) while VOX is on.
    vox: Option<(u8, f32)>,
    hold_to_talk: bool,
    groups: Vec<GroupRow>,
    talkers: Vec<Option<String>>,
    meters: Vec<Meter>,
    mic: Meter,
    selected: usize,
    peers: Vec<PeerRow>,
    log: VecDeque<(Instant, String)>,
    started: Instant,
}

impl State {
    fn new(title: String, hold_to_talk: bool) -> Self {
        Self {
            title,
            activity: Activity::Idle,
            status: String::new(),
            vox: None,
            hold_to_talk,
            groups: Vec::new(),
            talkers: Vec::new(),
            meters: Vec::new(),
            mic: Meter::default(),
            selected: 0,
            peers: Vec::new(),
            log: VecDeque::new(),
            started: Instant::now(),
        }
    }

    /// True while any bar needs redrawing to animate.
    fn animating(&self, now: Instant) -> bool {
        let mic_live = self.vox.is_some()
            && self
                .mic
                .at
                .is_some_and(|at| now.duration_since(at) < MIC_STALE);
        mic_live || self.meters.iter().any(|meter| meter.moving(now))
    }
}

struct Ui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    state: State,
}

static UI: Mutex<Option<Ui>> = Mutex::new(None);

/// Set while the redraw ticker thread is running.
static TICKING: AtomicBool = AtomicBool::new(false);

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
        state: State::new(title, hold_to_talk),
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

/// `Some((level, minimum voice dBFS))` while VOX is on.
pub fn set_vox(vox: Option<(u8, f32)>) {
    update(|state| state.vox = vox, || {});
    ensure_ticker();
}

pub fn set_groups(groups: Vec<GroupRow>, selected: usize) {
    update(
        |state| {
            state.meters = vec![Meter::default(); groups.len()];
            state.talkers = vec![None; groups.len()];
            state.groups = groups;
            state.selected = selected;
        },
        || {},
    );
}

pub fn set_selected(selected: usize) {
    update(|state| state.selected = selected, || {});
}

/// Who is talking on each group, by row.
pub fn set_talkers(talkers: Vec<Option<String>>) {
    update(|state| state.talkers = talkers, || {});
}

/// Received audio level on group row `index`, in dBFS.
pub fn meter(index: usize, db: f32) {
    let mut guard = UI.lock().unwrap();
    if let Some(ui) = guard.as_mut()
        && let Some(meter) = ui.state.meters.get_mut(index)
    {
        meter.push(db, Instant::now());
    }
    drop(guard);
    ensure_ticker();
}

/// The microphone's level, in dBFS. Shown only while VOX is on. Called
/// for every frame, so it only records; the ticker draws.
pub fn mic_level(db: f32) {
    let mut guard = UI.lock().unwrap();
    let Some(ui) = guard.as_mut() else {
        return;
    };
    if ui.state.vox.is_none() {
        return;
    }
    ui.state.mic.push(db, Instant::now());
    drop(guard);
    ensure_ticker();
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

/// Starts the redraw ticker if it isn't running. It stops itself, under
/// the UI lock, once nothing is animating; an update that lands after
/// that sees it stopped and starts a new one.
fn ensure_ticker() {
    if TICKING.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(|| {
        loop {
            thread::sleep(TICK);
            let mut guard = UI.lock().unwrap();
            let Some(ui) = guard.as_mut() else {
                TICKING.store(false, Ordering::SeqCst);
                return;
            };
            ui.draw();
            if !ui.state.animating(Instant::now()) {
                TICKING.store(false, Ordering::SeqCst);
                return;
            }
        }
    });
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
const MIC: Color = Color::Magenta;

fn panel(title: &str) -> Block<'_> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

fn render(frame: &mut Frame<'_>, state: &State) {
    let group_rows = state.groups.len() + usize::from(state.vox.is_some());
    let [header, middle, log, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(group_rows.max(state.peers.len()).max(1) as u16 + 2),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(frame.area());
    let [groups, peers] =
        Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(middle);

    let now = Instant::now();
    render_header(frame, header, state);
    render_groups(frame, groups, state, now);
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
    if let Some((level, _)) = state.vox {
        let vox = Line::from(vec![
            Span::styled(
                "VOX ",
                Style::default().fg(MIC).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("{level}/5"), Style::default().fg(MIC)),
        ])
        .right_aligned();
        frame.render_widget(Paragraph::new(vox), right);
    }
}

fn render_groups(frame: &mut Frame<'_>, area: Rect, state: &State, now: Instant) {
    let block = panel("Talkgroups");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [labels, bars] =
        Layout::horizontal([Constraint::Length(34), Constraint::Min(0)]).areas(inner);

    let mut label_lines = Vec::new();
    let mut bar_lines = Vec::new();
    for (index, group) in state.groups.iter().enumerate() {
        let selected = index == state.selected;
        let name_style = if selected {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let mut spans = vec![
            Span::styled(
                if selected { "▶ " } else { "  " },
                Style::default().fg(ACCENT),
            ),
            Span::styled(format!("{index} "), Style::default().fg(DIM)),
            Span::styled(group.name.clone(), name_style),
        ];
        match group.kind {
            "keyed" => spans.push(Span::styled(" keyed", Style::default().fg(Color::Yellow))),
            "open" => spans.push(Span::styled(" open", Style::default().fg(Color::Green))),
            _ => {}
        }
        if let Some(Some(talker)) = state.talkers.get(index) {
            spans.push(Span::styled(
                format!("  {talker}"),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        label_lines.push(Line::from(spans));
        let level = state.meters.get(index).map_or(BAR_MIN_DB, |m| m.shown(now));
        bar_lines.push(bar(bars.width, level, None, Color::Green));
    }
    if let Some((_, minimum)) = state.vox {
        label_lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled("MIC", Style::default().fg(MIC).add_modifier(Modifier::BOLD)),
            Span::styled("  your voice", Style::default().fg(DIM)),
        ]));
        bar_lines.push(bar(bars.width, state.mic.shown(now), Some(minimum), MIC));
    }
    frame.render_widget(Paragraph::new(label_lines), labels);
    frame.render_widget(Paragraph::new(bar_lines), bars);
}

/// A horizontal level bar `width` cells wide, in eighths of a cell, with
/// an optional `│` marker at `marker_db`.
fn bar(width: u16, db: f32, marker_db: Option<f32>, colour: Color) -> Line<'static> {
    const PARTS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let width = usize::from(width);
    if width == 0 {
        return Line::default();
    }
    let fraction = |db: f32| ((db - BAR_MIN_DB) / (BAR_MAX_DB - BAR_MIN_DB)).clamp(0.0, 1.0);
    let eighths = (fraction(db) * width as f32 * 8.0) as usize;
    let mut cells: Vec<char> = (0..width)
        .map(|cell| match eighths.saturating_sub(cell * 8) {
            0 => ' ',
            n if n >= 8 => '█',
            n => PARTS[n],
        })
        .collect();
    // The marker stays visible whether or not the bar has reached it: a
    // white line on empty cells, a white notch inside the bar.
    let marker = marker_db.map(|m| ((fraction(m) * width as f32) as usize).min(width - 1));
    if let Some(at) = marker
        && cells[at] == ' '
    {
        cells[at] = '│';
    }
    let style = |cell: usize| {
        if Some(cell) == marker {
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(colour)
        }
    };
    // Group neighbouring cells with the same style into one span.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style = style(0);
    for (index, cell) in cells.into_iter().enumerate() {
        if style(index) != run_style && !run.is_empty() {
            spans.push(Span::styled(std::mem::take(&mut run), run_style));
        }
        run_style = style(index);
        run.push(cell);
    }
    spans.push(Span::styled(run, run_style));
    Line::from(spans)
}

fn render_peers(frame: &mut Frame<'_>, area: Rect, state: &State) {
    let lines: Vec<Line> = if state.peers.is_empty() {
        vec![Line::styled(
            "looking for peers…",
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        )]
    } else {
        state
            .peers
            .iter()
            .map(|peer| {
                Line::from(vec![
                    Span::styled(
                        peer.name.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", peer.address), Style::default().fg(DIM)),
                    Span::styled("  UNTRUSTED", Style::default().fg(Color::Yellow)),
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
        spans.push(Span::styled(
            *key,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
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
        let mut state = State::new("Hangtime · Desk · port 4747 · 61b0:1a7d".into(), true);
        state.started = started;
        state.activity = Activity::Receiving;
        state.status = "RX  parrot on Test".into();
        state.vox = Some((3, -36.0));
        state.groups = vec![
            GroupRow {
                name: "All Call".into(),
                kind: "",
            },
            GroupRow {
                name: "Test".into(),
                kind: "open",
            },
            GroupRow {
                name: "Secure".into(),
                kind: "keyed",
            },
        ];
        state.talkers = vec![None, Some("parrot".into()), Some("Alice".into())];
        state.meters = vec![
            Meter::default(),
            Meter {
                peak_db: -12.0,
                at: Some(started),
            },
            Meter {
                peak_db: -40.0,
                at: Some(started),
            },
        ];
        state.mic = Meter {
            peak_db: -20.0,
            at: Some(started),
        };
        state.selected = 1;
        state.peers = vec![PeerRow {
            radio_id: 1,
            name: "parrot".into(),
            address: "127.0.0.1:4747".into(),
        }];
        state.log = VecDeque::from([
            (
                started,
                "microphone Headset (WH-1000XM4) (48000 Hz, 2 ch)".into(),
            ),
            (started, "parrot joined from 127.0.0.1:4747".into()),
        ]);
        state
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
        let text = screen(&sample(), 100, 17);
        println!("{text}");
        for expected in [
            "Hangtime · Desk",
            "RX  parrot on Test",
            "VOX 3/5",
            "Talkgroups",
            "▶ 1 Test open  parrot",
            "Secure keyed  Alice",
            "MIC",
            "Peers (1)",
            "UNTRUSTED",
            "parrot joined",
            "hold SPACE talk",
            "█",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in\n{text}");
        }
    }

    #[test]
    fn mic_row_only_with_vox() {
        let mut state = sample();
        state.vox = None;
        assert!(!screen(&state, 100, 17).contains("MIC"));
    }

    #[test]
    fn bars_fill_by_level_and_mark_the_vox_minimum() {
        let text = |line: Line| {
            line.spans
                .iter()
                .map(|s| s.content.to_string())
                .collect::<String>()
        };
        assert_eq!(text(bar(10, BAR_MIN_DB, None, Color::Green)).trim(), "");
        assert_eq!(text(bar(10, BAR_MAX_DB, None, Color::Green)), "██████████");
        assert_eq!(text(bar(10, -30.0, None, Color::Green)), "█████     ");
        let marked = text(bar(10, -54.0, Some(-30.0), Color::Green));
        assert_eq!(marked.chars().nth(5), Some('│'));
    }

    #[test]
    fn meters_decay_to_rest() {
        let start = Instant::now();
        let mut meter = Meter::default();
        meter.push(-10.0, start);
        assert!(meter.moving(start));
        assert!(meter.shown(start + Duration::from_millis(200)) < -30.0);
        assert!(!meter.moving(start + Duration::from_secs(1)));
    }

    #[test]
    fn survives_a_tiny_window() {
        screen(&sample(), 20, 5);
    }
}

#[cfg(test)]
mod marker_tests {
    use super::*;

    #[test]
    fn marker_shows_inside_a_full_bar() {
        let line = bar(10, BAR_MAX_DB, Some(-30.0), Color::Magenta);
        let marked: Vec<&Span> = line
            .spans
            .iter()
            .filter(|span| span.style.fg == Some(Color::White))
            .collect();
        assert_eq!(marked.len(), 1);
        assert_eq!(marked[0].content, "█");
    }
}
