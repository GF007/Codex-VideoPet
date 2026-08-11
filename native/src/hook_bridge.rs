use crate::{
    core::{now_ms, CodexEventRequest},
    runtime::API_ADDRESS,
};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

const MAX_INPUT_BYTES: usize = 512 * 1024;
const MAX_OUTBOX_FILES: usize = 2_048;
const MAX_OUTBOX_BYTES: u64 = 32 * 1024 * 1024;
const HTTP_TIMEOUT: Duration = Duration::from_millis(250);
const OWNER_ID: &str = "codex-pet-overlay";

pub fn run_from_stdin() {
    let mut input = std::io::stdin().take((MAX_INPUT_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    if input.read_to_end(&mut bytes).is_err() || bytes.len() > MAX_INPUT_BYTES {
        return;
    }
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return;
    };
    let Some(payload) = build_payload(&value) else {
        return;
    };
    let pending = persist_pending_event(&payload).ok();
    if post_event(&payload).is_ok() {
        if let Some(path) = pending {
            let _ = fs::remove_file(path);
        }
        return;
    }
    let _ = start_pet_once();
}

pub fn self_test() -> Result<(), String> {
    let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
    for (event, state, activity) in [
        ("SessionStart", "idle", "starting"),
        ("SessionEnd", "idle", "idle"),
        ("UserPromptSubmit", "running", "working"),
        ("PreToolUse", "running", "working"),
        ("PermissionRequest", "waiting", "waiting"),
        ("PostToolUse", "running", "working"),
        ("PreCompact", "running", "working"),
        ("PostCompact", "running", "working"),
        ("SubagentStart", "running", "subagent"),
        ("SubagentStop", "running", "working"),
        ("Stop", "review", "ready"),
    ] {
        let input = serde_json::json!({
            "hook_event_name": event,
            "session_id": session_id,
            "turn_id": "turn-1",
            "prompt": "must-not-persist",
            "transcript_path": "must-not-persist",
            "cwd": "must-not-persist",
            "tool_response": {"output": "must-not-persist"}
        });
        let payload = build_payload(&input).ok_or_else(|| format!("{event} was not mapped"))?;
        if payload.state != state || payload.activity != activity {
            return Err(format!("{event} mapping is incorrect"));
        }
        let serialized = serde_json::to_string(&payload)
            .map_err(|_| "unable to serialize self-test payload".to_owned())?;
        for sensitive in [
            "prompt",
            "transcript",
            "cwd",
            "tool_response",
            "must-not-persist",
        ] {
            if serialized.contains(sensitive) {
                return Err(format!("sensitive field leaked: {sensitive}"));
            }
        }
    }
    let failed = serde_json::json!({
        "hook_event_name": "PostToolUse",
        "session_id": session_id,
        "tool_response": {"exit_code": 1}
    });
    if build_payload(&failed).map(|payload| payload.activity) != Some("recovering".to_owned()) {
        return Err("failed tool activity mapping is incorrect".to_owned());
    }
    Ok(())
}

pub fn build_payload(input: &Value) -> Option<CodexEventRequest> {
    let object = input.as_object()?;
    let session_id = object.get("session_id")?.as_str()?.to_owned();
    if !is_session_id(&session_id) {
        return None;
    }
    let event = ["hook_event_name", "event_name", "event"]
        .iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))?
        .to_owned();
    let (state, activity) = map_hook(&event, input)?;
    let turn_id = object
        .get("turn_id")
        .and_then(Value::as_str)
        .filter(|value| is_turn_id(value))
        .map(str::to_owned);
    Some(CodexEventRequest {
        schema_version: 1,
        event_id: random_event_id().ok()?,
        session_id,
        turn_id,
        hook_event_name: event,
        state: state.to_owned(),
        activity,
        emitted_at_ms: now_ms(),
    })
}

