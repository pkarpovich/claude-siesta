# claude-siesta conventions

`README.md` carries what claude-siesta is, its commands, configuration and files; this file carries only the rules that govern how code is written here. The plan that built it, with the verified agterm, cc-map and transcript contracts behind every decision, is `docs/plans/completed/20260928-claude-siesta-v1.md`, and the runtime behaviour of the placeholder inside agterm is measured in `docs/spike-results.md`.

## Pure modules decide, IO shells execute

`paths`, `cli`, `config`, `ccmap`, `time`, `transcript`, `tree`, `rule` and `placeholder/view` take bytes, strings, `SystemTime` and parsed structs, and return decisions or values. They never spawn a process, read the clock or touch the filesystem beyond what they are handed; a function that reads a file does only that and hands the bytes to a pure one. That split is what makes the park rule, the idle computation and the tree parsing testable with fixtures and a fixed `now`.

`agterm`, `park`, `daemon`, `placeholder`, `resume`, `status`, `state` and `log` do the IO. Anything they call on agterm or on another process goes through the `AgtermOps` and `ProcessOps` traits, so a test drives the park sequence and the daemon tick with a recording fake and never signals a real pid or talks to a real agterm. A new agtermctl call is a new method on `AgtermOps`, implemented by `Agterm` and by the fake.

## No async, no threads

The daemon is a blocking `loop { tick(); sleep }` and the placeholder is a crossterm event loop with a 1 s poll timeout. Sessions are parked one at a time. A signal handler only stores an atomic flag, installed with `sigaction`; the loop checks the flag. Nothing here needs a runtime or a second thread.

## No sidebar marker

The daemon never calls `session status`, never sets a color, background, context or flag. The placeholder in the pane is the only marker: a `completed` status on every parked row would make agterm's attention navigation walk all of them. `grep -rn '"status"\|"background"\|"context"\|"flag"' src/agterm.rs` must stay empty.

## agtermctl

- Always the absolute path `/Applications/agterm.app/Contents/MacOS/agtermctl` (`agterm::AGTERMCTL`). launchd's PATH has no Homebrew, so a bare `agtermctl` works in a terminal and fails in the daemon.
- Every call passes `--json` and checks both the exit status and `ok`.
- `tree` always gets `--window <id>`: without it agterm shows only the frontmost window, and every other window's sessions silently disappear. Only `open` windows from `window list` are walked.
- Never pass `--select` to `session type` or anything else: it would steal the user's selection.
- Deserialization of agterm's JSON tolerates unknown and missing fields; `foreground` and `status` are absent in the normal idle state.

## The terminal is restored on every exit path

The placeholder enables raw mode, the alternate screen and mouse capture. A leaked mouse-capture mode makes the claude exec'd next receive escape-sequence garbage, so every exit path restores the terminal: the `TerminalGuard` on drop, the panic hook, and explicitly before the resume `exec` (which never returns to run a destructor). The order is the one the spike measured clean: `DisableMouseCapture`, `LeaveAlternateScreen`, raw mode off, cursor `Show`.

The placeholder must never fail to resume because of something optional: a missing state file, an agtermctl failure (fall back to the cwd basename for the name) or a broken `config.toml` (it is never read) do not stop it.

## Tests live inline

Tests go in a `#[cfg(test)] mod tests` block in the file they cover. A sibling `foo_test.rs` is not compiled unless something declares it, so it would sit unbuilt while the gate reported success. Fixtures that mirror agterm's real responses are in `tests/fixtures/`, with fake homes under `/home/x`: no path in `src/` names a real user home.

A test that needs the filesystem uses its own temp dir as the home. A test that needs a process spawns its own child (`sleep`, `bash`) and signals only that.

## Declare every module

Every new module file is declared the moment it is created, in `lib.rs` or its parent. An undeclared module is not compiled, and neither are its inline tests. `main.rs` is argument dispatch only; everything else is a `pub` module of the lib.

## Per-task gate

```
mise run check
! grep -rn '//' src/ | grep -v '://' | grep -v '///'
! grep -rn 'matches!' src/
```

`mise run check` is `cargo fmt --all -- --check`, then `cargo clippy --all-targets -- -D warnings`, then `cargo test`, then `plutil -lint` over the rendered launchd plist.

## Style

Follow the `rust-style` skill:

- No comments; clear names instead. Doc comments (`///`) only where a public item needs more than its name.
- `for` loops with mutable accumulators, not iterator chains.
- `let ... else` for early returns; the main path stays flat.
- `match` covers every variant of our own enums explicitly; no `_ =>` wildcard and no `matches!`.
- Destructure structs and tuples explicitly.
- Newtypes over bare strings for identifiers (`SessionId`, `ConvId`, `WindowId`); enums over `bool` parameters (`Mode`, `TailStart`).
- The daemon must never panic: an agterm that is not running, a missing transcript or a malformed cc-map entry is ordinary input, logged and skipped.
- The log never carries transcript text or a cwd.
