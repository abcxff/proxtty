# smartty

A smart PTY proxy that wraps an interactive terminal command, forwards terminal
I/O transparently, intercepts local input events, and renders overlay UI on top
of the child terminal session.

The motivating use case: run `ssh` inside a shell inside `smartty`, then
Option-click anywhere to open a local contextual menu over the remote session.

See [VISION.md](VISION.md) for the full design and [TODO.md](TODO.md) for the
roadmap.

> ## ⚠️ Tested on macOS + Ghostty only
>
> smartty was **developed and tested exclusively on macOS using the
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
  mouse reporting; otherwise the wheel drives smartty's own scrollback.
- **Crash-safe.** The terminal is restored on every exit path — normal exit,
  child exit, signals, and panics.

## Build

Requires a Rust toolchain.

```sh
cargo build            # debug
cargo build --release  # optimized → target/release/smartty
```

## Usage

Wrap any interactive command. With no command, smartty runs your `$SHELL`:

```sh
cargo run -- zsh
cargo run -- -- ssh my-server   # use `--` to pass flags through to the child
./target/release/smartty zsh
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

Optional. Loaded from `$SMARTTY_CONFIG`, else `~/.config/smartty/config.toml`. A
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

## Known limitations

- **Scrollback:** smartty runs on the outer terminal's alternate screen, so its
  own wheel scrollback (10,000 lines) replaces the terminal's native scrollback
  during a session; your pre-`smartty` screen is restored on exit.
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

Set `SMARTTY_DEBUG=/path/to/log` to append a diagnostic trace (raw input bytes,
scroll reactions) to a file — useful since stderr is on the alternate screen
while running. No overhead when unset.

## Recovering a stuck terminal

smartty restores your terminal on normal exit, child exit, and panics. If it is
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
