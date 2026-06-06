# proxtty

> [https://asciinema.org/a/BXgli9YxW7AmRQUz](https://asciinema.org/a/BXgli9YxW7AmRQUz)

A smart PTY proxy that wraps an interactive terminal command, forwards terminal
I/O transparently, intercepts local input events, and renders overlay UI on top
of the child terminal session.

The motivating use case: run `ssh` inside a shell inside `proxtty`, then
Option-click anywhere to open a local contextual menu over the remote session.

> ## ⚠️ Tested on macOS + Ghostty only
>
> proxtty was **developed and tested exclusively on macOS using the
> [Ghostty](https://ghostty.org) terminal.** That is the only configuration it
> has been exercised in.
>
> The code targets **macOS and Linux**, and the defaults (Option-click trigger,
> SGR mouse parsing, alternate-screen rendering) are standard enough that other
> modern terminals — iTerm2, WezTerm, Alacritty, kitty, Terminal.app — and Linux
> *should* work, but **none of them have been verified.** Expect rough edges
> outside macOS/Ghostty, especially around mouse and Option/Alt handling.

## What it does

- **Transparent passthrough.** Runs your command in a PTY and forwards I/O both
  ways, so `ssh`, `vim`, `tmux`, `less`, `fzf`, REPLs, etc. behave normally.
- **Local overlay menu.** A configurable context menu you can open over any
  session — including a remote `ssh` one — without the child process seeing it.
- **Parsed screen model.** Child output is parsed into a virtual screen
  (`vt100`), so overlays composite cleanly and redraw with no artifacts.
- **Mouse-aware.** Clicks, scroll, and drags are forwarded to apps that request
  mouse reporting; otherwise the wheel drives proxtty's own scrollback.
- **Crash-safe.** The terminal is restored on every exit path — normal exit,
  child exit, signals, and panics.

proxtty is a **library** (the proxy) plus a thin **binary** (the menu). The
binary is just one consumer of the library — see [Use as a library](#use-as-a-library).

## Build

Requires a Rust toolchain.

```sh
cargo build            # debug
cargo build --release  # optimized → target/release/proxtty
```

## Usage

Wrap any interactive command. With no command, proxtty runs your `$SHELL`:

```sh
cargo run -- zsh
cargo run -- -- ssh my-server   # use `--` to pass flags through to the child
./target/release/proxtty zsh
```

Inside the wrapped shell, use it like a normal terminal:

```sh
ssh my-server
vim
tmux
fzf
```

### Keys & mouse

| Action | Default |
| --- | --- |
| Open the menu | **Ctrl-Space**, **Option-click**, or **Ctrl-click** |
| Move selection | Arrow keys, or `j` / `k`, or mouse |
| Choose item | **Enter** or click |
| Close menu | **Esc**, `q`, or click outside |
| Scroll history | Mouse wheel (when the child isn't using the mouse) |
| Native text selection | **Shift+drag** (plain drag is captured for the trigger) |

## Configuration

Optional. Loaded from `$PROXTTY_CONFIG`, else `~/.config/proxtty/config.toml`. A
missing or invalid file falls back to built-in defaults.

```toml
hotkey  = "ctrl-space"   # ctrl-space | ctrl-] | ctrl-\ | ctrl-a .. ctrl-z
trigger = "any"          # any | option-click | ctrl-click
shell   = "/bin/zsh"     # used when no command is given on the CLI
border  = true

# Menu items, top to bottom. Each has a label and an action:
#   copy_screen  → copy the visible screen to the clipboard (OSC 52)
#   open_url     → open the first http(s) URL on screen
#   send         → send `text` to the child (e.g. run a command)
[[menu]]
label  = "Copy screen → clipboard"
action = "copy_screen"

[[menu]]
label  = "Open first URL"
action = "open_url"

[[menu]]
label  = "Send: clear"
action = "send"
text   = "clear\n"
```

## Use as a library

The proxy is a crate you can build your own terminal-overlay tools on. The
contract is small and un-opinionated:

- **You decide what passes through.** Every input event is surfaced; call
  `forward()` for the ones that should reach the child, and consume the rest.
- **One overlay layer, empty by default.** `set_overlay(bytes)` draws raw ANSI on
  top of the live child screen (transparent where you don't draw);
  `clear_overlay()` returns to emptiness. Purely visual, independent of input.
- **Opt into `ScreenChanged`** to react when the child repaints (status bars,
  content-derived overlays).

```rust
use std::ops::ControlFlow;
use proxtty::{Proxy, ProxyConfig, ProxyEvent, InputEvent};

let proxy = Proxy::start(&["zsh".into()], ProxyConfig::default())?;
let code = proxy.run_with(|proxy, event| match event {
    // Ctrl-Space: draw a box instead of passing it to the child.
    ProxyEvent::Input(InputEvent::Hotkey) => {
        proxy.set_overlay(b"\x1b[2;3H\x1b[7m hello \x1b[0m");
        ControlFlow::Continue(())
    }
    ProxyEvent::Input(ev) => { proxy.forward(ev); ControlFlow::Continue(()) }
    ProxyEvent::Exited(code) => ControlFlow::Break(code),
    _ => ControlFlow::Continue(()),
})?;
```

Key methods: `poll` / `run_with` (drive the loop), `forward` (pass an input to
the child), `set_overlay` / `clear_overlay`, `emit_raw` (OSC 52 etc.),
`visible_text`, `size`, `send`. Runnable examples:

```sh
cargo run --example minimal -- zsh         # hotkey toggles a box
cargo run --example click-menu -- zsh      # Option-click opens a menu at the cursor
cargo run --example statusbar -- top       # ScreenChanged-driven status bar
cargo run --example rainbow-mouse -- zsh   # a rainbow trail follows the mouse
```

## Known limitations

- **Scrollback:** proxtty runs on the outer terminal's alternate screen, so its
  own wheel scrollback (10,000 lines) replaces the terminal's native scrollback
  during a session; your pre-`proxtty` screen is restored on exit.
- **Text selection:** plain drag is captured for the Option-click trigger — use
  **Shift+drag** for native selection (the terminal's standard bypass).
- **Images:** terminal graphics protocols (Kitty, Sixel, iTerm inline images)
  and OSC 8 hyperlinks are not modeled and won't render.
- **Focus events** (`CSI ?1004`) are not propagated to the child.
- **Not benchmarked** under heavy output floods; the diff renderer clones the
  screen per changed frame.

Window title, child-initiated clipboard (OSC 52), the bell, cursor shape
(`DECSCUSR`), and application-cursor / keypad / bracketed-paste modes *are*
passed through.

## Debugging

Set `PROXTTY_DEBUG=/path/to/log` to append a diagnostic trace (raw input bytes,
scroll reactions) to a file — useful since stderr is on the alternate screen
while running. No overhead when unset.

## Recovering a stuck terminal

proxtty restores your terminal on normal exit, child exit, and panics. If it is
ever killed in a way that bypasses cleanup (e.g. `kill -9`) and your terminal is
left in a bad state, run:

```sh
reset
```

or, if the prompt is misbehaving:

```sh
stty sane
```

(You may need to type the command blind and press Enter.)
