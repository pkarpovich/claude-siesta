use std::time::Duration;

use crate::ccmap::MapEntry;
use crate::tree::{AgentStatus, Session};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Daemon,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    NotClaude,
    NotMapped,
    PidNotClaude,
    AgentWorking,
    Flagged,
    Selected,
    NotIdleEnough,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SkipReason::NotClaude => "not-claude",
            SkipReason::NotMapped => "not-mapped",
            SkipReason::PidNotClaude => "pid-not-claude",
            SkipReason::AgentWorking => "agent-working",
            SkipReason::Flagged => "flagged",
            SkipReason::Selected => "selected",
            SkipReason::NotIdleEnough => "not-idle-enough",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Park,
    Skip(SkipReason),
}

#[derive(Debug, Clone, Copy)]
pub struct RuleInput<'a> {
    pub session: &'a Session,
    pub entry: Option<&'a MapEntry>,
    pub pid_is_claude: bool,
    pub idle: Duration,
    pub park_after: Duration,
    pub mode: Mode,
}

pub fn decide(input: &RuleInput) -> Decision {
    let RuleInput {
        session,
        entry,
        pid_is_claude,
        idle,
        park_after,
        mode,
    } = *input;
    if !session.runs_claude() {
        return Decision::Skip(SkipReason::NotClaude);
    }
    let Some(entry) = entry else {
        return Decision::Skip(SkipReason::NotMapped);
    };
    let Some(_) = entry.pid else {
        return Decision::Skip(SkipReason::NotMapped);
    };
    if !pid_is_claude {
        return Decision::Skip(SkipReason::PidNotClaude);
    }
    match session.status {
        AgentStatus::Active => return Decision::Skip(SkipReason::AgentWorking),
        AgentStatus::Idle => {}
        AgentStatus::Completed => {}
        AgentStatus::Blocked => {}
    }
    if session.flagged {
        return Decision::Skip(SkipReason::Flagged);
    }
    match mode {
        Mode::Manual => Decision::Park,
        Mode::Daemon => {
            if session.active {
                return Decision::Skip(SkipReason::Selected);
            }
            if idle < park_after {
                return Decision::Skip(SkipReason::NotIdleEnough);
            }
            Decision::Park
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use super::*;
    use crate::ccmap::{ConvId, Profile, SessionId};

    const HOUR: Duration = Duration::from_secs(3600);
    const PARK_AFTER: Duration = Duration::from_secs(2 * 3600);

    #[derive(Clone, Copy)]
    enum Foreground {
        Claude,
        Shell,
        Placeholder,
        Editor,
    }

    #[derive(Clone, Copy)]
    enum Mapping {
        WithPid,
        WithoutPid,
        Missing,
    }

    #[derive(Clone, Copy)]
    struct Row {
        name: &'static str,
        foreground: Foreground,
        mapping: Mapping,
        pid_is_claude: bool,
        status: AgentStatus,
        flagged: bool,
        selected: bool,
        idle: Duration,
        mode: Mode,
        expected: Decision,
    }

    const PARKABLE: Row = Row {
        name: "all checks pass",
        foreground: Foreground::Claude,
        mapping: Mapping::WithPid,
        pid_is_claude: true,
        status: AgentStatus::Completed,
        flagged: false,
        selected: false,
        idle: Duration::from_secs(24 * 3600),
        mode: Mode::Daemon,
        expected: Decision::Park,
    };

    fn session(row: &Row) -> Session {
        let Row {
            foreground,
            status,
            flagged,
            selected,
            ..
        } = *row;
        let foreground = match foreground {
            Foreground::Claude => vec![
                "/Users/x/.local/bin/claude".to_string(),
                "--resume".to_string(),
                "c1".to_string(),
            ],
            Foreground::Shell => Vec::new(),
            Foreground::Placeholder => vec!["/Users/x/.local/bin/claude-siesta".to_string()],
            Foreground::Editor => vec!["vim".to_string()],
        };
        Session {
            id: SessionId::new("7320056A-2D3F-4B06-AB9A-1CB79BED0F3A"),
            name: "test".to_string(),
            active: selected,
            flagged,
            foreground,
            status,
            restore_command: None,
        }
    }

    fn entry(mapping: Mapping) -> Option<MapEntry> {
        let pid = match mapping {
            Mapping::WithPid => Some(68237),
            Mapping::WithoutPid => None,
            Mapping::Missing => return None,
        };
        Some(MapEntry {
            conv: ConvId::new("c1"),
            profile: Profile::Personal,
            cwd: PathBuf::from("/tmp"),
            ts: SystemTime::UNIX_EPOCH,
            pid,
        })
    }

    fn rows() -> Vec<Row> {
        vec![
            PARKABLE,
            Row {
                name: "shell prompt is not claude",
                foreground: Foreground::Shell,
                expected: Decision::Skip(SkipReason::NotClaude),
                ..PARKABLE
            },
            Row {
                name: "placeholder is not claude",
                foreground: Foreground::Placeholder,
                expected: Decision::Skip(SkipReason::NotClaude),
                ..PARKABLE
            },
            Row {
                name: "editor is not claude",
                foreground: Foreground::Editor,
                expected: Decision::Skip(SkipReason::NotClaude),
                ..PARKABLE
            },
            Row {
                name: "no cc-map entry",
                mapping: Mapping::Missing,
                expected: Decision::Skip(SkipReason::NotMapped),
                ..PARKABLE
            },
            Row {
                name: "cc-map entry without pid",
                mapping: Mapping::WithoutPid,
                expected: Decision::Skip(SkipReason::NotMapped),
                ..PARKABLE
            },
            Row {
                name: "pid is not a live claude",
                pid_is_claude: false,
                expected: Decision::Skip(SkipReason::PidNotClaude),
                ..PARKABLE
            },
            Row {
                name: "agent working",
                status: AgentStatus::Active,
                expected: Decision::Skip(SkipReason::AgentWorking),
                ..PARKABLE
            },
            Row {
                name: "flagged",
                flagged: true,
                expected: Decision::Skip(SkipReason::Flagged),
                ..PARKABLE
            },
            Row {
                name: "selected",
                selected: true,
                expected: Decision::Skip(SkipReason::Selected),
                ..PARKABLE
            },
            Row {
                name: "not idle enough",
                idle: HOUR,
                expected: Decision::Skip(SkipReason::NotIdleEnough),
                ..PARKABLE
            },
            Row {
                name: "idle exactly park_after parks",
                idle: PARK_AFTER,
                ..PARKABLE
            },
            Row {
                name: "idle status parks",
                status: AgentStatus::Idle,
                ..PARKABLE
            },
            Row {
                name: "blocked status parks",
                status: AgentStatus::Blocked,
                ..PARKABLE
            },
            Row {
                name: "manual parks a selected session",
                selected: true,
                mode: Mode::Manual,
                ..PARKABLE
            },
            Row {
                name: "manual parks a session idle one minute",
                idle: Duration::from_secs(60),
                mode: Mode::Manual,
                ..PARKABLE
            },
            Row {
                name: "manual refuses flagged",
                flagged: true,
                mode: Mode::Manual,
                expected: Decision::Skip(SkipReason::Flagged),
                ..PARKABLE
            },
            Row {
                name: "manual refuses agent working",
                status: AgentStatus::Active,
                mode: Mode::Manual,
                expected: Decision::Skip(SkipReason::AgentWorking),
                ..PARKABLE
            },
            Row {
                name: "manual refuses not claude",
                foreground: Foreground::Shell,
                mode: Mode::Manual,
                expected: Decision::Skip(SkipReason::NotClaude),
                ..PARKABLE
            },
            Row {
                name: "manual refuses not mapped",
                mapping: Mapping::Missing,
                mode: Mode::Manual,
                expected: Decision::Skip(SkipReason::NotMapped),
                ..PARKABLE
            },
            Row {
                name: "first failing check wins",
                foreground: Foreground::Shell,
                mapping: Mapping::Missing,
                pid_is_claude: false,
                status: AgentStatus::Active,
                flagged: true,
                selected: true,
                idle: Duration::ZERO,
                expected: Decision::Skip(SkipReason::NotClaude),
                ..PARKABLE
            },
            Row {
                name: "flagged wins over selected and not idle",
                flagged: true,
                selected: true,
                idle: Duration::ZERO,
                expected: Decision::Skip(SkipReason::Flagged),
                ..PARKABLE
            },
        ]
    }

    #[test]
    fn decision_table() {
        for row in rows() {
            let Row {
                name,
                mapping,
                pid_is_claude,
                idle,
                mode,
                expected,
                ..
            } = row;
            let session = session(&row);
            let entry = entry(mapping);
            let input = RuleInput {
                session: &session,
                entry: entry.as_ref(),
                pid_is_claude,
                idle,
                park_after: PARK_AFTER,
                mode,
            };
            assert_eq!(decide(&input), expected, "{name}");
        }
    }
}
