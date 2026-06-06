//! The PTY proxy engine.
//!
//! [`Proxy`] wraps an interactive command in a pseudo-terminal and presents a
//! small, un-opinionated library surface:
//!
//! - **You decide what passes through.** Every input event is surfaced via
//!   [`Proxy::poll`] as [`ProxyEvent::Input`]; call [`Proxy::forward`] for the
//!   ones that should reach the child, and simply don't forward the ones you
//!   consume. The proxy never routes input on its own.
//! - **One overlay layer, empty by default.** [`Proxy::set_overlay`] draws raw
//!   ANSI on top of the live child screen (transparent where you don't draw);
//!   [`Proxy::clear_overlay`] returns to emptiness. It's purely visual and
//!   independent of input.
//!
//! Everything driven by *child output* stays automatic: parsing into a `vt100`
//! screen, diff/repaint rendering, scrollback, out-of-band passthrough
//! (title/clipboard/bell/cursor-shape), and mirroring the child's input and
//! mouse-tracking modes onto the outer terminal. Consumers that want to react to
//! the child's screen opt into [`ProxyEvent::ScreenChanged`].
//!
//! Threading model (internal): a PTY-reader thread, a stdin-reader thread
//! (raw stdin → [`InputParser`]), and a SIGWINCH thread feed [`IoEvent`]s over a
//! bounded channel that `poll` drains.

use std::io::{self, Read, Write};
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossterm::terminal;
use portable_pty::PtySize;
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM, SIGWINCH};

use crate::input::{InputEvent, InputParser, MouseEvent, MouseKind};
use crate::mouse;
use crate::pty_session::{self, MasterHandle, PtySession};
use crate::renderer::Renderer;
use crate::screen::TerminalScreen;
use crate::term::RawModeGuard;

/// How long `run_with` blocks per `poll`, also bounding child-exit latency.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Configuration for a [`Proxy`].
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// Byte that is surfaced as [`InputEvent::Hotkey`], or `None` to disable
    /// hotkey interception (every byte flows through as [`InputEvent::Forward`]).
    pub hotkey: Option<u8>,
    /// Lines of scrollback to retain.
    pub scrollback: usize,
    /// Initial terminal size, or `None` to query the outer terminal.
    pub size: Option<(u16, u16)>,
    /// Emit [`ProxyEvent::ScreenChanged`] after each child repaint.
    pub screen_events: bool,
    /// Keep the outer terminal in any-event mouse tracking (`?1003`) so the
    /// consumer receives mouse *movement* (`MouseKind::Moved`), not just clicks.
    /// Generates a lot of events while the mouse moves.
    pub mouse_motion: bool,
}

impl Default for ProxyConfig {
    fn default() -> ProxyConfig {
        ProxyConfig {
            hotkey: Some(0x00), // Ctrl-Space
            scrollback: 10_000,
            size: None,
            screen_events: false,
            mouse_motion: false,
        }
    }
}

/// An event surfaced to the consumer by [`Proxy::poll`].
pub enum ProxyEvent {
    /// An input event from the outer terminal. The proxy does **not** forward it
    /// — call [`Proxy::forward`] to pass it to the child, or consume it.
    Input(InputEvent),
    /// The child repainted its screen (opt-in via [`ProxyConfig::screen_events`],
    /// coalesced to at most one per render). The child screen and any static
    /// overlay are already painted; read [`Proxy::visible_text`] / [`Proxy::size`]
    /// and re-set a dynamic overlay if you keep one.
    ScreenChanged,
    /// The outer terminal (and the child PTY) were resized.
    Resize { cols: u16, rows: u16 },
    /// The child process exited with this code.
    Exited(i32),
}

/// An event produced by one of the internal I/O threads.
enum IoEvent {
    Input(InputEvent),
    Output(Vec<u8>),
    Resize(u16, u16),
}

