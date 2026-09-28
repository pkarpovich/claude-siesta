use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Identity {
    device: u64,
    inode: u64,
}

/// The binary this process runs from: the path it was started by, and the file behind it.
///
/// The path is kept as invoked, symlinks and all. A cask links `bin/claude-siesta` to a
/// versioned directory that an upgrade deletes, so only the link is a path the launchd agent
/// can keep naming; the file it resolves to is what tells an upgrade apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Executable {
    path: PathBuf,
    identity: Identity,
}

impl Executable {
    pub fn current() -> Option<Executable> {
        let path = std::env::current_exe().ok()?;
        Executable::at(path)
    }

    pub fn at(path: PathBuf) -> Option<Executable> {
        let identity = identity(&path)?;
        Some(Executable { path, identity })
    }

    pub fn path(&self) -> &Path {
        let Executable { path, identity: _ } = self;
        path
    }

    pub fn swapped(&self) -> bool {
        let Executable { path, identity } = self;
        self::identity(path) != Some(*identity)
    }
}

fn identity(path: &Path) -> Option<Identity> {
    let found = fs::metadata(path).ok()?;
    Some(Identity {
        device: found.dev(),
        inode: found.ino(),
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "claude-siesta-executable-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn untouched_binary_is_not_swapped() {
        let dir = temp_dir("untouched");
        let binary = dir.join("claude-siesta");
        fs::write(&binary, b"v1").unwrap();
        let executable = Executable::at(binary.clone()).unwrap();
        assert!(!executable.swapped());
        assert_eq!(executable.path(), binary);
    }

    #[test]
    fn binary_replaced_by_rename_is_swapped() {
        let dir = temp_dir("rename");
        let binary = dir.join("claude-siesta");
        fs::write(&binary, b"v1").unwrap();
        let executable = Executable::at(binary.clone()).unwrap();
        let fresh = dir.join("claude-siesta.tmp");
        fs::write(&fresh, b"v2").unwrap();
        fs::rename(&fresh, &binary).unwrap();
        assert!(executable.swapped());
    }

    #[test]
    fn removed_binary_is_swapped() {
        let dir = temp_dir("removed");
        let binary = dir.join("claude-siesta");
        fs::write(&binary, b"v1").unwrap();
        let executable = Executable::at(binary.clone()).unwrap();
        fs::remove_file(&binary).unwrap();
        assert!(executable.swapped());
    }

    #[test]
    fn relinked_cask_symlink_is_swapped_and_keeps_the_link_path() {
        let dir = temp_dir("relink");
        let old = dir.join("0.1.0");
        let new = dir.join("0.1.1");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(old.join("claude-siesta"), b"v1").unwrap();
        fs::write(new.join("claude-siesta"), b"v2").unwrap();
        let link = dir.join("claude-siesta");
        symlink(old.join("claude-siesta"), &link).unwrap();
        let executable = Executable::at(link.clone()).unwrap();
        assert_eq!(executable.path(), link);
        assert!(!executable.swapped());
        fs::remove_file(&link).unwrap();
        symlink(new.join("claude-siesta"), &link).unwrap();
        assert!(executable.swapped());
    }
}
