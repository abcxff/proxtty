//! The child terminal's screen buffer.
//!
//! Milestone 6: instead of writing PTY output straight to stdout, `smartty` now
//! feeds it through a `vt100` parser that maintains a virtual screen — the grid
//! of cells, styles, cursor, and alternate-screen state the child believes it is
//! drawing to. Knowing what's on screen is what lets the renderer recomposite
//! cleanly after an overlay closes.

use vt100::Parser;

/// Scrollback retained by the parser. None for now; Milestone 11 adds a real
/// scrollback model. Keeping it at zero also keeps screen clones (used for diff
/// rendering) cheap.
const SCROLLBACK: usize = 0;

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
}
