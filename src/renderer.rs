//! Paints the child screen buffer onto the outer terminal.
//!
//! Milestone 7. The heavy lifting — turning a screen of cells/styles/cursor into
//! terminal escape codes — is done by `vt100` itself: `contents_diff` emits the
//! minimal byte stream to move the previously-painted screen to the current one,
//! and `contents_formatted` emits a full repaint (clearing first). We keep a copy
//! of the last-painted screen as the diff baseline.
//!
//! Two paint modes:
//!   - [`Renderer::render`] — incremental diff, used for normal output.
//!   - [`Renderer::repaint`] — full repaint, used after an overlay closes or a
//!     resize, where we want a clean slate that wipes whatever was on top.

use std::io::{self, Write};

use vt100::{Parser, Screen};

pub struct Renderer {
    /// The screen state currently displayed on the outer terminal.
    prev: Screen,
    out: io::Stdout,
}

impl Renderer {
    /// Create a renderer and clear the outer terminal so it matches our blank
    /// baseline (otherwise the first diff would paint over stale content).
    pub fn new(rows: u16, cols: u16) -> Renderer {
        let prev = Parser::new(rows, cols, 0).screen().clone();
        let mut out = io::stdout();
        let _ = out.write_all(b"\x1b[2J\x1b[H");
        let _ = out.flush();
        Renderer { prev, out }
    }

    /// Incrementally paint from the last displayed state to `screen`.
    pub fn render(&mut self, screen: &Screen) {
        let diff = screen.contents_diff(&self.prev);
        if !diff.is_empty() {
            let _ = self.out.write_all(&diff);
            let _ = self.out.flush();
        }
        self.prev = screen.clone();
    }

    /// Full repaint: clear the terminal and redraw `screen` from scratch. This
    /// is how an overlay is wiped — the menu was drawn on top of the terminal
    /// but never into `screen`, so repainting the child screen erases it.
    pub fn repaint(&mut self, screen: &Screen) {
        let full = screen.contents_formatted();
        let _ = self.out.write_all(&full);
        let _ = self.out.flush();
        self.prev = screen.clone();
    }

    /// Write raw bytes to the terminal (e.g. an overlay drawn on top of the
    /// already-painted child screen). Does not touch the diff baseline.
    pub fn write_raw(&mut self, bytes: &[u8]) {
        let _ = self.out.write_all(bytes);
        let _ = self.out.flush();
    }
}
