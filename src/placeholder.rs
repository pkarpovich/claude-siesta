pub mod view;

use std::ffi::OsString;
use std::fmt;
use std::io::{self, Stdout};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime};

use crossterm::cursor::Show;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::agterm::{Agterm, AgtermOps};
use crate::ccmap::{CcMapError, MapEntry, Profile, SessionId};
use crate::paths::Paths;
use crate::state::{self, ParkState};
use crate::time::{format_idle, format_local_minute};
use crate::transcript::{self, LastAssistant};
use view::ViewModel;

const POLL: Duration = Duration::from_secs(1);
const CONV_SHORT: usize = 8;

static RESUME: AtomicBool = AtomicBool::new(false);
static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);

extern "C" fn request_resume(_: nix::libc::c_int) {
    RESUME.store(true, Ordering::SeqCst);
}

#[derive(Debug)]
pub enum StartError {
    NotInAgterm,
    NotMapped,
    CcMap(CcMapError),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StartError::NotInAgterm => write!(f, "not inside an agterm session"),
            StartError::NotMapped => {
                write!(f, "no Claude conversation is mapped to this session")
            }
            StartError::CcMap(error) => write!(f, "{error}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    Resume,
    Quit,
    Redraw,
    Ignore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Resume,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeCommand {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, OsString)>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub name: String,
    pub entry: MapEntry,
    pub parked_at: Option<SystemTime>,
    pub last: Option<LastAssistant>,
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
    }
}

pub fn run(paths: &Paths) -> ExitCode {
    let (session, entry) = match start(paths, std::env::var("AGTERM_SESSION_ID").ok()) {
        Ok(started) => started,
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            return ExitCode::from(1);
        }
    };
    let parked_at = match state::read(paths, &session) {
        Some(ParkState {
            session: _,
            conv: _,
            profile: _,
            parked_at,
            last_assistant_at: _,
            placeholder_pid: _,
        }) => {
            let _ = state::set_placeholder_pid(paths, &session, std::process::id() as i32);
            Some(parked_at)
        }
        None => None,
    };
    let screen = Screen {
        name: session_name(&session, &entry),
        last: transcript::load_last(paths, &entry),
        entry,
        parked_at,
    };
    if let Err(error) = install_resume_handler() {
        eprintln!("claude-siesta: cannot install signal handler: {error}");
        return ExitCode::from(1);
    }
    install_panic_hook();
    let outcome = show(&screen, paths.home());
    restore_terminal();
    match outcome {
        Ok(Outcome::Quit) => ExitCode::SUCCESS,
        Ok(Outcome::Resume) => resume(paths, &session, resume_command(&screen.entry, paths)),
        Err(error) => {
            eprintln!("claude-siesta: terminal: {error}");
            ExitCode::from(1)
        }
    }
}

pub fn start(
    paths: &Paths,
    session_var: Option<String>,
) -> Result<(SessionId, MapEntry), StartError> {
    let Some(session) = session_var else {
        return Err(StartError::NotInAgterm);
    };
    if session.trim().is_empty() {
        return Err(StartError::NotInAgterm);
    }
    let session = SessionId::new(session.trim());
    let entry = match MapEntry::load(paths, &session) {
        Ok(Some(entry)) => entry,
        Ok(None) => return Err(StartError::NotMapped),
        Err(error) => return Err(StartError::CcMap(error)),
    };
    Ok((session, entry))
}

fn session_name(session: &SessionId, entry: &MapEntry) -> String {
    let agterm = Agterm::default();
    if let Ok(sessions) = agterm.all_sessions() {
        for (_, found) in sessions {
            if found.id == *session && !found.name.trim().is_empty() {
                return found.name;
            }
        }
    }
    fallback_name(session, entry)
}

pub fn fallback_name(session: &SessionId, entry: &MapEntry) -> String {
    let Some(name) = entry.cwd.file_name() else {
        return session.as_str().to_string();
    };
    name.to_string_lossy().into_owned()
}

