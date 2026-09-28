# claude-siesta v1: park idle Claude Code sessions in agterm

## Overview

The MBP runs one interactive Claude Code TUI per agterm session and none of them ever exit. Measured 2026-09-27: 40 claude processes, 7.8 GB RSS in total, swap 11.1 of 12 GB, and 29 of the 32 sessions running a live claude had been idle for more than 24 hours. An agterm restart makes it worse: every session's restore line is `claude --resume <conv>`, so all of them start at once.

A Claude Code conversation is fully persisted in its transcript, and `claude --resume <conv>` brings it back at the same point. claude-siesta uses that:

- `claude-siesta daemon` (launchd agent) polls agterm every 10 minutes and **parks** a session that has been idle for `park_after` (default 2 h): it kills that session's claude, pins the session's restore line to `claude-siesta`, and types ` claude-siesta` into the pane.
- `claude-siesta` with no arguments is the **placeholder**: a small full-screen TUI in the pane showing the session, how long it has been idle and the last assistant message. Enter, Space, a mouse click or SIGUSR1 resumes: the placeholder `exec`s `claude --resume <conv>` in place.
- Because the restore line is pinned to `claude-siesta`, a parked session comes back as a placeholder after an agterm restart instead of a live claude.

Acceptance scenarios (Pavel's real workflow):

1. Return after a day: a session sat idle for 24 h. Its claude is gone from `ps`, the pane shows the placeholder, Enter continues the conversation exactly where it was.
2. agterm restart: parked sessions come back as placeholders; a session idle for less than 2 h at quit comes back live and the daemon parks it once it crosses the threshold.

### Non-goals (v1)

- No sidebar marker of any kind: the daemon never calls `session status`, never sets a color, background or context. The placeholder in the pane is the marker. (A `completed` status would make agterm's attention navigation walk every parked row.)
- No second "pin only" threshold (`pin_after` from the brief was rejected): one threshold, `park_after`.
- No changes outside this repo. The dotfiles follow-ups (`reopen-cc.fish` learning the placeholder, `ws-unshelve` bringing sessions back parked, an agterm palette entry for `park`) are a separate later change.
- No tmux side, no second Mac, no split pane or scratch terminal (only the main pane is ever looked at), no notifications, no release/brew packaging.
- No SIGSTOP/freeze, no PTY wrapper around a live claude, no screen snapshot.

### Rejected alternatives

- Calling `cc-park.fish` to kill: it paints the row `completed`, which is exactly the marker we do not want, and its cwd fallback is unnecessary (every live claude in cc-map has a recorded pid, verified). Kill natively.
- `session new --command claude-siesta` instead of typing: agterm 0.32.0 has no primitive to replace the process of an existing pane; creating a new session loses the session id, its cc-map entry and its sidebar name. Typing into the pane (guarded) is what `reopen-cc`, `ws-unshelve` and `fork-cc` already do.
- A daemon-only design with a bare fish prompt as the "parked" state: rejected by Pavel, the placeholder with a Play affordance is the point.
- Idle by transcript file mtime: Claude appends bookkeeping records with no dialogue behind them, so mtime lies. Idle is the timestamp of the last `"type":"assistant"` record.

## Skills to invoke

Load each skill below with the Skill tool and follow its conventions before implementing any task in this plan.

- `rust-style` - every file under `src/` follows it (for loops over iterator chains, `let ... else`, no comments, explicit `match` with no wildcards, no `matches!`, explicit destructuring, newtypes and enums over strings and bools)
- `agterm` - the `agtermctl` command surface, `tree --json` fields and the addressing rules used by `src/agterm.rs`

## Context (from discovery)

- New empty repo (`git init`, branch `master`). Sibling Rust projects `moji` and `nikki` set the conventions: `mise.toml` with a pinned Rust and `rustfmt,clippy` components, edition 2024, mise tasks `build`/`test`/`lint`/`fmt`/`check`, tests inline in `#[cfg(test)] mod tests`, a `CLAUDE.md` with code rules only and a `README.md` with what the tool is.
- Launchd labels of Pavel's own tools are `dev.pkarpovich.<name>` (`dev.pkarpovich.mimi`, `dev.pkarpovich.nikki`, `dev.pkarpovich.moji`). This one is `dev.pkarpovich.claude-siesta`.
- Environment: macOS 27, Apple Silicon, fish 4.9 login shell, agterm 0.32.0, Claude Code at `$HOME/.local/bin/claude`.

### External contracts (the source of truth for this plan, all verified live 2026-09-27)

**agtermctl** lives at `/Applications/agterm.app/Contents/MacOS/agtermctl`. launchd's PATH has no Homebrew, so the daemon always uses that absolute path. Every call prints JSON with `--json`: `{"ok": true, "result": ...}`.

- `agtermctl window list --json` -> `result.windows[]` with `id`, `name`, `open` (bool). Only `open` windows are walked.
- `agtermctl tree --json --window <window-id>` -> `result.tree.workspaces[].sessions[]`. `tree` without `--window` shows only the frontmost window, so always pass it. Session fields used:
  - `id` (uppercase UUID string), `name`, `active` (bool: selected in its window), `flagged` (bool)
  - `foreground` (array of argv strings of the main pane's foreground process; ABSENT when the pane is at a shell prompt), e.g. `["/Users/x/.local/bin/claude", "--enable-auto-mode", "--resume", "c0ac..."]`
  - `status` (`active` | `completed` | `blocked`; ABSENT when idle)
  - `restoreCommand` (string, optional)
  - all other fields are ignored; deserialization must tolerate unknown and missing fields
- `agtermctl session restore "claude-siesta" --target <id> --window <window-id>` pins the pane's restore line. The line is TYPED into a login fish on the next launch (agterm Re-run mode), so a bare `claude-siesta` resolves through the interactive fish PATH.
- `agtermctl session type --stdin --target <id> --window <window-id>` types stdin into the pane, Enter included. Never pass `--select` (it would steal the user's selection).

**cc-map** is written by Pavel's Claude Code hooks: one JSON file per agterm session at `$HOME/.local/state/agterm/cc-map/<AGTERM_SESSION_ID>`:
```json
{"conv":"b1274f92-...","profile":"personal","cwd":"/Users/x/Projects/y","ts":1790419180,"pid":68237}
```
`conv` conversation id, `profile` `personal` | `work`, `ts` unix seconds of the last hook write, `pid` the agterm-side claude (may be absent or null on old entries). Extra keys (`tsession`, `twindow`) may be present and are ignored. The file name is the uppercase session UUID. claude-siesta only reads it, never writes or deletes it (a kill does not trigger the hook that removes entries, so the entry survives parking by design).

**Transcripts**: `<root>/projects/*/<conv>.jsonl` where `<root>` is `$HOME/.claude` for `personal` and `$HOME/.claude-work` for `work`. Glob the project dir, do not compute the slug. One JSON record per line; files reach 90 MB. An assistant record: `{"type":"assistant","timestamp":"2026-08-11T22:41:01.578Z","message":{"content":[{"type":"text","text":"..."},{"type":"tool_use",...}]}}`. `content` may also be a plain string.

**Resume command**: `CLAUDE_CODE_NO_FLICKER=1 claude --enable-auto-mode --resume <conv>`, plus `CLAUDE_CONFIG_DIR=$HOME/.claude-work` when the profile is `work`. Pavel's fish wrapper `claude` passes `--resume` straight through to the binary, so exec'ing the binary found on the pane's PATH is equivalent. After resume the SessionStart hook rewrites the cc-map pid and re-pins the restore line to `claude --resume <conv>` by itself.

## Development Approach

- **Testing approach**: TDD for the pure modules (transcript tail, idle, park rule, cc-map, tree parsing, config): write the failing test first. The IO shells (`agterm` process calls, signals, the TUI loop) are covered by the spike and the manual checks in Post-Completion.
- Complete each task fully before moving to the next; small focused changes.
- **CRITICAL: every task MUST include new/updated tests** for the code it changes, success and error cases, listed as separate checklist items.
- **CRITICAL: all tests must pass before starting the next task** - no exceptions.
- **CRITICAL: update this plan file when scope changes during implementation.**
- The pure modules never spawn processes, read the clock or touch the filesystem outside what they are handed: they take bytes, strings, `SystemTime` and parsed structs, and return decisions. That is what makes them testable.
- Never commit or push; Pavel commits when he asks. ASCII hyphens only. No hard-wrapped markdown.

## Code-Quality Rules (verify before marking each task complete)

Rust (from the `rust-style` skill):

- Control flow: `for` loops with mutable accumulators, not iterator chains (`filter`/`map`/`collect`, `sum`, `find`).
- Early returns: `let ... else`; `if let` only for a short branch with no else.
- Shadow variables through transformations; no `raw_`/`parsed_`/`trimmed_` prefixes.
- No comments at all: no inline comments, section dividers, TODOs or commented-out code. Doc comments only where rustdoc on a public item says something the name does not.
- Newtypes over strings for ids (`SessionId`, `ConvId`, `WindowId`); enums over bool parameters.
- Never a wildcard `_` arm in a `match` over our own enums; never `matches!`; destructure structs explicitly.

Per-task gate (before marking any checkbox `[x]`):

1. `mise run check` passes: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
2. `grep -rn '//' src/ | grep -v '://'` finds no comments other than `///` doc comments; `grep -rn 'matches!' src/` finds nothing.
3. Only after 1-2 pass: mark complete.

## Solution Overview

One binary, one crate with a lib (`src/lib.rs`, every module public to the bin and to tests) and a bin (`src/main.rs`, argument dispatch only). Modules:

| Module | Kind | Responsibility |
|---|---|---|
| `paths` | pure | `$HOME`-relative locations: cc-map dir, transcript roots, state dir, config file, log file |
| `ccmap` | pure + read | parse a cc-map entry into `MapEntry` |
| `transcript` | pure + read | read the last 400 KB of a transcript, find the last assistant record: timestamp + text |
| `tree` | pure | deserialize `window list` / `tree` JSON into `Window`/`Session` |
| `rule` | pure | decide `Park` or `Skip(reason)` for one session |
| `config` | pure + read | load `config.toml` with defaults, parse durations |
| `state` | read/write | the per-session state file |
| `agterm` | IO | wrappers over `agtermctl` calls |
| `park` | IO | the park sequence (kill, wait, state, pin, type) |
| `daemon` | IO | the poll loop |
| `placeholder` | IO + TUI | the ratatui screen, input, signal, resume `exec` |
| `status` | IO | the `status` table |
| `log` | IO | append one line per decision |

No async runtime, no threads beyond what a blocking loop needs. The daemon is a `loop { tick(); sleep(poll_interval) }`; the placeholder is a crossterm event loop with a timeout so it can redraw the idle clock and check the signal flag.

## Technical Details

### CLI

```
claude-siesta                      placeholder (runs in an agterm pane)
claude-siesta daemon               poll loop (launchd)
claude-siesta park <id|prefix>     park one session now
claude-siesta resume <id|prefix>   SIGUSR1 the session's placeholder
claude-siesta status               table of mapped sessions
```

Hand-written argument dispatch on `std::env::args()` (no clap: five subcommands, one positional). Unknown subcommand or a missing argument prints a usage line to stderr, exit 2.

`<id|prefix>` resolution: case-insensitive prefix match against session ids across all open windows' trees. Zero matches or more than one match is an error naming the candidates, exit 1.

### Config

`$HOME/.config/claude-siesta/config.toml`, optional, both keys optional:

```toml
park_after = "2h"
poll_interval = "10m"
```

Durations: an integer followed by `s`, `m`, `h` or `d`. An invalid file or value is an error at daemon start (exit 2, message names the key), not a silent default. Unknown keys are rejected (`deny_unknown_fields`).

### State file

`$HOME/.local/state/claude-siesta/<SESSION-ID>.json`, written atomically (temp file in the same dir + rename):

```json
{"session":"7320056A-...","conv":"c0acdbe6-...","profile":"personal","parked_at":1790500000,"last_assistant_at":1790400000,"placeholder_pid":null}
```

Written by `park`; the placeholder sets `placeholder_pid` to its own pid on start; the placeholder deletes it right before the resume `exec`; the daemon deletes files whose session id is in none of the open windows' trees. A missing or unreadable state file is never fatal to the placeholder.

### Log

`$HOME/.local/state/claude-siesta/claude-siesta.log`, append-only, one line per decision: RFC 3339 local time, action (`start`, `park`, `skip`, `park-failed`, `cleanup`, `tick-failed`), session id, conv id, idle in minutes, reason. Never any transcript text, never the cwd. `skip` lines are logged only for sessions whose foreground is claude (so the log is not 30 lines of "shell prompt" every tick). No rotation in v1.

### Park rule (`rule::decide`)

Input: a `Session` (from the tree), an `Option<MapEntry>`, whether the recorded pid is a live claude (bool resolved by the caller), the idle `Duration`, the `park_after` threshold, and a `Mode` enum (`Daemon` | `Manual`). Output: `Decision::Park` or `Decision::Skip(SkipReason)`. Checks, in this order, first failure wins:

1. `foreground` present and `foreground[0]` ends with `/claude` -> else `NotClaude`
2. map entry present with `conv` and `pid` -> else `NotMapped`
3. pid is a live claude -> else `PidNotClaude`
4. `status` is not `active` -> else `AgentWorking`
5. `flagged` is false -> else `Flagged`
6. `active` (selected) is false -> else `Selected` (Daemon mode only; Manual skips this check because Pavel may park the session he is looking at)
7. idle >= `park_after` -> else `NotIdleEnough` (Daemon mode only)

"pid is a live claude" is decided in `park`/`daemon` by `kill(pid, 0)` succeeding and the process's command name (`ps -o comm= -p <pid>`) ending with `claude`. This guards against a recycled pid.

### Idle (`transcript`)

Open the transcript, seek to `max(0, len - 400 KiB)`, read to the end, split on `\n`, drop the first (possibly partial) line when the seek offset was not 0, and scan every line: a line is a candidate when it parses as JSON with `"type": "assistant"` and a parsable RFC 3339 `timestamp`. Keep the last candidate. Its text is the concatenation of `message.content[].text` for items with `"type": "text"` (joined with a blank line), or `message.content` itself when that is a string. A record whose text is empty (tool calls only) still counts for the timestamp but the excerpt keeps the text of the last record that had text.

Idle = `now - last_assistant_at`; when no assistant record exists in the tail (or the transcript is not found), `now - MapEntry.ts`. A timestamp in the future yields zero idle, never a panic. The 400 KiB tail is a constant; parse RFC 3339 by hand into `SystemTime` (UTC `Z` suffix and numeric offsets both occur in the wild; support both) - no chrono.

### Park sequence (`park::run`)

1. Decide with `rule::decide`; a `Skip` returns without side effects.
2. `kill(pid, SIGTERM)`; poll `kill(pid, 0)` every 250 ms for up to 5 s; still alive -> `kill(pid, SIGKILL)`.
3. Poll `tree --window <w>` every 250 ms, up to 20 times, until the session's `foreground` is absent. Timeout -> log `park-failed` (reason `foreground-busy`) and stop: nothing is typed, no pin, no state file.
4. Write the state file.
5. `session restore "claude-siesta" --target <id> --window <w>`.
6. `session type --stdin --target <id> --window <w>` with stdin ` claude-siesta\n` (leading space keeps it out of fish history).
7. Log `park`.

Any agtermctl failure after the kill logs `park-failed` with the step name and the agtermctl stderr (first line). The daemon continues with the next session; the next tick sees the pane at a shell prompt (`NotClaude`) and leaves it alone, which is safe.

### Placeholder

- Start: read `AGTERM_SESSION_ID`; missing -> print `claude-siesta: not inside an agterm session` to stderr, exit 1. Read the cc-map entry; missing -> print `claude-siesta: no Claude conversation is mapped to this session`, exit 1.
- Data: session name from the trees (`window list` + `tree --window`, find by id; fall back to the cwd basename when agtermctl fails), `parked_at` from the state file when present, last assistant timestamp + text from the transcript.
- Screen (ratatui, English only), one centered bordered block that fits the pane:
  - title line: `▶  <session name>`
  - `<cwd, $HOME shortened to ~>`
  - `conv <first 8 chars>   idle 26h 12m   parked 2026-09-27 12:40` (omit `parked` without a state file)
  - a blank line, then the last assistant message: first 10 wrapped lines, in whatever language it is, plain text (no markdown rendering)
  - footer: `Enter / Space / click  resume      q  shell`
  - if the pane is too small for the block, show only the title line and the footer
- Event loop: `crossterm::event::poll` with a 1 s timeout; redraw the idle value each minute and on resize. Enter, Space, or any mouse button-down event -> resume. `q` or `Esc` -> clean exit 0. A SIGUSR1 sets an atomic flag (installed with `nix::sys::signal::sigaction`; the handler only stores the flag), checked every loop iteration -> resume.
- Enable raw mode, the alternate screen and mouse capture on start; on every exit path (resume, `q`, error, panic hook) disable mouse capture, leave the alternate screen and raw mode. A leaked mouse-capture mode would make the next claude receive garbage escape sequences.
- Resume: restore the terminal as above, delete the state file, then `exec` (`std::os::unix::process::CommandExt::exec`) `claude --enable-auto-mode --resume <conv>` with `CLAUDE_CODE_NO_FLICKER=1` and, for profile `work`, `CLAUDE_CONFIG_DIR=$HOME/.claude-work` added to the inherited environment. `claude` is resolved from the inherited PATH. If `exec` returns (binary not found), print the error and exit 1, leaving the pane at the fish prompt.

### Daemon

- Start: load config (fatal on error), create the state dir, log a `start` line with the thresholds.
- Tick: `window list`; for each open window `tree --window`; for each session in each workspace, gather inputs and call `park::run` in `Mode::Daemon`. A failed agtermctl call for the whole tick logs one line and the tick ends (agterm may not be running; that is normal at login). Then delete state files for session ids not seen in any tree.
- Sessions are parked one at a time, sequentially.
- SIGTERM/SIGINT: exit cleanly between ticks (the sleep is interruptible via a flag checked every second).

### Status

One row per session in any open window that has a cc-map entry: session name (truncated to 24 chars), conv (8 chars), idle (`26h 12m`), state (`live` when foreground is claude, `parked` when foreground ends with `claude-siesta` or a state file exists, else `shell`), and the last log action for that session if any. Plain aligned text columns, no color.

### launchd

`launchd/dev.pkarpovich.claude-siesta.plist`: `Label` `dev.pkarpovich.claude-siesta`, `ProgramArguments` = [`$HOME/.local/bin/claude-siesta`, `daemon`] (the install task substitutes the real home path), `RunAtLoad` true, `KeepAlive` true, `ThrottleInterval` 30, `StandardErrorPath` `$HOME/Library/Logs/claude-siesta.err.log`. mise tasks `install` (cargo build --release, copy the binary to `$HOME/.local/bin/claude-siesta`, render the plist into `$HOME/Library/LaunchAgents/`, `launchctl bootout` if loaded then `launchctl bootstrap gui/$(id -u)`) and `uninstall` (bootout, remove the plist; keep the binary and state).

## What Goes Where

- **Implementation Steps** (`[ ]` checkboxes): code, tests, repo files and docs in this repo.
- **Post-Completion** (no checkboxes): installing the agent on Pavel's Mac, the live acceptance scenarios, and the dotfiles follow-ups.

## Implementation Steps

### Task 1: Scaffold the crate and the toolchain

**Files:**
- Create: `mise.toml`
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/main.rs`
- Create: `src/cli.rs`
- Create: `.gitignore`

- [x] `mise.toml`: `rust = { version = "1.98.1", components = "rustfmt,clippy" }`, tasks `build`, `test`, `lint` (`cargo clippy --all-targets -- -D warnings`), `fmt`, `check` (fmt check, clippy, test) in that order
- [x] `Cargo.toml`: package `claude-siesta`, edition 2024, `rust-version = "1.98"`, dependencies `ratatui = "0.30.2"`, `crossterm = "0.29"`, `serde = { version = "1", features = ["derive"] }`, `serde_json = "1"`, `toml = "1.1"`, `nix = { version = "0.31", features = ["signal", "process"] }`; `[profile.release]` with `strip = true`, `lto = true`, `opt-level = "s"` for a small binary
- [x] `src/main.rs` with the argument dispatch from Technical Details / CLI, every subcommand a stub returning exit 1 with "not implemented"; `.gitignore` with `/target`
- [x] write a test for argument dispatch (a pure `parse_args(&[String]) -> Result<Command, UsageError>` in the lib, `src/cli.rs`; an extra argument is also a usage error): each subcommand, a missing `<id|prefix>`, an unknown subcommand
- [x] `mise run check` passes

### Task 2: Spike the three unverified runtime behaviours inside agterm

Nothing below has been run inside agterm yet; the rest of the placeholder design depends on it. This task produces a throwaway `examples/spike.rs` and a written result, not production code.

**Files:**
- Create: `examples/spike.rs`
- Create: `docs/spike-results.md`

- [x] `examples/spike.rs`: a minimal ratatui screen with raw mode, alternate screen and mouse capture; prints the last event it received; SIGUSR1 handler setting an atomic flag; on Enter, SIGUSR1 or a click it restores the terminal and `exec`s a program given as argv (e.g. `claude --resume <conv>` of a throwaway conversation, or `bash -c 'echo exec-ok; sleep 5'`)
- [x] run it in an agterm pane and record in `docs/spike-results.md`: does a mouse click arrive as a crossterm mouse down event; does `kill -USR1 <pid>` from another shell resume; after `exec` into claude, is the terminal clean (no mouse escape garbage, no alternate-screen leftovers) and does agterm's `tree --json` report the new `foreground` argv; does the pane return to the fish prompt when the exec'd program exits (run in a throwaway background session driven by agtermctl; the mouse check used an SGR click injected with `session type`, and a physical GUI click is left to the Post-Completion manual check)
- [x] measure the spike's RSS with `ps -o rss= -p <pid>` and record it (2944 KB, release build)
- [x] if any of the four behaviours fails, add a ⚠️ note here with the observed behaviour and adjust Task 11 before continuing (none failed; a claude exec'd by bare name still reports an absolute `foreground[0]`, so rule check 1 holds)
- [x] `mise run check` passes (the example must build and lint clean)

### Task 3: Paths, config and duration parsing

**Files:**
- Create: `src/paths.rs`
- Create: `src/config.rs`
- Create: `src/ccmap.rs` (only `SessionId` and `Profile`, which `Paths` needs; Task 4 adds the rest)
- Modify: `src/lib.rs`

- [x] `src/paths.rs`: a `Paths` struct built from a home directory (`Paths::from_home(PathBuf)`, plus `Paths::from_env()` reading `HOME`) exposing the cc-map dir, the transcript root for a `Profile`, the state dir, the state file for a `SessionId`, the log file and the config file, all from the External contracts / Technical Details sections
- [x] `src/config.rs`: `Config { park_after: Duration, poll_interval: Duration }` with defaults 2 h / 10 m; `Config::parse(&str) -> Result<Config, ConfigError>` via `toml` + serde with `deny_unknown_fields`; `Config::load(&Paths)` returns defaults when the file is absent and an error when it is invalid; a `parse_duration(&str)` for `<int><s|m|h|d>`
- [x] write tests: defaults for an empty file, both keys set, each unit, invalid unit, zero, negative/garbage, unknown key rejected
- [x] write tests for `Paths` with a fake home: every path is under it; `work` maps to `.claude-work`
- [x] `mise run check` passes

### Task 4: cc-map entries

**Files:**
- Modify: `src/ccmap.rs` (Task 3 created it with `SessionId` and `Profile`)
- Modify: `src/lib.rs`

- [x] newtypes `SessionId` (normalized to uppercase), `ConvId`; enum `Profile { Personal, Work }` (unknown profile string -> `Personal`)
- [x] `MapEntry { conv: ConvId, profile: Profile, cwd: PathBuf, ts: SystemTime, pid: Option<i32> }`; `MapEntry::parse(&str) -> Result<MapEntry, CcMapError>`, tolerating unknown keys, a null or missing `pid`, a missing `cwd` (empty path); a missing `conv` is an error (an empty `conv` or a missing `ts` is also an error; a missing `profile` is `Personal`)
- [x] `MapEntry::load(&Paths, &SessionId) -> Result<Option<MapEntry>, CcMapError>` (missing file = `Ok(None)`)
- [x] write tests with fixtures shaped exactly like the External contracts example: full entry, `pid: null`, no `pid`, extra `tsession`/`twindow`, `profile: work`, missing `conv` (error), malformed JSON (error)
- [x] `mise run check` passes

### Task 5: Transcript tail and idle

**Files:**
- Create: `src/transcript.rs`
- Create: `src/time.rs`
- Modify: `src/lib.rs`

- [x] `src/time.rs`: `parse_rfc3339(&str) -> Option<SystemTime>` supporting fractional seconds, `Z` and `+hh:mm`/`-hh:mm`; `format_local_minute(SystemTime) -> String` (`2026-09-27 12:40`, local time via `localtime_r` from `nix::libc`, which `nix` re-exports - no separate `libc` dependency); `format_idle(Duration) -> String` (`45m`, `3h 12m`, `26h 12m`)
- [x] `src/transcript.rs`: `LastAssistant { at: SystemTime, text: String }`; `last_assistant(bytes: &[u8], start: TailStart) -> Option<LastAssistant>` (`TailStart { FileStart, MidFile }` instead of a bool, per the enum-over-bool rule) implementing the Technical Details / Idle rules (partial first line dropped, text from the last record that had text, timestamp from the last assistant record); `find_transcript(&Paths, Profile, &ConvId) -> Option<PathBuf>` globbing `projects/*/<conv>.jsonl` with `std::fs::read_dir`; `read_tail(&Path) -> io::Result<(Vec<u8>, TailStart)>` reading at most 400 KiB
- [x] `idle(now, last: Option<&LastAssistant>, map: &MapEntry) -> Duration` with the cc-map `ts` fallback and future-timestamp clamp
- [x] write tests: content as array with text and tool_use items, content as string, last record tool-only (text from the earlier one), no assistant records, a partial first line when mid-file, malformed lines interleaved, `+02:00` offset timestamp, future timestamp -> zero idle, `find_transcript` in a temp dir for both profiles
- [x] `mise run check` passes

### Task 6: agterm tree model and parsing

**Files:**
- Create: `src/tree.rs`
- Create: `tests/fixtures/window-list.json`
- Create: `tests/fixtures/tree.json`
- Modify: `src/lib.rs`

- [x] `WindowId` newtype; `Window { id, open }`; `Session { id: SessionId, name, active, flagged, foreground: Vec<String>, status: AgentStatus, restore_command: Option<String> }` with `AgentStatus { Idle, Active, Completed, Blocked }` (absent -> `Idle`, unknown string -> `Idle`); serde with defaults so unknown and missing fields never fail
- [x] `parse_windows(&str) -> Result<Vec<Window>, TreeError>` and `parse_tree(&str) -> Result<Vec<Session>, TreeError>` flattening `result.tree.workspaces[].sessions[]`; `ok: false` in the response is an error carrying the response's message
- [x] `Session::runs_claude()` (`foreground[0]` ends with `/claude`) and `Session::runs_placeholder()` (ends with `claude-siesta`)
- [x] fixtures: a trimmed real `window list` response and a `tree` response with one live claude session, one at a shell prompt (no `foreground`), one flagged, one selected, one `status: active`, one running `claude-siesta`, one with `foreground` of `vim`, plus an extra unknown field on each
- [x] write tests over the fixtures: count, each field, `runs_claude`/`runs_placeholder`, `ok: false` error, malformed JSON error
- [x] `mise run check` passes

### Task 7: Park rule

**Files:**
- Create: `src/rule.rs`
- Modify: `src/lib.rs`

- [x] `Mode { Daemon, Manual }`, `Decision { Park, Skip(SkipReason) }`, `SkipReason { NotClaude, NotMapped, PidNotClaude, AgentWorking, Flagged, Selected, NotIdleEnough }`
- [x] `RuleInput { session: &Session, entry: Option<&MapEntry>, pid_is_claude: bool, idle: Duration, park_after: Duration, mode: Mode }` and `decide(&RuleInput) -> Decision` in the exact order of Technical Details / Park rule
- [x] write a table test: one row per `SkipReason` (each failing exactly that check), the all-pass `Park` row, Manual mode parking a selected session and a session idle 1 minute, Manual mode still refusing flagged / agent working / not claude / not mapped, idle exactly equal to `park_after` parks
- [x] `mise run check` passes

### Task 8: State file and log

**Files:**
- Create: `src/state.rs`
- Create: `src/log.rs`
- Modify: `src/lib.rs`

- [x] `src/state.rs`: `ParkState` with the fields of Technical Details / State file (serde); `write(&Paths, &ParkState)` atomic via temp file + rename in the state dir (create the dir if missing); `read(&Paths, &SessionId) -> Option<ParkState>` (unreadable = `None`); `set_placeholder_pid`; `remove`; `list(&Paths) -> Vec<ParkState>`; `cleanup(&Paths, seen: &[SessionId]) -> Vec<SessionId>` removing files for unseen ids
- [x] `src/log.rs`: `Action { Start, Park, Skip, ParkFailed, Cleanup }` and `append(&Paths, LogLine)` writing one line per the Technical Details / Log format; a pure `format_line(&LogLine, SystemTime) -> String` used by `append`; `last_action(&Paths, &SessionId) -> Option<String>` for `status` (reads the log's last 64 KiB)
- [x] write tests in a temp dir: write/read round trip, overwrite, corrupt file reads as `None`, `set_placeholder_pid` keeps other fields, `cleanup` removes only unseen ids, `list`; `format_line` never includes anything but the documented fields; `last_action` picks the newest line for the id
- [x] `mise run check` passes

### Task 9: agtermctl wrapper and the park sequence

**Files:**
- Create: `src/agterm.rs`
- Create: `src/park.rs`
- Modify: `src/lib.rs`

- [x] `src/agterm.rs`: `Agterm { bin: PathBuf }` with `Agterm::default()` using `/Applications/agterm.app/Contents/MacOS/agtermctl`; methods `windows()`, `tree(&WindowId)`, `restore(&WindowId, &SessionId, &str)`, `type_text(&WindowId, &SessionId, &str)` (stdin, no `--select`); every call passes `--json`, checks the exit status and `ok`, and returns the stderr/message on failure; `all_sessions() -> Result<Vec<(WindowId, Session)>, AgtermError>` over open windows; `resolve_prefix(&str)` per Technical Details / CLI
- [x] `src/park.rs`: a `ParkEnv` struct holding `&Agterm`, `&Paths`, `&Config`, `now` (keeps signatures under the parameter budget); `pid_is_claude(i32) -> bool` via `nix::sys::signal::kill(pid, None)` + `ps -o comm=`; `terminate(i32)` (SIGTERM, 5 s poll, SIGKILL); `run(&ParkEnv, &WindowId, &Session, Mode) -> ParkOutcome` implementing the seven steps of Technical Details / Park sequence and logging each outcome
- [x] make the `tree` re-poll and the kill poll take their interval and attempt counts from constants, and structure `run` so the decision (`rule::decide`) and the step ordering are exercised through a test double: a small trait `AgtermOps` implemented by `Agterm` and by a recording fake in tests (plus a `ProcessOps` trait for `is_claude`/`terminate` so the fake never signals real pids; `ParkEnv` also carries the foreground `Poll` so tests run it with a zero interval; `all_sessions`/`resolve_prefix` are provided methods on `AgtermOps`)
- [x] write tests with the fake: a `Skip` makes no calls; the happy path calls restore then type in that order with the exact arguments (` claude-siesta\n`, no `--select`); a foreground that never clears makes no restore/type call and no state file; a restore failure logs `park-failed` and does not type
- [x] write a test for `terminate` against a real child (`sleep 60` spawned by the test): it is gone afterwards; a child that ignores SIGTERM (`bash -c 'trap "" TERM; sleep 60'`) is SIGKILLed
- [x] `mise run check` passes

### Task 10: Daemon loop and the park subcommand

**Files:**
- Create: `src/daemon.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`

- [x] `src/daemon.rs`: `run(&Paths, Config) -> ExitCode` per Technical Details / Daemon: start log line, tick, cleanup of state files, interruptible sleep; SIGTERM/SIGINT set an atomic flag checked every second
- [x] `tick(&ParkEnv) -> TickReport` (parked / skipped / failed counts, plus the agterm error when the tick ended early) separate from the loop so it is callable from a test with the fake `AgtermOps`; the one log line for a failed tick uses a new `Action::TickFailed` (`tick-failed`)
- [x] wire `claude-siesta daemon` and `claude-siesta park <id|prefix>` (Manual mode, prints the outcome, exit 0 on park, 1 otherwise) in `src/main.rs`
- [x] write tests for `tick` with the fake: two windows, one parkable session and several skip cases -> exactly one park, state files of vanished sessions removed, an agterm failure ends the tick without panicking
- [x] `mise run check` passes

### Task 11: Placeholder TUI

**Files:**
- Create: `src/placeholder.rs`
- Create: `src/placeholder/view.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`

- [x] `src/placeholder/view.rs`: a pure `render(frame, &ViewModel)` and `ViewModel { name, cwd_display, conv_short, idle, parked_at: Option<String>, excerpt: String }` per Technical Details / Placeholder / Screen, including the small-pane fallback; `excerpt_lines(text, width, max_lines)` wrapping on char boundaries (multi-byte safe) (`excerpt` holds the raw text and `render` wraps it to the block's inner width, since the width is only known at draw time; a pane too short for 10 excerpt lines shows fewer before falling back to title + footer)
- [x] `src/placeholder.rs`: start checks and messages, data gathering, terminal setup and a guard that restores the terminal on drop and in a panic hook, the event loop (1 s poll, minute redraw, resize), SIGUSR1 flag, key and mouse handling, `q`/`Esc` exit, the resume `exec` with the environment from Technical Details / Placeholder / Resume; write `placeholder_pid` into the state file on start (the exec also runs in the cc-map `cwd` when it is an existing dir, so `--resume` finds the project; the placeholder does not load `config.toml`, so a broken config never blocks a resume)
- [x] apply whatever `docs/spike-results.md` found (Task 2) (restore order `DisableMouseCapture`, `LeaveAlternateScreen`, then raw mode off, plus a cursor `Show`)
- [x] write tests: `render` into a ratatui `TestBackend` for a normal and a tiny pane (assert the title, idle and footer text are present, the excerpt is cut at 10 lines), `excerpt_lines` with Cyrillic text and long words, a pure `resume_command(&MapEntry, &Paths) -> (program, args, env)` for both profiles, key/mouse -> action mapping as a pure function
- [x] `mise run check` passes

### Task 12: resume and status subcommands

**Files:**
- Create: `src/status.rs`
- Create: `src/resume.rs`
- Modify: `src/main.rs`
- Modify: `src/lib.rs`
- Modify: `src/park.rs`, `src/transcript.rs`, `src/placeholder.rs` (shared helpers)

- [x] `claude-siesta resume <id|prefix>`: resolve the session, read its state file, SIGUSR1 the `placeholder_pid` after checking it is alive and its command ends with `claude-siesta`; clear errors for no state file / no pid / dead pid; exit codes 0/1 (`resume::signal_placeholder` in `src/resume.rs`, a live pid running something else is a fourth error; `park::process_command` is split out of `pid_is_claude` for the `ps -o comm=` check)
- [x] `src/status.rs`: gather rows per Technical Details / Status and a pure `format_table(&[StatusRow]) -> String` with aligned columns (`gather` takes `&dyn AgtermOps`; `transcript::load_last` replaces the identical private helpers in `park` and `placeholder`)
- [x] write tests for `format_table` (alignment, truncation of long names, empty input prints a header only) and for the state column classification (plus `gather` with a fake agterm, and every `signal_placeholder` error path and a real SIGUSR1 delivery against spawned children)
- [x] `mise run check` passes

### Task 13: launchd agent and install tasks

**Files:**
- Create: `launchd/dev.pkarpovich.claude-siesta.plist`
- Modify: `mise.toml`

- [x] plist template per Technical Details / launchd with a `__HOME__` placeholder
- [x] mise tasks `install` and `uninstall` per Technical Details / launchd; `install` is idempotent (bootout of a loaded agent first) (the binary is copied to a temp name and renamed into place so running placeholders keep their inode; install/uninstall are not run here, that is Post-Completion)
- [x] validate the template with `plutil -lint` after substitution into a temp path inside `target/` (add this as part of the `check` task) (a `lint-plist` task, called as the last step of `check`; a broken template fails it)
- [x] `mise run check` passes

### Task 14: Verify acceptance criteria

- [x] every requirement in Overview and Technical Details is implemented; every non-goal is still a non-goal (no `session status`, `background`, `context` or `flag` call anywhere: `grep -rn '"status"\|"background"\|"context"\|"flag"' src/agterm.rs` finds none)
- [x] no path in `src/` is hardcoded to a user home (`grep -rn '/Users/' src/` finds nothing) (the test fixtures used `/Users/x`; they now use `/home/x`, including `tests/fixtures/tree.json`)
- [x] `mise run check` passes; `cargo build --release` passes and the release binary is under 5 MB (release binary 819616 bytes)
- [x] every `SkipReason` and every placeholder exit path is covered by a test (`resume` now takes the `ResumeCommand` so a test can exec a missing binary in a re-executed child test process: the state file is removed and the exit code is 1; a raised SIGUSR1 sets the resume flag; a terminal I/O error and a `sigaction` failure map straight to exit 1 and are not unit-testable)

### Task 15: [Final] Documentation

**Files:**
- Create: `README.md`
- Create: `CLAUDE.md`

- [ ] `README.md`: what claude-siesta does and why (the memory numbers), the commands, the config keys, the files it reads and writes, install/uninstall, how to resume (Enter/Space/click/`claude-siesta resume`), and the dependency on Pavel's cc-map hooks; one line per paragraph, no hard wraps
- [ ] `CLAUDE.md` in the moji/nikki style: code rules only (pure modules vs IO shells, tests inline, no async, no sidebar marker and why, the agtermctl absolute path and `--window` rule, never `--select`, terminal-restore guard on every exit path); point to this plan for the decisions
- [ ] move this plan to `docs/plans/completed/`

## Post-Completion

*Items requiring manual intervention or external systems - no checkboxes, informational only*

**Install and live checks** (Pavel, on the MBP):

- `mise run install`; `launchctl print gui/$(id -u)/dev.pkarpovich.claude-siesta` shows it running; `claude-siesta status` lists every mapped session, spot-check two idle values against their transcripts.
- Throwaway session with `park_after = "1m"`: the pane switches to the placeholder, `ps` no longer shows that claude, the sidebar row is untouched. Enter resumes with the prior messages there; repeat with Space, with a click, and with `claude-siesta resume <id>` from another pane.
- Flag a session and let it idle past the threshold: never parked. A session with `status active` (a long tool run) is never parked.
- Restore `park_after = "2h"`; after the first daemon tick, total claude RSS drops from ~7.8 GB to the few live sessions and swap usage falls.
- Quit and relaunch agterm with several parked sessions: they come back as placeholders; a session idle under 2 h comes back live and is parked by the daemon once it crosses 2 h.
- Scenario "return after a day": leave a session overnight, open it the next day, press Enter, continue.

**Follow-ups outside this repo (the stack, separate change):**

- `dotfiles/agterm/scripts/reopen-cc.fish`: when `foreground[0]` ends with `claude-siesta`, call `claude-siesta resume <id>` instead of treating the session as busy.
- `ws-unshelve.fish`: optionally bring shelved sessions back as `claude-siesta` instead of a live claude.
- An agterm palette entry for `claude-siesta park <active session>`.
