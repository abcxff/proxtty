# smartty Vision

## One-line description

**smartty** is a smart PTY proxy that wraps an interactive terminal command, forwards terminal I/O transparently, intercepts selected local input events, and renders local overlay UI on top of the child terminal session.

## Core idea

The goal is to run something like:

```sh
smartty zsh
```

Then inside that shell, the user can run normal interactive programs:

```sh
ssh server
vim
fzf
less
top
```

From the child program's point of view, it is running inside a normal terminal. From the user's point of view, it feels like a regular shell session, except `smartty` can add local terminal-native UI features on top.

The motivating use case is:

> Run `ssh` inside `zsh` inside a PTY controlled by `smartty`, then Option-click anywhere in the terminal to open a local contextual menu over the remote SSH session.

## Mental model

`smartty` sits between the user's real terminal and the child process:

```txt
real terminal app
    ↑↓
smartty
    ↑↓
PTY master
    ↑↓
PTY slave
    ↑↓
zsh / ssh / vim / fzf / etc.
```

Most of the time, `smartty` is transparent:

```txt
keyboard input  -> child PTY
PTY output      -> real terminal
```

But it can selectively intercept local events:

```txt
Option-click    -> open local overlay menu
hotkey          -> open command palette
right-click     -> inspect cell / URL / command
resize          -> resize child PTY and redraw
```

## What smartty is

`smartty` is best described as a:

- smart PTY proxy
- terminal interposer
- terminal middleware layer
- interactive PTY wrapper
- terminal overlay proxy

The most accurate technical description is:

> A smart PTY proxy that wraps an interactive command, forwards terminal I/O, intercepts local mouse/key events, and renders overlay UI on top of the child terminal session.

## What smartty is not

`smartty` is not primarily a terminal multiplexer like `tmux`.

It does not need, at least initially:

- panes
- windows
- sessions
- copy mode
- scrollback management
- remote attach/detach
- persistent server state

Those features could exist later, but they are not the core idea.

The core idea is much narrower:

> Make an existing terminal session locally extensible without breaking normal terminal behavior.

## Primary use case

The first real milestone should be:

```sh
smartty zsh
```

Then, inside the shell:

```sh
ssh my-server
```

Then:

1. The remote SSH session behaves normally.
2. Keyboard input passes through normally.
3. Full-screen apps like `vim`, `less`, and `fzf` mostly work.
4. Option-click is intercepted by `smartty`.
5. A local menu appears on top of the SSH screen.
6. Escape closes the menu.
7. The underlying SSH screen is restored cleanly.

## Why this is useful

A normal terminal emulator owns the UI layer, but child programs own the terminal contents. Once inside `ssh`, the local machine usually has very little contextual UI control over what is happening.

`smartty` creates a programmable layer between the terminal and the child session.

That enables local features such as:

- contextual menus over SSH sessions
- command palettes for terminal actions
- URL or path detection under the cursor
- local annotations or overlays
- terminal-aware inspection tools
- mouse gestures that do not get forwarded to the remote app
- custom keybindings independent of the remote host
- local automation around remote terminal workflows

## Key design principle

`smartty` should be transparent by default and invasive only when explicitly triggered.

Normal terminal behavior should pass through unchanged unless the user performs a configured local action.

Good default policy:

```txt
If smartty recognizes a local binding:
    handle it locally
else:
    forward it to the child PTY
```

## Dumb proxy vs smart proxy

### Dumb PTY proxy

A dumb proxy only forwards bytes:

```txt
real terminal input -> child PTY
child PTY output    -> real terminal output
```

This is simple and useful for early testing, logging, or basic hotkeys.

But it cannot safely draw overlays because it does not know what is currently on screen.

### Smart PTY proxy

A smart proxy parses child terminal output into a virtual screen buffer:

```txt
PTY output -> VT parser -> screen buffer -> renderer -> real terminal
```

This allows `smartty` to render overlays on top of the child terminal and restore the underlying screen afterward.

