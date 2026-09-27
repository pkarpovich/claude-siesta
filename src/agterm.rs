use std::fmt;
use std::io::{self, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::ccmap::SessionId;
use crate::tree::{self, Session, TreeError, WindowId};

pub const AGTERMCTL: &str = "/Applications/agterm.app/Contents/MacOS/agtermctl";

#[derive(Debug)]
pub enum AgtermError {
    Spawn(io::Error),
    Failed(String),
    Response(String),
    NoMatch(String),
    Ambiguous(Vec<SessionId>),
}

impl fmt::Display for AgtermError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgtermError::Spawn(error) => write!(f, "cannot run agtermctl: {error}"),
            AgtermError::Failed(message) => write!(f, "agtermctl failed: {message}"),
            AgtermError::Response(message) => write!(f, "invalid agtermctl response: {message}"),
            AgtermError::NoMatch(prefix) => write!(f, "no session matches {prefix}"),
            AgtermError::Ambiguous(ids) => {
                let mut names = Vec::new();
                for id in ids {
                    names.push(id.as_str());
                }
                write!(f, "ambiguous session prefix: {}", names.join(", "))
            }
        }
    }
}

impl From<TreeError> for AgtermError {
    fn from(error: TreeError) -> AgtermError {
        match error {
            TreeError::Syntax(message) => AgtermError::Response(message),
            TreeError::Agterm(message) => AgtermError::Failed(message),
        }
    }
}

pub trait AgtermOps {
    fn windows(&self) -> Result<Vec<tree::Window>, AgtermError>;
    fn tree(&self, window: &WindowId) -> Result<Vec<Session>, AgtermError>;
    fn restore(
        &self,
        window: &WindowId,
        session: &SessionId,
        command: &str,
    ) -> Result<(), AgtermError>;
    fn type_text(
        &self,
        window: &WindowId,
        session: &SessionId,
        text: &str,
    ) -> Result<(), AgtermError>;

    fn all_sessions(&self) -> Result<Vec<(WindowId, Session)>, AgtermError> {
        let mut all = Vec::new();
        for tree::Window { id, open } in self.windows()? {
            if !open {
                continue;
            }
            for session in self.tree(&id)? {
                all.push((id.clone(), session));
            }
        }
        Ok(all)
    }

    fn resolve_prefix(&self, prefix: &str) -> Result<(WindowId, Session), AgtermError> {
        resolve_prefix(self.all_sessions()?, prefix)
    }
}

pub fn resolve_prefix(
    sessions: Vec<(WindowId, Session)>,
    prefix: &str,
) -> Result<(WindowId, Session), AgtermError> {
    let prefix = prefix.to_uppercase();
    let mut matches = Vec::new();
    for (window, session) in sessions {
        if session.id.as_str().starts_with(&prefix) {
            matches.push((window, session));
        }
    }
    if matches.len() > 1 {
        let mut ids = Vec::new();
        for (_, session) in matches {
            ids.push(session.id);
        }
        return Err(AgtermError::Ambiguous(ids));
    }
    let Some(found) = matches.pop() else {
        return Err(AgtermError::NoMatch(prefix));
    };
    Ok(found)
}

#[derive(Debug, Clone)]
pub struct Agterm {
    pub bin: PathBuf,
}

impl Default for Agterm {
    fn default() -> Agterm {
        Agterm {
            bin: PathBuf::from(AGTERMCTL),
        }
    }
}

#[derive(Debug, Deserialize)]
struct Ack {
    ok: bool,
    error: Option<String>,
}

impl Agterm {
    fn call(&self, args: &[&str], stdin: Option<&str>) -> Result<String, AgtermError> {
        let mut command = Command::new(&self.bin);
        command.args(args).arg("--json");
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        match stdin {
            Some(_) => command.stdin(Stdio::piped()),
            None => command.stdin(Stdio::null()),
        };
        let mut child = command.spawn().map_err(AgtermError::Spawn)?;
        if let Some(text) = stdin {
            let Some(mut pipe) = child.stdin.take() else {
                return Err(AgtermError::Failed("no stdin pipe".to_string()));
            };
            pipe.write_all(text.as_bytes())
                .map_err(AgtermError::Spawn)?;
        }
        let output = child.wait_with_output().map_err(AgtermError::Spawn)?;
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        if let Ok(Ack { ok: false, error }) = serde_json::from_str::<Ack>(&stdout) {
            return Err(AgtermError::Failed(
                error.unwrap_or_else(|| "request failed".to_string()),
            ));
        }
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AgtermError::Failed(failure_message(
                &stderr,
                &stdout,
                &output.status.to_string(),
            )));
        }
        Ok(stdout)
    }

    fn acknowledge(&self, args: &[&str], stdin: Option<&str>) -> Result<(), AgtermError> {
        let stdout = self.call(args, stdin)?;
        let Ack { ok, error } = serde_json::from_str(&stdout)
            .map_err(|error| AgtermError::Response(error.to_string()))?;
        if !ok {
            return Err(AgtermError::Failed(
                error.unwrap_or_else(|| "request failed".to_string()),
            ));
        }
        Ok(())
    }
}

