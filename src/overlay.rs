//! Local overlay UI.
//!
//! Milestone 5 is the *crude* overlay: a menu drawn with raw ANSI escapes, with
//! no knowledge of what's underneath it. It opens, takes over input, and on close
//! erases its own area and restores the cursor. Because there's no screen buffer
//! yet, closing leaves a blank rectangle where the menu was — that hole is fixed
//! in Milestones 6–8 once `smartty` parses the child screen and can recomposite.

use crate::input::{InputEvent, MouseButton, MouseEvent, MouseKind};

/// Placeholder menu actions (wired to real behavior in Milestone 15).
const ITEMS: &[&str] = &[
    "Open URL",
    "Copy selection",
    "Send command",
    "Inspect cell",
    "Keybindings",
];

/// The currently active overlay, if any.
pub enum Overlay {
    None,
    Menu(MenuState),
}

impl Overlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, Overlay::None)
    }
}

/// State of an open context menu. Screen coordinates are 1-based (top-left of
/// the box), already clamped to fit within the terminal.
pub struct MenuState {
    items: Vec<&'static str>,
    selected: usize,
    col: u16,
    row: u16,
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
    pub fn new(anchor_col: u16, anchor_row: u16, size: (u16, u16)) -> MenuState {
        let (cols, rows) = size;
        let total_w = box_width() as u16;
        let total_h = box_height() as u16;

        // Largest valid 1-based top-left such that the box still fits.
        let max_col = cols.saturating_sub(total_w).saturating_add(1).max(1);
        let max_row = rows.saturating_sub(total_h).saturating_add(1).max(1);

        MenuState {
            items: ITEMS.to_vec(),
            selected: 0,
            col: anchor_col.clamp(1, max_col),
            row: anchor_row.clamp(1, max_row),
        }
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

/// Width of the widest menu label (display columns, ASCII placeholders).
fn inner_width() -> usize {
    ITEMS.iter().map(|s| s.chars().count()).max().unwrap_or(0)
}

/// Total box width including borders and one space of padding each side.
fn box_width() -> usize {
    inner_width() + 4
}

/// Total box height including the top and bottom borders.
fn box_height() -> usize {
    ITEMS.len() + 2
}

fn goto(row: u16, col: u16) -> String {
    format!("\x1b[{row};{col}H")
}

/// Bytes to emit when first opening the menu: save the cursor, hide it, draw.
pub fn open_sequence(menu: &MenuState) -> Vec<u8> {
    let mut s = String::from("\x1b7\x1b[?25l"); // DECSC + hide cursor
    s.push_str(&draw_box(menu));
    s.into_bytes()
}

/// Bytes to redraw the menu in place (e.g. after the selection moves).
pub fn redraw_sequence(menu: &MenuState) -> Vec<u8> {
    draw_box(menu).into_bytes()
}

/// Bytes to emit when closing: erase the menu area, restore and show the cursor.
pub fn close_sequence(menu: &MenuState) -> Vec<u8> {
    let mut s = clear_box(menu);
    s.push_str("\x1b8\x1b[?25h"); // DECRC + show cursor
    s.into_bytes()
}

fn draw_box(menu: &MenuState) -> String {
    let inner = inner_width();
    let mut s = String::new();

    // Top border.
    s.push_str(&goto(menu.row, menu.col));
    s.push('┌');
    for _ in 0..inner + 2 {
        s.push('─');
    }
    s.push('┐');

    // Items.
    for (i, item) in menu.items.iter().enumerate() {
        s.push_str(&goto(menu.row + 1 + i as u16, menu.col));
        s.push('│');
        let label = format!(" {item:<inner$} ");
        if i == menu.selected {
            s.push_str("\x1b[7m");
            s.push_str(&label);
            s.push_str("\x1b[27m");
        } else {
            s.push_str(&label);
        }
        s.push('│');
    }

    // Bottom border.
    s.push_str(&goto(menu.row + 1 + menu.items.len() as u16, menu.col));
    s.push('└');
    for _ in 0..inner + 2 {
        s.push('─');
    }
    s.push('┘');

    s
}

fn clear_box(menu: &MenuState) -> String {
    let blanks = " ".repeat(box_width());
    let mut s = String::new();
    for r in 0..box_height() as u16 {
        s.push_str(&goto(menu.row + r, menu.col));
        s.push_str(&blanks);
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
    // Whole-keypress matches: a real arrow key arrives as a 3-byte chunk, a lone
    // Escape as a single byte, so these are unambiguous.
    match bytes {
        [0x1b] => return MenuOutcome::Close,
        b"\x1b[A" => {
            menu.move_up();
            return MenuOutcome::Stay;
        }
        b"\x1b[B" => {
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

    let first_item_row = menu.row + 1;
    let last_item_row = menu.row + menu.items.len() as u16;
    let in_cols = click_col >= menu.col && click_col < menu.col + box_width() as u16;

    if in_cols && click_row >= first_item_row && click_row <= last_item_row {
        let idx = (click_row - first_item_row) as usize;
        if m.button == MouseButton::Left {
            return MenuOutcome::Selected(idx);
        }
        menu.selected = idx;
        return MenuOutcome::Stay;
    }

    // A click anywhere outside the menu dismisses it.
    MenuOutcome::Close
}
