use std::path::{Path, PathBuf};

use crate::ccmap::{Profile, SessionId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
}

impl Paths {
    pub fn from_home(home: PathBuf) -> Paths {
        Paths { home }
    }

    pub fn from_env() -> Option<Paths> {
        let home = std::env::var_os("HOME")?;
        if home.is_empty() {
            return None;
        }
        Some(Paths::from_home(PathBuf::from(home)))
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn cc_map_dir(&self) -> PathBuf {
        self.home.join(".local/state/agterm/cc-map")
    }

    pub fn transcript_root(&self, profile: Profile) -> PathBuf {
        match profile {
            Profile::Personal => self.home.join(".claude"),
            Profile::Work => self.home.join(".claude-work"),
        }
    }

    pub fn state_dir(&self) -> PathBuf {
        self.home.join(".local/state/claude-siesta")
    }

    pub fn state_file(&self, session: &SessionId) -> PathBuf {
        self.state_dir().join(format!("{}.json", session.as_str()))
    }

    pub fn log_file(&self) -> PathBuf {
        self.state_dir().join("claude-siesta.log")
    }

    pub fn park_lock_file(&self) -> PathBuf {
        self.state_dir().join("park.lock")
    }

    pub fn config_file(&self) -> PathBuf {
        self.home.join(".config/claude-siesta/config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths::from_home(PathBuf::from("/fake/home"))
    }

    #[test]
    fn every_path_is_under_home() {
        let paths = paths();
        let session = SessionId::new("7320056A-0000");
        let mut all = Vec::new();
        all.push(paths.cc_map_dir());
        all.push(paths.transcript_root(Profile::Personal));
        all.push(paths.transcript_root(Profile::Work));
        all.push(paths.state_dir());
        all.push(paths.state_file(&session));
        all.push(paths.log_file());
        all.push(paths.park_lock_file());
        all.push(paths.config_file());
        for path in all {
            assert!(path.starts_with("/fake/home"), "{path:?}");
        }
    }

    #[test]
    fn exact_locations() {
        let paths = paths();
        assert_eq!(
            paths.cc_map_dir(),
            PathBuf::from("/fake/home/.local/state/agterm/cc-map")
        );
        assert_eq!(
            paths.state_dir(),
            PathBuf::from("/fake/home/.local/state/claude-siesta")
        );
        assert_eq!(
            paths.log_file(),
            PathBuf::from("/fake/home/.local/state/claude-siesta/claude-siesta.log")
        );
        assert_eq!(
            paths.park_lock_file(),
            PathBuf::from("/fake/home/.local/state/claude-siesta/park.lock")
        );
        assert_eq!(
            paths.config_file(),
            PathBuf::from("/fake/home/.config/claude-siesta/config.toml")
        );
    }

    #[test]
    fn profiles_map_to_their_roots() {
        let paths = paths();
        assert_eq!(
            paths.transcript_root(Profile::Personal),
            PathBuf::from("/fake/home/.claude")
        );
        assert_eq!(
            paths.transcript_root(Profile::Work),
            PathBuf::from("/fake/home/.claude-work")
        );
    }

    #[test]
    fn state_file_uses_uppercase_session_id() {
        let paths = paths();
        let session = SessionId::new("7320056a-abcd");
        assert_eq!(
            paths.state_file(&session),
            PathBuf::from("/fake/home/.local/state/claude-siesta/7320056A-ABCD.json")
        );
    }
}