/// A handle around a command running in a proxied pseudo-terminal.
pub struct Proxy {
    size: (u16, u16),
    /// Overlay bytes drawn on top of the child screen; `None` is empty.
    overlay: Option<Vec<u8>>,
    /// Write half of the child PTY.
    writer: Box<dyn Write + Send>,
    master: MasterHandle,
    session: PtySession,
    rx: Receiver<IoEvent>,
    terminate: Arc<AtomicBool>,
    /// Restores the outer terminal on drop (and via a panic hook).
    _guard: RawModeGuard,
    screen: TerminalScreen,
    renderer: Renderer,
    /// The screen buffer changed since the last paint.
    dirty: bool,
    /// Scrollback view offset from the live bottom (0 = live). When non-zero the
    /// live output is not painted until the user scrolls back to the bottom.
    scroll_offset: usize,
    /// Input modes mirrored onto the outer terminal (start off = fresh raw mode).
    im_app_cursor: bool,
    im_app_keypad: bool,
    im_bracketed_paste: bool,
    /// Motion-tracking level mirrored onto the outer terminal (for tmux drags).
    outer_motion: mouse::OuterMotion,
    /// The configured hotkey byte (written to the child by `forward(Hotkey)`).
    hotkey: Option<u8>,
    /// Whether to surface [`ProxyEvent::ScreenChanged`].
    screen_events: bool,
    /// Force any-event mouse tracking on the outer terminal (see config).
    force_motion: bool,
}

impl Proxy {
    /// Spawn `command` in a PTY and take over the outer terminal. `command` must
    /// be non-empty (program plus args).
    pub fn start(command: &[String], config: ProxyConfig) -> anyhow::Result<Proxy> {
        // A zero (or unknown) terminal dimension would make the screen model
        // degenerate, so fall back to a sane default.
        let (mut cols, mut rows) = config
            .size
            .unwrap_or_else(|| terminal::size().unwrap_or((80, 24)));
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

        // Raw mode + alt screen + mouse reporting, restored on every exit path.
        let guard = RawModeGuard::enter()?;

        let (session, reader, writer) = PtySession::spawn(&command[0], &command[1..], size)?;
        let master = session.master();

        // Bounded so a flood of child output applies backpressure rather than
        // growing memory without bound.
        let (tx, rx) = sync_channel::<IoEvent>(256);
        spawn_pty_reader(reader, tx.clone());
        spawn_stdin_reader(tx.clone(), config.hotkey);
        spawn_resize_thread(tx.clone());
        drop(tx); // the proxy holds only the receiver; producers hold the clones.

        // External termination should still tear us down cleanly.
        let terminate = Arc::new(AtomicBool::new(false));
        for sig in [SIGTERM, SIGINT, SIGHUP] {
            signal_hook::flag::register(sig, Arc::clone(&terminate))?;
        }

        Ok(Proxy {
            size: (cols, rows),
            overlay: None,
            writer,
            master,
            session,
            rx,
            terminate,
            _guard: guard,
            screen: TerminalScreen::new(rows, cols, config.scrollback),
            renderer: Renderer::new(rows, cols),
            dirty: false,
            scroll_offset: 0,
            im_app_cursor: false,
            im_app_keypad: false,
            im_bracketed_paste: false,
            outer_motion: mouse::OuterMotion::Base,
            hotkey: config.hotkey,
            screen_events: config.screen_events,
            force_motion: config.mouse_motion,
        })
    }

    /// Wait up to `timeout` for the next event. Child output is parsed and
    /// rendered internally (coalesced); input is never forwarded on its own.
    /// Returns `None` if nothing the consumer needs to act on occurred.
    pub fn poll(&mut self, timeout: Duration) -> anyhow::Result<Option<ProxyEvent>> {
        if let Some(status) = self.session.child.try_wait()? {
            return Ok(Some(ProxyEvent::Exited(status.exit_code() as i32)));
        }
        if self.terminate.load(Ordering::Relaxed) {
            let _ = self.session.child.kill();
            let _ = self.session.child.wait();
            return Ok(Some(ProxyEvent::Exited(130)));
        }

        match self.rx.recv_timeout(timeout) {
            Ok(event) => {
                if let Some(pe) = self.handle(event) {
                    self.flush_render();
                    return Ok(Some(pe));
                }
                // An Output event was processed (marked dirty); fall through and
                // drain the rest of the burst before painting once.
            }
            Err(RecvTimeoutError::Timeout) => return Ok(self.flush_with_screen_event()),
            Err(RecvTimeoutError::Disconnected) => {
                let code = self
                    .session
                    .child
                    .try_wait()?
                    .map(|s| s.exit_code() as i32)
                    .unwrap_or(1);
                return Ok(Some(ProxyEvent::Exited(code)));
            }
        }

        while let Ok(event) = self.rx.try_recv() {
            if let Some(pe) = self.handle(event) {
                self.flush_render();
                return Ok(Some(pe));
            }
        }
        Ok(self.flush_with_screen_event())
    }