pub fn view_model(screen: &Screen, now: SystemTime, home: &Path) -> ViewModel {
    let Screen {
        name,
        entry,
        parked_at,
        last,
    } = screen;
    let conv = entry.conv.as_str();
    let conv_short = conv.get(..CONV_SHORT).unwrap_or(conv);
    let parked_at = (*parked_at).map(format_local_minute);
    let excerpt = match last {
        Some(LastAssistant { at: _, text }) => text.clone(),
        None => String::new(),
    };
    ViewModel {
        name: name.clone(),
        cwd_display: cwd_display(&entry.cwd, home),
        conv_short: conv_short.to_string(),
        idle: format_idle(transcript::idle(now, last.as_ref(), entry)),
        parked_at,
        excerpt,
    }
}

pub fn cwd_display(cwd: &Path, home: &Path) -> String {
    let Ok(rest) = cwd.strip_prefix(home) else {
        return cwd.display().to_string();
    };
    if rest.as_os_str().is_empty() {
        return String::from("~");
    }
    format!("~/{}", rest.display())
}

pub fn input_action(event: &Event) -> InputAction {
    match event {
        Event::Key(key) => {
            if key.kind != KeyEventKind::Press {
                return InputAction::Ignore;
            }
            key_action(key.code)
        }
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::Down(_) => InputAction::Resume,
            MouseEventKind::Up(_)
            | MouseEventKind::Drag(_)
            | MouseEventKind::Moved
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => InputAction::Ignore,
        },
        Event::Resize(_, _) => InputAction::Redraw,
        Event::FocusGained | Event::FocusLost | Event::Paste(_) => InputAction::Ignore,
    }
}

fn key_action(code: KeyCode) -> InputAction {
    match code {
        KeyCode::Enter | KeyCode::Char(' ') => InputAction::Resume,
        KeyCode::Char('q') | KeyCode::Esc => InputAction::Quit,
        KeyCode::Char(_)
        | KeyCode::Backspace
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Up
        | KeyCode::Down
        | KeyCode::Home
        | KeyCode::End
        | KeyCode::PageUp
        | KeyCode::PageDown
        | KeyCode::Tab
        | KeyCode::BackTab
        | KeyCode::Delete
        | KeyCode::Insert
        | KeyCode::F(_)
        | KeyCode::Null
        | KeyCode::CapsLock
        | KeyCode::ScrollLock
        | KeyCode::NumLock
        | KeyCode::PrintScreen
        | KeyCode::Pause
        | KeyCode::Menu
        | KeyCode::KeypadBegin
        | KeyCode::Media(_)
        | KeyCode::Modifier(_) => InputAction::Ignore,
    }
}

pub fn resume_command(entry: &MapEntry, paths: &Paths) -> ResumeCommand {
    let mut env = Vec::new();
    env.push((String::from("CLAUDE_CODE_NO_FLICKER"), OsString::from("1")));
    match entry.profile {
        Profile::Personal => {}
        Profile::Work => env.push((
            String::from("CLAUDE_CONFIG_DIR"),
            paths.transcript_root(Profile::Work).into_os_string(),
        )),
    }
    let cwd = match entry.cwd.as_os_str().is_empty() {
        true => None,
        false => Some(entry.cwd.clone()),
    };
    ResumeCommand {
        program: String::from("claude"),
        args: vec![
            String::from("--enable-auto-mode"),
            String::from("--resume"),
            entry.conv.as_str().to_string(),
        ],
        env,
        cwd,
    }
}

fn resume(paths: &Paths, session: &SessionId, command: ResumeCommand) -> ExitCode {
    let _ = state::remove(paths, session);
    let ResumeCommand {
        program,
        args,
        env,
        cwd,
    } = command;
    let mut command = Command::new(&program);
    command.args(&args).envs(env);
    if let Some(cwd) = cwd
        && cwd.is_dir()
    {
        command.current_dir(cwd);
    }
    let error = command.exec();
    eprintln!("claude-siesta: cannot run {program}: {error}");
    ExitCode::from(1)
}

