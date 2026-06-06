//! Local overlay UI (a context menu) — a consumer of the `smartty` proxy library.
//!
//! The menu is drawn with raw ANSI escapes and handed to `Proxy::set_overlay`,
//! which composites it over the live child screen; `Proxy::clear_overlay` wipes
//! it. This module only knows how to *draw* the menu and route input to it. Item
//! labels come from the user's config; what each item does is decided by `main`.

use smartty::{InputEvent, MouseButton, MouseEvent, MouseKind};

/// State of an open context menu. Screen coordinates are 1-based (top-left of
/// the box), already clamped to fit within the terminal.
pub struct MenuState {
    items: Vec<String>,
    selected: usize,
    col: u16,
    row: u16,
    border: bool,
}

/// What handling an input event did to the menu.
pub enum MenuOutcome {
    /// Menu stays open (selection may have changed — caller should redraw).
    Stay,
    /// Menu should close with no action.
    Close,
    /// An item was chosen.
    Selected(usize),
}

impl MenuState {
    /// Build a menu anchored near (`anchor_col`, `anchor_row`), clamped so the
    /// whole box fits inside a terminal of `size` = (cols, rows). Coordinates
    /// are 1-based.
    pub fn new(
        items: Vec<String>,
        border: bool,
        anchor_col: u16,
        anchor_row: u16,
        size: (u16, u16),
    ) -> MenuState {
        let mut menu = MenuState {
            items,
            selected: 0,
            col: 1,
            row: 1,
            border,
        };
        let (cols, rows) = size;
        let total_w = menu.box_width() as u16;
        let total_h = menu.box_height() as u16;
        let max_col = cols.saturating_sub(total_w).saturating_add(1).max(1);
        let max_row = rows.saturating_sub(total_h).saturating_add(1).max(1);
        menu.col = anchor_col.clamp(1, max_col);
        menu.row = anchor_row.clamp(1, max_row);
        menu
    }

    /// Widest label in display columns.
    fn inner_width(&self) -> usize {
        self.items.iter().map(|s| s.chars().count()).max().unwrap_or(0)
    }

    /// Total box width including one space of padding each side, plus borders.
    fn box_width(&self) -> usize {
        self.inner_width() + 2 + if self.border { 2 } else { 0 }
    }

    /// Total box height including borders.
    fn box_height(&self) -> usize {
        self.items.len() + if self.border { 2 } else { 0 }
    }

    /// 1-based screen row of the first item.
    fn first_item_row(&self) -> u16 {
        self.row + if self.border { 1 } else { 0 }
    }

    fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    fn move_down(&mut self) {
        if self.selected + 1 < self.items.len() {
            self.selected += 1;
        }
    }
}

fn goto(row: u16, col: u16) -> String {
    format!("\x1b[{row};{col}H")
}

/// Bytes to emit when first opening the menu: hide the cursor, then draw. The
/// cursor is restored by the renderer's repaint when the menu closes.
pub fn open_sequence(menu: &MenuState) -> Vec<u8> {
    let mut s = String::from("\x1b[?25l"); // hide cursor
    s.push_str(&draw(menu));
    s.into_bytes()
}

/// Bytes to redraw the menu in place (e.g. after the selection moves).
pub fn redraw_sequence(menu: &MenuState) -> Vec<u8> {
    draw(menu).into_bytes()
}

fn draw(menu: &MenuState) -> String {
    let inner = menu.inner_width();
    // Start from default attributes so the box never inherits the child's style.
    let mut s = String::from("\x1b[0m");

    if menu.border {
        s.push_str(&goto(menu.row, menu.col));
        s.push('┌');
        for _ in 0..inner + 2 {
            s.push('─');
        }
        s.push('┐');
    }

    for (i, item) in menu.items.iter().enumerate() {
        s.push_str(&goto(menu.first_item_row() + i as u16, menu.col));
        if menu.border {
            s.push('│');
        }
        let label = format!(" {item:<inner$} ");
        if i == menu.selected {
            s.push_str("\x1b[7m");
            s.push_str(&label);
            s.push_str("\x1b[0m");
        } else {
            s.push_str(&label);
        }
        if menu.border {
            s.push('│');
        }
    }

    if menu.border {
        s.push_str(&goto(menu.row + 1 + menu.items.len() as u16, menu.col));
        s.push('└');
        for _ in 0..inner + 2 {
            s.push('─');
        }
        s.push('┘');
    }

    s
}

/// Route an input event to the menu.
pub fn handle(menu: &mut MenuState, ev: &InputEvent) -> MenuOutcome {
    match ev {
        InputEvent::Hotkey => MenuOutcome::Close, // pressing the trigger again closes
        InputEvent::Mouse(m) => handle_mouse(menu, m),
        InputEvent::Forward(bytes) => handle_keys(menu, bytes),
    }
}

fn handle_keys(menu: &mut MenuState, bytes: &[u8]) -> MenuOutcome {
    // Whole-keypress matches: a real arrow key arrives as a single chunk, a lone
    // Escape as a single byte, so these are unambiguous.
    match bytes {
        [0x1b] => return MenuOutcome::Close,
        // Arrow keys: CSI form, plus the SS3 form used when the child has put the
        // terminal in application-cursor mode.
        b"\x1b[A" | b"\x1bOA" => {
            menu.move_up();
            return MenuOutcome::Stay;
        }
        b"\x1b[B" | b"\x1bOB" => {
            menu.move_down();
            return MenuOutcome::Stay;
        }
        _ => {}
    }
    for &b in bytes {
        match b {
            b'k' => menu.move_up(),
            b'j' => menu.move_down(),
            b'q' => return MenuOutcome::Close,
            b'\r' | b'\n' => return MenuOutcome::Selected(menu.selected),
            _ => {}
        }
    }
    MenuOutcome::Stay
}

fn handle_mouse(menu: &mut MenuState, m: &MouseEvent) -> MenuOutcome {
    if m.kind != MouseKind::Down {
        return MenuOutcome::Stay;
    }
    // Mouse coordinates are 0-based; box coordinates are 1-based.
    let click_col = m.col + 1;
    let click_row = m.row + 1;

    let first = menu.first_item_row();
    let last = first + menu.items.len() as u16; // exclusive
    let in_cols = click_col >= menu.col && click_col < menu.col + menu.box_width() as u16;

    if in_cols && click_row >= first && click_row < last {
        let idx = (click_row - first) as usize;
        if m.button == MouseButton::Left {
            return MenuOutcome::Selected(idx);
        }
        menu.selected = idx;
        return MenuOutcome::Stay;
    }

    // A click anywhere outside the menu dismisses it.
    MenuOutcome::Close
}
