use std::time::{Duration, SystemTime};

use crate::agterm::{AgtermError, AgtermOps};
use crate::ccmap::{ConvId, MapEntry};
use crate::log;
use crate::paths::Paths;
use crate::state::{self, ParkState};
use crate::time::format_idle;
use crate::transcript;
use crate::tree::Session;

const NAME_WIDTH: usize = 24;
const CONV_WIDTH: usize = 8;
const COLUMNS: usize = 5;
const GAP: usize = 2;
const HEADER: [&str; COLUMNS] = ["NAME", "CONV", "IDLE", "STATE", "LAST"];
const NO_ACTION: &str = "-";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Live,
    Parked,
    Shell,
}

impl SessionState {
    pub fn as_str(self) -> &'static str {
        match self {
            SessionState::Live => "live",
            SessionState::Parked => "parked",
            SessionState::Shell => "shell",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    pub name: String,
    pub conv: ConvId,
    pub idle: Duration,
    pub state: SessionState,
    pub last_action: Option<String>,
}

pub fn classify(session: &Session, park_state: Option<&ParkState>) -> SessionState {
    if session.runs_claude() {
        return SessionState::Live;
    }
    if session.runs_placeholder() {
        return SessionState::Parked;
    }
    match park_state {
        Some(_) => SessionState::Parked,
        None => SessionState::Shell,
    }
}

pub fn gather(
    agterm: &dyn AgtermOps,
    paths: &Paths,
    now: SystemTime,
) -> Result<Vec<StatusRow>, AgtermError> {
    let mut rows = Vec::new();
    for (_, session) in agterm.all_sessions()? {
        let Ok(Some(entry)) = MapEntry::load(paths, &session.id) else {
            continue;
        };
        let last = transcript::load_last(paths, &entry);
        let park_state = state::read(paths, &session.id);
        rows.push(StatusRow {
            state: classify(&session, park_state.as_ref()),
            idle: transcript::idle(
                now,
                last.as_ref(),
                &entry,
                state::resumed_at(paths, &session.id),
            ),
            last_action: log::last_action(paths, &session.id),
            conv: entry.conv,
            name: session.name,
        });
    }
    Ok(rows)
}

pub fn format_table(rows: &[StatusRow]) -> String {
    let mut table = vec![HEADER.map(String::from)];
    for row in rows {
        table.push(cells(row));
    }
    let mut widths = [0; COLUMNS];
    for cells in &table {
        for (index, cell) in cells.iter().enumerate() {
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for cells in &table {
        let mut line = String::new();
        for (index, cell) in cells.iter().enumerate() {
            line.push_str(cell);
            if index + 1 == COLUMNS {
                continue;
            }
            for _ in cell.chars().count()..widths[index] + GAP {
                line.push(' ');
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn cells(row: &StatusRow) -> [String; COLUMNS] {
    let StatusRow {
        name,
        conv,
        idle,
        state,
        last_action,
    } = row;
    let last_action = match last_action {
        Some(action) => action.clone(),
        None => String::from(NO_ACTION),
    };
    [
        truncate(name, NAME_WIDTH),
        truncate(conv.as_str(), CONV_WIDTH),
        format_idle(*idle),
        state.as_str().to_string(),
        last_action,
    ]
}

fn truncate(text: &str, max: usize) -> String {
    let mut out = String::new();
    for (index, ch) in text.chars().enumerate() {
        if index == max {
            break;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::ccmap::{Profile, SessionId};
    use crate::log::{Action, LogLine};
    use crate::tree::{AgentStatus, Window, WindowId};

    const WINDOW: &str = "AAAAAAAA-0000-0000-0000-000000000001";
    const LIVE: &str = "10000000-0000-0000-0000-000000000001";
    const PARKED: &str = "20000000-0000-0000-0000-000000000002";
    const UNMAPPED: &str = "30000000-0000-0000-0000-000000000003";
    const NOW: u64 = 1_790_500_000;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn session(id: &str, name: &str, foreground: &[&str]) -> Session {
        let mut argv = Vec::new();
        for arg in foreground {
            argv.push(arg.to_string());
        }
        Session {
            id: SessionId::new(id),
            name: name.to_string(),
            active: false,
            flagged: false,
            split: false,
            foreground: argv,
            status: AgentStatus::Idle,
            restore_command: None,
        }
    }

    fn park_state(id: &str) -> ParkState {
        ParkState {
            session: SessionId::new(id),
            conv: ConvId::new("c0acdbe6-1111-2222-3333-444444444444"),
            profile: Profile::Personal,
            parked_at: at(NOW),
            last_assistant_at: at(NOW - 3600),
            placeholder_pid: None,
        }
    }

    fn row(name: &str, idle_minutes: u64, state: SessionState, last: Option<&str>) -> StatusRow {
        StatusRow {
            name: name.to_string(),
            conv: ConvId::new("c0acdbe6-1111-2222-3333-444444444444"),
            idle: Duration::from_secs(idle_minutes * 60),
            state,
            last_action: last.map(String::from),
        }
    }

    #[test]
    fn claude_foreground_is_live() {
        let live = session(
            LIVE,
            "api",
            &["/home/x/.local/bin/claude", "--resume", "c0"],
        );
        assert_eq!(classify(&live, None), SessionState::Live);
        assert_eq!(classify(&live, Some(&park_state(LIVE))), SessionState::Live);
    }

    #[test]
    fn placeholder_foreground_is_parked() {
        let parked = session(PARKED, "web", &["/home/x/.local/bin/claude-siesta"]);
        assert_eq!(classify(&parked, None), SessionState::Parked);
    }

    #[test]
    fn state_file_at_shell_prompt_is_parked() {
        let shell = session(PARKED, "web", &[]);
        assert_eq!(
            classify(&shell, Some(&park_state(PARKED))),
            SessionState::Parked
        );
    }

    #[test]
    fn shell_prompt_or_other_program_is_shell() {
        assert_eq!(
            classify(&session(PARKED, "web", &[]), None),
            SessionState::Shell
        );
        assert_eq!(
            classify(&session(PARKED, "web", &["/usr/bin/vim"]), None),
            SessionState::Shell
        );
    }

    #[test]
    fn empty_input_prints_header_only() {
        assert_eq!(format_table(&[]), "NAME  CONV  IDLE  STATE  LAST\n");
    }

    #[test]
    fn columns_are_aligned() {
        let table = format_table(&[
            row("api", 45, SessionState::Live, Some("skip")),
            row("claude-siesta", 26 * 60 + 12, SessionState::Parked, None),
        ]);
        assert_eq!(
            table,
            concat!(
                "NAME           CONV      IDLE   STATE   LAST\n",
                "api            c0acdbe6  45m    live    skip\n",
                "claude-siesta  c0acdbe6  1d 2h  parked  -\n",
            )
        );
    }

    #[test]
    fn long_names_are_truncated_to_24_chars() {
        let table = format_table(&[row(
            "Проект с очень длинным названием",
            5,
            SessionState::Shell,
            Some("park"),
        )]);
        let mut lines = table.lines();
        lines.next();
        let line = lines.next().unwrap();
        assert!(
            line.starts_with("Проект с очень длинным н  c0acdbe6  5m"),
            "{line}"
        );
        assert!(!line.contains("названием"));
    }

    struct FakeAgterm {
        sessions: Vec<Session>,
    }

    impl AgtermOps for FakeAgterm {
        fn windows(&self) -> Result<Vec<Window>, AgtermError> {
            Ok(vec![Window {
                id: WindowId::new(WINDOW),
                open: true,
            }])
        }

        fn tree(&self, _window: &WindowId) -> Result<Vec<Session>, AgtermError> {
            Ok(self.sessions.clone())
        }

        fn restore(
            &self,
            _window: &WindowId,
            _session: &SessionId,
            _command: &str,
        ) -> Result<(), AgtermError> {
            panic!("status never pins a restore line");
        }

        fn type_text(
            &self,
            _window: &WindowId,
            _session: &SessionId,
            _text: &str,
        ) -> Result<(), AgtermError> {
            panic!("status never types");
        }
    }

    fn temp_home(name: &str) -> Paths {
        let dir: PathBuf = std::env::temp_dir().join(format!(
            "claude-siesta-status-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Paths::from_home(dir)
    }

    fn map(paths: &Paths, id: &str, ts: u64) {
        std::fs::create_dir_all(paths.cc_map_dir()).unwrap();
        std::fs::write(
            paths.cc_map_dir().join(id),
            format!(
                r#"{{"conv":"c0acdbe6-1111","profile":"personal","cwd":"/tmp/x","ts":{ts},"pid":1}}"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn gather_lists_only_mapped_sessions() {
        let paths = temp_home("gather");
        map(&paths, LIVE, NOW - 45 * 60);
        map(&paths, PARKED, NOW - 3 * 3600);
        state::write(&paths, &park_state(PARKED)).unwrap();
        log::append(
            &paths,
            &LogLine {
                action: Action::Park,
                session: Some(SessionId::new(PARKED)),
                conv: None,
                idle: None,
                reason: String::new(),
            },
        )
        .unwrap();
        let agterm = FakeAgterm {
            sessions: vec![
                session(LIVE, "api", &["/home/x/.local/bin/claude"]),
                session(PARKED, "web", &[]),
                session(UNMAPPED, "notes", &[]),
            ],
        };
        let rows = gather(&agterm, &paths, at(NOW)).unwrap();
        assert_eq!(
            rows,
            vec![
                StatusRow {
                    name: "api".to_string(),
                    conv: ConvId::new("c0acdbe6-1111"),
                    idle: Duration::from_secs(45 * 60),
                    state: SessionState::Live,
                    last_action: None,
                },
                StatusRow {
                    name: "web".to_string(),
                    conv: ConvId::new("c0acdbe6-1111"),
                    idle: Duration::from_secs(3 * 3600),
                    state: SessionState::Parked,
                    last_action: Some("park".to_string()),
                },
            ]
        );
    }
}
