use std::fs::OpenOptions;
use std::io::{self, Write};
use std::time::{Duration, SystemTime};

use crate::ccmap::{ConvId, SessionId};
use crate::paths::Paths;
use crate::time::format_local_rfc3339;
use crate::transcript::{TailStart, read_last_bytes};

const LAST_ACTION_TAIL_BYTES: u64 = 64 * 1024;
const EMPTY_FIELD: &str = "-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Start,
    Park,
    Skip,
    ParkFailed,
    Cleanup,
    TickFailed,
    Retire,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Start => "start",
            Action::Park => "park",
            Action::Skip => "skip",
            Action::ParkFailed => "park-failed",
            Action::Cleanup => "cleanup",
            Action::TickFailed => "tick-failed",
            Action::Retire => "retire",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub action: Action,
    pub session: Option<SessionId>,
    pub conv: Option<ConvId>,
    pub idle: Option<Duration>,
    pub reason: String,
}

pub fn format_line(line: &LogLine, now: SystemTime) -> String {
    let LogLine {
        action,
        session,
        conv,
        idle,
        reason,
    } = line;
    let session = match session {
        Some(session) => session.as_str(),
        None => EMPTY_FIELD,
    };
    let conv = match conv {
        Some(conv) => conv.as_str(),
        None => EMPTY_FIELD,
    };
    let idle = match idle {
        Some(idle) => format!("{}m", idle.as_secs() / 60),
        None => String::from(EMPTY_FIELD),
    };
    let mut words = Vec::new();
    for word in reason.split_whitespace() {
        words.push(word);
    }
    let reason = match words.is_empty() {
        true => String::from(EMPTY_FIELD),
        false => words.join(" "),
    };
    format!(
        "{} {} {session} {conv} {idle} {reason}\n",
        format_local_rfc3339(now),
        action.as_str()
    )
}

pub fn append(paths: &Paths, line: &LogLine) -> io::Result<()> {
    std::fs::create_dir_all(paths.state_dir())?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.log_file())?;
    file.write_all(format_line(line, SystemTime::now()).as_bytes())
}

pub fn last_action(paths: &Paths, session: &SessionId) -> Option<String> {
    let (bytes, start) = read_last_bytes(&paths.log_file(), LAST_ACTION_TAIL_BYTES).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines = text.split('\n');
    match start {
        TailStart::FileStart => {}
        TailStart::MidFile => {
            lines.next();
        }
    }
    let mut found = None;
    for line in lines {
        let mut fields = line.split(' ');
        let (Some(_), Some(action), Some(id)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if id != session.as_str() {
            continue;
        }
        found = Some(action.to_string());
    }
    found
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::time::parse_rfc3339;

    fn temp_home(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("claude-siesta-log-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn line(action: Action, session: &str, reason: &str) -> LogLine {
        LogLine {
            action,
            session: Some(SessionId::new(session)),
            conv: Some(ConvId::new("c0acdbe6")),
            idle: Some(Duration::from_secs(132 * 60 + 59)),
            reason: reason.to_string(),
        }
    }

    #[test]
    fn action_names() {
        assert_eq!(Action::Start.as_str(), "start");
        assert_eq!(Action::Park.as_str(), "park");
        assert_eq!(Action::Skip.as_str(), "skip");
        assert_eq!(Action::ParkFailed.as_str(), "park-failed");
        assert_eq!(Action::Cleanup.as_str(), "cleanup");
        assert_eq!(Action::TickFailed.as_str(), "tick-failed");
    }

    #[test]
    fn format_line_has_only_documented_fields() {
        let text = format_line(
            &line(
                Action::ParkFailed,
                "7320056a-aaaa",
                "restore: agterm said no",
            ),
            at(1790500000),
        );
        assert!(text.ends_with('\n'), "{text}");
        assert_eq!(text.matches('\n').count(), 1, "{text}");
        let mut fields = Vec::new();
        for field in text.trim_end().splitn(6, ' ') {
            fields.push(field);
        }
        assert_eq!(fields.len(), 6, "{text}");
        assert_eq!(parse_rfc3339(fields[0]), Some(at(1790500000)));
        assert_eq!(fields[1], "park-failed");
        assert_eq!(fields[2], "7320056A-AAAA");
        assert_eq!(fields[3], "c0acdbe6");
        assert_eq!(fields[4], "132m");
        assert_eq!(fields[5], "restore: agterm said no");
    }

    #[test]
    fn format_line_uses_placeholders_for_missing_fields() {
        let start = LogLine {
            action: Action::Start,
            session: None,
            conv: None,
            idle: None,
            reason: String::new(),
        };
        let text = format_line(&start, at(0));
        let mut fields = Vec::new();
        for field in text.trim_end().split(' ') {
            fields.push(field);
        }
        assert_eq!(fields[1..], ["start", "-", "-", "-", "-"]);
    }

    #[test]
    fn format_line_flattens_multiline_reason() {
        let text = format_line(
            &line(Action::ParkFailed, "AAAA", "first line\nsecond\tline\r\n"),
            at(0),
        );
        assert_eq!(text.matches('\n').count(), 1, "{text}");
        assert!(text.ends_with(" first line second line\n"), "{text}");
    }

    #[test]
    fn append_writes_lines_in_order() {
        let home = temp_home("append");
        let paths = Paths::from_home(home.clone());
        append(&paths, &line(Action::Skip, "AAAA", "selected")).unwrap();
        append(&paths, &line(Action::Park, "AAAA", "")).unwrap();
        let text = std::fs::read_to_string(paths.log_file()).unwrap();
        let mut lines = Vec::new();
        for line in text.lines() {
            lines.push(line);
        }
        assert_eq!(lines.len(), 2, "{text}");
        assert!(lines[0].contains(" skip AAAA "), "{text}");
        assert!(lines[1].contains(" park AAAA "), "{text}");
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn last_action_picks_newest_line_for_the_id() {
        let home = temp_home("last-action");
        let paths = Paths::from_home(home.clone());
        assert_eq!(last_action(&paths, &SessionId::new("AAAA")), None);
        append(&paths, &line(Action::Skip, "AAAA", "selected")).unwrap();
        append(&paths, &line(Action::Park, "BBBB", "")).unwrap();
        append(&paths, &line(Action::ParkFailed, "AAAA", "foreground-busy")).unwrap();
        append(&paths, &line(Action::Cleanup, "BBBB", "")).unwrap();
        assert_eq!(
            last_action(&paths, &SessionId::new("aaaa")),
            Some(String::from("park-failed"))
        );
        assert_eq!(
            last_action(&paths, &SessionId::new("BBBB")),
            Some(String::from("cleanup"))
        );
        assert_eq!(last_action(&paths, &SessionId::new("CCCC")), None);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn last_action_reads_only_the_tail() {
        let home = temp_home("last-action-tail");
        let paths = Paths::from_home(home.clone());
        append(&paths, &line(Action::Park, "OLD", "")).unwrap();
        let filler = format_line(&line(Action::Skip, "BBBB", "selected"), at(0));
        let mut text = std::fs::read_to_string(paths.log_file()).unwrap();
        while (text.len() as u64) < LAST_ACTION_TAIL_BYTES * 2 {
            text.push_str(&filler);
        }
        text.push_str(&format_line(&line(Action::Park, "NEW", ""), at(0)));
        std::fs::write(paths.log_file(), text).unwrap();
        assert_eq!(last_action(&paths, &SessionId::new("OLD")), None);
        assert_eq!(
            last_action(&paths, &SessionId::new("NEW")),
            Some(String::from("park"))
        );
        std::fs::remove_dir_all(home).unwrap();
    }
}