fn failure_message(stderr: &str, stdout: &str, status: &str) -> String {
    for text in [stderr, stdout] {
        let Some(line) = text.lines().next() else {
            continue;
        };
        let line = line.trim();
        if !line.is_empty() {
            return line.to_string();
        }
    }
    status.to_string()
}

impl AgtermOps for Agterm {
    fn windows(&self) -> Result<Vec<tree::Window>, AgtermError> {
        let stdout = self.call(&["window", "list"], None)?;
        Ok(tree::parse_windows(&stdout)?)
    }

    fn tree(&self, window: &WindowId) -> Result<Vec<Session>, AgtermError> {
        let stdout = self.call(&["tree", "--window", window.as_str()], None)?;
        Ok(tree::parse_tree(&stdout)?)
    }

    fn restore(
        &self,
        window: &WindowId,
        session: &SessionId,
        command: &str,
    ) -> Result<(), AgtermError> {
        self.acknowledge(
            &[
                "session",
                "restore",
                command,
                "--target",
                session.as_str(),
                "--window",
                window.as_str(),
            ],
            None,
        )
    }

    fn type_text(
        &self,
        window: &WindowId,
        session: &SessionId,
        text: &str,
    ) -> Result<(), AgtermError> {
        self.acknowledge(
            &[
                "session",
                "type",
                "--stdin",
                "--target",
                session.as_str(),
                "--window",
                window.as_str(),
            ],
            Some(text),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use super::*;

    const WINDOW_LIST: &str = include_str!("../tests/fixtures/window-list.json");
    const TREE: &str = include_str!("../tests/fixtures/tree.json");
    const OPEN_WINDOW: &str = "F800A0ED-F3A9-4CF7-ACCE-55E58C795C76";
    const SESSION: &str = "7320056A-2D3F-4B06-AB9A-1CB79BED0F3A";

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "claude-siesta-agterm-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_bin(dir: &Path, body: &str) -> Agterm {
        let bin = dir.join("agtermctl");
        std::fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        Agterm { bin }
    }

    fn recording_bin(dir: &Path) -> Agterm {
        fake_bin(
            dir,
            &format!(
                "printf '%s\\n' \"$@\" > '{args}'\ncat > '{stdin}'\necho '{{\"ok\":true,\"result\":{{}}}}'",
                args = dir.join("args").display(),
                stdin = dir.join("stdin").display(),
            ),
        )
    }

    fn recorded_args(dir: &Path) -> Vec<String> {
        let text = std::fs::read_to_string(dir.join("args")).unwrap();
        let mut args = Vec::new();
        for line in text.lines() {
            args.push(line.to_string());
        }
        args
    }

    fn fixture_bin(dir: &Path) -> Agterm {
        let windows = dir.join("windows.json");
        let tree = dir.join("tree.json");
        std::fs::write(&windows, WINDOW_LIST).unwrap();
        std::fs::write(&tree, TREE).unwrap();
        fake_bin(
            dir,
            &format!(
                "echo \"$@\" >> '{log}'\ncase \"$1\" in\n  window) cat '{windows}' ;;\n  tree) cat '{tree}' ;;\nesac",
                log = dir.join("calls").display(),
                windows = windows.display(),
                tree = tree.display(),
            ),
        )
    }

    fn all_fixture_sessions() -> Vec<(WindowId, Session)> {
        let mut all = Vec::new();
        for session in tree::parse_tree(TREE).unwrap() {
            all.push((WindowId::new(OPEN_WINDOW), session));
        }
        all
    }

    #[test]
    fn restore_passes_the_line_target_window_and_json() {
        let dir = temp_dir("restore");
        let agterm = recording_bin(&dir);
        agterm
            .restore(
                &WindowId::new(OPEN_WINDOW),
                &SessionId::new(SESSION),
                "claude-siesta",
            )
            .unwrap();
        assert_eq!(
            recorded_args(&dir),
            vec![
                "session",
                "restore",
                "claude-siesta",
                "--target",
                SESSION,
                "--window",
                OPEN_WINDOW,
                "--json",
            ]
        );
    }

    #[test]
    fn type_text_sends_stdin_without_select() {
        let dir = temp_dir("type");
        let agterm = recording_bin(&dir);
        agterm
            .type_text(
                &WindowId::new(OPEN_WINDOW),
                &SessionId::new(SESSION),
                " claude-siesta\n",
            )
            .unwrap();
        let args = recorded_args(&dir);
        assert_eq!(
            args,
            vec![
                "session",
                "type",
                "--stdin",
                "--target",
                SESSION,
                "--window",
                OPEN_WINDOW,
                "--json",
            ]
        );
        for arg in &args {
            assert_ne!(arg, "--select");
        }
        assert_eq!(
            std::fs::read_to_string(dir.join("stdin")).unwrap(),
            " claude-siesta\n"
        );
    }

    #[test]
    fn ok_false_response_is_an_error_with_its_message() {
        let dir = temp_dir("ok-false");
        let agterm = fake_bin(
            &dir,
            "echo '{\"error\":\"no such session: 00000000\",\"ok\":false}'\nexit 1",
        );
        let error = agterm
            .restore(
                &WindowId::new(OPEN_WINDOW),
                &SessionId::new("00000000"),
                "claude-siesta",
            )
            .unwrap_err();
        let AgtermError::Failed(message) = error else {
            panic!("expected Failed, got {error:?}");
        };
        assert_eq!(message, "no such session: 00000000");
    }

    #[test]
    fn nonzero_exit_reports_first_stderr_line() {
        let dir = temp_dir("stderr");
        let agterm = fake_bin(&dir, "echo 'socket not found' >&2\necho 'more' >&2\nexit 3");
        let error = agterm.windows().unwrap_err();
        let AgtermError::Failed(message) = error else {
            panic!("expected Failed, got {error:?}");
        };
        assert_eq!(message, "socket not found");
    }

    #[test]
    fn missing_binary_is_a_spawn_error() {
        let agterm = Agterm {
            bin: PathBuf::from("/nonexistent/agtermctl"),
        };
        let error = agterm.windows().unwrap_err();
        let AgtermError::Spawn(_) = error else {
            panic!("expected Spawn, got {error:?}");
        };
    }

    #[test]
    fn garbage_tree_is_a_response_error() {
        let dir = temp_dir("garbage");
        let agterm = fake_bin(&dir, "echo 'not json'");
        let error = agterm.tree(&WindowId::new(OPEN_WINDOW)).unwrap_err();
        let AgtermError::Response(_) = error else {
            panic!("expected Response, got {error:?}");
        };
    }

    #[test]
    fn all_sessions_walks_only_open_windows() {
        let dir = temp_dir("all");
        let agterm = fixture_bin(&dir);
        let all = agterm.all_sessions().unwrap();
        assert_eq!(all.len(), 7);
        for (window, _) in &all {
            assert_eq!(window.as_str(), OPEN_WINDOW);
        }
        let calls = std::fs::read_to_string(dir.join("calls")).unwrap();
        assert_eq!(
            calls,
            format!("window list --json\ntree --window {OPEN_WINDOW} --json\n")
        );
    }

    #[test]
    fn resolve_prefix_is_case_insensitive() {
        let (window, session) = resolve_prefix(all_fixture_sessions(), "7320056a").unwrap();
        assert_eq!(window.as_str(), OPEN_WINDOW);
        assert_eq!(session.id.as_str(), SESSION);
    }

    #[test]
    fn resolve_prefix_accepts_a_full_id() {
        let (_, session) = resolve_prefix(all_fixture_sessions(), SESSION).unwrap();
        assert_eq!(session.id.as_str(), SESSION);
    }

    #[test]
    fn resolve_prefix_without_match_is_an_error() {
        let error = resolve_prefix(all_fixture_sessions(), "zz").unwrap_err();
        let AgtermError::NoMatch(prefix) = error else {
            panic!("expected NoMatch, got {error:?}");
        };
        assert_eq!(prefix, "ZZ");
    }

    #[test]
    fn resolve_prefix_with_several_matches_names_the_candidates() {
        let mut sessions = all_fixture_sessions();
        let (window, session) = sessions[0].clone();
        sessions.push((
            window,
            Session {
                id: SessionId::new("7320FFFF-0000-0000-0000-000000000000"),
                ..session
            },
        ));
        let error = resolve_prefix(sessions, "7320").unwrap_err();
        let AgtermError::Ambiguous(ids) = &error else {
            panic!("expected Ambiguous, got {error:?}");
        };
        assert_eq!(
            ids,
            &vec![
                SessionId::new(SESSION),
                SessionId::new("7320FFFF-0000-0000-0000-000000000000"),
            ]
        );
        assert!(error.to_string().contains(SESSION));
    }

    #[test]
    fn resolve_prefix_through_the_wrapper() {
        let dir = temp_dir("resolve");
        let agterm = fixture_bin(&dir);
        let (_, session) = agterm.resolve_prefix("a7").unwrap();
        assert_eq!(session.id.as_str(), "A7000000-0000-0000-0000-000000000007");
    }
}
