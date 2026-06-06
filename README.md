# smartty

A smart PTY proxy that wraps an interactive terminal command, forwards terminal
I/O transparently, and (incrementally) intercepts local input events to render
overlay UI on top of the child terminal session.

See [VISION.md](VISION.md) for the full design and [TODO.md](TODO.md) for the
roadmap.

> **Platforms:** Linux and macOS only.

## Build

```sh
cargo build            # debug
cargo build --release  # optimized
```

## Run

Wrap any interactive command. With no command, smartty runs your `$SHELL`:

```sh
cargo run -- zsh
cargo run -- -- ssh my-server   # use `--` to pass flags to the child
./target/debug/smartty zsh
```

Inside the wrapped shell, normal programs work as usual:

```sh
ssh my-server
vim
fzf
less
top
```

## Current status

Implemented (Milestones 1–15): transparent proxy with crash-safe terminal
restoration and live resize; local input interception; a context-menu overlay
composited over a parsed (`vt100`) screen buffer; mouse forwarding to the child;
alternate-screen / input-mode correctness for full-screen apps; wheel-driven
scrollback; configuration; and working menu actions.

Press **Ctrl-Space** (or **Option-click** / Ctrl-click) to open the menu; arrow
keys or `j`/`k` move, Enter or click selects, Esc or an outside click closes.

Mouse clicks and wheel-scroll are forwarded to child apps (`vim`, `tmux`,
`less`, `fzf`) when they request mouse reporting; only the Option/Ctrl-click
trigger is held back. When the child isn't using the mouse, wheel-scroll moves
through `smartty`'s scrollback. The child's application-cursor, keypad, and
bracketed-paste modes are mirrored onto the outer terminal so keys and pastes
encode correctly.

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

- `smartty` owns the visible screen and repaints from its buffer; the outer
  terminal's native scrollback no longer accumulates child output (use the
  built-in wheel scrollback instead).
- Only press/release/scroll mouse events are forwarded to the child — drag/motion
  isn't yet (would require mirroring the child's motion-tracking mode onto the
  terminal).
- Focus reporting (`CSI ?1004`) is not propagated to the child, so apps that
  redraw on focus change won't see focus events.

Window title, child-initiated clipboard (OSC 52), the bell, and cursor shape
(`DECSCUSR`) are passed through to the outer terminal.

## Recovering a stuck terminal

smartty restores your terminal on normal exit, child exit, and panics. If it is
ever killed in a way that bypasses cleanup (e.g. `kill -9`) and your terminal is
left in raw mode, run:

```sh
reset
```

or, if the prompt is misbehaving:

```sh
stty sane
```

(You may need to type the command blind and press Enter.)
