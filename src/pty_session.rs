//! Child process running inside a pseudo-terminal.
//!
//! Wraps `portable-pty` to spawn the requested command attached to a PTY whose
//! master side `smartty` controls. The master is kept behind an `Arc<Mutex<_>>`
//! so the resize handler thread can adjust the window size while the I/O threads
//! own the reader and writer halves.

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

/// A shareable handle to the PTY master, used for resizing from other threads.
pub type MasterHandle = Arc<Mutex<Box<dyn MasterPty + Send>>>;

/// A spawned session plus its master reader (child → us) and writer (us → child).
type SpawnResult = (PtySession, Box<dyn Read + Send>, Box<dyn Write + Send>);

/// A spawned child attached to a PTY.
pub struct PtySession {
    master: MasterHandle,
    /// The running child process.
    pub child: Box<dyn Child + Send + Sync>,
}

impl PtySession {
    /// Spawn `program` (with `args`) in a fresh PTY of the given size.
    ///
    /// Returns the session plus the master reader (child output → us) and writer
    /// (our input → child). The reader and writer are handed to dedicated I/O
    /// threads; the session retains the master for resizing and the child handle
    /// for lifecycle management.
    pub fn spawn(program: &str, args: &[String], size: PtySize) -> anyhow::Result<SpawnResult> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size)?;

        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);

        // Inherit the parent environment explicitly so TERM/PATH/HOME and the
        // rest reach the child deterministically, then anchor it to our cwd.
        for (key, value) in std::env::vars() {
            cmd.env(key, value);
        }
        if let Ok(cwd) = std::env::current_dir() {
            cmd.cwd(cwd);
        }

        let child = pair.slave.spawn_command(cmd)?;

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;

        // Drop the slave handle so the child holds the only reference to the
        // slave side; otherwise the master never sees EOF when the child exits.
        drop(pair.slave);

        let session = PtySession {
            master: Arc::new(Mutex::new(pair.master)),
            child,
        };
        Ok((session, reader, writer))
    }

    /// A clone of the master handle for use by the resize thread.
    pub fn master(&self) -> MasterHandle {
        Arc::clone(&self.master)
    }
}

/// Resize the PTY referenced by `handle` to `rows` x `cols`.
pub fn resize(handle: &MasterHandle, rows: u16, cols: u16) {
    if let Ok(master) = handle.lock() {
        let _ = master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }
}
