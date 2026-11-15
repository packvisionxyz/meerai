use std::io::{self, Stdout, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use crossterm::cursor::Show;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::backend::CrosstermBackend;
use ratatui::{Frame, Terminal};

static PANIC_HOOK_ONCE: Once = Once::new();
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

pub struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalGuard {
    pub fn enter() -> Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        if let Err(err) = setup_screen() {
            restore_terminal();
            return Err(err);
        }
        let backend = CrosstermBackend::new(io::stdout());
        let mut terminal = match Terminal::new(backend) {
            Ok(terminal) => terminal,
            Err(err) => {
                restore_terminal();
                return Err(err.into());
            }
        };
        if let Err(err) = terminal.clear() {
            restore_terminal();
            return Err(err.into());
        }
        Ok(Self { terminal })
    }

    pub fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> Result<()> {
        self.terminal.draw(render)?;
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = self.terminal.show_cursor();
        restore_terminal();
    }
}

fn setup_screen() -> Result<()> {
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    if matches!(supports_keyboard_enhancement(), Ok(true))
        && execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        )
        .is_ok()
    {
        KEYBOARD_ENHANCED.store(true, Ordering::Relaxed);
    }
    Ok(())
}

pub fn restore_terminal() {
    if KEYBOARD_ENHANCED.swap(false, Ordering::Relaxed) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        Show
    );
    let _ = io::stdout().write_all(b"\x1b]0;\x07");
    let _ = io::stdout().flush();
}

/// Ring the bell and flag the terminal (title + OSC 9) while an approval waits.
pub fn notify_approval() {
    if std::env::var("MEERAI_NO_NOTIFY")
        .ok()
        .is_some_and(|v| matches!(v.trim(), "1" | "true" | "yes"))
    {
        return;
    }
    let mut out = io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.write_all(b"\x1b]0;meerai: approval needed\x07");
    let _ = out.write_all(b"\x1b]9;meerai needs your approval\x07");
    let _ = out.flush();
}

pub fn clear_notification() {
    let mut out = io::stdout();
    let _ = out.write_all(b"\x1b]0;meerai\x07");
    let _ = out.flush();
}

fn install_panic_hook() {
    PANIC_HOOK_ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        // Restore terminal state before the chained hook prints the panic message.
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous(info);
        }));
    });
}