fn map_hook(event: &str, input: &Value) -> Option<(&'static str, String)> {
    let result = match event {
        "SessionStart" => ("idle", "starting".to_owned()),
        "SessionEnd" => ("idle", "idle".to_owned()),
        "UserPromptSubmit" => ("running", "working".to_owned()),
        "PreToolUse" => (
            "running",
            tool_activity(
                input
                    .get("tool_name")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .to_owned(),
        ),
        "PermissionRequest" => ("waiting", "waiting".to_owned()),
        "PostToolUse" if shallow_failure(input) => ("running", "recovering".to_owned()),
        "PostToolUse" | "PreCompact" | "PostCompact" | "SubagentStop" => {
            ("running", "working".to_owned())
        }
        "SubagentStart" => ("running", "subagent".to_owned()),
        "Stop" => ("review", "ready".to_owned()),
        _ => return None,
    };
    Some(result)
}

fn shallow_failure(input: &Value) -> bool {
    input.get("is_error").and_then(Value::as_bool) == Some(true)
        || input
            .get("tool_response")
            .and_then(|value| value.get("is_error"))
            .and_then(Value::as_bool)
            == Some(true)
        || input
            .get("tool_response")
            .and_then(|value| value.get("ok"))
            .and_then(Value::as_bool)
            == Some(false)
        || input
            .get("tool_response")
            .and_then(|value| value.get("exit_code"))
            .and_then(Value::as_i64)
            .is_some_and(|code| code != 0)
}

fn tool_activity(tool_name: &str) -> &'static str {
    let tool = tool_name.to_ascii_lowercase();
    if tool == "bash" || tool.contains("exec") || tool.contains("shell") {
        "command"
    } else if tool.contains("read") || tool.contains("view") {
        "reading"
    } else if tool.contains("write") || tool.contains("edit") || tool.contains("patch") {
        "editing"
    } else if tool.contains("search") || tool.contains("find") || tool.contains("web") {
        "searching"
    } else if tool == "agent" || tool.contains("spawn_agent") {
        "subagent"
    } else {
        "working"
    }
}

fn is_session_id(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(index, ch)| match index {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        })
}

fn is_turn_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn random_event_id() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "unable to create event id".to_owned())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn events_directory() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| {
            root.join("com.local.codex-pet-overlay")
                .join("session-data")
                .join("events")
        })
        .ok_or_else(|| "LOCALAPPDATA is unavailable".to_owned())
}

fn persist_pending_event(payload: &CodexEventRequest) -> Result<PathBuf, String> {
    let directory = events_directory()?;
    fs::create_dir_all(&directory).map_err(|_| "unable to create event outbox".to_owned())?;
    let final_path = directory.join(format!("{}.json", payload.event_id));
    let temporary = directory.join(format!("{}.{}.tmp", payload.event_id, std::process::id()));
    let bytes = serde_json::to_vec(payload).map_err(|_| "unable to serialize event".to_owned())?;
    trim_event_outbox(&directory, bytes.len() as u64)?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| "unable to create pending event".to_owned())?;
        file.write_all(&bytes)
            .map_err(|_| "unable to write pending event".to_owned())?;
        file.sync_all()
            .map_err(|_| "unable to flush pending event".to_owned())?;
        drop(file);
        fs::rename(&temporary, &final_path)
            .map_err(|_| "unable to commit pending event".to_owned())?;
        Ok(final_path.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn trim_event_outbox(directory: &Path, incoming_bytes: u64) -> Result<(), String> {
    let mut files = fs::read_dir(directory)
        .map_err(|_| "unable to inspect event outbox".to_owned())?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some((metadata.modified().ok(), path, metadata.len()))
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.file_name().cmp(&right.1.file_name()))
    });
    let mut total_bytes = files.iter().map(|file| file.2).sum::<u64>();
    let mut remaining_files = files.len();
    for (_, path, length) in files {
        if remaining_files < MAX_OUTBOX_FILES
            && total_bytes.saturating_add(incoming_bytes) <= MAX_OUTBOX_BYTES
        {
            break;
        }
        fs::remove_file(path).map_err(|_| "unable to trim event outbox".to_owned())?;
        remaining_files = remaining_files.saturating_sub(1);
        total_bytes = total_bytes.saturating_sub(length);
    }
    if remaining_files >= MAX_OUTBOX_FILES
        || total_bytes.saturating_add(incoming_bytes) > MAX_OUTBOX_BYTES
    {
        return Err("event outbox capacity is exhausted".to_owned());
    }
    Ok(())
}

