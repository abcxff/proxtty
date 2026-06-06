//! The dumb-passthrough proxy loop.
//!
//! This is the Milestone 1–3 core: spawn the child in a PTY, forward bytes both
//! ways, forward terminal resizes, and tear down cleanly on exit. No parsing,
//! no overlays yet — just a near-transparent wrapper.
//!
//! Threading model:
//!   - reader thread: PTY master → our stdout
//!   - stdin thread:  our stdin  → PTY master
//!   - resize thread: SIGWINCH   → resize child PTY
//!   - main thread:   waits for the child, watches for termination signals

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossterm::terminal;
use portable_pty::PtySize;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGWINCH};

use crate::cli::Cli;
use crate::pty_session::{self, PtySession};
use crate::term::RawModeGuard;

/// Run the proxy for `cli`'s command. Returns the child's exit code.
pub fn run(cli: &Cli) -> anyhow::Result<i32> {
    // Query the outer terminal size before touching its mode.
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    // Enter raw mode. The guard restores the terminal on every exit path,
    // including panics, so the user's shell is never left wedged.
    let mut guard = RawModeGuard::enter()?;

    let (mut session, reader, writer) =
        PtySession::spawn(cli.program(), cli.args(), size)?;

    // PTY output → stdout.
    let reader_handle = thread::spawn(move || pump_to_stdout(reader));

    // stdin → PTY input. Detached: a blocking read on stdin can't be cleanly
    // interrupted, so we let the process exit reap it.
    thread::spawn(move || pump_from_stdin(writer));

    // SIGWINCH → resize child PTY to match the outer terminal.
    spawn_resize_thread(session.master());

    // Watch for termination signals. In raw mode the keyboard never raises
    // SIGINT (Ctrl-C is delivered to the child as a byte), but an external
    // `kill` should still tear us down gracefully.
    let terminate = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT, SIGHUP] {
        signal_hook::flag::register(sig, Arc::clone(&terminate))?;
    }

    let exit_code = loop {
        if let Some(status) = session.child.try_wait()? {
            break status.exit_code() as i32;
        }
        if terminate.load(Ordering::Relaxed) {
            let _ = session.child.kill();
            let _ = session.child.wait();
            break 130; // 128 + SIGINT, the conventional "terminated" code.
        }
        thread::sleep(Duration::from_millis(10));
    };

    // The reader thread ends naturally once the child exits and the master sees
    // EOF; give it a brief chance to flush the child's final output.
    let _ = reader_handle.join();

    guard.restore();
    Ok(exit_code)
}

/// Copy PTY output to stdout, flushing each chunk so interactive apps feel live.
fn pump_to_stdout(mut reader: Box<dyn Read + Send>) {
    let mut out = io::stdout();
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if out.write_all(&buf[..n]).is_err() {
                    break;
                }
                let _ = out.flush();
            }
        }
    }
}

/// Copy stdin to the PTY master. Runs until the process exits.
fn pump_from_stdin(mut writer: Box<dyn Write + Send>) {
    let stdin = io::stdin();
    let mut handle = stdin.lock();
    let mut buf = [0u8; 8192];
    loop {
        match handle.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if writer.write_all(&buf[..n]).is_err() {
                    break;
                }
                let _ = writer.flush();
            }
        }
    }
}

/// Spawn a thread that resizes the child PTY whenever the outer terminal does.
fn spawn_resize_thread(master: pty_session::MasterHandle) {
    let mut signals = match signal_hook::iterator::Signals::new([SIGWINCH]) {
        Ok(s) => s,
        Err(_) => return,
    };
    thread::spawn(move || {
        for _ in signals.forever() {
            if let Ok((cols, rows)) = terminal::size() {
                pty_session::resize(&master, rows, cols);
            }
        }
    });
}
