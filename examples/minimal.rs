//! Minimal proxtty library usage.
//!
//! Forwards every input to the child except the hotkey (Ctrl-Space), which
//! toggles a tiny overlay box on top of the live session.
//!
//! Run: `cargo run --example minimal -- zsh`

use std::ops::ControlFlow;

use proxtty::{InputEvent, Proxy, ProxyConfig, ProxyEvent};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = if args.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        args
    };

    let proxy = Proxy::start(&command, ProxyConfig::default())?;

    let mut shown = false;
    let code = proxy.run_with(move |proxy, event| match event {
        ProxyEvent::Input(InputEvent::Hotkey) => {
            shown = !shown;
            if shown {
                proxy.set_overlay(b"\x1b[?25l\x1b[2;3H\x1b[7m hello from proxtty \x1b[0m");
            } else {
                proxy.clear_overlay();
            }
            ControlFlow::Continue(())
        }
        ProxyEvent::Input(ev) => {
            proxy.forward(ev); // everything else passes through
            ControlFlow::Continue(())
        }
        ProxyEvent::Exited(code) => ControlFlow::Break(code),
        _ => ControlFlow::Continue(()),
    })?;

    std::process::exit(code);
}
