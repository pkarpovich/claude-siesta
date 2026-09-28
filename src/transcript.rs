use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;
use serde_json::Value;

use crate::ccmap::{ConvId, MapEntry, Profile};
use crate::paths::Paths;
use crate::time::parse_rfc3339;

const TAIL_BYTES: u64 = 400 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastAssistant {
    pub at: SystemTime,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TailStart {
    FileStart,
    MidFile,
}

#[derive(Debug, Deserialize)]
struct Record {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    #[serde(default)]
    content: Value,
}

pub fn last_assistant(bytes: &[u8], start: TailStart) -> Option<LastAssistant> {
    let mut lines = bytes.split(|byte| *byte == b'\n');
    match start {
        TailStart::FileStart => {}
        TailStart::MidFile => {
            lines.next();
        }
    }
    let mut at = None;
    let mut text = String::new();
    for line in lines {
        let Some((line_at, line_text)) = assistant_record(line) else {
            continue;
        };
        at = Some(line_at);
        if !line_text.trim().is_empty() {
            text = line_text;
        }
    }
    let at = at?;
    Some(LastAssistant { at, text })
}

fn assistant_record(line: &[u8]) -> Option<(SystemTime, String)> {
    let record: Record = serde_json::from_slice(line).ok()?;
    let Record {
        kind,
        timestamp,
        message,
    } = record;
    if kind.as_deref() != Some("assistant") {
        return None;
    }
    let at = parse_rfc3339(&timestamp?)?;
    let text = match message {
        Some(Message { content }) => content_text(content),
        None => String::new(),
    };
    Some((at, text))
}

fn content_text(content: Value) -> String {
    let items = match content {
        Value::String(text) => return text,
        Value::Array(items) => items,
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::Object(_) => {
            return String::new();
        }
    };
    let mut parts = Vec::new();
    for item in items {
        if item.get("type").and_then(Value::as_str) != Some("text") {
            continue;
        }
        let Some(text) = item.get("text").and_then(Value::as_str) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        parts.push(text.to_string());
    }
    parts.join("\n\n")
}

pub fn find_transcript(paths: &Paths, profile: Profile, conv: &ConvId) -> Option<PathBuf> {
    let projects = paths.transcript_root(profile).join("projects");
    let entries = std::fs::read_dir(projects).ok()?;
    let file_name = format!("{}.jsonl", conv.as_str());
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let candidate = entry.path().join(&file_name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub fn load_last(paths: &Paths, entry: &MapEntry) -> Option<LastAssistant> {
    let MapEntry {
        conv,
        profile,
        cwd: _,
        ts: _,
        pid: _,
    } = entry;
    let path = find_transcript(paths, *profile, conv)?;
    let (bytes, start) = read_tail(&path).ok()?;
    last_assistant(&bytes, start)
}

pub fn read_tail(path: &Path) -> io::Result<(Vec<u8>, TailStart)> {
    read_last_bytes(path, TAIL_BYTES)
}

pub fn read_last_bytes(path: &Path, limit: u64) -> io::Result<(Vec<u8>, TailStart)> {
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let offset = len.saturating_sub(limit);
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let start = match offset {
        0 => TailStart::FileStart,
        _ => TailStart::MidFile,
    };
    Ok((bytes, start))
}

pub fn idle(now: SystemTime, last: Option<&LastAssistant>, map: &MapEntry) -> Duration {
    let since = match last {
        Some(LastAssistant { at, text: _ }) => (*at).max(map.ts),
        None => map.ts,
    };
    now.duration_since(since).unwrap_or(Duration::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSISTANT_ARRAY: &str = r#"{"type":"assistant","timestamp":"2026-08-11T22:41:01.578Z","message":{"content":[{"type":"text","text":"First part."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}},{"type":"text","text":"Second part."}]}}"#;
    const ASSISTANT_STRING: &str = r#"{"type":"assistant","timestamp":"2026-08-11T22:42:00Z","message":{"content":"Plain string reply."}}"#;
    const ASSISTANT_TOOL_ONLY: &str = r#"{"type":"assistant","timestamp":"2026-08-11T22:50:00Z","message":{"content":[{"type":"tool_use","id":"t2","name":"Read","input":{}}]}}"#;
    const USER: &str =
        r#"{"type":"user","timestamp":"2026-08-11T23:00:00Z","message":{"content":"a question"}}"#;

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn lines(records: &[&str]) -> Vec<u8> {
        let mut text = String::new();
        for record in records {
            text.push_str(record);
            text.push('\n');
        }
        text.into_bytes()
    }

    fn entry(ts: u64) -> MapEntry {
        MapEntry {
            conv: ConvId::new("c1"),
            profile: Profile::Personal,
            cwd: PathBuf::new(),
            ts: at(ts),
            pid: None,
        }
    }

    fn temp_home(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "claude-siesta-transcript-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn content_array_joins_text_items() {
        let last = last_assistant(&lines(&[ASSISTANT_ARRAY]), TailStart::FileStart).unwrap();
        assert_eq!(last.text, "First part.\n\nSecond part.");
        assert_eq!(last.at, at(1786488061) + Duration::from_millis(578));
    }

    #[test]
    fn content_as_string() {
        let last = last_assistant(
            &lines(&[ASSISTANT_ARRAY, ASSISTANT_STRING]),
            TailStart::FileStart,
        )
        .unwrap();
        assert_eq!(last.text, "Plain string reply.");
        assert_eq!(last.at, parse_rfc3339("2026-08-11T22:42:00Z").unwrap());
    }

    #[test]
    fn tool_only_record_keeps_earlier_text() {
        let last = last_assistant(
            &lines(&[ASSISTANT_STRING, ASSISTANT_TOOL_ONLY, USER]),
            TailStart::FileStart,
        )
        .unwrap();
        assert_eq!(last.text, "Plain string reply.");
        assert_eq!(last.at, parse_rfc3339("2026-08-11T22:50:00Z").unwrap());
    }

    #[test]
    fn no_assistant_records() {
        assert_eq!(last_assistant(&lines(&[USER]), TailStart::FileStart), None);
        assert_eq!(last_assistant(b"", TailStart::FileStart), None);
    }

    #[test]
    fn partial_first_line_dropped_mid_file() {
        let partial = &ASSISTANT_STRING[10..];
        let bytes = lines(&[ASSISTANT_STRING, USER]);
        let last = last_assistant(&bytes, TailStart::MidFile);
        assert_eq!(last, None);
        let mut bytes = lines(&[partial]);
        bytes.extend(lines(&[ASSISTANT_ARRAY]));
        let last = last_assistant(&bytes, TailStart::MidFile).unwrap();
        assert_eq!(last.text, "First part.\n\nSecond part.");
    }

    #[test]
    fn malformed_lines_interleaved() {
        let bytes = lines(&[
            "not json",
            ASSISTANT_ARRAY,
            r#"{"type":"assistant","timestamp":"garbage","message":{"content":"bad time"}}"#,
            r#"{"type":"assistant","message":{"content":"no time"}}"#,
            "{\"type\":\"assistant\",",
            USER,
        ]);
        let last = last_assistant(&bytes, TailStart::FileStart).unwrap();
        assert_eq!(last.text, "First part.\n\nSecond part.");
    }

    #[test]
    fn offset_timestamp() {
        let bytes = lines(&[
            r#"{"type":"assistant","timestamp":"2026-08-12T00:41:01+02:00","message":{"content":"hi"}}"#,
        ]);
        let last = last_assistant(&bytes, TailStart::FileStart).unwrap();
        assert_eq!(last.at, at(1786488061));
    }

    #[test]
    fn idle_from_last_assistant() {
        let last = LastAssistant {
            at: at(4000),
            text: String::new(),
        };
        assert_eq!(
            idle(at(4600), Some(&last), &entry(1000)),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn idle_from_map_ts_after_resume() {
        let last = LastAssistant {
            at: at(1000),
            text: String::new(),
        };
        assert_eq!(
            idle(at(4600), Some(&last), &entry(4000)),
            Duration::from_secs(600)
        );
    }

    #[test]
    fn idle_falls_back_to_map_ts() {
        assert_eq!(idle(at(4600), None, &entry(4000)), Duration::from_secs(600));
    }

    #[test]
    fn future_timestamp_is_zero_idle() {
        let last = LastAssistant {
            at: at(9000),
            text: String::new(),
        };
        assert_eq!(idle(at(4600), Some(&last), &entry(0)), Duration::ZERO);
        assert_eq!(idle(at(4600), None, &entry(9000)), Duration::ZERO);
    }

    #[test]
    fn find_transcript_for_both_profiles() {
        let home = temp_home("find");
        let paths = Paths::from_home(home.clone());
        let conv = ConvId::new("b1274f92-0000");
        let personal = home.join(".claude/projects/-Users-x-a/b1274f92-0000.jsonl");
        let work = home.join(".claude-work/projects/-Users-x-b/b1274f92-0000.jsonl");
        std::fs::create_dir_all(home.join(".claude/projects/-Users-x-other")).unwrap();
        std::fs::create_dir_all(personal.parent().unwrap()).unwrap();
        std::fs::create_dir_all(work.parent().unwrap()).unwrap();
        std::fs::write(&personal, "").unwrap();
        std::fs::write(&work, "").unwrap();
        assert_eq!(
            find_transcript(&paths, Profile::Personal, &conv),
            Some(personal)
        );
        assert_eq!(find_transcript(&paths, Profile::Work, &conv), Some(work));
        assert_eq!(
            find_transcript(&paths, Profile::Personal, &ConvId::new("missing")),
            None
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn find_transcript_without_projects_dir() {
        let home = temp_home("empty");
        let paths = Paths::from_home(home.clone());
        assert_eq!(
            find_transcript(&paths, Profile::Work, &ConvId::new("c1")),
            None
        );
        std::fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn read_tail_of_small_and_large_files() {
        let home = temp_home("tail");
        let small = home.join("small.jsonl");
        std::fs::write(&small, ASSISTANT_STRING).unwrap();
        let (bytes, start) = read_tail(&small).unwrap();
        assert_eq!(bytes, ASSISTANT_STRING.as_bytes());
        assert_eq!(start, TailStart::FileStart);

        let large = home.join("large.jsonl");
        let mut content = lines(&[USER]).repeat(10_000);
        content.extend(lines(&[ASSISTANT_STRING]));
        std::fs::write(&large, &content).unwrap();
        let (bytes, start) = read_tail(&large).unwrap();
        assert_eq!(bytes.len() as u64, TAIL_BYTES);
        assert_eq!(start, TailStart::MidFile);
        let last = last_assistant(&bytes, start).unwrap();
        assert_eq!(last.text, "Plain string reply.");
        std::fs::remove_dir_all(home).unwrap();
    }
}