    /// Run the event loop, calling `handler` for each event until it returns
    /// `ControlFlow::Break(code)`. Owns the proxy and drops it (restoring the
    /// terminal) before returning — call this rather than holding a `Proxy`
    /// across `std::process::exit`, which skips destructors.
    pub fn run_with<F>(mut self, mut handler: F) -> anyhow::Result<i32>
    where
        F: FnMut(&mut Proxy, ProxyEvent) -> ControlFlow<i32>,
    {
        loop {
            if let Some(event) = self.poll(POLL_INTERVAL)? {
                if let ControlFlow::Break(code) = handler(&mut self, event) {
                    return Ok(code);
                }
            }
        }
    }

    /// Pass an input event through to the child (the default proxy handling):
    /// bytes are written verbatim, a wheel event drives scrollback when the child
    /// isn't using the mouse, other mouse events are re-encoded in the child's
    /// protocol, and the hotkey writes its configured byte.
    pub fn forward(&mut self, input: InputEvent) {
        match input {
            InputEvent::Hotkey => {
                if let Some(byte) = self.hotkey {
                    let _ = self.writer.write_all(&[byte]);
                    let _ = self.writer.flush();
                }
            }
            InputEvent::Mouse(m) => {
                if matches!(m.kind, MouseKind::ScrollUp | MouseKind::ScrollDown)
                    && !self.screen.child_wants_mouse()
                {
                    if self.screen.alternate_screen() {
                        // A full-screen app that isn't using the mouse (less, nano,
                        // vim w/o mouse): emulate "alternate scroll" by sending
                        // arrow keys, like a normal terminal does.
                        self.send_scroll_arrows(m.kind);
                    } else {
                        // Normal screen: drive proxtty's own scrollback.
                        self.scroll(m.kind);
                    }
                } else {
                    self.forward_mouse(&m);
                }
            }
            InputEvent::Forward(bytes) => {
                // Typing returns the view to the live bottom (like every terminal).
                self.snap_to_bottom();
                let _ = self.writer.write_all(&bytes);
                let _ = self.writer.flush();
            }
        }
    }

    /// Draw `bytes` (positioned ANSI) on top of the child screen. Cells you don't
    /// address keep showing the child; the proxy redraws this after each child
    /// update until you change or clear it. Successive calls draw over the
    /// previous overlay — to shrink it, [`Proxy::clear_overlay`] first.
    pub fn set_overlay(&mut self, bytes: &[u8]) {
        self.overlay = Some(bytes.to_vec());
        self.draw_overlay(bytes);
    }

    /// Replace the overlay and recomposite in a single paint: repaints the child
    /// (erasing the previous overlay) and draws the new overlay in one write, so
    /// there's no flash of clear-then-draw. Use this for an overlay that changes
    /// every frame (a cursor/mouse trail) to avoid flicker.
    pub fn replace_overlay(&mut self, bytes: &[u8]) {
        self.overlay = Some(bytes.to_vec());
        self.repaint_with_overlay();
    }

    /// Remove the overlay and repaint the child screen, wiping it.
    pub fn clear_overlay(&mut self) {
        self.overlay = None;
        self.repaint_with_overlay();
    }

    /// Whether an overlay is currently set.
    pub fn overlay_set(&self) -> bool {
        self.overlay.is_some()
    }

    /// Write a control sequence to the outer terminal out-of-band (e.g. an OSC 52
    /// clipboard request). Unlike [`Proxy::set_overlay`] this isn't remembered or
    /// recomposited.
    pub fn emit_raw(&mut self, bytes: &[u8]) {
        self.renderer.write_raw(bytes);
    }

    /// Plain-text contents of the currently visible rows.
    pub fn visible_text(&self) -> String {
        self.screen.visible_text()
    }

