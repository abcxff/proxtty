//! The child terminal's screen buffer.
//!
//! Child output flows through a `vt100` parser that maintains the virtual screen
//! the renderer paints. `vt100` models the visible grid but deliberately ignores
//! some out-of-band sequences (window title, clipboard requests, the bell, cursor
//! shape). Those still matter for fidelity (Milestone 12), so a [`PassThrough`]
//! callback captures them and we forward them to the outer terminal — they don't
//! touch the grid, so they're safe to emit between paints.

use std::cell::RefCell;
use std::rc::Rc;

use vt100::{Callbacks, Parser};

/// Lines of scrollback retained by the parser (Milestone 11). Bounded so the
/// per-frame screen clone used for diff rendering stays affordable.
const SCROLLBACK: usize = 1000;

/// A parsed model of the child's terminal screen.
pub struct TerminalScreen {
    parser: Parser<PassThrough>,
    /// Bytes vt100 handed us to forward verbatim to the outer terminal.
    passthrough: Rc<RefCell<Vec<u8>>>,
}

impl TerminalScreen {
    pub fn new(rows: u16, cols: u16) -> TerminalScreen {
        let passthrough = Rc::new(RefCell::new(Vec::new()));
        let callbacks = PassThrough {
            out: Rc::clone(&passthrough),
        };
        TerminalScreen {
            parser: Parser::new_with_callbacks(rows, cols, SCROLLBACK, callbacks),
            passthrough,
        }
    }

    /// Feed raw child output into the parser, advancing the screen state.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    /// Take any out-of-band bytes (title, clipboard, bell, cursor shape) that the
    /// parser collected during `process`, to forward to the outer terminal.
    pub fn take_passthrough(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.passthrough.borrow_mut())
    }

    /// Resize the virtual screen (after the outer terminal resizes).
    pub fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
    }

    /// The current screen state, for rendering.
    pub fn current(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Move the scrollback view `rows` lines back from the live bottom (0 = live).
    /// Returns the actual offset after clamping to the available scrollback.
    pub fn scroll_to(&mut self, rows: usize) -> usize {
        let screen = self.parser.screen_mut();
        screen.set_scrollback(rows);
        screen.scrollback()
    }

    /// Whether the child is currently using the alternate screen (e.g. `vim`).
    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// Whether the child has requested any mouse reporting.
    pub fn child_wants_mouse(&self) -> bool {
        self.parser.screen().mouse_protocol_mode() != vt100::MouseProtocolMode::None
    }

    /// Plain-text contents of the currently visible rows (used by menu actions).
    pub fn visible_text(&self) -> String {
        self.parser.screen().contents()
    }
}

/// Collects out-of-band escape sequences `vt100` doesn't model so they can be
/// replayed to the outer terminal.
struct PassThrough {
    out: Rc<RefCell<Vec<u8>>>,
}

impl PassThrough {
    fn osc(&self, prefix: &[u8], body: &[u8]) {
        let mut out = self.out.borrow_mut();
        out.extend_from_slice(prefix);
        out.extend_from_slice(body);
        out.push(0x07); // BEL terminator
    }
}

impl Callbacks for PassThrough {
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.out.borrow_mut().push(0x07);
    }

    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.osc(b"\x1b]2;", title);
    }

    fn set_window_icon_name(&mut self, _: &mut vt100::Screen, icon_name: &[u8]) {
        self.osc(b"\x1b]1;", icon_name);
    }

    fn copy_to_clipboard(&mut self, _: &mut vt100::Screen, ty: &[u8], data: &[u8]) {
        // Re-emit OSC 52 so a child like tmux/vim can drive the system clipboard
        // through smartty (and over SSH). `data` is already base64.
        let mut out = self.out.borrow_mut();
        out.extend_from_slice(b"\x1b]52;");
        out.extend_from_slice(ty);
        out.push(b';');
        out.extend_from_slice(data);
        out.push(0x07);
    }

    fn unhandled_csi(
        &mut self,
        _: &mut vt100::Screen,
        i1: Option<u8>,
        _i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        // DECSCUSR (cursor shape): CSI Ps SP q — space intermediate, final 'q'.
        // Forward it so the child's chosen cursor shape (bar/block/underline) is
        // honored. Other unhandled CSIs are left alone to avoid corruption.
        if c == 'q' && i1 == Some(b' ') {
            let joined = params
                .iter()
                .map(|p| {
                    p.iter()
                        .map(|n| n.to_string())
                        .collect::<Vec<_>>()
                        .join(":")
                })
                .collect::<Vec<_>>()
                .join(";");
            let mut out = self.out.borrow_mut();
            out.extend_from_slice(b"\x1b[");
            out.extend_from_slice(joined.as_bytes());
            out.extend_from_slice(b" q");
        }
    }
}
