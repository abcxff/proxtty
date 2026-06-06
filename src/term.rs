//! Outer-terminal setup and crash-safe restoration.
//!
//! The single most important correctness property of `proxtty` is that it must
//! never leave the user's terminal stuck in raw mode (or with mouse reporting on,
//! or the cursor hidden). If that happens the shell becomes unusable until
//! `reset` / `stty sane`. We guard against it on three paths: normal exit (Drop),
//! panics (a panic hook), and child exit (Drop runs as the app returns).
//!
//! Mouse reporting is enabled here so the input tap can see Option-click and
//! friends. We enable only button tracking (`?1000`) plus SGR encoding (`?1006`)
//! — not any-motion tracking — to avoid a flood of motion reports.
//!
//! `proxtty` also switches the outer terminal to its **alternate screen**. Since
//! it parses child output into its own screen buffer and repaints from there, it
//! is effectively a full-screen app (like `vim`/`tmux`); using the alt screen
//! means the outer terminal keeps no native scrollback to fight `proxtty`'s own
//! wheel scrollback, and the user's pre-`proxtty` screen is restored on exit.

use std::io::{self, Write};

use crossterm::terminal;

/// Switch to the alternate screen and enable button + SGR mouse reporting.
const SETUP: &[u8] = b"\x1b[?1049h\x1b[?1000h\x1b[?1006h";
/// Undo everything `proxtty` turned on: mouse reporting, input modes mirrored
/// from the child (bracketed paste, application cursor/keypad), the cursor shape,
/// show the cursor, then leave the alternate screen (restoring the user's
/// original screen). The alt-screen exit comes last so the resets apply first.
const TEARDOWN_VISUALS: &[u8] =
    b"\x1b[?1006l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?2004l\x1b[?1l\x1b>\x1b[0 q\x1b[?25h\x1b[?1049l";

/// RAII guard that puts the outer terminal into the mode `proxtty` needs and
/// restores it on drop. Dropping is idempotent — calling [`RawModeGuard::restore`]
/// early is safe and Drop will then do nothing more.
pub struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    /// Enter raw mode, enable mouse reporting, and install a panic hook that
    /// restores everything, so a panic anywhere can't brick the shell.
    pub fn enter() -> io::Result<RawModeGuard> {
        install_panic_hook();
        terminal::enable_raw_mode()?;
        let mut out = io::stdout();
        out.write_all(SETUP)?;
        out.flush()?;
        Ok(RawModeGuard { active: true })
    }

    /// Restore the terminal now. Safe to call more than once.
    pub fn restore(&mut self) {
        if self.active {
            teardown();
            self.active = false;
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Undo every terminal mutation: reset modes/cursor, leave the alternate screen,
/// raw mode off, flush.
fn teardown() {
    let mut out = io::stdout();
    let _ = out.write_all(TEARDOWN_VISUALS);
    let _ = out.flush();
    let _ = terminal::disable_raw_mode();
    let _ = out.flush();
}

/// Chain a terminal-restoring step in front of the existing panic hook, so a
/// panic prints its message to a sane terminal instead of a raw one.
fn install_panic_hook() {
    use std::sync::Once;
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            teardown();
            previous(info);
        }));
    });
}
