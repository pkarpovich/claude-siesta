use std::process::ExitCode;
use std::time::SystemTime;

use claude_siesta::agterm::{Agterm, AgtermOps};
use claude_siesta::cli::{self, Command, SessionQuery};
use claude_siesta::config::Config;
use claude_siesta::daemon;
use claude_siesta::park::{self, FOREGROUND_POLL, ParkEnv, ParkOutcome, SystemProcesses};
use claude_siesta::paths::Paths;
use claude_siesta::placeholder;
use claude_siesta::rule::Mode;

fn main() -> ExitCode {
    let mut args = Vec::new();
    for arg in std::env::args().skip(1) {
        args.push(arg);
    }
    let command = match cli::parse_args(&args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            eprintln!("{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match command {
        Command::Placeholder => run_placeholder(),
        Command::Daemon => run_daemon(),
        Command::Park(query) => park_now(query),
        Command::Resume(_) => not_implemented("resume"),
        Command::Status => not_implemented("status"),
    }
}

fn setup() -> Result<(Paths, Config), ExitCode> {
    let Some(paths) = Paths::from_env() else {
        eprintln!("claude-siesta: HOME is not set");
        return Err(ExitCode::from(1));
    };
    match Config::load(&paths) {
        Ok(config) => Ok((paths, config)),
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            Err(ExitCode::from(2))
        }
    }
}

fn run_placeholder() -> ExitCode {
    let Some(paths) = Paths::from_env() else {
        eprintln!("claude-siesta: HOME is not set");
        return ExitCode::from(1);
    };
    placeholder::run(&paths)
}

fn run_daemon() -> ExitCode {
    let (paths, config) = match setup() {
        Ok(setup) => setup,
        Err(code) => return code,
    };
    daemon::run(&paths, config)
}

fn park_now(query: SessionQuery) -> ExitCode {
    let (paths, config) = match setup() {
        Ok(setup) => setup,
        Err(code) => return code,
    };
    let SessionQuery(query) = query;
    let agterm = Agterm::default();
    let (window, session) = match agterm.resolve_prefix(&query) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            return ExitCode::from(1);
        }
    };
    let env = ParkEnv {
        agterm: &agterm,
        processes: &SystemProcesses,
        paths: &paths,
        config: &config,
        now: SystemTime::now(),
        foreground_poll: FOREGROUND_POLL,
    };
    match park::run(&env, &window, &session, Mode::Manual) {
        ParkOutcome::Parked => {
            println!("parked {}", session.id.as_str());
            ExitCode::SUCCESS
        }
        ParkOutcome::Skipped(reason) => {
            eprintln!(
                "claude-siesta: {} not parked: {}",
                session.id.as_str(),
                reason.as_str()
            );
            ExitCode::from(1)
        }
        ParkOutcome::Failed(failure) => {
            eprintln!(
                "claude-siesta: {} park failed: {}",
                session.id.as_str(),
                failure.reason()
            );
            ExitCode::from(1)
        }
    }
}

fn not_implemented(name: &str) -> ExitCode {
    eprintln!("claude-siesta: {name}: not implemented");
    ExitCode::from(1)
}
