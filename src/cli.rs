use std::fmt;

pub const USAGE: &str = "usage: claude-siesta [daemon | park <id|prefix> | resume <id|prefix> | status | install | uninstall]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionQuery(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Placeholder,
    Daemon,
    Park(SessionQuery),
    Resume(SessionQuery),
    Status,
    Install,
    Uninstall,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UsageError {
    UnknownSubcommand(String),
    MissingArgument(&'static str),
    UnexpectedArgument(String),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsageError::UnknownSubcommand(name) => write!(f, "unknown subcommand: {name}"),
            UsageError::MissingArgument(subcommand) => {
                write!(f, "{subcommand}: missing <id|prefix>")
            }
            UsageError::UnexpectedArgument(arg) => write!(f, "unexpected argument: {arg}"),
        }
    }
}

/// Parses the arguments that follow the program name.
pub fn parse_args(args: &[String]) -> Result<Command, UsageError> {
    let Some((subcommand, rest)) = args.split_first() else {
        return Ok(Command::Placeholder);
    };
    match subcommand.as_str() {
        "daemon" => no_more(rest, Command::Daemon),
        "status" => no_more(rest, Command::Status),
        "install" => no_more(rest, Command::Install),
        "uninstall" => no_more(rest, Command::Uninstall),
        "park" => {
            let (query, rest) = query_argument("park", rest)?;
            no_more(rest, Command::Park(query))
        }
        "resume" => {
            let (query, rest) = query_argument("resume", rest)?;
            no_more(rest, Command::Resume(query))
        }
        other => Err(UsageError::UnknownSubcommand(other.to_string())),
    }
}

fn query_argument<'a>(
    subcommand: &'static str,
    args: &'a [String],
) -> Result<(SessionQuery, &'a [String]), UsageError> {
    let Some((query, rest)) = args.split_first() else {
        return Err(UsageError::MissingArgument(subcommand));
    };
    if query.is_empty() {
        return Err(UsageError::MissingArgument(subcommand));
    }
    Ok((SessionQuery(query.clone()), rest))
}

fn no_more(rest: &[String], command: Command) -> Result<Command, UsageError> {
    let Some(extra) = rest.first() else {
        return Ok(command);
    };
    Err(UsageError::UnexpectedArgument(extra.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        let mut out = Vec::new();
        for item in list {
            out.push(item.to_string());
        }
        out
    }

    #[test]
    fn no_arguments_is_placeholder() {
        assert_eq!(parse_args(&args(&[])), Ok(Command::Placeholder));
    }

    #[test]
    fn daemon() {
        assert_eq!(parse_args(&args(&["daemon"])), Ok(Command::Daemon));
    }

    #[test]
    fn status() {
        assert_eq!(parse_args(&args(&["status"])), Ok(Command::Status));
    }

    #[test]
    fn install_and_uninstall() {
        assert_eq!(parse_args(&args(&["install"])), Ok(Command::Install));
        assert_eq!(parse_args(&args(&["uninstall"])), Ok(Command::Uninstall));
        assert_eq!(
            parse_args(&args(&["install", "extra"])),
            Err(UsageError::UnexpectedArgument("extra".to_string()))
        );
    }

    #[test]
    fn park_with_prefix() {
        assert_eq!(
            parse_args(&args(&["park", "7320"])),
            Ok(Command::Park(SessionQuery("7320".to_string())))
        );
    }

    #[test]
    fn resume_with_id() {
        assert_eq!(
            parse_args(&args(&["resume", "7320056A-1111"])),
            Ok(Command::Resume(SessionQuery("7320056A-1111".to_string())))
        );
    }

    #[test]
    fn park_missing_argument() {
        assert_eq!(
            parse_args(&args(&["park"])),
            Err(UsageError::MissingArgument("park"))
        );
    }

    #[test]
    fn resume_missing_argument() {
        assert_eq!(
            parse_args(&args(&["resume"])),
            Err(UsageError::MissingArgument("resume"))
        );
    }

    #[test]
    fn empty_argument_is_missing() {
        assert_eq!(
            parse_args(&args(&["park", ""])),
            Err(UsageError::MissingArgument("park"))
        );
    }

    #[test]
    fn unknown_subcommand() {
        assert_eq!(
            parse_args(&args(&["sleep"])),
            Err(UsageError::UnknownSubcommand("sleep".to_string()))
        );
    }

    #[test]
    fn extra_argument_rejected() {
        assert_eq!(
            parse_args(&args(&["daemon", "now"])),
            Err(UsageError::UnexpectedArgument("now".to_string()))
        );
        assert_eq!(
            parse_args(&args(&["park", "7320", "extra"])),
            Err(UsageError::UnexpectedArgument("extra".to_string()))
        );
    }
}
