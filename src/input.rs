//! Input tap over the raw outer-terminal byte stream.
//!
//! Rather than convert keystrokes to structured events and re-encode them (lossy
//! and easy to get wrong), `smartty` keeps the high-fidelity raw-byte passthrough
//! that Milestone 1 validated and *taps* the stream: it recognizes only the two
//! things it cares about — the local hotkey byte and SGR mouse reports — and
//! forwards everything else verbatim to the child.
//!
//! [`InputParser`] is a small state machine fed raw bytes via [`InputParser::feed`],
//! which returns an ordered list of [`InputEvent`]s. It is incremental: a mouse
//! report split across two reads is buffered until complete, while a lone `ESC`
//! (e.g. pressing Escape in `vim`) is forwarded immediately so it isn't swallowed.

use std::mem;

const ESC: u8 = 0x1b;
/// Give up buffering an escape sequence that grows past this — it's malformed.
const MAX_SEQ: usize = 64;

/// Which mouse button an event concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    None,
}

/// The kind of mouse interaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseKind {
    Down,
    Up,
    Drag,
    Moved,
    ScrollUp,
    ScrollDown,
    /// Horizontal wheel (trackpads emit these alongside vertical scrolling).
    ScrollLeft,
    ScrollRight,
}

/// A decoded SGR mouse report. Coordinates are 0-based (column, row).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseKind,
    pub button: MouseButton,
    pub col: u16,
    pub row: u16,
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// Something the parser extracted from the input stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    /// Bytes to forward verbatim to the child PTY.
    Forward(Vec<u8>),
    /// The local hotkey was pressed.
    Hotkey,
    /// A mouse report from the outer terminal.
    Mouse(MouseEvent),
}

#[derive(PartialEq)]
enum State {
    Ground,
    Esc,
    Csi,
    Mouse,
}

/// Incremental parser tapping the raw input stream.
pub struct InputParser {
    hotkey: u8,
    state: State,
    /// The in-progress escape sequence (including the leading `ESC`).
    seq: Vec<u8>,
    /// Bytes accumulated for the next `Forward` event.
    forward: Vec<u8>,
    /// Events produced by the current `feed` call.
    out: Vec<InputEvent>,
}

impl InputParser {
    /// Create a parser whose local trigger is the byte `hotkey`.
    pub fn new(hotkey: u8) -> InputParser {
        InputParser {
            hotkey,
            state: State::Ground,
            seq: Vec::new(),
            forward: Vec::new(),
            out: Vec::new(),
        }
    }

    /// Feed a chunk of raw input and return the events it yields, in order.
    pub fn feed(&mut self, data: &[u8]) -> Vec<InputEvent> {
        for &b in data {
            self.byte(b);
        }
        self.boundary();
        mem::take(&mut self.out)
    }

    fn flush_forward(&mut self) {
        if !self.forward.is_empty() {
            self.out
                .push(InputEvent::Forward(mem::take(&mut self.forward)));
        }
    }

    fn byte(&mut self, b: u8) {
        match self.state {
            State::Ground => {
                if b == self.hotkey {
                    self.flush_forward();
                    self.out.push(InputEvent::Hotkey);
                } else if b == ESC {
                    self.state = State::Esc;
                    self.seq.clear();
                    self.seq.push(b);
                } else {
                    self.forward.push(b);
                }
            }
            State::Esc => {
                if b == b'[' {
                    self.seq.push(b);
                    self.state = State::Csi;
                } else {
                    // Not a CSI (e.g. Alt-<key> sends ESC then the key). Forward
                    // the ESC and re-handle this byte from the ground state.
                    self.forward.append(&mut self.seq);
                    self.state = State::Ground;
                    self.byte(b);
                }
            }
            State::Csi => {
                self.seq.push(b);
                if self.seq.len() == 3 && b == b'<' {
                    // ESC [ < ... is an SGR mouse report.
                    self.state = State::Mouse;
                } else if (0x40..=0x7e).contains(&b) {
                    // Final byte of a non-mouse CSI (cursor keys, etc.); forward it.
                    self.forward.append(&mut self.seq);
                    self.state = State::Ground;
                } else if self.seq.len() > MAX_SEQ {
                    self.forward.append(&mut self.seq);
                    self.state = State::Ground;
                }
            }
            State::Mouse => {
                self.seq.push(b);
                if b == b'M' || b == b'm' {
                    if let Some(ev) = parse_sgr_mouse(&self.seq) {
                        self.flush_forward();
                        self.out.push(InputEvent::Mouse(ev));
                    }
                    self.seq.clear();
                    self.state = State::Ground;
                } else if self.seq.len() > MAX_SEQ {
                    self.forward.append(&mut self.seq);
                    self.state = State::Ground;
                }
            }
        }
    }

