//! proxtty — a smart PTY proxy as a library.
//!
//! [`Proxy`] wraps an interactive command in a pseudo-terminal: it forwards
//! terminal I/O, lets you decide per input event whether it reaches the child,
//! and gives you one overlay layer to render on top of the live child screen.
//! The bundled `proxtty` binary is just one consumer of this library (it adds a
//! context menu); see `examples/` for minimal standalone usage.
//!
//! ```no_run
//! use std::ops::ControlFlow;
//! use proxtty::{Proxy, ProxyConfig, ProxyEvent, InputEvent};
//!
//! let proxy = Proxy::start(&["zsh".into()], ProxyConfig::default())?;
//! let code = proxy.run_with(|proxy, event| match event {
//!     ProxyEvent::Input(InputEvent::Hotkey) => {
//!         proxy.set_overlay(b"\x1b[1;1H\x1b[7m hello \x1b[0m");
//!         ControlFlow::Continue(())
//!     }
//!     ProxyEvent::Input(ev) => {
//!         proxy.forward(ev); // pass everything else through to the child
//!         ControlFlow::Continue(())
//!     }
//!     ProxyEvent::Exited(code) => ControlFlow::Break(code),
//!     _ => ControlFlow::Continue(()),
//! })?;
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! Supported platforms: Linux and macOS (developed and tested on macOS/Ghostty).

pub mod debug;
mod input;
mod mouse;
mod proxy;
mod pty_session;
mod renderer;
mod screen;
mod term;

pub use input::{InputEvent, MouseButton, MouseEvent, MouseKind};
pub use proxy::{Proxy, ProxyConfig, ProxyEvent};
