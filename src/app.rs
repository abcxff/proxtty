//! The proxy event loop.
//!
//! Milestones 1–3 were a pair of dumb byte pumps. Milestones 4–5 add local input
//! interception and a crude overlay, which require a single owner of application
//! state. So I/O now flows as [`AppEvent`]s into one event loop on the main
//! thread, which decides — per the input policy — whether to forward input to the
//! child, open/close the overlay, or route input to an open overlay.
//!
//! Threading model:
//!   - reader thread: PTY master  → `AppEvent::Output`
//!   - stdin thread:  raw stdin   → `InputParser` → `AppEvent::Input`
//!   - resize thread: SIGWINCH    → `AppEvent::Resize`
//!   - main thread:   event loop; also watches child exit + termination signals

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossterm::terminal;
use portable_pty::PtySize;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGWINCH};

use crate::cli::Cli;
use crate::input::{InputEvent, InputParser, MouseButton, MouseEvent, MouseKind};
use crate::overlay::{self, MenuOutcome, MenuState, Overlay};
use crate::pty_session::{self, MasterHandle, PtySession};
use crate::term::RawModeGuard;

/// Local trigger byte. Ctrl-Space (NUL) by default; configurable in Milestone 14.
const HOTKEY: u8 = 0x00;

/// An event delivered to the main loop from one of the I/O threads.
enum AppEvent {
    Input(InputEvent),
    Output(Vec<u8>),
    Resize(u16, u16),
}

/// Run the proxy for `cli`'s command. Returns the child's exit code.
pub fn run(cli: &Cli) -> anyhow::Result<i32> {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    // Raw mode + mouse reporting, restored on every exit path including panics.
    let mut guard = RawModeGuard::enter()?;

    let (mut session, reader, writer) = PtySession::spawn(cli.program(), cli.args(), size)?;

    // Bounded so a flood of child output applies backpressure rather than growing
    // memory without bound.
    let (tx, rx) = sync_channel::<AppEvent>(256);

    spawn_pty_reader(reader, tx.clone());
    spawn_stdin_reader(tx.clone());
    spawn_resize_thread(tx.clone());
    drop(tx); // main holds only the receiver; producers hold the clones.

    // External termination should still tear us down cleanly. In raw mode the
    // keyboard never raises SIGINT (Ctrl-C goes to the child as a byte).
    let terminate = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT, SIGHUP] {
        signal_hook::flag::register(sig, Arc::clone(&terminate))?;
    }

    let mut app = App::new((cols, rows), writer, session.master());

    let exit_code = loop {
        if let Some(status) = session.child.try_wait()? {
            break status.exit_code() as i32;
        }
        if terminate.load(Ordering::Relaxed) {
            let _ = session.child.kill();
            let _ = session.child.wait();
            break 130;
        }
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(event) => app.handle(event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                break session.child.try_wait()?.map(|s| s.exit_code() as i32).unwrap_or(1);
            }
        }
    };

    // Leave the screen tidy if the overlay was open at exit.
    app.force_close_overlay();
    guard.restore();
    Ok(exit_code)
}

/// Owns all mutable application state touched by the event loop.
struct App {
    size: (u16, u16),
    overlay: Overlay,
    writer: Box<dyn Write + Send>,
    out: io::Stdout,
    master: MasterHandle,
    /// Child output captured while an overlay is open, replayed on close.
    buffered: Vec<u8>,
}

/// Result of dispatching an input event to an open overlay.
enum OverlayAct {
    /// No overlay was open; handle the event against the ground state.
    NotOpen,
    /// Redraw the overlay with these bytes.
    Redraw(Vec<u8>),
    /// Close the overlay.
    Close,
}

impl App {
    fn new(size: (u16, u16), writer: Box<dyn Write + Send>, master: MasterHandle) -> App {
        App {
            size,
            overlay: Overlay::None,
            writer,
            out: io::stdout(),
            master,
            buffered: Vec::new(),
        }
    }

    fn handle(&mut self, event: AppEvent) {
        match event {
            AppEvent::Output(bytes) => self.on_output(bytes),
            AppEvent::Input(ev) => self.on_input(ev),
            AppEvent::Resize(cols, rows) => self.on_resize(cols, rows),
        }
    }

    fn on_output(&mut self, bytes: Vec<u8>) {
        if self.overlay.is_open() {
            // Suppress display while the overlay is up; replay on close.
            self.buffered.extend_from_slice(&bytes);
        } else {
            let _ = self.out.write_all(&bytes);
            let _ = self.out.flush();
        }
    }

