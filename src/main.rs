use std::process::ExitCode;

use claude_siesta::cli::{self, Command};

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
        Command::Placeholder => not_implemented("placeholder"),
        Command::Daemon => not_implemented("daemon"),
        Command::Park(_) => not_implemented("park"),
        Command::Resume(_) => not_implemented("resume"),
        Command::Status => not_implemented("status"),
    }
}

fn not_implemented(name: &str) -> ExitCode {
    eprintln!("claude-siesta: {name}: not implemented");
    ExitCode::from(1)
}
