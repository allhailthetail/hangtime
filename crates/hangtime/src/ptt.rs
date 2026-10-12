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

/// Runs the PTT loop until the user quits. Calls `on_ptt` each time the
/// key goes down or up.
pub fn run(mut on_ptt: impl FnMut(bool)) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let enhanced = !cfg!(windows) && terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        )?;
    }
    let hold = cfg!(windows) || enhanced;
    let result = event_loop(hold, &mut on_ptt);
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

fn event_loop(hold: bool, on_ptt: &mut impl FnMut(bool)) -> io::Result<()> {
    if hold {
        notice("hold SPACE to talk, Q to quit");
    } else {
        notice("this terminal can't report key release: SPACE starts and stops talking, Q quits");
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
                on_ptt(false);
            }
            return Ok(());
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
            on_ptt(keyed);
        }
    }
}
