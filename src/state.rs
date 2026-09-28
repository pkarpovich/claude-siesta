use std::fs::{File, OpenOptions};
use std::io;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::ccmap::{ConvId, Profile, SessionId};
use crate::paths::Paths;
use crate::time::{from_unix_seconds, unix_seconds};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParkState {
    pub session: SessionId,
    pub conv: ConvId,
    pub profile: Profile,
    pub parked_at: SystemTime,
    pub last_assistant_at: SystemTime,
    pub placeholder_pid: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct StateFile {
    session: String,
    conv: String,
    profile: String,
    parked_at: u64,
    last_assistant_at: u64,
    placeholder_pid: Option<i32>,
}

impl ParkState {
    fn to_file(&self) -> StateFile {
        let ParkState {
            session,
            conv,
            profile,
            parked_at,
            last_assistant_at,
            placeholder_pid,
        } = self;
        StateFile {
            session: session.as_str().to_string(),
            conv: conv.as_str().to_string(),
            profile: profile.as_str().to_string(),
            parked_at: unix_seconds(*parked_at),
            last_assistant_at: unix_seconds(*last_assistant_at),
            placeholder_pid: *placeholder_pid,
        }
    }

    fn from_file(file: StateFile) -> ParkState {
        let StateFile {
            session,
            conv,
            profile,
            parked_at,
            last_assistant_at,
            placeholder_pid,
        } = file;
        ParkState {
            session: SessionId::new(&session),
            conv: ConvId::new(&conv),
            profile: Profile::parse(&profile),
            parked_at: from_unix_seconds(parked_at),
            last_assistant_at: from_unix_seconds(last_assistant_at),
            placeholder_pid,
        }
    }
}

pub fn write(paths: &Paths, state: &ParkState) -> io::Result<()> {
    let dir = paths.state_dir();
    std::fs::create_dir_all(&dir)?;
    let text = serde_json::to_string(&state.to_file()).map_err(io::Error::other)?;
    let target = paths.state_file(&state.session);
    let temp = dir.join(format!(
        ".{}.{}.tmp",
        state.session.as_str(),
        std::process::id()
    ));
    std::fs::write(&temp, text)?;
    let renamed = std::fs::rename(&temp, &target);
    if renamed.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    renamed
}

pub fn read(paths: &Paths, session: &SessionId) -> Option<ParkState> {
    let text = std::fs::read_to_string(paths.state_file(session)).ok()?;
    let file: StateFile = serde_json::from_str(&text).ok()?;
    Some(ParkState::from_file(file))
}

pub fn set_placeholder_pid(paths: &Paths, session: &SessionId, pid: Option<i32>) -> io::Result<()> {
    let Some(state) = read(paths, session) else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("no park state for {}", session.as_str()),
        ));
    };
    let state = ParkState {
        placeholder_pid: pid,
        ..state
    };
    write(paths, &state)
}

/// Blocks until no other claude-siesta process is parking or cleaning up, and holds that off
/// until the returned file is dropped.
pub fn lock_parking(paths: &Paths) -> io::Result<File> {
    std::fs::create_dir_all(paths.state_dir())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(paths.park_lock_file())?;
    file.lock()?;
    Ok(file)
}

