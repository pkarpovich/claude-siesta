use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use crate::paths::Paths;

const DEFAULT_PARK_AFTER: Duration = Duration::from_secs(2 * 60 * 60);
const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub park_after: Duration,
    pub poll_interval: Duration,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            park_after: DEFAULT_PARK_AFTER,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    Read { path: PathBuf, error: io::Error },
    Syntax(String),
    InvalidDuration { key: &'static str, value: String },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Read { path, error } => {
                write!(f, "cannot read {}: {error}", path.display())
            }
            ConfigError::Syntax(message) => write!(f, "invalid config: {message}"),
            ConfigError::InvalidDuration { key, value } => write!(
                f,
                "invalid config: {key} = {value:?} is not a duration like 30s, 10m, 2h or 1d"
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    park_after: Option<String>,
    poll_interval: Option<String>,
}

impl Config {
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let file: ConfigFile =
            toml::from_str(text).map_err(|error| ConfigError::Syntax(error.to_string()))?;
        let ConfigFile {
            park_after,
            poll_interval,
        } = file;
        let defaults = Config::default();
        Ok(Config {
            park_after: duration_or("park_after", park_after, defaults.park_after)?,
            poll_interval: duration_or("poll_interval", poll_interval, defaults.poll_interval)?,
        })
    }

    pub fn load(paths: &Paths) -> Result<Config, ConfigError> {
        let path = paths.config_file();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(error) => return Err(ConfigError::Read { path, error }),
        };
        Config::parse(&text)
    }
}

fn duration_or(
    key: &'static str,
    value: Option<String>,
    default: Duration,
) -> Result<Duration, ConfigError> {
    let Some(value) = value else {
        return Ok(default);
    };
    let Some(duration) = parse_duration(&value) else {
        return Err(ConfigError::InvalidDuration { key, value });
    };
    Ok(duration)
}

/// Parses `<positive integer><s|m|h|d>`, e.g. `90s`, `10m`, `2h`, `1d`.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let unit = text.chars().last()?;
    let seconds_per_unit: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 60 * 60,
        'd' => 24 * 60 * 60,
        _ => return None,
    };
    let digits = &text[..text.len() - unit.len_utf8()];
    if digits.is_empty() {
        return None;
    }
    for byte in digits.bytes() {
        if !byte.is_ascii_digit() {
            return None;
        }
    }
    let count: u64 = digits.parse().ok()?;
    if count == 0 {
        return None;
    }
    let seconds = count.checked_mul(seconds_per_unit)?;
    Some(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 60 * 60;

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "claude-siesta-config-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn empty_file_gives_defaults() {
        let config = Config::parse("").unwrap();
        assert_eq!(config.park_after, Duration::from_secs(2 * HOUR));
        assert_eq!(config.poll_interval, Duration::from_secs(10 * 60));
    }

    #[test]
    fn both_keys_set() {
        let config = Config::parse("park_after = \"3h\"\npoll_interval = \"30s\"\n").unwrap();
        assert_eq!(config.park_after, Duration::from_secs(3 * HOUR));
        assert_eq!(config.poll_interval, Duration::from_secs(30));
    }

    #[test]
    fn one_key_set_keeps_other_default() {
        let config = Config::parse("park_after = \"1m\"").unwrap();
        assert_eq!(config.park_after, Duration::from_secs(60));
        assert_eq!(config.poll_interval, Duration::from_secs(10 * 60));
    }

    #[test]
    fn each_unit() {
        assert_eq!(parse_duration("45s"), Some(Duration::from_secs(45)));
        assert_eq!(parse_duration("10m"), Some(Duration::from_secs(600)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(2 * HOUR)));
        assert_eq!(parse_duration("1d"), Some(Duration::from_secs(24 * HOUR)));
    }

    #[test]
    fn invalid_unit() {
        assert_eq!(parse_duration("5w"), None);
        assert_eq!(parse_duration("5ms"), None);
        assert_eq!(parse_duration("5H"), None);
        assert_eq!(parse_duration("5ч"), None);
    }

    #[test]
    fn zero_rejected() {
        assert_eq!(parse_duration("0m"), None);
        assert_eq!(parse_duration("000s"), None);
    }

    #[test]
    fn negative_and_garbage_rejected() {
        let mut inputs = Vec::new();
        inputs.push("-5m");
        inputs.push("+5m");
        inputs.push("");
        inputs.push("m");
        inputs.push("5");
        inputs.push("abc");
        inputs.push("1.5h");
        inputs.push("5 m");
        inputs.push(" 5m");
        inputs.push("99999999999999999999d");
        for input in inputs {
            assert_eq!(parse_duration(input), None, "{input:?}");
        }
    }

    #[test]
    fn invalid_value_names_the_key() {
        let error = Config::parse("poll_interval = \"soon\"").unwrap_err();
        let ConfigError::InvalidDuration { key, value } = &error else {
            panic!("unexpected error: {error}");
        };
        assert_eq!(*key, "poll_interval");
        assert_eq!(value, "soon");
        assert!(error.to_string().contains("poll_interval"));
    }

    #[test]
    fn non_string_value_rejected() {
        let error = Config::parse("park_after = 7200").unwrap_err();
        assert!(error.to_string().contains("park_after"), "{error}");
    }

    #[test]
    fn unknown_key_rejected() {
        let error = Config::parse("park_after = \"2h\"\npin_after = \"1h\"\n").unwrap_err();
        let ConfigError::Syntax(message) = &error else {
            panic!("unexpected error: {error}");
        };
        assert!(message.contains("pin_after"), "{message}");
    }

    #[test]
    fn malformed_toml_rejected() {
        let error = Config::parse("park_after = ").unwrap_err();
        let ConfigError::Syntax(_) = error else {
            panic!("unexpected error: {error}");
        };
    }

    #[test]
    fn load_missing_file_gives_defaults() {
        let home = temp_home("missing");
        let config = Config::load(&Paths::from_home(home.clone())).unwrap();
        assert_eq!(config, Config::default());
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn load_reads_file() {
        let home = temp_home("present");
        let paths = Paths::from_home(home.clone());
        let file = paths.config_file();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "park_after = \"4h\"\n").unwrap();
        let config = Config::load(&paths).unwrap();
        assert_eq!(config.park_after, Duration::from_secs(4 * HOUR));
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn load_invalid_file_is_error() {
        let home = temp_home("invalid");
        let paths = Paths::from_home(home.clone());
        let file = paths.config_file();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "park_after = \"forever\"\n").unwrap();
        let error = Config::load(&paths).unwrap_err();
        let ConfigError::InvalidDuration { key, value: _ } = error else {
            panic!("unexpected error: {error}");
        };
        assert_eq!(key, "park_after");
        std::fs::remove_dir_all(home).unwrap();
    }
}
