//! Option-click context menu, built only on the `proxtty` library.
//!
//! Demonstrates the consumer deciding what a mouse event means: an Option-click
//! (Alt+left, or Ctrl+left as a fallback) opens a small menu *at the click
//! point*; arrows/j/k or a click move the selection; Enter or a click on an item
//! runs that command in the child shell; Esc / `q` / an outside click dismisses.
//! Everything else passes straight through.
//!
//! Run: `cargo run --example click-menu -- zsh`
//! Then Option-click anywhere.

use std::ops::ControlFlow;

use proxtty::{InputEvent, MouseButton, MouseEvent, MouseKind, Proxy, ProxyConfig, ProxyEvent};

/// Menu entries; selecting one sends `"<item>\n"` to the child.
const ITEMS: &[&str] = &["echo hi", "date", "clear"];

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = if args.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        args
    };

    // No keyboard hotkey for this example — the trigger is the mouse.
    let config = ProxyConfig {
        hotkey: None,
        ..ProxyConfig::default()
    };

    let proxy = Proxy::start(&command, config)?;

    let mut menu: Option<Menu> = None;
    let code = proxy.run_with(move |proxy, event| {
        match event {
            ProxyEvent::Input(ev) => {
                // If a menu is open, route the event to it; otherwise look for the
                // trigger. Compute the action under a scoped borrow of `menu`,
                // then act (so we can reassign `menu`).
                let action = menu.as_mut().map(|m| m.handle(&ev));
                match action {
                    Some(MenuAct::Stay) => {
                        if let Some(m) = &menu {
                            proxy.set_overlay(&m.draw());
                        }
                    }
                    Some(MenuAct::Close) => {
                        menu = None;
                        proxy.clear_overlay();
                    }
                    Some(MenuAct::Run(idx)) => {
                        menu = None;
                        proxy.clear_overlay();
                        let _ = proxy.send(format!("{}\n", ITEMS[idx]).as_bytes());
                    }
                    None => match ev {
                        InputEvent::Mouse(m) if is_option_click(&m) => {
                            let new = Menu::new(m.col, m.row, proxy.size());
                            proxy.set_overlay(&new.draw());
                            menu = Some(new);
                        }
                        other => proxy.forward(other),
                    },
                }
            }
            // The proxy already repainted; drop a now-stale menu.
            ProxyEvent::Resize { .. } => {
                if menu.take().is_some() {
                    proxy.clear_overlay();
                }
            }
            ProxyEvent::ScreenChanged => {}
            ProxyEvent::Exited(code) => return ControlFlow::Break(code),
        }
        ControlFlow::Continue(())
    })?;

    std::process::exit(code);
}

/// Option-click is the trigger (Ctrl-click as a fallback for terminals that
/// swallow Alt). The child still gets plain and right clicks.
fn is_option_click(m: &MouseEvent) -> bool {
    m.kind == MouseKind::Down && m.button == MouseButton::Left && (m.alt || m.ctrl)
}

/// What handling an event did to the menu.
enum MenuAct {
    Stay,
    Close,
    Run(usize),
}

/// A small menu anchored (1-based) at the click point, clamped to the screen.
struct Menu {
    col: u16,
    row: u16,
    selected: usize,
}

impl Menu {
    fn new(click_col: u16, click_row: u16, size: (u16, u16)) -> Menu {
        let (cols, rows) = size;
        let w = box_width() as u16;
        let h = box_height() as u16;
        // Mouse coords are 0-based; screen coords are 1-based. Clamp so the box fits.
        let max_col = cols.saturating_sub(w).saturating_add(1).max(1);
        let max_row = rows.saturating_sub(h).saturating_add(1).max(1);
        Menu {
            col: (click_col + 1).clamp(1, max_col),
            row: (click_row + 1).clamp(1, max_row),
            selected: 0,
        }
    }

    fn up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn down(&mut self) {
        if self.selected + 1 < ITEMS.len() {
            self.selected += 1;
        }
    }

    fn handle(&mut self, ev: &InputEvent) -> MenuAct {
        match ev {
            InputEvent::Hotkey => MenuAct::Close,
            InputEvent::Mouse(m) => {
                if m.kind != MouseKind::Down {
                    return MenuAct::Stay;
                }
                let first = self.row + 1;
                let last = first + ITEMS.len() as u16; // exclusive
                let cc = m.col + 1;
                let cr = m.row + 1;
                let in_cols = cc >= self.col && cc < self.col + box_width() as u16;
                if in_cols && cr >= first && cr < last {
                    MenuAct::Run((cr - first) as usize)
                } else {
                    MenuAct::Close
                }
            }
            InputEvent::Forward(bytes) => match bytes.as_slice() {
                [0x1b] => MenuAct::Close,
                b"\x1b[A" | b"\x1bOA" => {
                    self.up();
                    MenuAct::Stay
                }
                b"\x1b[B" | b"\x1bOB" => {
                    self.down();
                    MenuAct::Stay
                }
                _ => {
                    for &b in bytes {
                        match b {
                            b'k' => self.up(),
                            b'j' => self.down(),
                            b'q' => return MenuAct::Close,
                            b'\r' | b'\n' => return MenuAct::Run(self.selected),
                            _ => {}
                        }
                    }
                    MenuAct::Stay
                }
            },
        }
    }

    /// Positioned ANSI for the menu box (hides the cursor; the proxy restores it
    /// on `clear_overlay`).
    fn draw(&self) -> Vec<u8> {
        let inner = inner_width();
        let goto = |r: u16, c: u16| format!("\x1b[{r};{c}H");
        let mut s = String::from("\x1b[?25l\x1b[0m");

        s.push_str(&goto(self.row, self.col));
        s.push('┌');
        for _ in 0..inner + 2 {
            s.push('─');
        }
        s.push('┐');

        for (i, item) in ITEMS.iter().enumerate() {
            s.push_str(&goto(self.row + 1 + i as u16, self.col));
            s.push('│');
            let label = format!(" {item:<inner$} ");
            if i == self.selected {
                s.push_str("\x1b[7m");
                s.push_str(&label);
                s.push_str("\x1b[0m");
            } else {
                s.push_str(&label);
            }
            s.push('│');
        }

        s.push_str(&goto(self.row + 1 + ITEMS.len() as u16, self.col));
        s.push('└');
        for _ in 0..inner + 2 {
            s.push('─');
        }
        s.push('┘');

        s.into_bytes()
    }
}

fn inner_width() -> usize {
    ITEMS.iter().map(|s| s.len()).max().unwrap_or(0)
}

fn box_width() -> usize {
    inner_width() + 4
}

fn box_height() -> usize {
    ITEMS.len() + 2
}
