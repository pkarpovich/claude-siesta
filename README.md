# claude-siesta

claude-siesta parks idle Claude Code sessions in agterm: it kills a claude that has been idle for hours and leaves a tiny placeholder in its pane that brings the conversation back with one key.

Every agterm session on the MBP runs an interactive Claude Code TUI, and none of them ever exit. Measured 2026-09-27: 40 claude processes, 7.8 GB RSS in total, swap at 11.1 of 12 GB, and 29 of the 32 sessions running a live claude had been idle for more than 24 hours. An agterm restart makes it worse, because every session's restore line is `claude --resume <conv>` and all of them start at once.

A Claude Code conversation is fully persisted in its transcript, and `claude --resume <conv>` brings it back at the same point. So an idle claude costs about 200 MB for nothing, while the placeholder that replaces it costs about 3 MB.

The full design, and the reasoning behind every decision, is in `docs/plans/completed/20260928-claude-siesta-v1.md`; the coding conventions are in `CLAUDE.md`.

## How it works

- `claude-siesta daemon` runs as a launchd agent and polls agterm every `poll_interval`. A session whose claude has been idle for `park_after` is **parked**: its claude is killed (SIGTERM, SIGKILL after 5 s), the session's restore line is pinned to `claude-siesta`, and ` claude-siesta` is typed into the pane once it is back at the fish prompt.
- `claude-siesta` with no arguments is the **placeholder**: a small full-screen view showing the session name, the working directory, the conversation id, how long it has been idle, when it was parked and the start of the last assistant message. Resuming `exec`s `claude --resume <conv>` in place, in the same pane and the same process.
- Because the restore line is pinned to `claude-siesta`, a parked session comes back as a placeholder after an agterm restart instead of a live claude. After a resume, Claude Code's own hooks re-pin the restore line to `claude --resume <conv>`.

Idle is measured from the timestamp of the last assistant message in the transcript, not from the file's mtime: Claude Code appends bookkeeping records with no dialogue behind them. When the transcript has no assistant message, the cc-map entry's `ts` is used.

A session is parked only when all of these hold, checked in this order: its main pane's foreground is a claude; it has a cc-map entry with a conversation and a pid; that pid is still a live claude (guards against a recycled pid); agterm does not report the agent as working (`status active`); the session is not flagged; it is not the selected session of its window; it has been idle for at least `park_after`. A manual `park` skips the last two checks.

There is deliberately no sidebar marker: the daemon never sets a status, color, background or context on a session. The placeholder in the pane is the marker, and a `completed` status would make agterm's attention navigation walk every parked row.

## Resuming

In the placeholder pane, press **Enter** or **Space**, or click anywhere in the pane. `q` or `Esc` leaves the placeholder for a plain fish prompt without resuming.

From any other shell, `claude-siesta resume <id|prefix>` sends SIGUSR1 to that session's placeholder, which resumes the same way.

## Commands

```
claude-siesta                      the placeholder (run inside an agterm pane)
claude-siesta daemon               the poll loop (what launchd runs)
claude-siesta park <id|prefix>     park one session now
claude-siesta resume <id|prefix>   resume one parked session from another shell
claude-siesta status               one row per mapped session: name, conv, idle, state, last log action
```

`<id|prefix>` is matched case-insensitively against the start of the session ids in every open agterm window; zero or several matches is an error that names the candidates. A usage error exits 2, any other failure exits 1.

`park` refuses a session that is not a mapped, live, idle claude, or one that is flagged or whose agent is working, and prints the reason. It parks the selected session and ignores `park_after`.

`status` prints `live` for a session whose foreground is claude, `parked` for one running the placeholder or holding a state file, and `shell` otherwise.

## Configuration

`$HOME/.config/claude-siesta/config.toml`, optional, both keys optional:

```toml
park_after = "2h"
poll_interval = "10m"
```

- `park_after` (default `2h`): how long a claude must be idle before the daemon parks it.
- `poll_interval` (default `10m`): how often the daemon looks at agterm.

A duration is an integer followed by `s`, `m`, `h` or `d`. An invalid value or an unknown key stops the daemon and `park` with an error that names the key (exit 2); it is never replaced by a silent default. The placeholder never reads the config, so a broken file never blocks a resume.

## Files

Read:

- `$HOME/.local/state/agterm/cc-map/<SESSION-ID>`: one JSON file per agterm session with the conversation id, profile, cwd, last-write time and claude pid. claude-siesta only reads these, never writes or deletes them.
- `$HOME/.claude/projects/*/<conv>.jsonl` (profile `personal`) and `$HOME/.claude-work/projects/*/<conv>.jsonl` (profile `work`): the transcripts, of which only the last 400 KiB is read.
- `$HOME/.config/claude-siesta/config.toml`.
- agterm itself, through `/Applications/agterm.app/Contents/MacOS/agtermctl` (`window list`, `tree`, `session restore`, `session type`).

Written:

- `$HOME/.local/state/claude-siesta/<SESSION-ID>.json`: one state file per parked session (session, conv, profile, when it was parked, when claude last answered, the placeholder's pid). The placeholder deletes it on resume; the daemon deletes files of sessions that no longer exist.
- `$HOME/.local/state/claude-siesta/claude-siesta.log`: one line per decision (time, action, session, conv, idle minutes, reason). It never contains transcript text or paths, and it is not rotated.
- `$HOME/Library/Logs/claude-siesta.err.log`: the daemon's stderr, written by launchd.

## Dependency on the cc-map hooks

claude-siesta knows which conversation runs in which session only through cc-map, which Pavel's Claude Code hooks write: one entry per agterm session with `conv`, `profile`, `cwd`, `ts` and the claude `pid`. A session without an entry, or with an entry that has no pid, is never parked. On resume the SessionStart hook rewrites the entry's pid and re-pins the restore line, which is what makes a resumed session parkable again. Killing claude does not trigger the hook that removes an entry, so the entry survives parking by design.

## Install

```
mise run install
```

Builds the release binary, copies it to `$HOME/.local/bin/claude-siesta` (through a temporary name and a rename, so placeholders already running keep their binary), renders `launchd/dev.pkarpovich.claude-siesta.plist` into `$HOME/Library/LaunchAgents/` and (re)loads the agent `dev.pkarpovich.claude-siesta`. Running it again upgrades in place. `$HOME/.local/bin` must be on the interactive fish PATH, because agterm types the pinned `claude-siesta` restore line into a login fish.

```
mise run uninstall
```

Unloads the agent and removes its plist. The binary, the state files and the log stay.

## Build

```
mise run check
```

Runs `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` and `plutil -lint` over the rendered plist. macOS only.