    /// The current terminal size as `(cols, rows)`.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// The child's cursor position as `(col, row)`, 0-based.
    pub fn cursor(&self) -> (u16, u16) {
        let (row, col) = self.screen.current().cursor_position();
        (col, row)
    }

    /// Current scrollback view offset in lines from the live bottom (0 = live).
    /// Useful for anchoring an overlay to the scrolled content.
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// The text of the visible cell at (`col`, `row`), 0-based, or `None` if it's
    /// empty/blank. Respects the current scrollback offset.
    pub fn cell(&self, col: u16, row: u16) -> Option<String> {
        let cell = self.screen.current().cell(row, col)?;
        let text = cell.contents();
        if text.is_empty() || text == " " {
            None
        } else {
            Some(text.to_string())
        }
    }

    /// Write bytes directly to the child PTY (e.g. to run a command).
    pub fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()
    }

    // --- internals ---

    /// Apply an I/O event. Returns the consumer-facing event, if any.
    fn handle(&mut self, event: IoEvent) -> Option<ProxyEvent> {
        match event {
            IoEvent::Output(bytes) => {
                self.on_output(&bytes);
                None
            }
            IoEvent::Input(ev) => Some(ProxyEvent::Input(ev)),
            IoEvent::Resize(cols, rows) => {
                self.on_resize(cols, rows);
                Some(ProxyEvent::Resize { cols, rows })
            }
        }
    }

    /// Update the screen buffer; defer painting to `flush_render`.
    fn on_output(&mut self, bytes: &[u8]) {
        self.screen.process(bytes);
        // Forward out-of-band sequences vt100 doesn't model (title, clipboard,
        // bell, cursor shape) to the outer terminal. They don't touch the grid.
        let passthrough = self.screen.take_passthrough();
        if !passthrough.is_empty() {
            self.renderer.write_raw(&passthrough);
        }
        self.dirty = true;
    }

    /// Paint pending output (unless the scrollback view is frozen) and redraw the
    /// overlay on top. Returns whether the child screen was repainted.
    fn flush_render(&mut self) -> bool {
        self.sync_input_modes();
        self.sync_mouse_mode();
        if !(self.dirty && self.scroll_offset == 0) {
            return false;
        }
        if self.overlay.is_some() {
            // An overlay was drawn with raw ANSI, moving the real cursor and color
            // state out from under vt100's diff (which assumes the terminal is
            // exactly where it last left it). A full repaint is absolute — it
            // resets cursor and attributes from scratch — and compositing the
            // overlay into the same write avoids any flash.
            self.repaint_with_overlay();
        } else {
            self.renderer.render(self.screen.current());
            self.dirty = false;
        }
        true
    }

    /// `flush_render`, plus a `ScreenChanged` event when one is warranted.
    fn flush_with_screen_event(&mut self) -> Option<ProxyEvent> {
        let painted = self.flush_render();
        if painted && self.screen_events {
            Some(ProxyEvent::ScreenChanged)
        } else {
            None
        }
    }

    /// Draw overlay `bytes` on top of the current screen (no child repaint), then
    /// park the cursor (see [`Proxy::cursor_park`]). One write. Used by
    /// `set_overlay` to drop a fixed overlay (e.g. a menu) onto the live screen.
    fn draw_overlay(&mut self, bytes: &[u8]) {
        let mut out = bytes.to_vec();
        out.extend_from_slice(&self.cursor_park());
        self.renderer.write_raw(&out);
    }

    /// Full child repaint with the overlay composited on top, in a single write
    /// (no flash between clearing the screen and drawing the overlay). Used after
    /// scroll/resize and whenever the overlay or child changes while one is set.
    fn repaint_with_overlay(&mut self) {
        let mut overlay = self.overlay.clone().unwrap_or_default();
        overlay.extend_from_slice(&self.cursor_park());
        self.renderer.repaint_with(self.screen.current(), &overlay);
        self.dirty = false;
    }

    /// A cursor move that parks the real cursor at the child's *current* position
    /// so it tracks the child instead of wherever the overlay's drawing left it
    /// (otherwise it lags a step behind while typing). Visibility is the overlay's
    /// call; skipped while viewing scrollback (the live cursor isn't in view).
    fn cursor_park(&self) -> Vec<u8> {
        if self.scroll_offset == 0 {
            let (row, col) = self.screen.current().cursor_position();
            format!("\x1b[{};{}H", row + 1, col + 1).into_bytes()
        } else {
            Vec::new()
        }
    }

    /// Mirror the child's input modes (application cursor/keypad, bracketed
    /// paste) onto the outer terminal so the keys and pastes it sends us are
    /// encoded the way the child expects.
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

    /// Mirror the child's mouse motion-tracking level onto the outer terminal so
    /// drags (e.g. resizing a tmux pane) are reported to us and forwarded. When
    /// the consumer requested motion tracking, force any-event tracking instead.
    fn sync_mouse_mode(&mut self) {
        let desired = if self.force_motion {
            mouse::OuterMotion::Any
        } else {
            mouse::desired_motion(self.screen.current().mouse_protocol_mode())
        };
        if desired != self.outer_motion {
            let seq = mouse::motion_transition(self.outer_motion, desired);
            self.renderer.write_raw(&seq);
            self.outer_motion = desired;
        }
    }

    /// Return the view to the live bottom and repaint, if it was scrolled back.
    fn snap_to_bottom(&mut self) {
        if self.scroll_offset != 0 {
            self.scroll_offset = self.screen.scroll_to(0);
            self.repaint_with_overlay();
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
        self.repaint_with_overlay();
        // While viewing history, hide the cursor: vt100 reports the *live* cursor
        // position (near the bottom), so the repaint would otherwise leave it
        // blinking over scrolled-back content. Returning to the live bottom
        // (offset 0) repaints with the real cursor restored.
        if self.scroll_offset > 0 {
            self.renderer.write_raw(b"\x1b[?25l");
        }
    }

    /// Emulate alternate-scroll: send arrow keys to the child for a wheel event,
    /// in the form (CSI vs SS3) the child's cursor-key mode expects.
    fn send_scroll_arrows(&mut self, kind: MouseKind) {
        const LINES: usize = 3;
        let app_cursor = self.screen.current().application_cursor();
        let seq: &[u8] = match (kind, app_cursor) {
            (MouseKind::ScrollUp, false) => b"\x1b[A",
            (MouseKind::ScrollUp, true) => b"\x1bOA",
            (MouseKind::ScrollDown, false) => b"\x1b[B",
            (MouseKind::ScrollDown, true) => b"\x1bOB",
            _ => return,
        };
        for _ in 0..LINES {
            let _ = self.writer.write_all(seq);
        }
        let _ = self.writer.flush();
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
        // A scrollback offset is meaningless after a reflow; return to the live
        // bottom and repaint (keeping the overlay — the consumer may rebuild it
        // on the Resize event if it's size-dependent).
        self.scroll_offset = self.screen.scroll_to(0);
        self.repaint_with_overlay();
    }
}

