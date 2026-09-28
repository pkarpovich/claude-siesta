use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, SystemTime};

use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};

use crate::agterm::Agterm;
use crate::ccmap::SessionId;
use crate::config::Config;
use crate::executable::Executable;
use crate::log::{Action, LogLine};
use crate::park::{self, FOREGROUND_POLL, ParkEnv, ParkOutcome, SystemProcesses, write_log};
use crate::paths::Paths;
use crate::rule::Mode;
use crate::state;

const STOP_CHECK: Duration = Duration::from_secs(1);

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn request_stop(_: nix::libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    pub parked: usize,
    pub skipped: usize,
    pub failed: usize,
    pub agterm_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    Elapsed,
    Stopped,
    Replaced,
}

pub fn run(paths: &Paths, config: Config) -> ExitCode {
    let dir = paths.state_dir();
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("claude-siesta: cannot create {}: {error}", dir.display());
        return ExitCode::from(1);
    }
    if let Err(error) = install_stop_handlers() {
        eprintln!("claude-siesta: cannot install signal handlers: {error}");
        return ExitCode::from(1);
    }
    let Config {
        park_after,
        poll_interval,
    } = config;
    write_log(
        paths,
        &LogLine {
            action: Action::Start,
            session: None,
            conv: None,
            idle: None,
            reason: format!(
                "park_after={}s poll_interval={}s",
                park_after.as_secs(),
                poll_interval.as_secs()
            ),
        },
    );
    let executable = Executable::current();
    let replaced = || match &executable {
        Some(executable) => executable.swapped(),
        None => false,
    };
    let agterm = Agterm::default();
    loop {
        let env = ParkEnv {
            agterm: &agterm,
            processes: &SystemProcesses,
            paths,
            config: &config,
            now: SystemTime::now(),
            foreground_poll: FOREGROUND_POLL,
        };
        tick(&env, &STOP);
        match wait(poll_interval, STOP_CHECK, &STOP, &replaced) {
            Wake::Elapsed => {}
            Wake::Stopped => return ExitCode::SUCCESS,
            Wake::Replaced => {
                write_log(
                    paths,
                    &LogLine {
                        action: Action::Retire,
                        session: None,
                        conv: None,
                        idle: None,
                        reason: "binary replaced".to_string(),
                    },
                );
                return ExitCode::SUCCESS;
            }
        }
    }
}

pub fn tick(env: &ParkEnv, stop: &AtomicBool) -> TickReport {
    let mut report = TickReport::default();
    let sessions = match env.agterm.all_sessions() {
        Ok(sessions) => sessions,
        Err(error) => {
            let message = error.to_string();
            write_log(env.paths, &tick_failed_line(message.clone()));
            report.agterm_error = Some(message);
            return report;
        }
    };
    for (window, session) in &sessions {
        if stop.load(Ordering::SeqCst) {
            return report;
        }
        match park::run(env, window, session, Mode::Daemon) {
            ParkOutcome::Parked => report.parked += 1,
            ParkOutcome::Skipped(_) => report.skipped += 1,
            ParkOutcome::Failed(_) => report.failed += 1,
        }
    }
    cleanup(env);
    report
}

fn cleanup(env: &ParkEnv) {
    let _lock = match state::lock_parking(env.paths) {
        Ok(lock) => lock,
        Err(error) => {
            write_log(env.paths, &tick_failed_line(format!("cleanup: {error}")));
            return;
        }
    };
    let sessions = match env.agterm.all_sessions() {
        Ok(sessions) => sessions,
        Err(error) => {
            write_log(env.paths, &tick_failed_line(format!("cleanup: {error}")));
            return;
        }
    };
    let mut seen = Vec::new();
    for (_, session) in sessions {
        seen.push(session.id);
    }
    for id in state::cleanup(env.paths, &seen) {
        write_log(env.paths, &cleanup_line(id));
    }
}

fn tick_failed_line(reason: String) -> LogLine {
    LogLine {
        action: Action::TickFailed,
        session: None,
        conv: None,
        idle: None,
        reason,
    }
}

