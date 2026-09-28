use std::process::ExitCode;
use std::time::SystemTime;

use claude_siesta::agterm::{Agterm, AgtermOps};
use claude_siesta::cli::{self, Command, SessionQuery};
use claude_siesta::config::Config;
use claude_siesta::daemon;
use claude_siesta::park::{self, FOREGROUND_POLL, ParkEnv, ParkOutcome, SystemProcesses};
use claude_siesta::paths::Paths;
use claude_siesta::placeholder;
use claude_siesta::resume;
use claude_siesta::rule::Mode;
use claude_siesta::service::{self, Installed};
use claude_siesta::status;

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
        Command::Resume(query) => resume_now(query),
        Command::Status => show_status(),
        Command::Install => install_service(),
        Command::Uninstall => uninstall_service(),
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

fn resume_now(query: SessionQuery) -> ExitCode {
    let Some(paths) = Paths::from_env() else {
        eprintln!("claude-siesta: HOME is not set");
        return ExitCode::from(1);
    };
    let SessionQuery(query) = query;
    let (_, session) = match Agterm::default().resolve_prefix(&query) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            return ExitCode::from(1);
        }
    };
    match resume::signal_placeholder(&paths, &session.id) {
        Ok(pid) => {
            println!("resumed {} (placeholder pid {pid})", session.id.as_str());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            ExitCode::from(1)
        }
    }
}

fn show_status() -> ExitCode {
    let Some(paths) = Paths::from_env() else {
        eprintln!("claude-siesta: HOME is not set");
        return ExitCode::from(1);
    };
    match status::gather(&Agterm::default(), &paths, SystemTime::now()) {
        Ok(rows) => {
            print!("{}", status::format_table(&rows));
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            ExitCode::from(1)
        }
    }
}

fn install_service() -> ExitCode {
    let Some(paths) = Paths::from_env() else {
        eprintln!("claude-siesta: HOME is not set");
        return ExitCode::from(1);
    };
    match service::install(&paths) {
        Ok(Installed { program, agent }) => {
            println!(
                "loaded {} from {} running {}",
                service::LABEL,
                agent.display(),
                program.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            ExitCode::from(1)
        }
    }
}

fn uninstall_service() -> ExitCode {
    let Some(paths) = Paths::from_env() else {
        eprintln!("claude-siesta: HOME is not set");
        return ExitCode::from(1);
    };
    match service::uninstall(&paths) {
        Ok(layout) => {
            println!(
                "unloaded {} and removed {}",
                service::LABEL,
                layout.agent.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("claude-siesta: {error}");
            ExitCode::from(1)
        }
    }
}
