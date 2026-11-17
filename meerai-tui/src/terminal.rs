use std::io::{self, Stdout, Write};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

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
static STATUS: AtomicU8 = AtomicU8::new(u8::MAX);

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
    let mut out = io::stdout();
    let _ = out.write_all(b"\x1b]2;meerai\x07");
    let _ = out.write_all(b"\x1b]6;1;bg;default\x07");
    let _ = out.flush();
    STATUS.store(u8::MAX, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalStatus {
    Idle,
    Working,
    Approval,
}

fn notifications_disabled() -> bool {
    std::env::var("MEERAI_NO_NOTIFY")
        .ok()
        .is_some_and(|value| matches!(value.trim(), "1" | "true" | "yes"))
}

/// Set the window title and, on iTerm2, the tab background color.
pub fn set_status(status: TerminalStatus, title: Option<&str>) {
    if notifications_disabled() {
        return;
    }
    let name = title
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("meerai");
    let (code, title, color) = match status {
        TerminalStatus::Idle => (0u8, format!("M | {name} ●"), None),
        TerminalStatus::Working => (1, format!("M | {name} · working ●"), Some((230, 180, 80))),
        TerminalStatus::Approval => (
            2,
            format!("M | {name} · approval needed ●"),
            Some((224, 92, 92)),
        ),
    };
    if STATUS.swap(code, Ordering::Relaxed) == code {
        return;
    }
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]2;{title}\x07");
    match color {
        Some((r, g, b)) => {
            let _ = write!(out, "\x1b]6;1;bg;{r};{g};{b}\x07");
        }
        None => {
            let _ = out.write_all(b"\x1b]6;1;bg;default\x07");
        }
    }
    let _ = out.flush();
}

/// Ring the bell and send an OSC 9 notification while an approval waits.
pub fn notify_approval() {
    if notifications_disabled() {
        return;
    }
    let mut out = io::stdout();
    let _ = out.write_all(b"\x07");
    let _ = out.write_all(b"\x1b]9;meerai needs your approval\x07");
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
