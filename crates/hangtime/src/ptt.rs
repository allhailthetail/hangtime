//! Push-to-talk from the terminal. Global hotkeys arrive in phase 4.
//!
//! Holding a key needs key-release events. Windows consoles report them;
//! other terminals only do with the kitty keyboard protocol. Without it,
//! SPACE toggles transmit instead.

use std::io::{self, Write};

use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal};

/// Runs the PTT loop until the user quits. Calls `set_ptt` on each change.
pub fn run(mut set_ptt: impl FnMut(bool)) -> io::Result<()> {
    terminal::enable_raw_mode()?;
    let enhanced = !cfg!(windows) && terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        )?;
    }
    let hold = cfg!(windows) || enhanced;
    let result = event_loop(hold, &mut set_ptt);
    if enhanced {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = terminal::disable_raw_mode();
    println!();
    result
}

/// Prints a line without disturbing the status line. Safe in raw mode.
pub fn notice(message: &str) {
    print!("\r\n{message}\r\n");
    let _ = io::stdout().flush();
}

fn event_loop(hold: bool, set_ptt: &mut impl FnMut(bool)) -> io::Result<()> {
    if hold {
        notice("hold SPACE to talk, Q to quit");
    } else {
        notice("this terminal can't report key release: SPACE starts and stops talking, Q quits");
    }
    show(false)?;
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
                set_ptt(false);
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
            set_ptt(keyed);
            show(keyed)?;
        }
    }
}

fn show(keyed: bool) -> io::Result<()> {
    let status = if keyed {
        "[TX] transmitting"
    } else {
        "[  ] idle        "
    };
    print!("\r{status}");
    io::stdout().flush()
}
