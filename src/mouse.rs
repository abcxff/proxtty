//! Forwarding outer-terminal mouse events to the child.
//!
//! Milestone 9. The outer terminal reports mouse activity to `proxtty` (so it can
//! catch the Option-click trigger). When `proxtty` doesn't want an event itself
//! and the child has asked for mouse reporting, the event is re-encoded in the
//! child's requested protocol and forwarded — so clicking/scrolling reaches apps
//! like `vim`, `tmux`, `less` and `fzf`.
//!
//! `proxtty` keeps the outer terminal in normal button-tracking mode, so it only
//! ever sees press/release/scroll — not motion. Forwarding drag/motion would
//! require mirroring the child's motion mode onto the outer terminal; that's left
//! for later. The encoders below still handle motion bytes for completeness.

use vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::input::{MouseButton, MouseEvent, MouseKind};

/// The motion-tracking level proxtty enables on the *outer* terminal, on top of
/// the always-on button tracking (`?1000`) it needs for its own trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OuterMotion {
    /// Button tracking only (`?1000`): press/release/wheel.
    Base,
    /// Button-event tracking (`?1002`): also motion while a button is held.
    Button,
    /// Any-event tracking (`?1003`): motion reported always.
    Any,
}

/// The outer-terminal motion level needed to satisfy the child's requested mode.
/// Drag/motion only reaches the child if the outer terminal reports it to us.
pub fn desired_motion(mode: MouseProtocolMode) -> OuterMotion {
    match mode {
        MouseProtocolMode::ButtonMotion => OuterMotion::Button,
        MouseProtocolMode::AnyMotion => OuterMotion::Any,
        _ => OuterMotion::Base,
    }
}

/// Escape codes to move the outer terminal's motion tracking `from` -> `to`.
/// `?1000` stays on throughout (proxtty's trigger needs it), so this only toggles
/// the `?1002`/`?1003` overlay.
pub fn motion_transition(from: OuterMotion, to: OuterMotion) -> Vec<u8> {
    let mut seq = Vec::new();
    match from {
        OuterMotion::Button => seq.extend_from_slice(b"\x1b[?1002l"),
        OuterMotion::Any => seq.extend_from_slice(b"\x1b[?1003l"),
        OuterMotion::Base => {}
    }
    match to {
        OuterMotion::Button => seq.extend_from_slice(b"\x1b[?1002h"),
        OuterMotion::Any => seq.extend_from_slice(b"\x1b[?1003h"),
        OuterMotion::Base => {}
    }
    seq
}

/// Whether an event of `kind` should be forwarded under the child's `mode`.
pub fn should_forward(mode: MouseProtocolMode, kind: MouseKind) -> bool {
    let wheel = matches!(
        kind,
        MouseKind::ScrollUp | MouseKind::ScrollDown | MouseKind::ScrollLeft | MouseKind::ScrollRight
    );
    match mode {
        MouseProtocolMode::None => false,
        // X10: report only presses (and wheel).
        MouseProtocolMode::Press => kind == MouseKind::Down || wheel,
        MouseProtocolMode::PressRelease => {
            matches!(kind, MouseKind::Down | MouseKind::Up) || wheel
        }
        MouseProtocolMode::ButtonMotion => {
            matches!(kind, MouseKind::Down | MouseKind::Up | MouseKind::Drag) || wheel
        }
        MouseProtocolMode::AnyMotion => true,
    }
}

/// Encode `m` for the child using `encoding`. Returns `None` when it can't be
/// represented (legacy encodings can't carry large coordinates).
pub fn encode(m: &MouseEvent, encoding: MouseProtocolEncoding) -> Option<Vec<u8>> {
    let cb = button_byte(m);
    match encoding {
        MouseProtocolEncoding::Sgr => {
            let final_byte = if m.kind == MouseKind::Up { 'm' } else { 'M' };
            let col = m.col as u32 + 1;
            let row = m.row as u32 + 1;
            Some(format!("\x1b[<{cb};{col};{row}{final_byte}").into_bytes())
        }
        // Legacy X10 encoding (and best-effort for UTF-8): ESC [ M, then three
        // bytes Cb+32, Cx+32, Cy+32. Releases report button 3.
        MouseProtocolEncoding::Default | MouseProtocolEncoding::Utf8 => {
            let cb = if m.kind == MouseKind::Up {
                (cb & !0b11) | 0b11
            } else {
                cb
            };
            let b0 = 32 + cb;
            let bx = 32 + m.col as u32 + 1;
            let by = 32 + m.row as u32 + 1;
            if b0 > 255 || bx > 255 || by > 255 {
                return None;
            }
            Some(vec![0x1b, b'[', b'M', b0 as u8, bx as u8, by as u8])
        }
    }
}

