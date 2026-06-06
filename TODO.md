# smartty TODO

A task list to build `smartty` toward the VISION.md goal: a smart PTY proxy that
wraps an interactive command, forwards terminal I/O transparently, intercepts
local input events, and renders overlay UI on top of the child session.

> **North star (final MVP):** `smartty zsh` → `ssh my-server` → Option-click
> opens a local menu over SSH → Escape closes it → the SSH screen restores
> cleanly → normal terminal behavior is unaffected.

Tasks are ordered to match the suggested development order. Complete each
milestone before moving on; later milestones depend on earlier correctness.

---

## Milestone 0 — Project scaffold

1. `cargo init` the Rust project (binary crate named `smartty`)
2. Add dependencies: `portable-pty`, `crossterm`, `vt100` (defer `ratatui`, `vte`, `unicode-width`)
3. Set up module skeleton: `main.rs`, `cli.rs`, `app.rs`, `pty_session.rs`, `event_loop.rs`, `input.rs`, `mouse.rs`, `screen.rs`, `renderer.rs`, `overlay.rs`, `config.rs`, `actions.rs`
4. Add `README.md` with build/run instructions and the `reset` / `stty sane` recovery note
5. Confirm `cargo build` and `cargo run -- zsh` compile and launch (even as a stub)

## Milestone 1 — Dumb PTY passthrough (`smartty zsh` works)

6. Parse CLI args: the command + args to run (default to `$SHELL`)
7. Enter raw mode on the outer terminal
8. Spawn the requested command in a child PTY via `portable-pty`
9. Forward outer stdin bytes → PTY master
10. Forward PTY output bytes → outer stdout
11. Detect child exit and break the loop
12. Smoke test inside `smartty zsh`: `echo hello`, `pwd`, `ls`, `cat`, `python3`, `node`, `ssh some-host`, `vim`, `less`, `fzf`

## Milestone 2 — Lifecycle & cleanup correctness

13. Restore raw mode on normal exit
14. Restore raw mode on child exit / Ctrl-D / EOF
15. Install a cleanup guard (RAII drop guard) that restores the terminal on panic
16. Handle SIGINT / SIGTERM gracefully (restore terminal, reap child)
17. Reap / kill the child process when smartty exits
18. Test crash paths — terminal must not be left in raw mode
19. Document recovery commands (`reset`, `stty sane`) in README

## Milestone 3 — Resize forwarding

20. Query initial outer terminal size and create the PTY with matching rows/cols
21. Listen for resize events / SIGWINCH
22. Resize the child PTY on outer resize
23. Test: `stty size`, `vim`, `less`, `top` all adjust correctly when the window resizes

**→ MVP 2 reached:** usable PTY proxy — resize works; Ctrl-C/Ctrl-D sane; `ssh`, `vim`, `fzf` pass through.

## Milestone 4 — Local input interception