    /// Called at the end of each `feed` chunk to resolve a dangling sequence.
    fn boundary(&mut self) {
        match self.state {
            // A lone ESC: forward it now so `vim` etc. see Escape without delay.
            State::Esc => {
                self.forward.append(&mut self.seq);
                self.state = State::Ground;
            }
            // A partial CSI or mouse report: keep buffering across reads. Mouse
            // reports arrive atomically in practice, so this rarely persists.
            State::Csi | State::Mouse | State::Ground => {}
        }
        self.flush_forward();
    }
}

/// Parse `ESC [ < Cb ; Cx ; Cy (M|m)` into a [`MouseEvent`].
fn parse_sgr_mouse(seq: &[u8]) -> Option<MouseEvent> {
    let len = seq.len();
    if len < 6 {
        return None;
    }
    let press = seq[len - 1] == b'M';
    let body = std::str::from_utf8(&seq[3..len - 1]).ok()?;
    let mut parts = body.split(';');
    let cb: u32 = parts.next()?.parse().ok()?;
    let cx: u32 = parts.next()?.parse().ok()?;
    let cy: u32 = parts.next()?.parse().ok()?;

    let shift = cb & 4 != 0;
    let alt = cb & 8 != 0;
    let ctrl = cb & 16 != 0;
    let motion = cb & 32 != 0;
    let wheel = cb & 64 != 0;
    let low = cb & 3;

    let (kind, button) = if wheel {
        // SGR wheel encodes direction in the low two bits: 0 up, 1 down, 2 left,
        // 3 right. Misreading left/right (which trackpads emit during a vertical
        // swipe) as "down" was the cause of scroll jitter.
        let kind = match low {
            0 => MouseKind::ScrollUp,
            1 => MouseKind::ScrollDown,
            2 => MouseKind::ScrollLeft,
            _ => MouseKind::ScrollRight,
        };
        (kind, MouseButton::None)
    } else {
        let button = match low {
            0 => MouseButton::Left,
            1 => MouseButton::Middle,
            2 => MouseButton::Right,
            _ => MouseButton::None,
        };
        let kind = if motion {
            if button == MouseButton::None {
                MouseKind::Moved
            } else {
                MouseKind::Drag
            }
        } else if press {
            MouseKind::Down
        } else {
            MouseKind::Up
        };
        (kind, button)
    };

    Some(MouseEvent {
        kind,
        button,
        col: cx.saturating_sub(1) as u16,
        row: cy.saturating_sub(1) as u16,
        shift,
        alt,
        ctrl,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOTKEY: u8 = 0x00; // Ctrl-Space

    fn parse(chunks: &[&[u8]]) -> Vec<InputEvent> {
        let mut p = InputParser::new(HOTKEY);
        let mut all = Vec::new();
        for c in chunks {
            all.extend(p.feed(c));
        }
        all
    }

    fn fwd(bytes: &[u8]) -> InputEvent {
        InputEvent::Forward(bytes.to_vec())
    }

    #[test]
    fn plain_text_is_forwarded() {
        assert_eq!(parse(&[b"hello"]), vec![fwd(b"hello")]);
    }

    #[test]
    fn hotkey_alone() {
        assert_eq!(parse(&[&[HOTKEY]]), vec![InputEvent::Hotkey]);
    }

    #[test]
    fn hotkey_splits_surrounding_text() {
        assert_eq!(
            parse(&[b"ab\x00cd"]),
            vec![fwd(b"ab"), InputEvent::Hotkey, fwd(b"cd")]
        );
    }

    #[test]
    fn lone_escape_is_forwarded() {
        assert_eq!(parse(&[&[ESC]]), vec![fwd(&[ESC])]);
    }

    #[test]
    fn alt_key_is_forwarded() {
        // Alt-a arrives as ESC then 'a'.
        assert_eq!(parse(&[b"\x1ba"]), vec![fwd(b"\x1ba")]);
    }

    #[test]
    fn cursor_key_is_forwarded() {
        assert_eq!(parse(&[b"\x1b[A"]), vec![fwd(b"\x1b[A")]);
    }

    #[test]
    fn sgr_left_press() {
        let evs = parse(&[b"\x1b[<0;1;1M"]);
        assert_eq!(
            evs,
            vec![InputEvent::Mouse(MouseEvent {
                kind: MouseKind::Down,
                button: MouseButton::Left,
                col: 0,
                row: 0,
                shift: false,
                alt: false,
                ctrl: false,
            })]
        );
    }

    #[test]
    fn option_left_press() {
        // Cb = 8 -> left button with Meta/Alt modifier.
        let evs = parse(&[b"\x1b[<8;5;3M"]);
        assert_eq!(
            evs,
            vec![InputEvent::Mouse(MouseEvent {
                kind: MouseKind::Down,
                button: MouseButton::Left,
                col: 4,
                row: 2,
                shift: false,
                alt: true,
                ctrl: false,
            })]
        );
    }

    #[test]
    fn sgr_release() {
        let evs = parse(&[b"\x1b[<0;1;1m"]);
        assert_eq!(evs[0], {
            InputEvent::Mouse(MouseEvent {
                kind: MouseKind::Up,
                button: MouseButton::Left,
                col: 0,
                row: 0,
                shift: false,
                alt: false,
                ctrl: false,
            })
        });
    }

    #[test]
    fn wheel_up() {
        let evs = parse(&[b"\x1b[<64;1;1M"]);
        assert!(matches!(
            evs[0],
            InputEvent::Mouse(MouseEvent {
                kind: MouseKind::ScrollUp,
                ..
            })
        ));
    }

    #[test]
    fn wheel_directions() {
        // The regression: Cb 66/67 are horizontal wheel, not ScrollDown.
        let kind = |bytes: &[u8]| match parse(&[bytes])[0] {
            InputEvent::Mouse(m) => m.kind,
            _ => panic!("expected mouse"),
        };
        assert_eq!(kind(b"\x1b[<64;1;1M"), MouseKind::ScrollUp);
        assert_eq!(kind(b"\x1b[<65;1;1M"), MouseKind::ScrollDown);
        assert_eq!(kind(b"\x1b[<66;1;1M"), MouseKind::ScrollLeft);
        assert_eq!(kind(b"\x1b[<67;1;1M"), MouseKind::ScrollRight);
    }

    #[test]
    fn mouse_report_split_across_reads() {
        // The report is fragmented; the parser must buffer until complete.
        let evs = parse(&[b"\x1b[<0;10;", b"5M"]);
        assert_eq!(
            evs,
            vec![InputEvent::Mouse(MouseEvent {
                kind: MouseKind::Down,
                button: MouseButton::Left,
                col: 9,
                row: 4,
                shift: false,
                alt: false,
                ctrl: false,
            })]
        );
    }

    #[test]
    fn forward_then_mouse_ordering() {
        let evs = parse(&[b"x\x1b[<0;1;1M"]);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0], fwd(b"x"));
        assert!(matches!(evs[1], InputEvent::Mouse(_)));
    }
}
