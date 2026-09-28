use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::executable::Executable;
use crate::paths::Paths;

const LAUNCHCTL: &str = "/bin/launchctl";
const UNLOAD_TIMEOUT: Duration = Duration::from_secs(5);
const UNLOAD_POLL: Duration = Duration::from_millis(100);

pub const LABEL: &str = "dev.pkarpovich.claude-siesta";

#[derive(Debug)]
pub enum ServiceError {
    Executable,
    Directory {
        path: PathBuf,
        source: std::io::Error,
    },
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    Remove {
        path: PathBuf,
        source: std::io::Error,
    },
    Launchctl(std::io::Error),
    Bootstrap(PathBuf),
    Bootout,
}

impl fmt::Display for ServiceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServiceError::Executable => write!(f, "the running binary could not be located"),
            ServiceError::Directory { path, source } => {
                write!(f, "{} could not be created: {source}", path.display())
            }
            ServiceError::Write { path, source } => {
                write!(f, "{} could not be written: {source}", path.display())
            }
            ServiceError::Remove { path, source } => {
                write!(f, "{} could not be removed: {source}", path.display())
            }
            ServiceError::Launchctl(source) => write!(f, "launchctl could not be run: {source}"),
            ServiceError::Bootstrap(path) => {
                write!(f, "launchctl refused to load {}", path.display())
            }
            ServiceError::Bootout => write!(f, "{LABEL} is still loaded after bootout"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub agent: PathBuf,
    pub log: PathBuf,
    pub errors: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub program: PathBuf,
    pub agent: PathBuf,
}

pub fn layout(paths: &Paths) -> Layout {
    let home = paths.home();
    let logs = home.join("Library").join("Logs").join("claude-siesta");
    Layout {
        agent: home
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{LABEL}.plist")),
        log: logs.join("claude-siesta.log"),
        errors: logs.join("claude-siesta.err.log"),
    }
}

pub fn install(paths: &Paths) -> Result<Installed, ServiceError> {
    let Some(executable) = Executable::current() else {
        return Err(ServiceError::Executable);
    };
    let program = executable.path().to_path_buf();
    let layout = layout(paths);
    unload()?;
    let Layout {
        agent,
        log,
        errors: _,
    } = &layout;
    create_dir(parent_of(agent))?;
    create_dir(parent_of(log))?;
    let contents = agent_plist(&program, &layout);
    std::fs::write(agent, contents).map_err(|source| ServiceError::Write {
        path: agent.clone(),
        source,
    })?;
    bootstrap(agent)?;
    Ok(Installed {
        program,
        agent: agent.clone(),
    })
}

pub fn uninstall(paths: &Paths) -> Result<Layout, ServiceError> {
    let layout = layout(paths);
    unload()?;
    remove_file(&layout.agent)?;
    Ok(layout)
}

fn unload() -> Result<(), ServiceError> {
    let target = format!("gui/{}/{LABEL}", nix::unistd::getuid());
    Command::new(LAUNCHCTL)
        .args(["bootout", &target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(ServiceError::Launchctl)?;
    let deadline = Instant::now() + UNLOAD_TIMEOUT;
    while Instant::now() < deadline {
        let loaded = Command::new(LAUNCHCTL)
            .args(["print", &target])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(ServiceError::Launchctl)?;
        if !loaded.success() {
            return Ok(());
        }
        thread::sleep(UNLOAD_POLL);
    }
    Err(ServiceError::Bootout)
}

fn bootstrap(agent: &Path) -> Result<(), ServiceError> {
    let domain = format!("gui/{}", nix::unistd::getuid());
    let status = Command::new(LAUNCHCTL)
        .arg("bootstrap")
        .arg(&domain)
        .arg(agent)
        .status()
        .map_err(ServiceError::Launchctl)?;
    if status.success() {
        return Ok(());
    }
    Err(ServiceError::Bootstrap(agent.to_path_buf()))
}

fn parent_of(path: &Path) -> &Path {
    let Some(parent) = path.parent() else {
        return path;
    };
    parent
}

fn create_dir(path: &Path) -> Result<(), ServiceError> {
    std::fs::create_dir_all(path).map_err(|source| ServiceError::Directory {
        path: path.to_path_buf(),
        source,
    })
}

fn remove_file(path: &Path) -> Result<(), ServiceError> {
    let Err(source) = std::fs::remove_file(path) else {
        return Ok(());
    };
    if source.kind() == std::io::ErrorKind::NotFound {
        return Ok(());
    }
    Err(ServiceError::Remove {
        path: path.to_path_buf(),
        source,
    })
}

pub fn agent_plist(program: &Path, layout: &Layout) -> String {
    let Layout {
        agent: _,
        log,
        errors,
    } = layout;
    let program = escape(&program.display().to_string());
    let log = escape(&log.display().to_string());
    let errors = escape(&errors.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{LABEL}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{program}</string>
		<string>daemon</string>
	</array>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<dict>
		<key>PathState</key>
		<dict>
			<key>{program}</key>
			<true/>
		</dict>
	</dict>
	<key>ThrottleInterval</key>
	<integer>30</integer>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{errors}</string>
</dict>
</plist>
"#
    )
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Paths {
        Paths::from_home(PathBuf::from("/home/test"))
    }

    #[test]
    fn layout_lives_under_home() {
        let Layout { agent, log, errors } = layout(&paths());
        assert_eq!(
            agent,
            PathBuf::from("/home/test/Library/LaunchAgents/dev.pkarpovich.claude-siesta.plist")
        );
        assert_eq!(
            log,
            PathBuf::from("/home/test/Library/Logs/claude-siesta/claude-siesta.log")
        );
        assert_eq!(
            errors,
            PathBuf::from("/home/test/Library/Logs/claude-siesta/claude-siesta.err.log")
        );
    }

    #[test]
    fn plist_runs_the_daemon_from_the_program_path_and_keeps_it_alive_on_it() {
        let plist = agent_plist(
            Path::new("/opt/homebrew/bin/claude-siesta"),
            &layout(&paths()),
        );
        assert!(plist.contains("<string>dev.pkarpovich.claude-siesta</string>"));
        assert!(plist.contains(
            "<string>/opt/homebrew/bin/claude-siesta</string>\n\t\t<string>daemon</string>"
        ));
        assert!(plist.contains("<key>PathState</key>"));
        assert!(plist.contains("<key>/opt/homebrew/bin/claude-siesta</key>"));
        assert!(plist.contains("<integer>30</integer>"));
    }

    #[test]
    fn plist_escapes_xml() {
        let plist = agent_plist(Path::new("/tmp/a&b<c>/claude-siesta"), &layout(&paths()));
        assert!(plist.contains("/tmp/a&amp;b&lt;c&gt;/claude-siesta"));
        assert!(!plist.contains("a&b"));
    }

    #[test]
    fn plist_passes_plutil_lint() {
        let dir =
            std::env::temp_dir().join(format!("claude-siesta-service-lint-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("{LABEL}.plist"));
        std::fs::write(
            &file,
            agent_plist(
                Path::new("/opt/homebrew/bin/claude-siesta"),
                &layout(&paths()),
            ),
        )
        .unwrap();
        let status = Command::new("/usr/bin/plutil")
            .arg("-lint")
            .arg(&file)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
