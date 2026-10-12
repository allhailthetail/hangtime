//! Push-to-talk from the terminal. Global hotkeys arrive in phase 4.
//!
//! Holding a key needs key-release events. Windows consoles report them;
//! other terminals only do with the kitty keyboard protocol. Without it,
//! SPACE toggles transmit instead.

use std::io::{self, Write};
use std::sync::Mutex;

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal};

/// Width the status line is padded to, so shorter text clears longer.
const STATUS_WIDTH: usize = 40;

/// Text on the status line, redrawn after each notice.
static STATUS: Mutex<String> = Mutex::new(String::new());

/// What the user did at the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// The PTT key went down (true) or up (false).
    Ptt(bool),
    /// A number key: select that talkgroup.
    Select(usize),
    /// Tab: select the next talkgroup.
    Next,
    /// V: turn VOX on or off.
    ToggleVox,
    /// + or -: make VOX more or less sensitive.
    VoxSensitivity(i8),
}

/// Runs the keyboard loop until the user quits.
pub fn run(mut on_input: impl FnMut(Input)) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let enhanced = !cfg!(windows) && terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        )?;
    }
    let hold = cfg!(windows) || enhanced;
    let result = event_loop(hold, &mut on_input);
    if enhanced {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

/// Prints a line in place of the status line, then redraws the status
/// line below it. Safe in raw mode.
pub fn notice(message: &str) {
    print!("\r{message:<STATUS_WIDTH$}\r\n");
    redraw();
}

/// Replaces the status line.
pub fn set_status(text: &str) {
    if let Ok(mut status) = STATUS.lock() {
        text.clone_into(&mut status);
    }
    redraw();
}

fn redraw() {
    if let Ok(status) = STATUS.lock() {
        print!("\r{:<STATUS_WIDTH$}", *status);
    }
    let _ = io::stdout().flush();
}

fn event_loop(hold: bool, on_input: &mut impl FnMut(Input)) -> io::Result<()> {
    if hold {
        notice(
            "hold SPACE to talk, V for VOX (+/- sensitivity), TAB or 0-9 for talkgroup, Q quits",
        );
    } else {
        notice("this terminal can't report key release: SPACE starts and stops talking");
        notice("V toggles VOX (+/- sensitivity), TAB or 0-9 changes talkgroup, Q quits");
    }
    let mut keyed = false;
    loop {
        let Event::Key(key) = event::read()? else {
            continue;
        };
        let ctrl_c =
            key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL);
        if key.kind == KeyEventKind::Press
            && (ctrl_c || matches!(key.code, KeyCode::Char('q' | 'Q') | KeyCode::Esc))
        {
            if keyed {
                on_input(Input::Ptt(false));
            }
            return Ok(());
        }
        if key.kind == KeyEventKind::Press {
            match key.code {
                KeyCode::Tab => on_input(Input::Next),
                KeyCode::Char('v' | 'V') => on_input(Input::ToggleVox),
                KeyCode::Char('+' | '=') => on_input(Input::VoxSensitivity(1)),
                KeyCode::Char('-' | '_') => on_input(Input::VoxSensitivity(-1)),
                KeyCode::Char(c @ '0'..='9') => {
                    on_input(Input::Select(c as usize - '0' as usize));
                }
                _ => {}
            }
        }
        if key.code != KeyCode::Char(' ') {
            continue;
        }
        let want = match (hold, key.kind) {
            (true, KeyEventKind::Release) => false,
            (true, _) => true,
            (false, KeyEventKind::Press) => !keyed,
            (false, _) => keyed,
        };
        if want != keyed {
            keyed = want;
            on_input(Input::Ptt(keyed));
        }
    }
}
