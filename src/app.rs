//! The proxy event loop.
//!
//! Output no longer goes straight to stdout: it flows through a `vt100` parser
//! ([`TerminalScreen`]) and is painted by the [`Renderer`], which keeps a copy of
//! what's on screen so it can diff (fast incremental paints) or repaint (clean
//! slate). That parsed screen is what makes overlay compositing clean — the menu
//! is drawn on top of the painted screen, and closing it just repaints the child
//! screen, wiping the menu with no leftover rectangle.
//!
//! Rendering is *coalesced*: a burst of output events updates the screen buffer
//! and marks it dirty, then a single paint happens once the burst drains. While
//! an overlay is open, painting is deferred entirely and the screen is repainted
//! on close.
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

use crate::actions;
use crate::config::{ActionSpec, Config, TriggerMods};
use crate::input::{InputEvent, InputParser, MouseButton, MouseEvent, MouseKind};
use crate::mouse;
use crate::overlay::{self, MenuOutcome, MenuState, Overlay};
use crate::pty_session::{self, MasterHandle, PtySession};
use crate::renderer::Renderer;
use crate::screen::TerminalScreen;
use crate::term::RawModeGuard;

/// An event delivered to the main loop from one of the I/O threads.
enum AppEvent {
    Input(InputEvent),
    Output(Vec<u8>),
    Resize(u16, u16),
}

