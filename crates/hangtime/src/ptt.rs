//! Keyboard input: push-to-talk and radio controls. Global hotkeys
//! arrive in phase 4.
//!
//! Holding a key needs key-release events. Windows consoles report them;
//! other terminals only do with the kitty keyboard protocol. Without it,
//! SPACE toggles transmit instead.

use std::io;

use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::{execute, terminal};

use crate::ui;

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
    /// M: mute or unmute the selected talkgroup.
    ToggleMute,
}

/// Reads the keyboard until the user quits. Expects [`ui::start`] to have
/// put the terminal in raw mode.
pub fn run(mut on_input: impl FnMut(Input)) -> io::Result<()> {
    let enhanced = !cfg!(windows) && terminal::supports_keyboard_enhancement().unwrap_or(false);
    if enhanced {
        execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES)
        )?;
    }
    let hold = cfg!(windows) || enhanced;
    ui::set_hold_to_talk(hold);
    let result = event_loop(hold, &mut on_input);
    if enhanced {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    result
}

fn event_loop(hold: bool, on_input: &mut impl FnMut(Input)) -> io::Result<()> {
    let mut keyed = false;
    loop {
        let key = match event::read()? {
            Event::Key(key) => key,
            Event::Resize(..) => {
                ui::redraw();
                continue;
            }
            _ => continue,
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
                KeyCode::Char(c @ '0'..='9') => {
                    on_input(Input::Select(c as usize - '0' as usize));
                }
                KeyCode::Char('v' | 'V') => on_input(Input::ToggleVox),
                KeyCode::Char('m' | 'M') => on_input(Input::ToggleMute),
                KeyCode::Char('+' | '=') => on_input(Input::VoxSensitivity(1)),
                KeyCode::Char('-' | '_') => on_input(Input::VoxSensitivity(-1)),
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