fn post_event(payload: &CodexEventRequest) -> Result<(), String> {
    let address: SocketAddr = API_ADDRESS
        .parse()
        .map_err(|_| "invalid Codex Pet endpoint".to_owned())?;
    let mut stream = TcpStream::connect_timeout(&address, HTTP_TIMEOUT)
        .map_err(|_| "Codex Pet is offline".to_owned())?;
    let _ = stream.set_read_timeout(Some(HTTP_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HTTP_TIMEOUT));
    let body = serde_json::to_vec(payload).map_err(|_| "unable to serialize event".to_owned())?;
    let authorization = std::env::var("CODEX_PET_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| format!("Authorization: Bearer {value}\r\n"))
        .unwrap_or_default();
    let request = format!(
        "POST /v1/codex-event HTTP/1.1\r\nHost: {API_ADDRESS}\r\nContent-Type: application/json\r\n{authorization}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .and_then(|_| stream.write_all(&body))
        .map_err(|_| "unable to send event".to_owned())?;
    let mut response = [0u8; 64];
    let count = stream
        .read(&mut response)
        .map_err(|_| "unable to read event acknowledgement".to_owned())?;
    let status = std::str::from_utf8(&response[..count]).unwrap_or_default();
    if status.starts_with("HTTP/1.1 2") || status.starts_with("HTTP/1.0 2") {
        Ok(())
    } else {
        Err("Codex Pet rejected the event".to_owned())
    }
}

fn start_pet_once() -> Result<(), String> {
    let executable =
        std::env::current_exe().map_err(|_| "unable to locate Codex Pet".to_owned())?;
    let lock = launch_lock_path()?;
    if !claim_launch_lock(&lock)? {
        return Ok(());
    }
    let spawn = Command::new(executable)
        .arg("--from-hook")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW.0)
        .spawn()
        .map(|_| ())
        .map_err(|_| "unable to start Codex Pet".to_owned());
    if spawn.is_err() {
        let _ = fs::remove_file(lock);
    }
    spawn
}

fn launch_lock_path() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("com.local.codex-pet-overlay").join("launch.lock"))
        .ok_or_else(|| "LOCALAPPDATA is unavailable".to_owned())
}

fn claim_launch_lock(path: &Path) -> Result<bool, String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|_| "unable to create launch lock directory".to_owned())?;
    }
    match OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(mut file) => {
            let _ = writeln!(file, "{} {}", std::process::id(), now_ms());
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let stale = fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|elapsed| elapsed > Duration::from_secs(30));
            if stale {
                let _ = fs::remove_file(path);
                return claim_launch_lock(path);
            }
            Ok(false)
        }
        Err(_) => Err("unable to create launch lock".to_owned()),
    }
}

pub fn release_launch_lock() {
    if let Ok(path) = launch_lock_path() {
        let _ = fs::remove_file(path);
    }
}

pub fn owner_id() -> &'static str {
    OWNER_ID
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_hook_is_minimized_and_tool_activity_is_preserved() {
        let input = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "session_id": "019fcc02-569f-7790-92ff-209aaca4275c",
            "turn_id": "turn-1",
            "tool_name": "shell_command",
            "prompt": "secret",
            "cwd": "secret"
        });
        let payload = build_payload(&input).unwrap();
        assert_eq!(payload.activity, "command");
        assert_eq!(payload.state, "running");
        let serialized = serde_json::to_string(&payload).unwrap();
        assert!(!serialized.contains("secret"));
        assert_eq!(payload.event_id.len(), 64);
    }

    #[test]
    fn invalid_session_and_turn_ids_fail_closed() {
        assert!(build_payload(&serde_json::json!({
            "hook_event_name": "Stop",
            "session_id": "not-a-session"
        }))
        .is_none());
        let payload = build_payload(&serde_json::json!({
            "hook_event_name": "Stop",
            "session_id": "019fcc02-569f-7790-92ff-209aaca4275c",
            "turn_id": "contains spaces"
        }))
        .unwrap();
        assert!(payload.turn_id.is_none());
    }

    #[test]
    fn mapping_self_test_covers_every_supported_event() {
        self_test().unwrap();
    }
}
