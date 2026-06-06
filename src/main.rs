//! smartty — a smart PTY proxy.
//!
//! Wraps an interactive command, forwards terminal I/O transparently, and (in
//! later milestones) intercepts local input events to render overlay UI on top
//! of the child terminal session. Supported platforms: Linux and macOS.

mod app;
mod cli;
mod input;
mod overlay;
mod pty_session;
mod term;

use cli::Cli;

fn main() {
    let cli = Cli::parse();

    match app::run(&cli) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // The RawModeGuard inside `run` has already restored the terminal by
            // the time we get here, so this prints cleanly.
            eprintln!("smartty: {err:#}");
            std::process::exit(1);
        }
    }
}
