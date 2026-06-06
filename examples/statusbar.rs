//! Demonstrates `ScreenChanged`: a one-line status bar overlay that updates as
//! the child screen changes. Try running a busy program under it:
//!
//! Run: `cargo run --example statusbar -- top`

use std::ops::ControlFlow;

use proxtty::{Proxy, ProxyConfig, ProxyEvent};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = if args.is_empty() {
        vec!["/bin/sh".to_string()]
    } else {
        args
    };

    let config = ProxyConfig {
        screen_events: true, // opt into ScreenChanged
        ..ProxyConfig::default()
    };

    let proxy = Proxy::start(&command, config)?;
    let code = proxy.run_with(move |proxy, event| match event {
        // Regenerate the bar from the current screen each time the child repaints.
        ProxyEvent::ScreenChanged | ProxyEvent::Resize { .. } => {
            draw_status(proxy);
            ControlFlow::Continue(())
        }
        ProxyEvent::Input(ev) => {
            proxy.forward(ev);
            ControlFlow::Continue(())
        }
        ProxyEvent::Exited(code) => ControlFlow::Break(code),
    })?;

    std::process::exit(code);
}

/// Draw a reverse-video status bar across the bottom row, derived from the
/// current screen contents.
fn draw_status(proxy: &mut Proxy) {
    let (cols, rows) = proxy.size();
    let glyphs = proxy
        .visible_text()
        .chars()
        .filter(|c| !c.is_whitespace())
        .count();

    let label = format!(" proxtty  {cols}x{rows}  {glyphs} glyphs ");
    let visible: String = label.chars().take(cols as usize).collect();
    let pad = (cols as usize).saturating_sub(visible.chars().count());

    let mut bar = format!("\x1b[{rows};1H\x1b[7m{visible}");
    bar.push_str(&" ".repeat(pad));
    bar.push_str("\x1b[0m");
    proxy.set_overlay(bar.as_bytes());
}
