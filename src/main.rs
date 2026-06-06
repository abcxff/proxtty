//! smartty — a smart PTY proxy.
//!
//! Wraps an interactive command, forwards terminal I/O transparently, and (in
//! later milestones) intercepts local input events to render overlay UI on top
//! of the child terminal session. Supported platforms: Linux and macOS.

mod actions;
mod app;
mod cli;
mod config;
mod input;
mod mouse;
mod overlay;
mod pty_session;
mod renderer;
mod screen;
mod term;

use cli::Cli;

fn main() {
    let config = config::load();
    let command = Cli::parse().resolve(config.shell.as_deref());

    match app::run(&command, &config) {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // The RawModeGuard inside `run` has already restored the terminal by
            // the time we get here, so this prints cleanly.
            eprintln!("smartty: {err:#}");
            std::process::exit(1);
        }
    }
}