    fn on_input(&mut self, ev: InputEvent) {
        // First, give an open overlay a chance to consume the event. The borrow
        // of `self.overlay` ends with this expression so we can mutate `self`
        // afterward to redraw/close.
        let act = if let Overlay::Menu(menu) = &mut self.overlay {
            match overlay::handle(menu, &ev) {
                MenuOutcome::Stay => OverlayAct::Redraw(overlay::redraw_sequence(menu)),
                MenuOutcome::Close => OverlayAct::Close,
                MenuOutcome::Selected(_idx) => OverlayAct::Close, // placeholder action
            }
        } else {
            OverlayAct::NotOpen
        };

        match act {
            OverlayAct::Redraw(seq) => self.write_raw(&seq),
            OverlayAct::Close => self.close_overlay(),
            OverlayAct::NotOpen => self.on_input_ground(ev),
        }
    }

    /// Input policy when no overlay is open.
    fn on_input_ground(&mut self, ev: InputEvent) {
        match ev {
            InputEvent::Hotkey => self.open_overlay(default_anchor()),
            InputEvent::Mouse(m) if is_overlay_trigger(&m) => {
                self.open_overlay((m.col + 1, m.row + 1))
            }
            // Non-trigger mouse events are dropped for now; forwarding them to the
            // child according to its requested mouse modes is Milestone 9.
            InputEvent::Mouse(_) => {}
            InputEvent::Forward(bytes) => {
                let _ = self.writer.write_all(&bytes);
                let _ = self.writer.flush();
            }
        }
    }

    fn on_resize(&mut self, cols: u16, rows: u16) {
        self.size = (cols, rows);
        pty_session::resize(&self.master, rows, cols);
        // The crude overlay can't reflow against a resized screen, so dismiss it.
        if self.overlay.is_open() {
            self.close_overlay();
        }
    }

    fn open_overlay(&mut self, anchor: (u16, u16)) {
        let menu = MenuState::new(anchor.0, anchor.1, self.size);
        let seq = overlay::open_sequence(&menu);
        self.overlay = Overlay::Menu(menu);
        self.write_raw(&seq);
    }

    fn close_overlay(&mut self) {
        if let Overlay::Menu(menu) = std::mem::replace(&mut self.overlay, Overlay::None) {
            let seq = overlay::close_sequence(&menu);
            self.write_raw(&seq);
        }
        // Replay whatever the child emitted while the overlay was up.
        if !self.buffered.is_empty() {
            let buffered = std::mem::take(&mut self.buffered);
            let _ = self.out.write_all(&buffered);
            let _ = self.out.flush();
        }
    }

    /// Restore the screen if we exit with the overlay still open.
    fn force_close_overlay(&mut self) {
        if self.overlay.is_open() {
            self.close_overlay();
        }
    }

    fn write_raw(&mut self, bytes: &[u8]) {
        let _ = self.out.write_all(bytes);
        let _ = self.out.flush();
    }
}

/// Where the hotkey-triggered menu appears when there's no click position.
fn default_anchor() -> (u16, u16) {
    (3, 2)
}

/// Does this mouse event open the overlay? Option-click is the primary trigger;
/// Ctrl-click and right-click are fallbacks for terminals that swallow Alt.
fn is_overlay_trigger(m: &MouseEvent) -> bool {
    if m.kind != MouseKind::Down {
        return false;
    }
    match m.button {
        MouseButton::Left => m.alt || m.ctrl,
        MouseButton::Right => true,
        _ => false,
    }
}

/// PTY output → `AppEvent::Output`.
fn spawn_pty_reader(mut reader: Box<dyn Read + Send>, tx: SyncSender<AppEvent>) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(AppEvent::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Raw stdin → `InputParser` → `AppEvent::Input`. Detached: a blocking stdin
/// read can't be cleanly interrupted, so the process exit reaps it.
fn spawn_stdin_reader(tx: SyncSender<AppEvent>) {
    thread::spawn(move || {
        let mut parser = InputParser::new(HOTKEY);
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        let mut buf = [0u8; 8192];
        loop {
            match handle.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for ev in parser.feed(&buf[..n]) {
                        if tx.send(AppEvent::Input(ev)).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
}

/// SIGWINCH → `AppEvent::Resize`.
fn spawn_resize_thread(tx: SyncSender<AppEvent>) {
    let mut signals = match signal_hook::iterator::Signals::new([SIGWINCH]) {
        Ok(s) => s,
        Err(_) => return,
    };
    thread::spawn(move || {
        for _ in signals.forever() {
            if let Ok((cols, rows)) = terminal::size() {
                if tx.send(AppEvent::Resize(cols, rows)).is_err() {
                    break;
                }
            }
        }
    });
}