fn cleanup_line(session: SessionId) -> LogLine {
    LogLine {
        action: Action::Cleanup,
        session: Some(session),
        conv: None,
        idle: None,
        reason: String::new(),
    }
}

pub fn wait(
    total: Duration,
    step: Duration,
    stop: &AtomicBool,
    replaced: &dyn Fn() -> bool,
) -> Wake {
    let mut waited = Duration::ZERO;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Wake::Stopped;
        }
        if replaced() {
            return Wake::Replaced;
        }
        if waited >= total {
            return Wake::Elapsed;
        }
        let nap = step.min(total - waited);
        sleep(nap);
        waited += nap;
    }
}

fn install_stop_handlers() -> nix::Result<()> {
    let action = SigAction::new(
        SigHandler::Handler(request_stop),
        SaFlags::empty(),
        SigSet::empty(),
    );
    for signal in [Signal::SIGTERM, Signal::SIGINT] {
        unsafe { sigaction(signal, &action) }?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::time::Instant;

    use super::*;
    use crate::agterm::{AgtermError, AgtermOps};
    use crate::ccmap::{ConvId, Profile};
    use crate::state::ParkState;
    use crate::tree::{AgentStatus, Session, Window, WindowId};

    const WINDOW_A: &str = "AAAAAAAA-0000-0000-0000-000000000001";
    const WINDOW_B: &str = "BBBBBBBB-0000-0000-0000-000000000002";
    const WINDOW_CLOSED: &str = "CCCCCCCC-0000-0000-0000-000000000003";
    const PARKABLE: &str = "10000000-0000-0000-0000-000000000001";
    const SELECTED: &str = "20000000-0000-0000-0000-000000000002";
    const SHELL: &str = "30000000-0000-0000-0000-000000000003";
    const FLAGGED: &str = "40000000-0000-0000-0000-000000000004";
    const WORKING: &str = "50000000-0000-0000-0000-000000000005";
    const FRESH: &str = "60000000-0000-0000-0000-000000000006";
    const VANISHED: &str = "70000000-0000-0000-0000-000000000007";
    const BUSY: &str = "80000000-0000-0000-0000-000000000008";
    const NOW: u64 = 1_790_500_000;
    const DAY: u64 = 24 * 60 * 60;
    const CLAUDE: &str = "/home/x/.local/bin/claude";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Failing {
        Nothing,
        Windows,
        Tree,
    }

    struct FakeAgterm {
        trees: RefCell<HashMap<String, Vec<Vec<Session>>>>,
        failing: Failing,
        restored: RefCell<Vec<String>>,
        typed: RefCell<Vec<String>>,
    }

    impl FakeAgterm {
        fn new(trees: Vec<(&str, Vec<Vec<Session>>)>, failing: Failing) -> FakeAgterm {
            let mut map = HashMap::new();
            for (window, responses) in trees {
                map.insert(window.to_string(), responses);
            }
            FakeAgterm {
                trees: RefCell::new(map),
                failing,
                restored: RefCell::new(Vec::new()),
                typed: RefCell::new(Vec::new()),
            }
        }
    }

    impl AgtermOps for FakeAgterm {
        fn windows(&self) -> Result<Vec<Window>, AgtermError> {
            match self.failing {
                Failing::Windows => {
                    return Err(AgtermError::Failed("agterm is not running".to_string()));
                }
                Failing::Tree => {}
                Failing::Nothing => {}
            }
            Ok(vec![
                Window {
                    id: WindowId::new(WINDOW_A),
                    open: true,
                },
                Window {
                    id: WindowId::new(WINDOW_CLOSED),
                    open: false,
                },
                Window {
                    id: WindowId::new(WINDOW_B),
                    open: true,
                },
            ])
        }

        fn tree(&self, window: &WindowId) -> Result<Vec<Session>, AgtermError> {
            match self.failing {
                Failing::Tree => return Err(AgtermError::Failed("socket closed".to_string())),
                Failing::Windows => {}
                Failing::Nothing => {}
            }
            let mut trees = self.trees.borrow_mut();
            let Some(responses) = trees.get_mut(window.as_str()) else {
                panic!("tree asked for unexpected window {}", window.as_str());
            };
            match responses.len() {
                0 => Err(AgtermError::Failed("no tree".to_string())),
                1 => Ok(responses[0].clone()),
                _ => Ok(responses.remove(0)),
            }
        }

        fn restore(
            &self,
            _window: &WindowId,
            session: &SessionId,
            _command: &str,
        ) -> Result<(), AgtermError> {
            self.restored
                .borrow_mut()
                .push(session.as_str().to_string());
            Ok(())
        }

        fn type_text(
            &self,
            _window: &WindowId,
            session: &SessionId,
            _text: &str,
        ) -> Result<(), AgtermError> {
            self.typed.borrow_mut().push(session.as_str().to_string());
            Ok(())
        }
    }

    struct FakeProcesses {
        terminated: RefCell<Vec<i32>>,
    }

    impl park::ProcessOps for FakeProcesses {
        fn is_claude(&self, _pid: i32) -> bool {
            true
        }

        fn terminate(&self, pid: i32) {
            self.terminated.borrow_mut().push(pid);
        }
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn temp_home(name: &str) -> Paths {
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "claude-siesta-daemon-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Paths::from_home(dir)
    }

    fn map(paths: &Paths, session: &str, pid: i32, ts: u64) {
        let dir = paths.cc_map_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(session),
            format!(
                r#"{{"conv":"conv-{pid}","profile":"personal","cwd":"/home/x/y","ts":{ts},"pid":{pid}}}"#
            ),
        )
        .unwrap();
    }

    fn parked_state(session: &str) -> ParkState {
        ParkState {
            session: SessionId::new(session),
            conv: ConvId::new("conv-old"),
            profile: Profile::Personal,
            parked_at: at(NOW - DAY),
            last_assistant_at: at(NOW - 2 * DAY),
            placeholder_pid: None,
        }
    }

    fn session(id: &str, foreground: &[&str]) -> Session {
        let mut argv = Vec::new();
        for arg in foreground {
            argv.push(arg.to_string());
        }
        Session {
            id: SessionId::new(id),
            name: id.to_string(),
            active: false,
            flagged: false,
            split: false,
            foreground: argv,
            status: AgentStatus::Idle,
            restore_command: None,
        }
    }

    fn live(id: &str) -> Session {
        session(id, &[CLAUDE, "--resume", "conv"])
    }

    fn run_tick(paths: &Paths, agterm: &FakeAgterm, processes: &FakeProcesses) -> TickReport {
        run_tick_with_stop(paths, agterm, processes, &AtomicBool::new(false))
    }

    fn run_tick_with_stop(
        paths: &Paths,
        agterm: &FakeAgterm,
        processes: &FakeProcesses,
        stop: &AtomicBool,
    ) -> TickReport {
        let config = Config::default();
        let env = ParkEnv {
            agterm,
            processes,
            paths,
            config: &config,
            now: at(NOW),
            foreground_poll: park::Poll {
                interval: Duration::ZERO,
                attempts: 3,
            },
        };
        tick(&env, stop)
    }

    #[test]
    fn tick_parks_exactly_one_session_and_cleans_vanished_state() {
        let paths = temp_home("tick");
        map(&paths, PARKABLE, 101, NOW - DAY);
        map(&paths, SELECTED, 102, NOW - DAY);
        map(&paths, FLAGGED, 104, NOW - DAY);
        map(&paths, WORKING, 105, NOW - DAY);
        map(&paths, FRESH, 106, NOW - 5 * 60);
        state::write(&paths, &parked_state(SHELL)).unwrap();
        state::write(&paths, &parked_state(VANISHED)).unwrap();

        let selected = Session {
            active: true,
            ..live(SELECTED)
        };
        let flagged = Session {
            flagged: true,
            ..live(FLAGGED)
        };
        let working = Session {
            status: AgentStatus::Active,
            ..live(WORKING)
        };
        let window_a = vec![live(PARKABLE), selected.clone(), session(SHELL, &[])];
        let window_a_after_kill = vec![session(PARKABLE, &[]), selected, session(SHELL, &[])];
        let window_b = vec![flagged, working, live(FRESH)];
        let agterm = FakeAgterm::new(
            vec![
                (
                    WINDOW_A,
                    vec![window_a.clone(), window_a, window_a_after_kill],
                ),
                (WINDOW_B, vec![window_b]),
            ],
            Failing::Nothing,
        );
        let processes = FakeProcesses {
            terminated: RefCell::new(Vec::new()),
        };

        let report = run_tick(&paths, &agterm, &processes);

        assert_eq!(
            report,
            TickReport {
                parked: 1,
                skipped: 5,
                failed: 0,
                agterm_error: None,
            }
        );
        assert_eq!(processes.terminated.borrow().clone(), vec![101]);
        assert_eq!(agterm.restored.borrow().clone(), vec![PARKABLE.to_string()]);
        assert_eq!(agterm.typed.borrow().clone(), vec![PARKABLE.to_string()]);
        assert!(paths.state_file(&SessionId::new(PARKABLE)).exists());
        assert!(paths.state_file(&SessionId::new(SHELL)).exists());
        assert!(!paths.state_file(&SessionId::new(VANISHED)).exists());
        let log = std::fs::read_to_string(paths.log_file()).unwrap();
        assert!(log.contains(&format!(" cleanup {VANISHED} ")));
        assert!(log.contains(&format!(" park {PARKABLE} ")));
        assert!(log.contains("selected"));
        assert!(log.contains("flagged"));
        assert!(!log.contains(&format!(" skip {SHELL} ")));
    }

    #[test]
    fn windows_failure_ends_the_tick_without_cleanup() {
        let paths = temp_home("windows-fail");
        state::write(&paths, &parked_state(VANISHED)).unwrap();
        let agterm = FakeAgterm::new(Vec::new(), Failing::Windows);
        let processes = FakeProcesses {
            terminated: RefCell::new(Vec::new()),
        };

        let report = run_tick(&paths, &agterm, &processes);

        assert_eq!(
            report,
            TickReport {
                parked: 0,
                skipped: 0,
                failed: 0,
                agterm_error: Some("agtermctl failed: agterm is not running".to_string()),
            }
        );
        assert!(paths.state_file(&SessionId::new(VANISHED)).exists());
        let log = std::fs::read_to_string(paths.log_file()).unwrap();
        assert_eq!(log.lines().count(), 1);
        assert!(log.contains(" tick-failed - - - agtermctl failed: agterm is not running"));
    }

    #[test]
    fn tree_failure_ends_the_tick_without_parking() {
        let paths = temp_home("tree-fail");
        map(&paths, PARKABLE, 101, NOW - DAY);
        state::write(&paths, &parked_state(VANISHED)).unwrap();
        let agterm = FakeAgterm::new(vec![(WINDOW_A, vec![vec![live(PARKABLE)]])], Failing::Tree);
        let processes = FakeProcesses {
            terminated: RefCell::new(Vec::new()),
        };

        let report = run_tick(&paths, &agterm, &processes);

        assert_eq!(report.parked, 0);
        assert_eq!(
            report.agterm_error,
            Some("agtermctl failed: socket closed".to_string())
        );
        assert_eq!(processes.terminated.borrow().clone(), Vec::<i32>::new());
        assert!(paths.state_file(&SessionId::new(VANISHED)).exists());
    }

    #[test]
    fn tick_continues_after_a_failed_park() {
        let paths = temp_home("failed-park");
        map(&paths, BUSY, 108, NOW - DAY);
        map(&paths, PARKABLE, 101, NOW - DAY);
        state::write(&paths, &parked_state(VANISHED)).unwrap();
        let agterm = FakeAgterm::new(
            vec![
                (WINDOW_A, vec![vec![live(BUSY)]]),
                (
                    WINDOW_B,
                    vec![
                        vec![live(PARKABLE)],
                        vec![live(PARKABLE)],
                        vec![session(PARKABLE, &[])],
                    ],
                ),
            ],
            Failing::Nothing,
        );
        let processes = FakeProcesses {
            terminated: RefCell::new(Vec::new()),
        };

        let report = run_tick(&paths, &agterm, &processes);

        assert_eq!(
            report,
            TickReport {
                parked: 1,
                skipped: 0,
                failed: 1,
                agterm_error: None,
            }
        );
        assert_eq!(processes.terminated.borrow().clone(), vec![108, 101]);
        assert_eq!(agterm.restored.borrow().clone(), vec![PARKABLE.to_string()]);
        assert!(!paths.state_file(&SessionId::new(VANISHED)).exists());
    }

    #[test]
    fn cleanup_keeps_state_of_a_session_that_appeared_during_the_tick() {
        let paths = temp_home("appeared");
        state::write(&paths, &parked_state(FRESH)).unwrap();
        state::write(&paths, &parked_state(VANISHED)).unwrap();
        let agterm = FakeAgterm::new(
            vec![
                (WINDOW_A, vec![Vec::new(), vec![session(FRESH, &[])]]),
                (WINDOW_B, vec![Vec::new()]),
            ],
            Failing::Nothing,
        );
        let processes = FakeProcesses {
            terminated: RefCell::new(Vec::new()),
        };

        run_tick(&paths, &agterm, &processes);

        assert!(paths.state_file(&SessionId::new(FRESH)).exists());
        assert!(!paths.state_file(&SessionId::new(VANISHED)).exists());
    }

    #[test]
    fn stop_during_tick_parks_nothing_more_and_skips_cleanup() {
        let paths = temp_home("stop-tick");
        map(&paths, PARKABLE, 101, NOW - DAY);
        state::write(&paths, &parked_state(VANISHED)).unwrap();
        let agterm = FakeAgterm::new(
            vec![
                (WINDOW_A, vec![vec![live(PARKABLE)]]),
                (WINDOW_B, vec![Vec::new()]),
            ],
            Failing::Nothing,
        );
        let processes = FakeProcesses {
            terminated: RefCell::new(Vec::new()),
        };

        let report = run_tick_with_stop(&paths, &agterm, &processes, &AtomicBool::new(true));

        assert_eq!(report, TickReport::default());
        assert_eq!(processes.terminated.borrow().clone(), Vec::<i32>::new());
        assert!(paths.state_file(&SessionId::new(VANISHED)).exists());
    }

    #[test]
    fn wait_returns_at_once_when_stop_is_set() {
        let stop = AtomicBool::new(true);
        let started = Instant::now();
        assert_eq!(
            wait(
                Duration::from_secs(60),
                Duration::from_secs(1),
                &stop,
                &|| false
            ),
            Wake::Stopped
        );
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn wait_returns_at_once_when_the_binary_is_replaced() {
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        assert_eq!(
            wait(
                Duration::from_secs(60),
                Duration::from_secs(1),
                &stop,
                &|| true
            ),
            Wake::Replaced
        );
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn wait_elapses_without_stop() {
        let stop = AtomicBool::new(false);
        let started = Instant::now();
        assert_eq!(
            wait(
                Duration::from_millis(30),
                Duration::from_millis(10),
                &stop,
                &|| false
            ),
            Wake::Elapsed
        );
        assert!(started.elapsed() >= Duration::from_millis(30));
    }

    #[test]
    fn wait_notices_a_stop_set_while_sleeping() {
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let setter = std::sync::Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            sleep(Duration::from_millis(30));
            setter.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        assert_eq!(
            wait(
                Duration::from_secs(60),
                Duration::from_millis(10),
                &stop,
                &|| false
            ),
            Wake::Stopped
        );
        handle.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
