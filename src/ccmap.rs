use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::paths::Paths;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionId(String);

impl SessionId {
    pub fn new(id: &str) -> SessionId {
        SessionId(id.to_uppercase())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConvId(String);

impl ConvId {
    pub fn new(id: &str) -> ConvId {
        ConvId(id.to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Personal,
    Work,
}

impl Profile {
    pub fn parse(text: &str) -> Profile {
        match text {
            "work" => Profile::Work,
            _ => Profile::Personal,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Personal => "personal",
            Profile::Work => "work",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEntry {
    pub conv: ConvId,
    pub profile: Profile,
    pub cwd: PathBuf,
    pub ts: SystemTime,
    pub pid: Option<i32>,
}

#[derive(Debug)]
pub enum CcMapError {
    Read { path: PathBuf, error: io::Error },
    Syntax(String),
    MissingConv,
}

impl fmt::Display for CcMapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CcMapError::Read { path, error } => {
                write!(f, "cannot read {}: {error}", path.display())
            }
            CcMapError::Syntax(message) => write!(f, "invalid cc-map entry: {message}"),
            CcMapError::MissingConv => write!(f, "invalid cc-map entry: no conv"),
        }
    }
}

#[derive(Debug, Deserialize)]
struct EntryFile {
    conv: Option<String>,
    profile: Option<String>,
    cwd: Option<PathBuf>,
    ts: u64,
    pid: Option<i32>,
}

impl MapEntry {
    pub fn parse(text: &str) -> Result<MapEntry, CcMapError> {
        let file: EntryFile =
            serde_json::from_str(text).map_err(|error| CcMapError::Syntax(error.to_string()))?;
        let EntryFile {
            conv,
            profile,
            cwd,
            ts,
            pid,
        } = file;
        let Some(conv) = conv else {
            return Err(CcMapError::MissingConv);
        };
        if conv.is_empty() {
            return Err(CcMapError::MissingConv);
        }
        let profile = match profile {
            Some(profile) => Profile::parse(&profile),
            None => Profile::Personal,
        };
        Ok(MapEntry {
            conv: ConvId::new(&conv),
            profile,
            cwd: cwd.unwrap_or_default(),
            ts: SystemTime::UNIX_EPOCH + Duration::from_secs(ts),
            pid,
        })
    }

    pub fn load(paths: &Paths, session: &SessionId) -> Result<Option<MapEntry>, CcMapError> {
        let path = paths.cc_map_dir().join(session.as_str());
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(CcMapError::Read { path, error }),
        };
        let entry = MapEntry::parse(&text)?;
        Ok(Some(entry))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"{"conv":"b1274f92-3c1d-4e2a-9f00-1234567890ab","profile":"personal","cwd":"/home/x/Projects/y","ts":1790419180,"pid":68237}"#;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn temp_home(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("claude-siesta-ccmap-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn session_id_is_uppercase() {
        assert_eq!(SessionId::new("7320056a-abcd").as_str(), "7320056A-ABCD");
    }

    #[test]
    fn full_entry() {
        let entry = MapEntry::parse(FULL).unwrap();
        assert_eq!(
            entry,
            MapEntry {
                conv: ConvId::new("b1274f92-3c1d-4e2a-9f00-1234567890ab"),
                profile: Profile::Personal,
                cwd: PathBuf::from("/home/x/Projects/y"),
                ts: at(1790419180),
                pid: Some(68237),
            }
        );
    }

    #[test]
    fn null_pid() {
        let entry = MapEntry::parse(
            r#"{"conv":"c1","profile":"personal","cwd":"/tmp","ts":1790419180,"pid":null}"#,
        )
        .unwrap();
        assert_eq!(entry.pid, None);
    }

    #[test]
    fn missing_pid() {
        let entry =
            MapEntry::parse(r#"{"conv":"c1","profile":"personal","cwd":"/tmp","ts":1790419180}"#)
                .unwrap();
        assert_eq!(entry.pid, None);
        assert_eq!(entry.ts, at(1790419180));
    }

    #[test]
    fn extra_keys_ignored() {
        let entry = MapEntry::parse(
            r#"{"conv":"c1","profile":"personal","cwd":"/tmp","ts":1790419180,"pid":5,"tsession":"main:1","twindow":"@3"}"#,
        )
        .unwrap();
        assert_eq!(entry.conv, ConvId::new("c1"));
        assert_eq!(entry.pid, Some(5));
    }

    #[test]
    fn work_profile() {
        let entry = MapEntry::parse(
            r#"{"conv":"c1","profile":"work","cwd":"/tmp","ts":1790419180,"pid":5}"#,
        )
        .unwrap();
        assert_eq!(entry.profile, Profile::Work);
    }

    #[test]
    fn unknown_or_missing_profile_is_personal() {
        let entry =
            MapEntry::parse(r#"{"conv":"c1","profile":"other","cwd":"/tmp","ts":1790419180}"#)
                .unwrap();
        assert_eq!(entry.profile, Profile::Personal);
        let entry = MapEntry::parse(r#"{"conv":"c1","cwd":"/tmp","ts":1790419180}"#).unwrap();
        assert_eq!(entry.profile, Profile::Personal);
    }

    #[test]
    fn missing_cwd_is_empty_path() {
        let entry =
            MapEntry::parse(r#"{"conv":"c1","profile":"personal","ts":1790419180}"#).unwrap();
        assert_eq!(entry.cwd, PathBuf::new());
    }

    #[test]
    fn missing_conv_is_error() {
        let error =
            MapEntry::parse(r#"{"profile":"personal","cwd":"/tmp","ts":1790419180,"pid":5}"#)
                .unwrap_err();
        let CcMapError::MissingConv = error else {
            panic!("unexpected error: {error}");
        };
    }

    #[test]
    fn empty_conv_is_error() {
        let error = MapEntry::parse(r#"{"conv":"","ts":1790419180}"#).unwrap_err();
        let CcMapError::MissingConv = error else {
            panic!("unexpected error: {error}");
        };
    }

    #[test]
    fn malformed_json_is_error() {
        let error = MapEntry::parse(r#"{"conv":"c1","ts":"#).unwrap_err();
        let CcMapError::Syntax(_) = error else {
            panic!("unexpected error: {error}");
        };
    }

    #[test]
    fn load_missing_file_is_none() {
        let home = temp_home("missing");
        let paths = Paths::from_home(home.clone());
        let entry = MapEntry::load(&paths, &SessionId::new("ABC")).unwrap();
        assert_eq!(entry, None);
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn load_reads_uppercase_file_name() {
        let home = temp_home("present");
        let paths = Paths::from_home(home.clone());
        std::fs::create_dir_all(paths.cc_map_dir()).unwrap();
        std::fs::write(paths.cc_map_dir().join("7320056A-ABCD"), FULL).unwrap();
        let entry = MapEntry::load(&paths, &SessionId::new("7320056a-abcd"))
            .unwrap()
            .unwrap();
        assert_eq!(entry.pid, Some(68237));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn load_malformed_file_is_error() {
        let home = temp_home("malformed");
        let paths = Paths::from_home(home.clone());
        std::fs::create_dir_all(paths.cc_map_dir()).unwrap();
        std::fs::write(paths.cc_map_dir().join("ABC"), "not json").unwrap();
        let error = MapEntry::load(&paths, &SessionId::new("ABC")).unwrap_err();
        let CcMapError::Syntax(_) = error else {
            panic!("unexpected error: {error}");
        };
        std::fs::remove_dir_all(home).unwrap();
    }
}
