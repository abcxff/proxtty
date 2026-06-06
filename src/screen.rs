//! The child terminal's screen buffer.
//!
//! Milestone 6: instead of writing PTY output straight to stdout, `smartty` now
//! feeds it through a `vt100` parser that maintains a virtual screen — the grid
//! of cells, styles, cursor, and alternate-screen state the child believes it is
//! drawing to. Knowing what's on screen is what lets the renderer recomposite
//! cleanly after an overlay closes.

use vt100::Parser;

/// Lines of scrollback retained by the parser (Milestone 11). Bounded so the
/// per-frame screen clone used for diff rendering stays affordable.
const SCROLLBACK: usize = 1000;

/// A parsed model of the child's terminal screen.
pub struct TerminalScreen {
    parser: Parser,
}

impl TerminalScreen {
    pub fn new(rows: u16, cols: u16) -> TerminalScreen {
        TerminalScreen {
            parser: Parser::new(rows, cols, SCROLLBACK),
        }
    }

    /// Feed raw child output into the parser, advancing the screen state.
    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
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
