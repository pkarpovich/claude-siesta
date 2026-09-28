use std::fmt;

use serde::Deserialize;

use crate::ccmap::SessionId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowId(String);

impl WindowId {
    pub fn new(id: &str) -> WindowId {
        WindowId(id.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub id: WindowId,
    pub open: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Active,
    Completed,
    Blocked,
}

impl AgentStatus {
    pub fn parse(text: Option<&str>) -> AgentStatus {
        match text {
            Some("active") => AgentStatus::Active,
            Some("completed") => AgentStatus::Completed,
            Some("blocked") => AgentStatus::Blocked,
            Some(_) => AgentStatus::Idle,
            None => AgentStatus::Idle,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub active: bool,
    pub flagged: bool,
    pub split: bool,
    pub foreground: Vec<String>,
    pub status: AgentStatus,
    pub restore_command: Option<String>,
}

impl Session {
    pub fn runs_claude(&self) -> bool {
        let Some(program) = self.foreground.first() else {
            return false;
        };
        program.ends_with("/claude")
    }

    pub fn runs_placeholder(&self) -> bool {
        let Some(program) = self.foreground.first() else {
            return false;
        };
        program.ends_with("claude-siesta")
    }
}

#[derive(Debug)]
pub enum TreeError {
    Syntax(String),
    Agterm(String),
}

impl fmt::Display for TreeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TreeError::Syntax(message) => write!(f, "invalid agtermctl response: {message}"),
            TreeError::Agterm(message) => write!(f, "agtermctl failed: {message}"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Response<T> {
    ok: bool,
    result: Option<T>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WindowsResult {
    #[serde(default)]
    windows: Vec<WireWindow>,
}

#[derive(Debug, Deserialize)]
struct WireWindow {
    id: String,
    #[serde(default)]
    open: bool,
}

#[derive(Debug, Deserialize)]
struct TreeResult {
    tree: WireTree,
}

#[derive(Debug, Deserialize)]
struct WireTree {
    #[serde(default)]
    workspaces: Vec<WireWorkspace>,
}

#[derive(Debug, Deserialize)]
struct WireWorkspace {
    #[serde(default)]
    sessions: Vec<WireSession>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireSession {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    flagged: bool,
    #[serde(default)]
    split: bool,
    #[serde(default)]
    has_split: bool,
    #[serde(default)]
    foreground: Vec<String>,
    status: Option<String>,
    restore_command: Option<String>,
}

fn unwrap_response<T>(text: &str) -> Result<T, TreeError>
where
    T: for<'de> Deserialize<'de>,
{
    let response: Response<T> =
        serde_json::from_str(text).map_err(|error| TreeError::Syntax(error.to_string()))?;
    let Response { ok, result, error } = response;
    if !ok {
        let message = error.unwrap_or_else(|| "request failed".to_string());
        return Err(TreeError::Agterm(message));
    }
    let Some(result) = result else {
        return Err(TreeError::Syntax("no result".to_string()));
    };
    Ok(result)
}

pub fn parse_windows(text: &str) -> Result<Vec<Window>, TreeError> {
    let WindowsResult { windows: wire } = unwrap_response(text)?;
    let mut windows = Vec::new();
    for WireWindow { id, open } in wire {
        windows.push(Window {
            id: WindowId::new(&id),
            open,
        });
    }
    Ok(windows)
}

pub fn parse_tree(text: &str) -> Result<Vec<Session>, TreeError> {
    let TreeResult {
        tree: WireTree { workspaces },
    } = unwrap_response(text)?;
    let mut sessions = Vec::new();
    for WireWorkspace { sessions: wire } in workspaces {
        for WireSession {
            id,
            name,
            active,
            flagged,
            split,
            has_split,
            foreground,
            status,
            restore_command,
        } in wire
        {
            sessions.push(Session {
                id: SessionId::new(&id),
                name,
                active,
                flagged,
                split: split || has_split,
                foreground,
                status: AgentStatus::parse(status.as_deref()),
                restore_command,
            });
        }
    }
    Ok(sessions)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW_LIST: &str = include_str!("../tests/fixtures/window-list.json");
    const TREE: &str = include_str!("../tests/fixtures/tree.json");

    fn session(sessions: &[Session], id: &str) -> Session {
        let id = SessionId::new(id);
        for session in sessions {
            if session.id == id {
                return session.clone();
            }
        }
        panic!("no session {}", id.as_str());
    }

    #[test]
    fn windows_from_fixture() {
        let windows = parse_windows(WINDOW_LIST).unwrap();
        assert_eq!(
            windows,
            vec![
                Window {
                    id: WindowId::new("F800A0ED-F3A9-4CF7-ACCE-55E58C795C76"),
                    open: true,
                },
                Window {
                    id: WindowId::new("0B1C2D3E-4F50-6172-8394-A5B6C7D8E9F0"),
                    open: false,
                },
            ]
        );
    }

    #[test]
    fn tree_flattens_every_workspace() {
        let sessions = parse_tree(TREE).unwrap();
        assert_eq!(sessions.len(), 7);
    }

    #[test]
    fn live_claude_session_fields() {
        let sessions = parse_tree(TREE).unwrap();
        let live = session(&sessions, "7320056A-2D3F-4B06-AB9A-1CB79BED0F3A");
        assert_eq!(
            live,
            Session {
                id: SessionId::new("7320056A-2D3F-4B06-AB9A-1CB79BED0F3A"),
                name: "new tuclaw desktop app".to_string(),
                active: false,
                flagged: false,
                split: false,
                foreground: vec![
                    "/home/x/.local/bin/claude".to_string(),
                    "--enable-auto-mode".to_string(),
                    "--resume".to_string(),
                    "c0acdbe6-d414-4622-a394-ee560f5b2646".to_string(),
                ],
                status: AgentStatus::Completed,
                restore_command: Some(
                    "CLAUDE_CODE_NO_FLICKER=1 claude --enable-auto-mode --resume c0acdbe6-d414-4622-a394-ee560f5b2646"
                        .to_string()
                ),
            }
        );
        assert!(live.runs_claude());
        assert!(!live.runs_placeholder());
    }

    #[test]
    fn shell_prompt_session() {
        let sessions = parse_tree(TREE).unwrap();
        let shell = session(&sessions, "B2000000-0000-0000-0000-000000000002");
        assert!(shell.foreground.is_empty());
        assert_eq!(shell.status, AgentStatus::Idle);
        assert_eq!(shell.restore_command, None);
        assert!(!shell.runs_claude());
        assert!(!shell.runs_placeholder());
    }

    #[test]
    fn flagged_session() {
        let sessions = parse_tree(TREE).unwrap();
        let flagged = session(&sessions, "C3000000-0000-0000-0000-000000000003");
        assert!(flagged.flagged);
        assert!(!flagged.active);
        assert!(flagged.runs_claude());
    }

    #[test]
    fn selected_session() {
        let sessions = parse_tree(TREE).unwrap();
        let selected = session(&sessions, "D4000000-0000-0000-0000-000000000004");
        assert!(selected.active);
        assert!(!selected.flagged);
    }

    #[test]
    fn agent_working_session() {
        let sessions = parse_tree(TREE).unwrap();
        let working = session(&sessions, "E5000000-0000-0000-0000-000000000005");
        assert_eq!(working.status, AgentStatus::Active);
        assert!(working.runs_claude());
    }

    #[test]
    fn placeholder_session() {
        let sessions = parse_tree(TREE).unwrap();
        let parked = session(&sessions, "F6000000-0000-0000-0000-000000000006");
        assert!(parked.runs_placeholder());
        assert!(!parked.runs_claude());
        assert_eq!(parked.restore_command, Some("claude-siesta".to_string()));
    }

    #[test]
    fn other_program_session() {
        let sessions = parse_tree(TREE).unwrap();
        let editor = session(&sessions, "A7000000-0000-0000-0000-000000000007");
        assert_eq!(editor.foreground[0], "vim");
        assert_eq!(editor.status, AgentStatus::Blocked);
        assert!(!editor.runs_claude());
        assert!(!editor.runs_placeholder());
    }

    #[test]
    fn unknown_status_is_idle() {
        assert_eq!(AgentStatus::parse(Some("thinking")), AgentStatus::Idle);
        assert_eq!(AgentStatus::parse(None), AgentStatus::Idle);
    }

    #[test]
    fn missing_fields_default_and_id_is_uppercased() {
        let sessions = parse_tree(
            r#"{"ok":true,"result":{"tree":{"workspaces":[{"sessions":[{"id":"abc-def"}]},{}]}}}"#,
        )
        .unwrap();
        assert_eq!(
            sessions,
            vec![Session {
                id: SessionId::new("ABC-DEF"),
                name: String::new(),
                active: false,
                flagged: false,
                split: false,
                foreground: Vec::new(),
                status: AgentStatus::Idle,
                restore_command: None,
            }]
        );
    }

    #[test]
    fn split_session_is_marked_split() {
        let sessions = parse_tree(
            r#"{"ok":true,"result":{"tree":{"workspaces":[{"sessions":[{"id":"a","split":true,"foreground":["/usr/bin/claude"]}]}]}}}"#,
        )
        .unwrap();
        assert!(sessions[0].split);
    }

    #[test]
    fn hidden_split_session_is_marked_split() {
        let sessions = parse_tree(
            r#"{"ok":true,"result":{"tree":{"workspaces":[{"sessions":[{"id":"a","split":false,"hasSplit":true,"foreground":["/usr/bin/claude"]}]}]}}}"#,
        )
        .unwrap();
        assert!(sessions[0].split);
    }

    #[test]
    fn program_named_notclaude_is_not_claude() {
        let sessions = parse_tree(
            r#"{"ok":true,"result":{"tree":{"workspaces":[{"sessions":[{"id":"a","foreground":["/usr/bin/notclaude"]}]}]}}}"#,
        )
        .unwrap();
        assert!(!sessions[0].runs_claude());
    }

    #[test]
    fn claude_without_path_is_not_claude() {
        let sessions = parse_tree(
            r#"{"ok":true,"result":{"tree":{"workspaces":[{"sessions":[{"id":"a","foreground":["claude"]}]}]}}}"#,
        )
        .unwrap();
        assert!(!sessions[0].runs_claude());
    }

    #[test]
    fn not_ok_is_error_with_message() {
        let error =
            parse_tree(r#"{"error":"no such window: nonexistent","ok":false}"#).unwrap_err();
        let TreeError::Agterm(message) = error else {
            panic!("unexpected error: {error}");
        };
        assert_eq!(message, "no such window: nonexistent");
        let error = parse_windows(r#"{"ok":false}"#).unwrap_err();
        let TreeError::Agterm(_) = error else {
            panic!("unexpected error: {error}");
        };
    }

    #[test]
    fn ok_without_result_is_error() {
        let error = parse_tree(r#"{"ok":true}"#).unwrap_err();
        let TreeError::Syntax(_) = error else {
            panic!("unexpected error: {error}");
        };
    }

    #[test]
    fn malformed_json_is_error() {
        let error = parse_tree(r#"{"ok":true,"result":"#).unwrap_err();
        let TreeError::Syntax(_) = error else {
            panic!("unexpected error: {error}");
        };
        let error = parse_windows("not json").unwrap_err();
        let TreeError::Syntax(_) = error else {
            panic!("unexpected error: {error}");
        };
    }
}