pub fn remove(paths: &Paths, session: &SessionId) -> io::Result<()> {
    match std::fs::remove_file(paths.state_file(session)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub fn mark_resumed(paths: &Paths, session: &SessionId, at: SystemTime) -> io::Result<()> {
    std::fs::create_dir_all(paths.state_dir())?;
    std::fs::write(paths.resumed_file(session), unix_seconds(at).to_string())
}

pub fn resumed_at(paths: &Paths, session: &SessionId) -> Option<SystemTime> {
    let text = std::fs::read_to_string(paths.resumed_file(session)).ok()?;
    let seconds = text.trim().parse().ok()?;
    Some(from_unix_seconds(seconds))
}

fn state_file_ids(paths: &Paths) -> Vec<SessionId> {
    ids_with_suffix(paths, ".json")
}

fn ids_with_suffix(paths: &Paths, suffix: &str) -> Vec<SessionId> {
    let mut ids = Vec::new();
    let Ok(entries) = std::fs::read_dir(paths.state_dir()) else {
        return ids;
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with('.') {
            continue;
        }
        let Some(id) = name.strip_suffix(suffix) else {
            continue;
        };
        ids.push(SessionId::new(id));
    }
    ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    ids
}

pub fn list(paths: &Paths) -> Vec<ParkState> {
    let mut states = Vec::new();
    for id in state_file_ids(paths) {
        let Some(state) = read(paths, &id) else {
            continue;
        };
        states.push(state);
    }
    states
}

pub fn cleanup(paths: &Paths, seen: &[SessionId]) -> Vec<SessionId> {
    for id in ids_with_suffix(paths, ".resumed") {
        if seen.contains(&id) {
            continue;
        }
        let _ = std::fs::remove_file(paths.resumed_file(&id));
    }
    let mut removed = Vec::new();
    for id in state_file_ids(paths) {
        if seen.contains(&id) {
            continue;
        }
        if std::fs::remove_file(paths.state_file(&id)).is_err() {
            continue;
        }
        removed.push(id);
    }
    removed
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use super::*;

    fn temp_home(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("claude-siesta-state-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn state(session: &str) -> ParkState {
        ParkState {
            session: SessionId::new(session),
            conv: ConvId::new("c0acdbe6-1111-2222-3333-444444444444"),
            profile: Profile::Work,
            parked_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1790500000),
            last_assistant_at: SystemTime::UNIX_EPOCH + Duration::from_secs(1790400000),
            placeholder_pid: None,
        }
    }

    #[test]
    fn round_trip() {
        let home = temp_home("round-trip");
        let paths = Paths::from_home(home.clone());
        let written = state("7320056a-aaaa");
        write(&paths, &written).unwrap();
        assert_eq!(
            read(&paths, &SessionId::new("7320056A-AAAA")),
            Some(written)
        );
        let text =
            std::fs::read_to_string(paths.state_file(&SessionId::new("7320056A-AAAA"))).unwrap();
        assert_eq!(
            text,
            r#"{"session":"7320056A-AAAA","conv":"c0acdbe6-1111-2222-3333-444444444444","profile":"work","parked_at":1790500000,"last_assistant_at":1790400000,"placeholder_pid":null}"#
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn overwrite_replaces_and_leaves_no_temp_files() {
        let home = temp_home("overwrite");
        let paths = Paths::from_home(home.clone());
        write(&paths, &state("AAAA")).unwrap();
        let second = ParkState {
            profile: Profile::Personal,
            placeholder_pid: Some(42),
            ..state("AAAA")
        };
        write(&paths, &second).unwrap();
        assert_eq!(read(&paths, &SessionId::new("AAAA")), Some(second));
        let mut names = Vec::new();
        for entry in std::fs::read_dir(paths.state_dir()).unwrap() {
            names.push(entry.unwrap().file_name().into_string().unwrap());
        }
        assert_eq!(names, vec![String::from("AAAA.json")]);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn missing_and_corrupt_read_as_none() {
        let home = temp_home("corrupt");
        let paths = Paths::from_home(home.clone());
        assert_eq!(read(&paths, &SessionId::new("AAAA")), None);
        std::fs::create_dir_all(paths.state_dir()).unwrap();
        std::fs::write(paths.state_file(&SessionId::new("AAAA")), "{not json").unwrap();
        assert_eq!(read(&paths, &SessionId::new("AAAA")), None);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn set_placeholder_pid_keeps_other_fields() {
        let home = temp_home("pid");
        let paths = Paths::from_home(home.clone());
        write(&paths, &state("AAAA")).unwrap();
        set_placeholder_pid(&paths, &SessionId::new("AAAA"), Some(4242)).unwrap();
        let expected = ParkState {
            placeholder_pid: Some(4242),
            ..state("AAAA")
        };
        assert_eq!(read(&paths, &SessionId::new("AAAA")), Some(expected));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn set_placeholder_pid_none_clears_it() {
        let home = temp_home("pid-clear");
        let paths = Paths::from_home(home.clone());
        write(&paths, &state("AAAA")).unwrap();
        set_placeholder_pid(&paths, &SessionId::new("AAAA"), Some(4242)).unwrap();
        set_placeholder_pid(&paths, &SessionId::new("AAAA"), None).unwrap();
        assert_eq!(read(&paths, &SessionId::new("AAAA")), Some(state("AAAA")));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn park_lock_excludes_a_second_holder_until_dropped() {
        let home = temp_home("park-lock");
        let paths = Paths::from_home(home.clone());
        let held = lock_parking(&paths).unwrap();
        let second = File::open(paths.park_lock_file()).unwrap();
        assert!(second.try_lock().is_err());
        drop(held);
        second.try_lock().unwrap();
        assert!(list(&paths).is_empty());
        assert!(cleanup(&paths, &[]).is_empty());
        assert!(paths.park_lock_file().exists());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn set_placeholder_pid_without_state_is_not_found() {
        let home = temp_home("pid-missing");
        let paths = Paths::from_home(home.clone());
        let error = set_placeholder_pid(&paths, &SessionId::new("AAAA"), Some(1)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(read(&paths, &SessionId::new("AAAA")), None);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn remove_deletes_and_tolerates_missing() {
        let home = temp_home("remove");
        let paths = Paths::from_home(home.clone());
        write(&paths, &state("AAAA")).unwrap();
        remove(&paths, &SessionId::new("AAAA")).unwrap();
        assert_eq!(read(&paths, &SessionId::new("AAAA")), None);
        remove(&paths, &SessionId::new("AAAA")).unwrap();
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn list_skips_log_temp_and_corrupt_files() {
        let home = temp_home("list");
        let paths = Paths::from_home(home.clone());
        assert_eq!(list(&paths), Vec::new());
        write(&paths, &state("BBBB")).unwrap();
        write(&paths, &state("AAAA")).unwrap();
        std::fs::write(paths.state_file(&SessionId::new("CCCC")), "garbage").unwrap();
        std::fs::write(paths.log_file(), "a log line\n").unwrap();
        std::fs::write(paths.state_dir().join(".DDDD.1.tmp"), "{}").unwrap();
        assert_eq!(list(&paths), vec![state("AAAA"), state("BBBB")]);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn cleanup_removes_only_unseen_ids() {
        let home = temp_home("cleanup");
        let paths = Paths::from_home(home.clone());
        write(&paths, &state("AAAA")).unwrap();
        write(&paths, &state("BBBB")).unwrap();
        std::fs::write(paths.state_file(&SessionId::new("CCCC")), "garbage").unwrap();
        std::fs::write(paths.log_file(), "a log line\n").unwrap();
        mark_resumed(&paths, &SessionId::new("BBBB"), SystemTime::UNIX_EPOCH).unwrap();
        mark_resumed(&paths, &SessionId::new("DDDD"), SystemTime::UNIX_EPOCH).unwrap();
        let removed = cleanup(&paths, &[SessionId::new("bbbb")]);
        assert_eq!(
            removed,
            vec![SessionId::new("AAAA"), SessionId::new("CCCC")]
        );
        assert_eq!(read(&paths, &SessionId::new("AAAA")), None);
        assert_eq!(read(&paths, &SessionId::new("BBBB")), Some(state("BBBB")));
        assert!(paths.log_file().exists());
        assert!(paths.resumed_file(&SessionId::new("BBBB")).exists());
        assert!(!paths.resumed_file(&SessionId::new("DDDD")).exists());
        assert_eq!(cleanup(&paths, &[SessionId::new("BBBB")]), Vec::new());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn resumed_marker_round_trip() {
        let home = temp_home("resumed");
        let paths = Paths::from_home(home.clone());
        let session = SessionId::new("AAAA");
        assert_eq!(resumed_at(&paths, &session), None);
        let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1790500000);
        mark_resumed(&paths, &session, at).unwrap();
        assert_eq!(resumed_at(&paths, &session), Some(at));
        std::fs::write(paths.resumed_file(&session), "garbage").unwrap();
        assert_eq!(resumed_at(&paths, &session), None);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn cleanup_without_state_dir_is_empty() {
        let home = temp_home("cleanup-empty");
        let paths = Paths::from_home(home.clone());
        assert_eq!(cleanup(&paths, &[]), Vec::new());
        std::fs::remove_dir_all(home).unwrap();
    }
}