/// PTY output → `IoEvent::Output`.
fn spawn_pty_reader(mut reader: Box<dyn Read + Send>, tx: SyncSender<IoEvent>) {
    thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(IoEvent::Output(buf[..n].to_vec())).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Raw stdin → `InputParser` → `IoEvent::Input`. Detached: a blocking stdin read
/// can't be cleanly interrupted, so the process exit reaps it.
fn spawn_stdin_reader(tx: SyncSender<IoEvent>, hotkey: Option<u8>) {
    thread::spawn(move || {
        let mut parser = InputParser::new(hotkey);
        let stdin = io::stdin();
        let mut handle = stdin.lock();
        let mut buf = [0u8; 8192];
        loop {
            match handle.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    crate::debug::raw(&buf[..n]);
                    for ev in parser.feed(&buf[..n]) {
                        if tx.send(IoEvent::Input(ev)).is_err() {
                            return;
                        }
                    }
                }
            }
        }
    });
}

/// SIGWINCH → `IoEvent::Resize`.
fn spawn_resize_thread(tx: SyncSender<IoEvent>) {
    let mut signals = match signal_hook::iterator::Signals::new([SIGWINCH]) {
        Ok(s) => s,
        Err(_) => return,
    };
    thread::spawn(move || {
        for _ in signals.forever() {
            if let Ok((cols, rows)) = terminal::size() {
                if tx.send(IoEvent::Resize(cols, rows)).is_err() {
                    break;
                }
            }
        }
    });
}