/// Reconstruct the SGR/X10 "button byte" (Cb) from a decoded event.
fn button_byte(m: &MouseEvent) -> u32 {
    let mut cb = match m.kind {
        MouseKind::ScrollUp => 64,
        MouseKind::ScrollDown => 65,
        MouseKind::ScrollLeft => 66,
        MouseKind::ScrollRight => 67,
        _ => match m.button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::None => 3,
        },
    };
    if matches!(m.kind, MouseKind::Drag | MouseKind::Moved) {
        cb += 32; // motion bit
    }
    if m.shift {
        cb += 4;
    }
    if m.alt {
        cb += 8;
    }
    if m.ctrl {
        cb += 16;
    }
    cb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: MouseKind, button: MouseButton, col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            button,
            col,
            row,
            shift: false,
            alt: false,
            ctrl: false,
        }
    }

    #[test]
    fn sgr_left_down() {
        let e = ev(MouseKind::Down, MouseButton::Left, 0, 0);
        assert_eq!(encode(&e, MouseProtocolEncoding::Sgr).unwrap(), b"\x1b[<0;1;1M");
    }

    #[test]
    fn sgr_left_up_uses_lowercase_m() {
        let e = ev(MouseKind::Up, MouseButton::Left, 0, 0);
        assert_eq!(encode(&e, MouseProtocolEncoding::Sgr).unwrap(), b"\x1b[<0;1;1m");
    }

    #[test]
    fn sgr_alt_left_down() {
        let mut e = ev(MouseKind::Down, MouseButton::Left, 4, 2);
        e.alt = true;
        assert_eq!(encode(&e, MouseProtocolEncoding::Sgr).unwrap(), b"\x1b[<8;5;3M");
    }

    #[test]
    fn sgr_scroll_up() {
        let e = ev(MouseKind::ScrollUp, MouseButton::None, 0, 0);
        assert_eq!(
            encode(&e, MouseProtocolEncoding::Sgr).unwrap(),
            b"\x1b[<64;1;1M"
        );
    }

    #[test]
    fn legacy_left_down() {
        let e = ev(MouseKind::Down, MouseButton::Left, 0, 0);
        // Cb=0 -> 32 (space), Cx=1 -> 33 ('!'), Cy=1 -> 33 ('!').
        assert_eq!(
            encode(&e, MouseProtocolEncoding::Default).unwrap(),
            vec![0x1b, b'[', b'M', 32, 33, 33]
        );
    }

    #[test]
    fn legacy_release_reports_button_three() {
        let e = ev(MouseKind::Up, MouseButton::Left, 0, 0);
        // release -> Cb low bits = 3 -> 32 + 3 = 35.
        assert_eq!(
            encode(&e, MouseProtocolEncoding::Default).unwrap()[3],
            35
        );
    }

    #[test]
    fn motion_mirroring() {
        use MouseProtocolMode::*;
        assert_eq!(desired_motion(None), OuterMotion::Base);
        assert_eq!(desired_motion(PressRelease), OuterMotion::Base);
        assert_eq!(desired_motion(ButtonMotion), OuterMotion::Button);
        assert_eq!(desired_motion(AnyMotion), OuterMotion::Any);

        assert_eq!(motion_transition(OuterMotion::Base, OuterMotion::Base), b"");
        assert_eq!(
            motion_transition(OuterMotion::Base, OuterMotion::Button),
            b"\x1b[?1002h"
        );
        assert_eq!(
            motion_transition(OuterMotion::Button, OuterMotion::Base),
            b"\x1b[?1002l"
        );
        assert_eq!(
            motion_transition(OuterMotion::Button, OuterMotion::Any),
            b"\x1b[?1002l\x1b[?1003h"
        );
    }

    #[test]
    fn forwarding_policy() {
        use MouseProtocolMode::*;
        assert!(!should_forward(None, MouseKind::Down));
        assert!(should_forward(Press, MouseKind::Down));
        assert!(!should_forward(Press, MouseKind::Up));
        assert!(should_forward(PressRelease, MouseKind::Up));
        assert!(!should_forward(PressRelease, MouseKind::Drag));
        assert!(should_forward(ButtonMotion, MouseKind::Drag));
        assert!(should_forward(AnyMotion, MouseKind::Moved));
        assert!(should_forward(Press, MouseKind::ScrollUp));
    }
}
