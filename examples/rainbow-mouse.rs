//! A rainbow trail that follows the **mouse pointer** and scrolls with the
//! content.
//!
//! Uses `ProxyConfig::mouse_motion` so the outer terminal reports mouse movement.
//! Each dot remembers the scrollback offset where it was placed, so wheel-scroll
//! slides the trail along with the text under it (dots that scroll off-screen
//! disappear, and come back when you scroll down). Clicks/drags/scroll still pass
//! through to the child.
//!
//! Run: `cargo run --example rainbow-mouse -- zsh`, then move the mouse, type,
//! and scroll. (Tested on Ghostty.)

use std::collections::VecDeque;
use std::ops::ControlFlow;

use smartty::{InputEvent, MouseKind, Proxy, ProxyConfig, ProxyEvent};

/// How many recent mouse positions to keep in the trail.
const TRAIL: usize = 16;

/// A trail dot, anchored to the content by the scroll offset when it was placed.
#[derive(Clone, Copy, PartialEq)]
struct Dot {
    col: u16,
    row: u16,
    offset: usize,
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = if args.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        args
    };

    let config = ProxyConfig {
        mouse_motion: true, // report movement so the trail can follow the pointer
        scrollback: 1000,   // so wheel-scroll has somewhere to go
        ..ProxyConfig::default()
    };

    let proxy = Proxy::start(&command, config)?;

    let mut trail: VecDeque<Dot> = VecDeque::new();
    let mut hue: f32 = 0.0;

    let code = proxy.run_with(move |proxy, event| {
        match event {
            ProxyEvent::Input(InputEvent::Mouse(m)) => {
                // Clicks/drags/scroll pass through to the child (scroll may change
                // the offset); pure motion just drives the trail.
                if m.kind != MouseKind::Moved {
                    proxy.forward(InputEvent::Mouse(m));
                }

                // Drop a fresh head at the pointer, anchored to the *current*
                // scroll offset, so the rainbow follows the mouse and a scroll
                // tick streams the trail away with the content.
                let dot = Dot {
                    col: m.col,
                    row: m.row,
                    offset: proxy.scroll_offset(),
                };
                if trail.front() != Some(&dot) {
                    trail.push_front(dot);
                    trail.truncate(TRAIL);
                    hue = (hue + 17.0) % 360.0;
                }

                draw(proxy, &trail, hue);
            }
            // Keyboard etc. passes through. Child output from it makes the proxy
            // repaint and recomposite the stored trail, so it stays put.
            ProxyEvent::Input(ev) => proxy.forward(ev),
            ProxyEvent::Resize { .. } => {
                trail.clear();
                proxy.clear_overlay();
            }
            ProxyEvent::ScreenChanged => {}
            ProxyEvent::Exited(code) => return ControlFlow::Break(code),
        }
        ControlFlow::Continue(())
    })?;

    std::process::exit(code);
}

/// Repaint the child and composite the trail in a single write (no clear-then-
/// draw flicker). Clears the overlay if there's nothing visible to draw.
fn draw(proxy: &mut Proxy, trail: &VecDeque<Dot>, hue: f32) {
    let bytes = render_trail(proxy, trail, hue);
    if bytes.is_empty() {
        proxy.clear_overlay();
    } else {
        proxy.replace_overlay(&bytes);
    }
}

/// Build overlay bytes: for each dot (shifted by how far the view has scrolled,
/// skipping any off-screen), draw a solid rainbow block over empty cells, or the
/// underlying character in white over the rainbow where there's text — so the
/// trail tints the screen without hiding it. The real cursor is hidden while
/// drawing, then parked back at the child's cursor so it doesn't flicker.
fn render_trail(proxy: &Proxy, trail: &VecDeque<Dot>, base_hue: f32) -> Vec<u8> {
    let offset = proxy.scroll_offset();
    let (_, rows) = proxy.size();
    let n = trail.len() as f32;
    let mut s = String::from("\x1b[?25l");
    let mut drew = false;
    for (i, dot) in trail.iter().enumerate() {
        // Scrolling up (offset grows) moves content — and the dot — downward.
        let screen_row = dot.row as i64 + (offset as i64 - dot.offset as i64);
        if screen_row < 0 || screen_row >= rows as i64 {
            continue; // scrolled off the visible area
        }
        let hue = (base_hue + i as f32 * 22.0) % 360.0;
        let value = 1.0 - (i as f32 / n) * 0.85; // head bright, tail dim
        let (r, g, b) = hsv_to_rgb(hue, 1.0, value);
        s.push_str(&format!("\x1b[{};{}H", screen_row + 1, dot.col + 1));
        match proxy.cell(dot.col, screen_row as u16) {
            // Text under the trail: keep it readable as white on the rainbow.
            Some(ch) => {
                s.push_str(&format!("\x1b[38;2;255;255;255;48;2;{r};{g};{b}m{ch}\x1b[0m"));
            }
            // Empty cell: a solid rainbow block.
            None => {
                s.push_str(&format!("\x1b[38;2;{r};{g};{b}m\u{2588}\x1b[0m"));
            }
        }
        drew = true;
    }
    if !drew {
        return Vec::new();
    }
    // Park the cursor back where the child has it (the prompt) and show it again,
    // so it stays put instead of flickering. While scrolled into history, leave
    // it hidden (the live cursor isn't in view).
    if offset == 0 {
        let (col, row) = proxy.cursor();
        s.push_str(&format!("\x1b[{};{}H\x1b[?25h", row + 1, col + 1));
    }
    s.into_bytes()
}

/// HSV (h in degrees, s/v in 0..=1) to 8-bit RGB.
fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - (((h / 60.0) % 2.0) - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h as u16 / 60) % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}
