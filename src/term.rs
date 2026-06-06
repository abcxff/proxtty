//! Outer-terminal raw-mode management and crash-safe restoration.
//!
//! The single most important correctness property of `smartty` is that it must
//! never leave the user's terminal stuck in raw mode. If that happens the shell
//! becomes unusable until `reset` / `stty sane`. We guard against it on three
//! paths: normal exit (Drop), panics (a panic hook), and child exit (Drop runs
//! when the guard goes out of scope as the app returns).

use std::io::{self, Write};

use crossterm::terminal;

/// RAII guard that enables raw mode on construction and restores the terminal
/// on drop. Dropping is idempotent — calling [`RawModeGuard::restore`] early is
/// safe and the Drop will simply do nothing more.
pub struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    /// Enter raw mode on the outer terminal and install a panic hook that
    /// restores it, so a panic anywhere in the program can't brick the shell.
    pub fn enter() -> io::Result<RawModeGuard> {
        install_panic_hook();
        terminal::enable_raw_mode()?;
        Ok(RawModeGuard { active: true })
    }

    /// Restore the terminal now. Safe to call more than once.
    pub fn restore(&mut self) {
        if self.active {
            let _ = terminal::disable_raw_mode();
            // Best-effort flush so any final child output is visible.
            let _ = io::stdout().flush();
            self.active = false;
        }
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Chain a terminal-restoring step in front of the existing panic hook, so a
/// panic prints its message to a sane terminal instead of a raw one.
fn install_panic_hook() {
    use std::sync::Once;
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = terminal::disable_raw_mode();
            let _ = io::stdout().flush();
            previous(info);
        }));
    });
}