fn install_resume_handler() -> nix::Result<()> {
    let action = SigAction::new(
        SigHandler::Handler(request_resume),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    unsafe { sigaction(Signal::SIGUSR1, &action) }?;
    Ok(())
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

fn restore_terminal() {
    if !TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let _ = execute!(
        io::stdout(),
        DisableMouseCapture,
        LeaveAlternateScreen,
        Show
    );
    let _ = disable_raw_mode();
}

fn show(screen: &Screen, home: &Path) -> io::Result<Outcome> {
    let _guard = TerminalGuard;
    TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    event_loop(&mut terminal, screen, home)
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    screen: &Screen,
    home: &Path,
) -> io::Result<Outcome> {
    let mut drawn = None;
    loop {
        if RESUME.load(Ordering::SeqCst) {
            return Ok(Outcome::Resume);
        }
        let model = view_model(screen, SystemTime::now(), home);
        if drawn.as_ref() != Some(&model) {
            terminal.draw(|frame| view::render(frame, &model))?;
            drawn = Some(model);
        }
        if !event::poll(POLL)? {
            continue;
        }
        match input_action(&event::read()?) {
            InputAction::Resume => return Ok(Outcome::Resume),
            InputAction::Quit => return Ok(Outcome::Quit),
            InputAction::Redraw => drawn = None,
            InputAction::Ignore => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use crossterm::event::{KeyEvent, KeyModifiers, MouseButton, MouseEvent};
    use nix::sys::signal::raise;

    use super::*;
    use crate::ccmap::ConvId;

    const EXEC_CHILD: &str = "CLAUDE_SIESTA_TEST_EXEC_CHILD";
    const MISSING_PROGRAM: &str = "claude-siesta-test-missing-binary";

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "claude-siesta-placeholder-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn entry(profile: Profile) -> MapEntry {
        MapEntry {
            conv: ConvId::new("c0acdbe6-1111-2222-3333-444444444444"),
            profile,
            cwd: PathBuf::from("/home/x/Projects/siesta"),
            ts: at(1_000),
            pid: Some(4242),
        }
    }

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn mouse(kind: MouseEventKind) -> Event {
        Event::Mouse(MouseEvent {
            kind,
            column: 3,
            row: 4,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn resume_keys_and_click() {
        assert_eq!(input_action(&key(KeyCode::Enter)), InputAction::Resume);
        assert_eq!(input_action(&key(KeyCode::Char(' '))), InputAction::Resume);
        assert_eq!(
            input_action(&mouse(MouseEventKind::Down(MouseButton::Left))),
            InputAction::Resume
        );
        assert_eq!(
            input_action(&mouse(MouseEventKind::Down(MouseButton::Right))),
            InputAction::Resume
        );
    }

    #[test]
    fn quit_keys() {
        assert_eq!(input_action(&key(KeyCode::Char('q'))), InputAction::Quit);
        assert_eq!(input_action(&key(KeyCode::Esc)), InputAction::Quit);
    }

    #[test]
    fn ignored_input() {
        assert_eq!(input_action(&key(KeyCode::Char('x'))), InputAction::Ignore);
        assert_eq!(input_action(&key(KeyCode::Up)), InputAction::Ignore);
        let mut release = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        assert_eq!(input_action(&Event::Key(release)), InputAction::Ignore);
        assert_eq!(
            input_action(&mouse(MouseEventKind::Up(MouseButton::Left))),
            InputAction::Ignore
        );
        assert_eq!(
            input_action(&mouse(MouseEventKind::Moved)),
            InputAction::Ignore
        );
        assert_eq!(
            input_action(&mouse(MouseEventKind::ScrollDown)),
            InputAction::Ignore
        );
        assert_eq!(input_action(&Event::FocusLost), InputAction::Ignore);
        assert_eq!(
            input_action(&Event::Paste(String::from("x"))),
            InputAction::Ignore
        );
    }

    #[test]
    fn resize_redraws() {
        assert_eq!(input_action(&Event::Resize(80, 24)), InputAction::Redraw);
    }

    #[test]
    fn personal_resume_command() {
        let paths = Paths::from_home(PathBuf::from("/home/x"));
        let command = resume_command(&entry(Profile::Personal), &paths);
        assert_eq!(
            command,
            ResumeCommand {
                program: String::from("claude"),
                args: vec![
                    String::from("--enable-auto-mode"),
                    String::from("--resume"),
                    String::from("c0acdbe6-1111-2222-3333-444444444444"),
                ],
                env: vec![(String::from("CLAUDE_CODE_NO_FLICKER"), OsString::from("1"))],
                cwd: Some(PathBuf::from("/home/x/Projects/siesta")),
            }
        );
    }

    #[test]
    fn work_resume_command_sets_config_dir() {
        let paths = Paths::from_home(PathBuf::from("/home/x"));
        let ResumeCommand {
            program,
            args,
            env,
            cwd: _,
        } = resume_command(&entry(Profile::Work), &paths);
        assert_eq!(program, "claude");
        assert_eq!(args[2], "c0acdbe6-1111-2222-3333-444444444444");
        assert_eq!(
            env,
            vec![
                (String::from("CLAUDE_CODE_NO_FLICKER"), OsString::from("1")),
                (
                    String::from("CLAUDE_CONFIG_DIR"),
                    OsString::from("/home/x/.claude-work")
                ),
            ]
        );
    }

    #[test]
    fn resume_command_without_cwd() {
        let paths = Paths::from_home(PathBuf::from("/home/x"));
        let entry = MapEntry {
            cwd: PathBuf::new(),
            ..entry(Profile::Personal)
        };
        assert_eq!(resume_command(&entry, &paths).cwd, None);
    }

    #[test]
    fn start_outside_agterm() {
        let paths = Paths::from_home(temp_home("outside"));
        let error = start(&paths, None).unwrap_err();
        assert_eq!(error.to_string(), "not inside an agterm session");
        let error = start(&paths, Some(String::from("  "))).unwrap_err();
        assert_eq!(error.to_string(), "not inside an agterm session");
    }

    #[test]
    fn start_without_map_entry() {
        let paths = Paths::from_home(temp_home("unmapped"));
        let error = start(&paths, Some(String::from("7320056a-0000"))).unwrap_err();
        assert_eq!(
            error.to_string(),
            "no Claude conversation is mapped to this session"
        );
    }

    #[test]
    fn start_with_corrupt_map_entry() {
        let home = temp_home("corrupt");
        let paths = Paths::from_home(home);
        std::fs::create_dir_all(paths.cc_map_dir()).unwrap();
        std::fs::write(paths.cc_map_dir().join("7320056A-0000"), "{not json").unwrap();
        let error = start(&paths, Some(String::from("7320056A-0000"))).unwrap_err();
        assert!(
            error.to_string().starts_with("invalid cc-map entry"),
            "{error}"
        );
    }

    #[test]
    fn start_with_map_entry() {
        let paths = Paths::from_home(temp_home("mapped"));
        std::fs::create_dir_all(paths.cc_map_dir()).unwrap();
        std::fs::write(
            paths.cc_map_dir().join("7320056A-0000"),
            r#"{"conv":"c0acdbe6-1","profile":"work","cwd":"/p","ts":1790419180,"pid":1}"#,
        )
        .unwrap();
        let (session, entry) = start(&paths, Some(String::from("7320056a-0000"))).unwrap();
        assert_eq!(session.as_str(), "7320056A-0000");
        assert_eq!(entry.conv.as_str(), "c0acdbe6-1");
        assert_eq!(entry.profile, Profile::Work);
    }

    #[test]
    fn view_model_from_last_assistant() {
        let screen = Screen {
            name: String::from("siesta"),
            entry: entry(Profile::Personal),
            parked_at: Some(at(1_790_500_000)),
            last: Some(LastAssistant {
                at: at(10_000),
                text: String::from("Done."),
            }),
        };
        let now = at(10_000 + 26 * 3600 + 12 * 60 + 30);
        let model = view_model(&screen, now, Path::new("/home/x"));
        assert_eq!(
            model,
            ViewModel {
                name: String::from("siesta"),
                cwd_display: String::from("~/Projects/siesta"),
                conv_short: String::from("c0acdbe6"),
                idle: String::from("26h 12m"),
                parked_at: Some(format_local_minute(at(1_790_500_000))),
                excerpt: String::from("Done."),
            }
        );
    }

    #[test]
    fn view_model_falls_back_to_map_ts() {
        let screen = Screen {
            name: String::from("siesta"),
            entry: entry(Profile::Personal),
            parked_at: None,
            last: None,
        };
        let model = view_model(&screen, at(1_000 + 45 * 60), Path::new("/elsewhere"));
        assert_eq!(model.idle, "45m");
        assert_eq!(model.parked_at, None);
        assert_eq!(model.excerpt, "");
        assert_eq!(model.cwd_display, "/home/x/Projects/siesta");
    }

    #[test]
    fn short_conv_is_kept_whole() {
        let screen = Screen {
            name: String::from("s"),
            entry: MapEntry {
                conv: ConvId::new("abc"),
                ..entry(Profile::Personal)
            },
            parked_at: None,
            last: None,
        };
        assert_eq!(
            view_model(&screen, at(2_000), Path::new("/")).conv_short,
            "abc"
        );
    }

    #[test]
    fn cwd_display_shortens_home() {
        let home = Path::new("/home/x");
        assert_eq!(cwd_display(Path::new("/home/x"), home), "~");
        assert_eq!(cwd_display(Path::new("/home/x/a/b"), home), "~/a/b");
        assert_eq!(cwd_display(Path::new("/home/xy"), home), "/home/xy");
        assert_eq!(cwd_display(Path::new("/tmp"), home), "/tmp");
    }

    #[test]
    fn fallback_name_uses_cwd_basename() {
        let session = SessionId::new("7320056a");
        assert_eq!(fallback_name(&session, &entry(Profile::Personal)), "siesta");
        let entry = MapEntry {
            cwd: PathBuf::new(),
            ..entry(Profile::Personal)
        };
        assert_eq!(fallback_name(&session, &entry), "7320056A");
    }

    #[test]
    fn sigusr1_requests_resume() {
        install_resume_handler().unwrap();
        RESUME.store(false, Ordering::SeqCst);
        raise(Signal::SIGUSR1).unwrap();
        assert!(RESUME.swap(false, Ordering::SeqCst));
    }

    #[test]
    fn failed_exec_removes_state_and_exits_1() {
        let Some(home) = std::env::var_os(EXEC_CHILD) else {
            let home = temp_home("exec");
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "placeholder::tests::failed_exec_removes_state_and_exits_1",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(EXEC_CHILD, &home)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{stderr}");
            assert!(
                stderr.contains(&format!("claude-siesta: cannot run {MISSING_PROGRAM}")),
                "{stderr}"
            );
            return;
        };
        let paths = Paths::from_home(PathBuf::from(home));
        let session = SessionId::new("7320056A-0000");
        state::write(
            &paths,
            &ParkState {
                session: session.clone(),
                conv: ConvId::new("c0acdbe6-1"),
                profile: Profile::Personal,
                parked_at: at(2_000),
                last_assistant_at: at(1_000),
                placeholder_pid: None,
            },
        )
        .unwrap();
        let command = ResumeCommand {
            program: String::from(MISSING_PROGRAM),
            args: Vec::new(),
            env: Vec::new(),
            cwd: None,
        };
        assert_eq!(resume(&paths, &session, command), ExitCode::from(1));
        assert_eq!(state::read(&paths, &session), None);
    }
}
