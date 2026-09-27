use std::process::Command;
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use crate::agterm::AgtermOps;
use crate::ccmap::{MapEntry, SessionId};
use crate::config::Config;
use crate::log::{self, Action, LogLine};
use crate::paths::Paths;
use crate::rule::{self, Decision, Mode, RuleInput, SkipReason};
use crate::state::{self, ParkState};
use crate::transcript::{self, LastAssistant};
use crate::tree::{Session, WindowId};

pub const RESTORE_LINE: &str = "claude-siesta";
pub const PLACEHOLDER_INPUT: &str = " claude-siesta\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Poll {
    pub interval: Duration,
    pub attempts: u32,
}

pub const KILL_POLL: Poll = Poll {
    interval: Duration::from_millis(250),
    attempts: 20,
};

pub const FOREGROUND_POLL: Poll = Poll {
    interval: Duration::from_millis(250),
    attempts: 20,
};

pub trait ProcessOps {
    fn is_claude(&self, pid: i32) -> bool;
    fn terminate(&self, pid: i32);
}

pub struct SystemProcesses;

impl ProcessOps for SystemProcesses {
    fn is_claude(&self, pid: i32) -> bool {
        pid_is_claude(pid)
    }

    fn terminate(&self, pid: i32) {
        terminate(pid, KILL_POLL);
    }
}