For the overlay-menu use case, `smartty` eventually needs to become a smart proxy.

## Terminal model

The important terms:

```txt
terminal = the user-facing app/window, such as Ghostty, iTerm2, Alacritty, WezTerm
TTY      = the Unix terminal device abstraction
PTY      = pseudo-terminal, a fake terminal pair used to run interactive programs
```

In a regular terminal emulator:

```txt
User
 ↓
Terminal emulator
 ↓
PTY master
 ↓
PTY slave, such as /dev/pts/3
 ↓
Shell / program
```

In `smartty`:

```txt
User
 ↓
Outer terminal emulator
 ↓
smartty
 ↓
PTY master
 ↓
PTY slave
 ↓
Shell / program
```

`smartty` pretends to be a terminal emulator from the child program's point of view.

## Overlay philosophy

Overlays should be local UI.

They should not mutate the child PTY unless the user chooses an action that explicitly sends input or commands to the child.

Examples of overlays:

- context menu
- command palette
- URL picker
- small inspector popup
- keybinding help
- connection/session metadata
- search box

Render order:

```txt
1. render child terminal screen
2. render smartty overlay on top
3. place cursor appropriately
```

When the overlay closes, `smartty` should redraw the child screen from its internal buffer.

## Input policy

`smartty` receives input from the outer terminal first.

It then decides whether to handle or forward the input.

Example policy:

```txt
if overlay is open:
    route keyboard/mouse to overlay
else if input matches smartty binding:
    handle locally
else:
    forward to child PTY
```

For mouse input:

```txt
Option-click -> local smartty menu
normal click -> forward to child app when appropriate
```

Mouse forwarding should respect child terminal mouse modes eventually, but the first implementation can be conservative.

## Important constraints

### Option-click may not always work

Different terminals handle Option/Alt-click differently. Some terminals may reserve it for their own behavior or may not report the modifier cleanly.

Fallback bindings should exist:

- Ctrl-click
- right-click
- prefix key then click
- keyboard shortcut

### Full terminal correctness is hard

The difficult parts are not the menu itself. The difficult parts are preserving the illusion that the child process is connected to a normal xterm-like terminal.

Risk areas:

- alternate screen
- resize handling
- mouse reporting modes
- cursor visibility and shape
- bracketed paste
- focus events
- UTF-8
- wide Unicode cells
- combining characters
- truecolor and text attributes
- scroll regions
- nested terminal apps
- compatibility with `vim`, `ssh`, `fzf`, `less`, `tmux`, and `ncurses`

## Suggested initial stack

For Rust:

```txt
portable-pty   -> spawn and manage child PTY
vt100          -> parse terminal output into a screen buffer
crossterm      -> raw mode, input events, mouse capture, resize events
ratatui        -> optional higher-level overlay UI
```

For a more custom implementation:

```txt
portable-pty + vte + custom Cell grid + custom renderer
```

For the first real prototype, prefer:

```txt
portable-pty + vt100 + crossterm
```

## Long-term product direction

`smartty` could become a programmable terminal middleware layer.

Possible future features:

- local contextual menus over SSH
- terminal command palette
- plugin system
- URL/path detection
- remote host awareness
- session metadata overlays
- local macro/actions system
- command injection helpers
- AI-assisted shell overlays
- terminal object inspection
- structured scrollback/search
- project-specific keybindings
- remote-safe clipboard helpers

But the first version should stay narrow:

> Run a command, proxy its PTY, intercept a local trigger, show a menu, restore the screen.

## Success criteria

The MVP succeeds when this works reliably:

```sh
smartty zsh
```

Inside it:

```sh
ssh server
vim
fzf
less
```

And while inside those programs:

- normal input/output works
- terminal resize works
- Option-click or fallback trigger opens a local menu
- closing the menu restores the terminal view
- the child program does not receive intercepted local events
- the terminal is restored cleanly on exit or crash

## Guiding sentence

`smartty` should feel like a normal terminal session until the user asks for something smarter.
