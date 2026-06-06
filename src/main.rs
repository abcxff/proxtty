//! proxtty CLI — a context-menu overlay built on the `proxtty` proxy library.
//!
//! This binary is just one *consumer* of the library: it forwards all input to
//! the child except a hotkey / trigger-click that opens a configurable menu,
//! which it draws via `Proxy::set_overlay`. The proxy itself (transparent
//! forwarding, screen model, rendering, scrollback) lives in `src/lib.rs`.

mod actions;
mod cli;
mod config;
mod overlay;

use std::ops::ControlFlow;

use proxtty::{InputEvent, MouseButton, MouseEvent, MouseKind, Proxy, ProxyConfig, ProxyEvent};

use cli::Cli;
use config::{ActionSpec, Config, TriggerMods};
use overlay::{MenuOutcome, MenuState};

fn main() {
    let config = config::load();
    let command = Cli::parse().resolve(config.shell.as_deref());

    let proxy_config = ProxyConfig {
        hotkey: Some(config.hotkey_byte()),
        scrollback: 10_000,
        size: None,
        screen_events: false,
        mouse_motion: false,
    };

    let result = Proxy::start(&command, proxy_config).and_then(|proxy| {
        let mut menu = MenuConsumer::new(&config);
        proxy.run_with(move |proxy, event| menu.handle(proxy, event))
    });

    match result {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            // run_with dropped the Proxy (restoring the terminal) before
            // returning, so this prints cleanly.
            eprintln!("proxtty: {err:#}");
            std::process::exit(1);
        }
    }
}

/// Drives the context menu on top of the proxy.
struct MenuConsumer {
    labels: Vec<String>,
    actions: Vec<ActionSpec>,
    border: bool,
    trigger: TriggerMods,
    /// The open menu, if any. Kept in sync with the proxy's overlay.
    menu: Option<MenuState>,
}

impl MenuConsumer {
    fn new(config: &Config) -> MenuConsumer {
        MenuConsumer {
            labels: config.menu.iter().map(|m| m.label.clone()).collect(),
            actions: config.menu.iter().map(|m| m.action.clone()).collect(),
            border: config.border,
            trigger: config.trigger_mods(),
            menu: None,
        }
    }

    fn handle(&mut self, proxy: &mut Proxy, event: ProxyEvent) -> ControlFlow<i32> {
        match event {
            ProxyEvent::Input(ev) => self.on_input(proxy, ev),
            // The proxy already repainted; dismiss a now-stale menu.
            ProxyEvent::Resize { .. } => self.close(proxy),
            ProxyEvent::ScreenChanged => {}
            ProxyEvent::Exited(code) => return ControlFlow::Break(code),
        }
        ControlFlow::Continue(())
    }

    fn on_input(&mut self, proxy: &mut Proxy, ev: InputEvent) {
        if let Some(menu) = &mut self.menu {
            match overlay::handle(menu, &ev) {
                MenuOutcome::Stay => proxy.set_overlay(&overlay::redraw_sequence(menu)),
                MenuOutcome::Close => self.close(proxy),
                MenuOutcome::Selected(idx) => {
                    self.run_action(proxy, idx);
                    self.close(proxy);
                }
            }
        } else {
            match ev {
                InputEvent::Hotkey => self.open(proxy, default_anchor()),
                InputEvent::Mouse(m) if self.is_trigger(&m) => {
                    self.open(proxy, (m.col + 1, m.row + 1))
                }
                // Everything else passes straight through to the child.
                other => proxy.forward(other),
            }
        }
    }

    fn open(&mut self, proxy: &mut Proxy, anchor: (u16, u16)) {
        if self.labels.is_empty() {
            return;
        }
        let menu = MenuState::new(self.labels.clone(), self.border, anchor.0, anchor.1, proxy.size());
        proxy.set_overlay(&overlay::open_sequence(&menu));
        self.menu = Some(menu);
    }

    fn close(&mut self, proxy: &mut Proxy) {
        if self.menu.take().is_some() {
            proxy.clear_overlay();
        }
    }

    fn run_action(&mut self, proxy: &mut Proxy, idx: usize) {
        let Some(action) = self.actions.get(idx).cloned() else {
            return;
        };
        match action {
            ActionSpec::Send { text } => {
                let _ = proxy.send(text.as_bytes());
            }
            ActionSpec::CopyScreen => {
                let text = proxy.visible_text();
                proxy.emit_raw(&actions::osc52_copy(&text));
            }
            ActionSpec::OpenUrl => {
                let text = proxy.visible_text();
                if let Some(url) = actions::find_url(&text) {
                    actions::open_url(url);
                }
            }
        }
    }

    /// Does this mouse event open the overlay, per the configured trigger?
    fn is_trigger(&self, m: &MouseEvent) -> bool {
        if m.kind != MouseKind::Down || m.button != MouseButton::Left {
            return false;
        }
        match self.trigger {
            TriggerMods::Alt => m.alt,
            TriggerMods::Ctrl => m.ctrl,
            TriggerMods::Any => m.alt || m.ctrl,
        }
    }
}

/// Where the hotkey-triggered menu appears when there's no click position.
fn default_anchor() -> (u16, u16) {
    (3, 2)
}