pub fn pid_is_claude(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    if kill(Pid::from_raw(pid), None).is_err() {
        return false;
    }
    let Ok(output) = Command::new("/bin/ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .ends_with("claude")
}

pub fn terminate(pid: i32, poll: Poll) {
    if pid <= 0 {
        return;
    }
    let pid = Pid::from_raw(pid);
    if kill(pid, Signal::SIGTERM).is_err() {
        return;
    }
    for _ in 0..poll.attempts {
        if kill(pid, None).is_err() {
            return;
        }
        sleep(poll.interval);
    }
    if kill(pid, None).is_err() {
        return;
    }
    let _ = kill(pid, Signal::SIGKILL);
}

pub struct ParkEnv<'a> {
    pub agterm: &'a dyn AgtermOps,
    pub processes: &'a dyn ProcessOps,
    pub paths: &'a Paths,
    pub config: &'a Config,
    pub now: SystemTime,
    pub foreground_poll: Poll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkStep {
    Tree,
    SessionGone,
    ForegroundBusy,
    State,
    Restore,
    Type,
}

impl ParkStep {
    pub fn as_str(self) -> &'static str {
        match self {
            ParkStep::Tree => "tree",
            ParkStep::SessionGone => "session-gone",
            ParkStep::ForegroundBusy => "foreground-busy",
            ParkStep::State => "state",
            ParkStep::Restore => "restore",
            ParkStep::Type => "type",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParkFailure {
    pub step: ParkStep,
    pub detail: Option<String>,
}

impl ParkFailure {
    fn reason(&self) -> String {
        let ParkFailure { step, detail } = self;
        match detail {
            Some(detail) => format!("{}: {}", step.as_str(), first_line(detail)),
            None => step.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParkOutcome {
    Parked,
    Skipped(SkipReason),
    Failed(ParkFailure),
}

pub fn run(env: &ParkEnv, window: &WindowId, session: &Session, mode: Mode) -> ParkOutcome {
    let entry = MapEntry::load(env.paths, &session.id).unwrap_or_default();
    let last = last_assistant(env.paths, entry.as_ref());
    let idle = match &entry {
        Some(entry) => transcript::idle(env.now, last.as_ref(), entry),
        None => Duration::ZERO,
    };
    let pid_is_claude = match &entry {
        Some(MapEntry {
            pid: Some(pid),
            conv: _,
            profile: _,
            cwd: _,
            ts: _,
        }) => env.processes.is_claude(*pid),
        Some(MapEntry {
            pid: None,
            conv: _,
            profile: _,
            cwd: _,
            ts: _,
        }) => false,
        None => false,
    };
    let decision = rule::decide(&RuleInput {
        session,
        entry: entry.as_ref(),
        pid_is_claude,
        idle,
        park_after: env.config.park_after,
        mode,
    });
    let line = |action: Action, reason: String| LogLine {
        action,
        session: Some(session.id.clone()),
        conv: entry.as_ref().map(|entry| entry.conv.clone()),
        idle: Some(idle),
        reason,
    };
    match decision {
        Decision::Park => {}
        Decision::Skip(reason) => {
            if session.runs_claude() {
                write_log(env.paths, &line(Action::Skip, reason.as_str().to_string()));
            }
            return ParkOutcome::Skipped(reason);
        }
    }
    let Some(MapEntry {
        conv,
        profile,
        ts,
        pid: Some(pid),
        cwd: _,
    }) = entry.clone()
    else {
        return ParkOutcome::Skipped(SkipReason::NotMapped);
    };
    env.processes.terminate(pid);
    let parked = park_terminated(
        env,
        window,
        &session.id,
        ParkState {
            session: session.id.clone(),
            conv,
            profile,
            parked_at: env.now,
            last_assistant_at: match &last {
                Some(LastAssistant { at, text: _ }) => *at,
                None => ts,
            },
            placeholder_pid: None,
        },
    );
    match parked {
        Ok(()) => {
            write_log(env.paths, &line(Action::Park, String::new()));
            ParkOutcome::Parked
        }
        Err(failure) => {
            write_log(env.paths, &line(Action::ParkFailed, failure.reason()));
            ParkOutcome::Failed(failure)
        }
    }
}

fn park_terminated(
    env: &ParkEnv,
    window: &WindowId,
    session: &SessionId,
    park_state: ParkState,
) -> Result<(), ParkFailure> {
    wait_for_shell(env, window, session)?;
    state::write(env.paths, &park_state).map_err(|error| ParkFailure {
        step: ParkStep::State,
        detail: Some(error.to_string()),
    })?;
    let pinned = env
        .agterm
        .restore(window, session, RESTORE_LINE)
        .map_err(|error| ParkFailure {
            step: ParkStep::Restore,
            detail: Some(error.to_string()),
        });
    let typed = pinned.and_then(|()| {
        env.agterm
            .type_text(window, session, PLACEHOLDER_INPUT)
            .map_err(|error| ParkFailure {
                step: ParkStep::Type,
                detail: Some(error.to_string()),
            })
    });
    if typed.is_err() {
        let _ = state::remove(env.paths, session);
    }
    typed
}

fn wait_for_shell(env: &ParkEnv, window: &WindowId, id: &SessionId) -> Result<(), ParkFailure> {
    let Poll { interval, attempts } = env.foreground_poll;
    for attempt in 0..attempts {
        if attempt > 0 {
            sleep(interval);
        }
        let sessions = env.agterm.tree(window).map_err(|error| ParkFailure {
            step: ParkStep::Tree,
            detail: Some(error.to_string()),
        })?;
        let mut found = None;
        for session in sessions {
            if &session.id == id {
                found = Some(session);
                break;
            }
        }
        let Some(session) = found else {
            return Err(ParkFailure {
                step: ParkStep::SessionGone,
                detail: None,
            });
        };
        if session.foreground.is_empty() {
            return Ok(());
        }
    }
    Err(ParkFailure {
        step: ParkStep::ForegroundBusy,
        detail: None,
    })
}

fn last_assistant(paths: &Paths, entry: Option<&MapEntry>) -> Option<LastAssistant> {
    let MapEntry {
        conv,
        profile,
        cwd: _,
        ts: _,
        pid: _,
    } = entry?;
    let path = transcript::find_transcript(paths, *profile, conv)?;
    let (bytes, start) = transcript::read_tail(&path).ok()?;
    transcript::last_assistant(&bytes, start)
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or_default()
}

fn write_log(paths: &Paths, line: &LogLine) {
    if let Err(error) = log::append(paths, line) {
        eprintln!("claude-siesta: cannot write log: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::Stdio;

    use super::*;
    use crate::agterm::AgtermError;
    use crate::tree::{self, AgentStatus};

    const SESSION: &str = "7320056A-2D3F-4B06-AB9A-1CB79BED0F3A";
    const WINDOW: &str = "F800A0ED-F3A9-4CF7-ACCE-55E58C795C76";
    const CONV: &str = "c0acdbe6-d414-4622-a394-ee560f5b2646";
    const PID: i32 = 68237;
    const MAP_TS: u64 = 1_790_000_000;
    const NOW: u64 = 1_790_500_000;
    const CLAUDE: &str = "/Users/x/.local/bin/claude";

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Windows,
        Tree(String),
        Restore(String, String, String),
        Type(String, String, String),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Failing {
        Nothing,
        Restore,
        Type,
    }

    struct FakeAgterm {
        trees: RefCell<Vec<Vec<Session>>>,
        failing: Failing,
        calls: RefCell<Vec<Call>>,
    }

    impl FakeAgterm {
        fn new(trees: Vec<Vec<Session>>, failing: Failing) -> FakeAgterm {
            FakeAgterm {
                trees: RefCell::new(trees),
                failing,
                calls: RefCell::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl AgtermOps for FakeAgterm {
        fn windows(&self) -> Result<Vec<tree::Window>, AgtermError> {
            self.calls.borrow_mut().push(Call::Windows);
            Ok(vec![tree::Window {
                id: WindowId::new(WINDOW),
                open: true,
            }])
        }

        fn tree(&self, window: &WindowId) -> Result<Vec<Session>, AgtermError> {
            self.calls
                .borrow_mut()
                .push(Call::Tree(window.as_str().to_string()));
            let mut trees = self.trees.borrow_mut();
            match trees.len() {
                0 => Err(AgtermError::Failed("no tree".to_string())),
                1 => Ok(trees[0].clone()),
                _ => Ok(trees.remove(0)),
            }
        }

        fn restore(
            &self,
            window: &WindowId,
            session: &SessionId,
            command: &str,
        ) -> Result<(), AgtermError> {
            self.calls.borrow_mut().push(Call::Restore(
                window.as_str().to_string(),
                session.as_str().to_string(),
                command.to_string(),
            ));
            match self.failing {
                Failing::Restore => Err(AgtermError::Failed("no such session\nsecond".to_string())),
                Failing::Type => Ok(()),
                Failing::Nothing => Ok(()),
            }
        }

        fn type_text(
            &self,
            window: &WindowId,
            session: &SessionId,
            text: &str,
        ) -> Result<(), AgtermError> {
            self.calls.borrow_mut().push(Call::Type(
                window.as_str().to_string(),
                session.as_str().to_string(),
                text.to_string(),
            ));
            match self.failing {
                Failing::Type => Err(AgtermError::Failed("socket closed".to_string())),
                Failing::Restore => Ok(()),
                Failing::Nothing => Ok(()),
            }
        }
    }

    struct FakeProcesses {
        claude: bool,
        terminated: RefCell<Vec<i32>>,
    }

    impl FakeProcesses {
        fn new(claude: bool) -> FakeProcesses {
            FakeProcesses {
                claude,
                terminated: RefCell::new(Vec::new()),
            }
        }
    }

    impl ProcessOps for FakeProcesses {
        fn is_claude(&self, _pid: i32) -> bool {
            self.claude
        }

        fn terminate(&self, pid: i32) {
            self.terminated.borrow_mut().push(pid);
        }
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn temp_home(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("claude-siesta-park-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn home_with_entry(name: &str) -> Paths {
        let paths = Paths::from_home(temp_home(name));
        let dir = paths.cc_map_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(SESSION),
            format!(
                r#"{{"conv":"{CONV}","profile":"personal","cwd":"/Users/x/Projects/y","ts":{MAP_TS},"pid":{PID}}}"#
            ),
        )
        .unwrap();
        paths
    }

    fn session(foreground: &[&str], active: bool) -> Session {
        let mut argv = Vec::new();
        for arg in foreground {
            argv.push(arg.to_string());
        }
        Session {
            id: SessionId::new(SESSION),
            name: "tuclaw".to_string(),
            active,
            flagged: false,
            foreground: argv,
            status: AgentStatus::Idle,
            restore_command: None,
        }
    }

    fn live() -> Session {
        session(&[CLAUDE, "--resume", CONV], false)
    }

    fn shell() -> Session {
        session(&[], false)
    }

    fn park(
        paths: &Paths,
        agterm: &FakeAgterm,
        processes: &FakeProcesses,
        target: &Session,
        mode: Mode,
    ) -> ParkOutcome {
        let config = Config::default();
        let env = ParkEnv {
            agterm,
            processes,
            paths,
            config: &config,
            now: at(NOW),
            foreground_poll: Poll {
                interval: Duration::ZERO,
                attempts: 3,
            },
        };
        run(&env, &WindowId::new(WINDOW), target, mode)
    }

    #[test]
    fn skip_at_shell_prompt_makes_no_calls_and_no_log() {
        let paths = home_with_entry("skip-shell");
        let agterm = FakeAgterm::new(vec![vec![shell()]], Failing::Nothing);
        let processes = FakeProcesses::new(true);
        let outcome = park(&paths, &agterm, &processes, &shell(), Mode::Daemon);
        assert_eq!(outcome, ParkOutcome::Skipped(SkipReason::NotClaude));
        assert_eq!(agterm.calls(), Vec::new());
        assert_eq!(processes.terminated.borrow().clone(), Vec::<i32>::new());
        assert!(!paths.state_file(&SessionId::new(SESSION)).exists());
        assert!(!paths.log_file().exists());
    }

    #[test]
    fn skip_of_live_claude_logs_skip_without_side_effects() {
        let paths = home_with_entry("skip-selected");
        let agterm = FakeAgterm::new(vec![vec![live()]], Failing::Nothing);
        let processes = FakeProcesses::new(true);
        let selected = session(&[CLAUDE], true);
        let outcome = park(&paths, &agterm, &processes, &selected, Mode::Daemon);
        assert_eq!(outcome, ParkOutcome::Skipped(SkipReason::Selected));
        assert_eq!(agterm.calls(), Vec::new());
        assert_eq!(processes.terminated.borrow().clone(), Vec::<i32>::new());
        assert!(!paths.state_file(&SessionId::new(SESSION)).exists());
        let log = std::fs::read_to_string(paths.log_file()).unwrap();
        assert!(log.contains(" skip "));
        assert!(log.contains("selected"));
    }

    #[test]
    fn skip_when_pid_is_not_claude_does_not_terminate() {
        let paths = home_with_entry("skip-pid");
        let agterm = FakeAgterm::new(vec![vec![live()]], Failing::Nothing);
        let processes = FakeProcesses::new(false);
        let outcome = park(&paths, &agterm, &processes, &live(), Mode::Daemon);
        assert_eq!(outcome, ParkOutcome::Skipped(SkipReason::PidNotClaude));
        assert_eq!(processes.terminated.borrow().clone(), Vec::<i32>::new());
        assert_eq!(agterm.calls(), Vec::new());
    }

    #[test]
    fn happy_path_pins_then_types() {
        let paths = home_with_entry("happy");
        let project = paths
            .transcript_root(crate::ccmap::Profile::Personal)
            .join("projects/-Users-x-Projects-y");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join(format!("{CONV}.jsonl")),
            "{\"type\":\"assistant\",\"timestamp\":\"2026-09-01T00:00:00Z\",\"message\":{\"content\":\"done\"}}\n",
        )
        .unwrap();
        let agterm = FakeAgterm::new(vec![vec![live()], vec![shell()]], Failing::Nothing);
        let processes = FakeProcesses::new(true);
        let outcome = park(&paths, &agterm, &processes, &live(), Mode::Daemon);
        assert_eq!(outcome, ParkOutcome::Parked);
        assert_eq!(processes.terminated.borrow().clone(), vec![PID]);
        assert_eq!(
            agterm.calls(),
            vec![
                Call::Tree(WINDOW.to_string()),
                Call::Tree(WINDOW.to_string()),
                Call::Restore(
                    WINDOW.to_string(),
                    SESSION.to_string(),
                    "claude-siesta".to_string()
                ),
                Call::Type(
                    WINDOW.to_string(),
                    SESSION.to_string(),
                    " claude-siesta\n".to_string()
                ),
            ]
        );
        let written = state::read(&paths, &SessionId::new(SESSION)).unwrap();
        assert_eq!(
            written,
            ParkState {
                session: SessionId::new(SESSION),
                conv: crate::ccmap::ConvId::new(CONV),
                profile: crate::ccmap::Profile::Personal,
                parked_at: at(NOW),
                last_assistant_at: crate::time::parse_rfc3339("2026-09-01T00:00:00Z").unwrap(),
                placeholder_pid: None,
            }
        );
        assert_eq!(
            log::last_action(&paths, &SessionId::new(SESSION)),
            Some("park".to_string())
        );
    }

    #[test]
    fn manual_mode_parks_selected_session_and_falls_back_to_map_ts() {
        let paths = home_with_entry("manual");
        let agterm = FakeAgterm::new(vec![vec![shell()]], Failing::Nothing);
        let processes = FakeProcesses::new(true);
        let selected = session(&[CLAUDE], true);
        let outcome = park(&paths, &agterm, &processes, &selected, Mode::Manual);
        assert_eq!(outcome, ParkOutcome::Parked);
        let written = state::read(&paths, &SessionId::new(SESSION)).unwrap();
        assert_eq!(written.last_assistant_at, at(MAP_TS));
    }

    #[test]
    fn foreground_that_never_clears_stops_before_pinning() {
        let paths = home_with_entry("busy");
        let agterm = FakeAgterm::new(vec![vec![live()]], Failing::Nothing);
        let processes = FakeProcesses::new(true);
        let outcome = park(&paths, &agterm, &processes, &live(), Mode::Daemon);
        assert_eq!(
            outcome,
            ParkOutcome::Failed(ParkFailure {
                step: ParkStep::ForegroundBusy,
                detail: None,
            })
        );
        assert_eq!(
            agterm.calls(),
            vec![
                Call::Tree(WINDOW.to_string()),
                Call::Tree(WINDOW.to_string()),
                Call::Tree(WINDOW.to_string()),
            ]
        );
        assert!(!paths.state_file(&SessionId::new(SESSION)).exists());
        let log = std::fs::read_to_string(paths.log_file()).unwrap();
        assert!(log.contains(" park-failed "));
        assert!(log.contains("foreground-busy"));
    }

    #[test]
    fn vanished_session_fails_without_pinning() {
        let paths = home_with_entry("gone");
        let agterm = FakeAgterm::new(vec![Vec::new()], Failing::Nothing);
        let processes = FakeProcesses::new(true);
        let outcome = park(&paths, &agterm, &processes, &live(), Mode::Daemon);
        assert_eq!(
            outcome,
            ParkOutcome::Failed(ParkFailure {
                step: ParkStep::SessionGone,
                detail: None,
            })
        );
        assert_eq!(agterm.calls(), vec![Call::Tree(WINDOW.to_string())]);
    }

    #[test]
    fn restore_failure_logs_and_does_not_type() {
        let paths = home_with_entry("restore-fails");
        let agterm = FakeAgterm::new(vec![vec![shell()]], Failing::Restore);
        let processes = FakeProcesses::new(true);
        let outcome = park(&paths, &agterm, &processes, &live(), Mode::Daemon);
        let ParkOutcome::Failed(ParkFailure { step, detail: _ }) = outcome else {
            panic!("expected a failure, got {outcome:?}");
        };
        assert_eq!(step, ParkStep::Restore);
        let calls = agterm.calls();
        assert_eq!(
            calls.last(),
            Some(&Call::Restore(
                WINDOW.to_string(),
                SESSION.to_string(),
                "claude-siesta".to_string()
            ))
        );
        for call in &calls {
            if let Call::Type(..) = call {
                panic!("typed after a failed restore");
            }
        }
        assert!(!paths.state_file(&SessionId::new(SESSION)).exists());
        let log = std::fs::read_to_string(paths.log_file()).unwrap();
        assert!(log.contains(" park-failed "));
        assert!(log.contains("restore: agtermctl failed: no such session"));
        assert!(!log.contains("second"));
    }

    #[test]
    fn type_failure_logs_park_failed() {
        let paths = home_with_entry("type-fails");
        let agterm = FakeAgterm::new(vec![vec![shell()]], Failing::Type);
        let processes = FakeProcesses::new(true);
        let outcome = park(&paths, &agterm, &processes, &live(), Mode::Daemon);
        let ParkOutcome::Failed(ParkFailure { step, detail: _ }) = outcome else {
            panic!("expected a failure, got {outcome:?}");
        };
        assert_eq!(step, ParkStep::Type);
        assert_eq!(
            log::last_action(&paths, &SessionId::new(SESSION)),
            Some("park-failed".to_string())
        );
    }

    fn reap_in_background(
        child: std::process::Child,
    ) -> std::thread::JoinHandle<std::process::ExitStatus> {
        let mut child = child;
        std::thread::spawn(move || child.wait().unwrap())
    }

    const TEST_KILL_POLL: Poll = Poll {
        interval: Duration::from_millis(50),
        attempts: 10,
    };

    #[test]
    fn terminate_stops_a_child_with_sigterm() {
        let child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let reaper = reap_in_background(child);
        terminate(pid, TEST_KILL_POLL);
        let status = reaper.join().unwrap();
        assert_eq!(status.signal(), Some(Signal::SIGTERM as i32));
        assert!(kill(Pid::from_raw(pid), None).is_err());
    }

    #[test]
    fn terminate_sigkills_a_child_that_ignores_sigterm() {
        let mut child = std::process::Command::new("bash")
            .args(["-c", "trap '' TERM; echo ready; exec sleep 60"])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let mut ready = String::new();
        BufReader::new(stdout).read_line(&mut ready).unwrap();
        assert_eq!(ready.trim(), "ready");
        let pid = child.id() as i32;
        let reaper = reap_in_background(child);
        terminate(pid, TEST_KILL_POLL);
        let status = reaper.join().unwrap();
        assert_eq!(status.signal(), Some(Signal::SIGKILL as i32));
    }

    #[test]
    fn pid_is_claude_rejects_other_processes() {
        assert!(!pid_is_claude(std::process::id() as i32));
        assert!(!pid_is_claude(0));
        assert!(!pid_is_claude(-1));
    }
}