/// Run the proxy for `command` with the given `config`. Returns the child's exit
/// code. `command` is guaranteed non-empty by the caller.
pub fn run(command: &[String], config: &Config) -> anyhow::Result<i32> {
    // A terminal that reports a zero dimension (or no size at all) would make the
    // screen model degenerate, so fall back to a sane default.
    let (mut cols, mut rows) = terminal::size().unwrap_or((80, 24));
    if cols == 0 {
        cols = 80;
    }
    if rows == 0 {
        rows = 24;
    }
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    // Raw mode + mouse reporting, restored on every exit path including panics.
    let mut guard = RawModeGuard::enter()?;

    let (mut session, reader, writer) = PtySession::spawn(&command[0], &command[1..], size)?;

    // Bounded so a flood of child output applies backpressure rather than growing
    // memory without bound.
    let (tx, rx) = sync_channel::<AppEvent>(256);

    spawn_pty_reader(reader, tx.clone());
    spawn_stdin_reader(tx.clone(), config.hotkey_byte());
    spawn_resize_thread(tx.clone());
    drop(tx); // main holds only the receiver; producers hold the clones.

    // External termination should still tear us down cleanly. In raw mode the
    // keyboard never raises SIGINT (Ctrl-C goes to the child as a byte).
    let terminate = Arc::new(AtomicBool::new(false));
    for sig in [SIGTERM, SIGINT, SIGHUP] {
        signal_hook::flag::register(sig, Arc::clone(&terminate))?;
    }

    let mut app = App::new((cols, rows), writer, session.master(), config);

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
            Ok(event) => {
                app.handle(event);
                // Drain the rest of the burst before painting once.
                while let Ok(event) = rx.try_recv() {
                    app.handle(event);
                }
                app.flush_render();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                break session
                    .child
                    .try_wait()?
                    .map(|s| s.exit_code() as i32)
                    .unwrap_or(1);
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
    /// Input to forward to the child PTY.
    writer: Box<dyn Write + Send>,
    master: MasterHandle,
    screen: TerminalScreen,
    renderer: Renderer,
    /// The screen buffer changed since the last paint.
    dirty: bool,
    /// Scrollback view offset from the live bottom (0 = live). When non-zero the
    /// view is frozen and live output is not painted until the user scrolls back.
    scroll_offset: usize,
    /// Last input modes mirrored onto the outer terminal (so its keys/pastes are
    /// encoded the way the child expects). All start off, matching a fresh raw
    /// terminal.
    im_app_cursor: bool,
    im_app_keypad: bool,
    im_bracketed_paste: bool,
    /// Menu item labels and the action each performs (parallel, from config).
    menu_labels: Vec<String>,
    menu_actions: Vec<ActionSpec>,
    /// Whether to draw a border around the menu.
    menu_border: bool,
    /// Which mouse modifiers open the overlay.
    trigger: TriggerMods,
}

/// Result of dispatching an input event to an open overlay.
enum OverlayAct {
    /// No overlay was open; handle the event against the ground state.
    NotOpen,
    /// Redraw the overlay with these bytes.
    Redraw(Vec<u8>),
    /// Close the overlay.
    Close,
    /// An item was chosen; run its action, then close.
    Selected(usize),
}

impl App {
    fn new(
        size: (u16, u16),
        writer: Box<dyn Write + Send>,
        master: MasterHandle,
        config: &Config,
    ) -> App {
        let (cols, rows) = size;
        let menu_labels = config.menu.iter().map(|m| m.label.clone()).collect();
        let menu_actions = config.menu.iter().map(|m| m.action.clone()).collect();
        App {
            size,
            overlay: Overlay::None,
            writer,
            master,
            screen: TerminalScreen::new(rows, cols),
            renderer: Renderer::new(rows, cols),
            dirty: false,
            scroll_offset: 0,
            im_app_cursor: false,
            im_app_keypad: false,
            im_bracketed_paste: false,
            menu_labels,
            menu_actions,
            menu_border: config.border,
            trigger: config.trigger_mods(),
        }
    }

    fn handle(&mut self, event: AppEvent) {
        match event {
            AppEvent::Output(bytes) => self.on_output(&bytes),
            AppEvent::Input(ev) => self.on_input(ev),
            AppEvent::Resize(cols, rows) => self.on_resize(cols, rows),
        }
    }

    /// Update the screen buffer; defer painting to `flush_render`.
    fn on_output(&mut self, bytes: &[u8]) {
        self.screen.process(bytes);
        self.dirty = true;
    }

    /// Paint pending output, unless an overlay is up or the scrollback view is
    /// frozen (both wait until they close / return to the live bottom).
    fn flush_render(&mut self) {
        self.sync_input_modes();
        if self.dirty && !self.overlay.is_open() && self.scroll_offset == 0 {
            self.renderer.render(self.screen.current());
            self.dirty = false;
        }
    }

    /// Mirror the child's input modes (application cursor/keypad, bracketed
    /// paste) onto the outer terminal so the keys and pastes it sends us are
    /// encoded the way the child expects. Mouse modes are managed separately
    /// because `smartty` always needs the mouse for its own trigger.
    fn sync_input_modes(&mut self) {
        let screen = self.screen.current();
        let app_cursor = screen.application_cursor();
        let app_keypad = screen.application_keypad();
        let bracketed = screen.bracketed_paste();

        let mut seq: Vec<u8> = Vec::new();
        if app_cursor != self.im_app_cursor {
            seq.extend_from_slice(if app_cursor { b"\x1b[?1h" } else { b"\x1b[?1l" });
            self.im_app_cursor = app_cursor;
        }
        if app_keypad != self.im_app_keypad {
            seq.extend_from_slice(if app_keypad { b"\x1b=" } else { b"\x1b>" });
            self.im_app_keypad = app_keypad;
        }
        if bracketed != self.im_bracketed_paste {
            seq.extend_from_slice(if bracketed { b"\x1b[?2004h" } else { b"\x1b[?2004l" });
            self.im_bracketed_paste = bracketed;
        }
        if !seq.is_empty() {
            self.renderer.write_raw(&seq);
        }
    }

    fn on_input(&mut self, ev: InputEvent) {
        // Bring the screen current before acting on input, so an overlay opens
        // over a freshly-painted screen.
        self.flush_render();

        let act = if let Overlay::Menu(menu) = &mut self.overlay {
            match overlay::handle(menu, &ev) {
                MenuOutcome::Stay => OverlayAct::Redraw(overlay::redraw_sequence(menu)),
                MenuOutcome::Close => OverlayAct::Close,
                MenuOutcome::Selected(idx) => OverlayAct::Selected(idx),
            }
        } else {
            OverlayAct::NotOpen
        };

        match act {
            OverlayAct::Redraw(seq) => self.renderer.write_raw(&seq),
            OverlayAct::Close => self.close_overlay(),
            OverlayAct::Selected(idx) => {
                // Run the action first (it may write to the child), then close
                // so the repaint reflects any change.
                self.run_menu_action(idx);
                self.close_overlay();
            }
            OverlayAct::NotOpen => self.on_input_ground(ev),
        }
    }

    /// Input policy when no overlay is open.
    fn on_input_ground(&mut self, ev: InputEvent) {
        match ev {
            InputEvent::Hotkey => self.open_overlay(default_anchor()),
            InputEvent::Mouse(m) if self.is_overlay_trigger(&m) => {
                self.open_overlay((m.col + 1, m.row + 1))
            }
            // Wheel scrolling drives smartty's scrollback when the child isn't
            // using the mouse and isn't on the alternate screen.
            InputEvent::Mouse(m)
                if matches!(m.kind, MouseKind::ScrollUp | MouseKind::ScrollDown)
                    && !self.screen.child_wants_mouse()
                    && !self.screen.alternate_screen() =>
            {
                self.scroll(m.kind)
            }
            // Otherwise hand the event to the child if it has asked for mouse
            // reporting, re-encoded in its requested protocol.
            InputEvent::Mouse(m) => self.forward_mouse(&m),
            InputEvent::Forward(bytes) => {
                let _ = self.writer.write_all(&bytes);
                let _ = self.writer.flush();
            }
        }
    }

    /// Move the scrollback view up or down by a few lines and repaint it.
    fn scroll(&mut self, kind: MouseKind) {
        const STEP: usize = 3;
        let target = match kind {
            MouseKind::ScrollUp => self.scroll_offset + STEP,
            MouseKind::ScrollDown => self.scroll_offset.saturating_sub(STEP),
            _ => return,
        };
        self.scroll_offset = self.screen.scroll_to(target);
        // Repaint the (possibly frozen) view. Once back at the live bottom this
        // shows current output and normal diff rendering resumes.
        self.renderer.repaint(self.screen.current());
        self.dirty = false;
    }

    /// Re-encode a mouse event and forward it to the child if it wants mouse.
    fn forward_mouse(&mut self, m: &MouseEvent) {
        let screen = self.screen.current();
        let mode = screen.mouse_protocol_mode();
        let encoding = screen.mouse_protocol_encoding();
        if !mouse::should_forward(mode, m.kind) {
            return;
        }
        if let Some(bytes) = mouse::encode(m, encoding) {
            let _ = self.writer.write_all(&bytes);
            let _ = self.writer.flush();
        }
    }

    fn on_resize(&mut self, cols: u16, rows: u16) {
        self.size = (cols, rows);
        self.screen.resize(rows, cols);
        pty_session::resize(&self.master, rows, cols);
        // Return to the live bottom — a scrollback offset is meaningless after a
        // reflow — dismiss the overlay, and repaint the resized screen cleanly.
        self.scroll_offset = self.screen.scroll_to(0);
        self.overlay = Overlay::None;
        self.renderer.repaint(self.screen.current());
        self.dirty = false;
    }

    fn open_overlay(&mut self, anchor: (u16, u16)) {
        if self.menu_labels.is_empty() {
            return;
        }
        let menu = MenuState::new(
            self.menu_labels.clone(),
            self.menu_border,
            anchor.0,
            anchor.1,
            self.size,
        );
        let seq = overlay::open_sequence(&menu);
        self.overlay = Overlay::Menu(menu);
        self.renderer.write_raw(&seq);
    }

    /// Run the action bound to menu item `idx`.
    fn run_menu_action(&mut self, idx: usize) {
        let Some(action) = self.menu_actions.get(idx).cloned() else {
            return;
        };
        match action {
            ActionSpec::Send { text } => {
                let _ = self.writer.write_all(text.as_bytes());
                let _ = self.writer.flush();
            }
            ActionSpec::CopyScreen => {
                let text = self.screen.visible_text();
                self.renderer.write_raw(&actions::osc52_copy(&text));
            }
            ActionSpec::OpenUrl => {
                let text = self.screen.visible_text();
                if let Some(url) = actions::find_url(&text) {
                    open_url(url);
                }
            }
        }
    }

    /// Does this mouse event open the overlay, per the configured trigger?
    fn is_overlay_trigger(&self, m: &MouseEvent) -> bool {
        if m.kind != MouseKind::Down || m.button != MouseButton::Left {
            return false;
        }
        match self.trigger {
            TriggerMods::Alt => m.alt,
            TriggerMods::Ctrl => m.ctrl,
            TriggerMods::Any => m.alt || m.ctrl,
        }
    }

    /// Close the overlay and repaint the child screen, which erases the menu.
    fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
        self.renderer.repaint(self.screen.current());
        self.dirty = false;
    }

    /// Restore the screen if we exit with the overlay still open.
    fn force_close_overlay(&mut self) {
        if self.overlay.is_open() {
            self.close_overlay();
        }
    }
}

/// Where the hotkey-triggered menu appears when there's no click position.
fn default_anchor() -> (u16, u16) {
    (3, 2)
}

/// Open `url` in the user's default application, reaping the helper process so it
/// doesn't linger as a zombie.
fn open_url(url: String) {
    use std::process::Command;
    if let Ok(mut child) = Command::new(actions::opener()).arg(url).spawn() {
        thread::spawn(move || {
            let _ = child.wait();
        });
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
fn spawn_stdin_reader(tx: SyncSender<AppEvent>, hotkey: u8) {
    thread::spawn(move || {
        let mut parser = InputParser::new(hotkey);
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
