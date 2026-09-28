# Spike results: placeholder runtime behaviour inside agterm

Run 2026-09-28 on agterm 0.32.0 (commit 1ffc3ee8), macOS 27, fish 4.9, with `examples/spike.rs` built in release mode.

## Method

The spike ran in a throwaway background agterm session created with `session new --no-select` (a login fish in this repo's dir). The session was driven from another pane: `session type --stdin` started the spike and sent keys, `kill -USR1` sent the signal, `tree --json --window` read `foreground`, and `session text` read the screen. The session was closed afterwards.

To check the terminal state after `exec`, the spike exec'd a small python probe. The probe puts the tty in raw mode and sends DECRQM queries (`CSI ? Pm $ p`) for the mouse modes 1000, 1002, 1003 and 1006 and the alternate screen 1049. A reply of `2` means reset and `1` means set. As a control, the same probe run right after `printf '\033[?1000h'` reported `1000;1`, so the probe does see a set mode.

## Results

| Behaviour | Result |
|---|---|
| Key event arrives | yes: `Key(KeyEvent { code: Char('x'), kind: Press, .. })` shown as the last event; a typed newline arrives as `KeyCode::Enter` and resumes |
| Mouse click arrives as a crossterm mouse down | parsing yes, real click not verified: an SGR click `ESC[<0;10;5M` written into the pane's input by `session type` arrived as `MouseEventKind::Down` and resumed. A physical click in the agterm GUI could not be automated from the agent and is left to the Post-Completion manual check ("repeat with a click") |
| `kill -USR1 <pid>` from another shell resumes | yes: `spike: resume via signal`, the exec'd program ran |
| Terminal clean after `exec` | yes: DECRQM after exec reported `1000;2 1002;2 1003;2 1006;2 1049;2` (mouse capture and alternate screen all reset). Exec into `claude` drew its TUI with no escape garbage or alt-screen leftovers |
| `tree --json` reports the new `foreground` | yes, within 1 s. argv is whatever the exec'd process reports: `["bash", "-c", "echo exec-ok; sleep 8"]` for bash (argv[0] exactly as passed), but `["/Users/<user>/.local/bin/claude"]` for claude exec'd as the bare name `claude` through a PATH lookup |
| Pane returns to the fish prompt when the exec'd program exits | yes: `foreground` absent, `foregroundShell: fish`, prompt shown |

## RSS

The release build of the spike idling in its event loop: `ps -o rss=` = 2944 KB (about 2.9 MB), against roughly 200 MB per idle claude.

## Consequences for Task 11

None of the four behaviours failed, so the placeholder design stands as written. Notes to carry forward:

- A resumed claude reports an absolute argv[0] even when exec'd by bare name, so `rule` check 1 (`foreground[0]` ends with `/claude`) matches a claude started by the placeholder. That comes from claude itself, not from `exec`: a bare-name `bash` kept `bash`.
- The restore order used by the spike (`DisableMouseCapture`, `LeaveAlternateScreen`, `disable_raw_mode`, then `exec`) leaves the terminal clean. Keep that order in the placeholder guard.
- A fresh `claude` in an untrusted folder shows the trust dialog first. That does not affect `--resume` of an existing conversation.
