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

Implemented (Milestones 1–8): transparent proxy with crash-safe terminal
restoration and live resize; local input interception; and a context-menu
overlay composited over a parsed (`vt100`) screen buffer, so closing it
repaints the child screen cleanly with no leftover artifacts.

Press **Ctrl-Space** (or **Option-click** / Ctrl-click / right-click) to open
the menu; arrow keys or `j`/`k` move, Enter selects (placeholder), Esc or a
click outside closes.

Known limitations (addressed by later milestones):

- `smartty` now owns the visible screen and repaints from its buffer; the outer
  terminal's native scrollback no longer accumulates child output (scrollback is
  Milestone 11).
- While `smartty` runs, the terminal's mouse is captured: child apps don't
  receive clicks yet (Milestone 9), and wheel-scroll is intercepted.

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