24. Switch input path to structured events (crossterm) where needed
25. Implement policy: matches smartty binding → handle locally; else → forward to child PTY
26. Add a first keyboard trigger (e.g. `Ctrl-Space` / `Ctrl-\` / `Ctrl-]`)
27. Ensure the local shortcut is NOT forwarded to the child
28. Verify normal typing still works and Ctrl-C still reaches the child when not intercepted
29. Enable mouse capture; detect click events
30. Detect Option-click; wire fallback triggers (Ctrl-click, right-click, prefix+click, keyboard shortcut)

**→ MVP 3 reached:** local keyboard shortcut + mouse click detected and withheld from the child.

## Milestone 5 — Crude overlay (no screen buffer yet)

31. Pause input forwarding while the overlay is open
32. Draw a simple ANSI menu (save cursor, draw, clear area on close)
33. Route input to the overlay while open (Escape closes; arrows move; Enter selects)
34. Resume normal child forwarding after close
35. Accept hacky restoration here — known limitation until the screen buffer exists

**→ MVP 4 reached:** local menu opens, receives input, Escape closes, child input resumes.

## Milestone 6 — VT parser & screen buffer

36. Feed PTY output bytes into the `vt100` parser instead of straight to stdout
37. Maintain current screen state (cells, styles, cursor position/visibility)
38. Support normal screen and alternate screen
39. Track foreground/background colors and text attributes

## Milestone 7 — Renderer

40. Render the parsed screen to the outer terminal (start with full redraws)
41. Draw characters with fg/bg colors and bold/underline/inverse attributes
42. Handle cursor position, visibility, and shape
43. Verify correctness against high-output and interactive apps before any optimization

## Milestone 8 — Clean overlay compositing

44. Model overlay state: `enum Overlay { None, Menu(..), CommandPalette(..) }`
45. Render order: child screen → overlay → cursor
46. Input routing: overlay open → overlay; else binding → open overlay; else → child PTY
47. Implement context menu: Option-click at (x,y) opens menu there; Escape closes; arrows + Enter act
48. On overlay close, redraw the child screen from the internal buffer (clean restore)

**→ MVP 5 reached:** smart rendering — parser-backed screen buffer, full-screen renderer, overlay composites and restores cleanly.

## Milestone 9 — Mouse forwarding policy

49. Decide intercept vs. forward per mouse event
50. Track child mouse reporting modes (normal, button-event, any-event, SGR)
51. Re-encode and forward mouse events to the child when it has requested reporting and smartty isn't using them

## Milestone 10 — Alternate-screen correctness

52. Overlays render correctly on top of alt-screen apps
53. Closing an overlay restores the alt-screen app view
54. Exiting a full-screen app restores the previous shell screen
55. Alt-screen content does not pollute scrollback
56. Trigger the overlay inside `vim`, `less`, `fzf`, `top`, `ssh host` and confirm restore

**→ MVP 6 reached (core promise):** `smartty zsh` → `ssh` → Option-click menu over SSH → Escape restores → terminal clean on exit.

## Milestone 11 — Scrollback

57. Keep a ring buffer of normal-screen rows that scroll off
58. Exclude alt-screen rows from scrollback
59. Add a local scrollback view mode (later)

## Milestone 12 — Compatibility pass

60. Run the test matrix: zsh/bash/fish prompt editing, `ssh`, `vim`, `nvim`, `less`, `fzf`, `top`, `htop`, `nano`, `emacs -nw`, `tmux` inside smartty, python/node REPL, `sudo`, `man`
61. Fix bug areas: resize, cursor pos/visibility/shape, alt screen, bracketed paste, focus events, mouse modes, wide Unicode, combining chars, emoji, truecolor, line wrapping, scroll regions, nested terminals

## Milestone 13 — Performance

62. Add diff rendering (previous frame vs. next; redraw only changed cells)
63. Batch stdout writes; avoid redundant style changes and excessive flushes
64. Benchmark high-output: `yes | head -100000`, `find /usr`, `cat large-file`, `cargo build`
65. Check interactive performance in `vim`, `fzf`, `top`

## Milestone 14 — Configuration

66. Load a TOML config (`[bindings]`, `[overlay]`)
67. Make local keybindings, mouse trigger, fallback trigger configurable
68. Make menu actions, default shell command, and overlay style configurable

## Milestone 15 — Actions / plugin model

69. Define the action model: menu item → action → optional local command / PTY input / overlay update
70. Implement built-in actions: copy visible text, copy word/URL under cursor, open URL, send text to child PTY, paste command, inspect cell, session info, show keybindings
71. Allow external-command actions (plugin path)

---

## Cross-cutting risks to watch

- **Terminal restoration** — cleanup guard early; test crash paths; document `reset`.
- **Option-click support** — varies by terminal; keep fallbacks; test Ghostty, iTerm2, Terminal.app, Alacritty, WezTerm.
- **Emulation correctness** — lean on `vt100`; grow the test matrix incrementally.
- **Overlay artifacts** — treat the crude overlay as a throwaway prototype; move to the parsed screen buffer quickly.
