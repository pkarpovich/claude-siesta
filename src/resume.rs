use std::fmt;

use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

use crate::ccmap::SessionId;
use crate::park::process_command;
use crate::paths::Paths;
use crate::state::{self, ParkState};

const PLACEHOLDER_COMMAND: &str = "claude-siesta";

#[derive(Debug)]
pub enum ResumeError {
    NoState(SessionId),
    NoPid(SessionId),
    NotRunning(i32),
    NotPlaceholder { pid: i32, command: String },
    Signal { pid: i32, error: nix::Error },
}

impl fmt::Display for ResumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResumeError::NoState(session) => {
                write!(f, "{} is not parked (no state file)", session.as_str())
            }
            ResumeError::NoPid(session) => write!(
                f,
                "{} has no placeholder pid (the placeholder has not started)",
                session.as_str()
            ),
            ResumeError::NotRunning(pid) => write!(f, "placeholder pid {pid} is not running"),
            ResumeError::NotPlaceholder { pid, command } => {
                write!(f, "pid {pid} is {command}, not {PLACEHOLDER_COMMAND}")
            }
            ResumeError::Signal { pid, error } => write!(f, "cannot signal pid {pid}: {error}"),
        }
    }
}

pub fn signal_placeholder(paths: &Paths, session: &SessionId) -> Result<i32, ResumeError> {
    let Some(ParkState {
        placeholder_pid,
        session: _,
        conv: _,
        profile: _,
        parked_at: _,
        last_assistant_at: _,
    }) = state::read(paths, session)
    else {
        return Err(ResumeError::NoState(session.clone()));
    };
    let Some(pid) = placeholder_pid else {
        return Err(ResumeError::NoPid(session.clone()));
    };
    let Some(command) = process_command(pid) else {
        return Err(ResumeError::NotRunning(pid));
    };
    if !command.ends_with(PLACEHOLDER_COMMAND) {
        return Err(ResumeError::NotPlaceholder { pid, command });
    }
    kill(Pid::from_raw(pid), Signal::SIGUSR1)
        .map_err(|error| ResumeError::Signal { pid, error })?;
    Ok(pid)
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::{Child, Command};
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::ccmap::{ConvId, Profile};

    const SESSION: &str = "7320056A-0000-0000-0000-000000000001";

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "claude-siesta-resume-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn park(paths: &Paths, placeholder_pid: Option<i32>) {
        state::write(
            paths,
            &ParkState {
                session: SessionId::new(SESSION),
                conv: ConvId::new("c0acdbe6-1111-2222-3333-444444444444"),
                profile: Profile::Personal,
                parked_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_500_000),
                last_assistant_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_400_000),
                placeholder_pid,
            },
        )
        .unwrap();
    }

    fn child_pid(child: &Child) -> i32 {
        i32::try_from(child.id()).unwrap()
    }

    #[test]
    fn no_state_file() {
        let paths = Paths::from_home(temp_home("no-state"));
        let result = signal_placeholder(&paths, &SessionId::new(SESSION));
        let Err(ResumeError::NoState(session)) = result else {
            panic!("expected NoState, got {result:?}");
        };
        assert_eq!(session.as_str(), SESSION);
    }

    #[test]
    fn no_placeholder_pid() {
        let paths = Paths::from_home(temp_home("no-pid"));
        park(&paths, None);
        let result = signal_placeholder(&paths, &SessionId::new(SESSION));
        let Err(ResumeError::NoPid(_)) = result else {
            panic!("expected NoPid, got {result:?}");
        };
    }

    #[test]
    fn dead_pid() {
        let paths = Paths::from_home(temp_home("dead-pid"));
        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child_pid(&child);
        child.wait().unwrap();
        park(&paths, Some(pid));
        let result = signal_placeholder(&paths, &SessionId::new(SESSION));
        let Err(ResumeError::NotRunning(found)) = result else {
            panic!("expected NotRunning, got {result:?}");
        };
        assert_eq!(found, pid);
    }

    #[test]
    fn live_pid_of_another_program_is_not_signalled() {
        let paths = Paths::from_home(temp_home("other-program"));
        let mut child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        park(&paths, Some(child_pid(&child)));
        let result = signal_placeholder(&paths, &SessionId::new(SESSION));
        let still_running = child.try_wait().unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        let Err(ResumeError::NotPlaceholder { pid, command }) = result else {
            panic!("expected NotPlaceholder, got {result:?}");
        };
        assert_eq!(pid, child_pid(&child));
        assert!(command.ends_with("sleep"), "{command}");
        assert_eq!(still_running, None);
    }

    #[test]
    fn placeholder_receives_sigusr1() {
        let home = temp_home("placeholder");
        let paths = Paths::from_home(home.clone());
        let program = home.join("claude-siesta");
        std::os::unix::fs::symlink("/bin/sleep", &program).unwrap();
        let mut child = Command::new(&program).arg("60").spawn().unwrap();
        park(&paths, Some(child_pid(&child)));
        let result = signal_placeholder(&paths, &SessionId::new(SESSION));
        let status = child.wait().unwrap();
        assert_eq!(result.unwrap(), child_pid(&child));
        assert_eq!(status.signal(), Some(nix::libc::SIGUSR1));
    }
}
