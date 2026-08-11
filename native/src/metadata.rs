use crate::core::{sanitize_title, ThreadMetadata};
use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(5);
const THREAD_LIST_PAGE_LIMIT: u32 = 100;
const MAX_THREAD_LIST_PAGES: usize = 64;
const THREAD_SOURCE_KINDS: [&str; 10] = [
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];

fn command_app_server() -> Result<Command, String> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or_else(|| "Windows system root unavailable".to_owned())?;
    let codex_path = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("npm").join("codex.cmd"))
        .filter(|path| path.is_file())
        .ok_or_else(|| "Codex CLI wrapper unavailable".to_owned())?;
    let mut command = Command::new(codex_path);
    command.args(["app-server", "--stdio"]);
    command.current_dir(system_root.join("System32"));
    command.env_remove("CODEX_PET_TOKEN");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    Ok(command)
}

fn stop_child_tree(child: &mut Child) {
    #[cfg(windows)]
    if let Some(taskkill) = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|path| path.join("System32").join("taskkill.exe"))
        .filter(|path| path.is_file())
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new(taskkill)
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(0x08000000)
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

struct ChildGuard(Option<Child>);

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self(Some(child))
    }

    fn child_mut(&mut self) -> &mut Child {
        self.0.as_mut().expect("child guard is populated")
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.0.take() {
            stop_child_tree(&mut child);
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

fn recv_json_id(
    receiver: &Receiver<serde_json::Value>,
    expected_id: u64,
) -> Result<serde_json::Value, String> {
    let deadline = Instant::now() + APP_SERVER_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("app server timed out".to_owned());
        }
        match receiver.recv_timeout(remaining) {
            Ok(message)
                if message.get("id").and_then(|value| value.as_u64()) == Some(expected_id) =>
            {
                return Ok(message)
            }
            Ok(_) => continue,
            Err(_) => return Err("app server response unavailable".to_owned()),
        }
    }
}

struct AppServerSession {
    child: ChildGuard,
    stdin: ChildStdin,
    receiver: Receiver<serde_json::Value>,
}

fn start_app_server() -> Result<AppServerSession, String> {
    let mut child = ChildGuard::new(
        command_app_server()?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "codex app server is unavailable".to_owned())?,
    );
    let mut stdin = child
        .child_mut()
        .stdin
        .take()
        .ok_or_else(|| "app server stdin unavailable".to_owned())?;
    let stdout = child
        .child_mut()
        .stdout
        .take()
        .ok_or_else(|| "app server stdout unavailable".to_owned())?;
    let (sender, receiver) = mpsc::sync_channel::<serde_json::Value>(32);
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) {
                if sender.send(value).is_err() {
                    break;
                }
            }
        }
    });

    let initialize = serde_json::json!({
        "method": "initialize",
        "id": 1,
        "params": {
            "clientInfo": {
                "name": "codex_pet_overlay",
                "title": "Codex Pet Overlay",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": { "experimentalApi": false }
        }
    });
    writeln!(stdin, "{initialize}").map_err(|_| "app server write failed".to_owned())?;
    stdin
        .flush()
        .map_err(|_| "app server write failed".to_owned())?;
    recv_json_id(&receiver, 1)?;

    let initialized = serde_json::json!({ "method": "initialized", "params": {} });
    writeln!(stdin, "{initialized}").map_err(|_| "app server write failed".to_owned())?;
    stdin
        .flush()
        .map_err(|_| "app server write failed".to_owned())?;
    Ok(AppServerSession {
        child,
        stdin,
        receiver,
    })
}

pub fn fetch_thread_metadata(session_id: &str) -> Result<ThreadMetadata, String> {
    let AppServerSession {
        mut child,
        mut stdin,
        receiver,
    } = start_app_server()?;
    let read = serde_json::json!({
        "method": "thread/read",
        "id": 2,
        "params": { "threadId": session_id, "includeTurns": false }
    });
    writeln!(stdin, "{read}").map_err(|_| "app server write failed".to_owned())?;
    stdin
        .flush()
        .map_err(|_| "app server write failed".to_owned())?;
    let response = recv_json_id(&receiver, 2)?;
    drop(stdin);
    child.stop();

    parse_thread_metadata(session_id, &response)
}

pub fn fetch_active_thread_ids() -> Result<HashSet<String>, String> {
    let AppServerSession {
        mut child,
        mut stdin,
        receiver,
    } = start_app_server()?;
    let mut active_thread_ids = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    for page in 0..MAX_THREAD_LIST_PAGES {
        let request_id = page as u64 + 2;
        let list = serde_json::json!({
            "method": "thread/list",
            "id": request_id,
            "params": {
                "archived": false,
                "cursor": cursor.as_deref(),
                "limit": THREAD_LIST_PAGE_LIMIT,
                "sourceKinds": THREAD_SOURCE_KINDS,
                "useStateDbOnly": true
            }
        });
        writeln!(stdin, "{list}").map_err(|_| "app server write failed".to_owned())?;
        stdin
            .flush()
            .map_err(|_| "app server write failed".to_owned())?;
        let response = recv_json_id(&receiver, request_id)?;
        let (page_ids, next_cursor) = parse_thread_list_page(&response)?;
        active_thread_ids.extend(page_ids);
        let Some(next_cursor) = next_cursor else {
            drop(stdin);
            child.stop();
            return Ok(active_thread_ids);
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            return Err("thread list cursor repeated".to_owned());
        }
        cursor = Some(next_cursor);
    }
    Err("thread list pagination limit exceeded".to_owned())
}

fn parse_thread_list_page(
    response: &serde_json::Value,
) -> Result<(HashSet<String>, Option<String>), String> {
    if response.get("error").is_some() {
        return Err("active thread list unavailable".to_owned());
    }
    let data = response
        .pointer("/result/data")
        .and_then(|value| value.as_array())
        .ok_or_else(|| "active thread list unavailable".to_owned())?;
    let ids = data
        .iter()
        .filter(|thread| thread.get("ephemeral").and_then(|value| value.as_bool()) == Some(false))
        .filter_map(|thread| {
            thread
                .get("id")
                .and_then(|value| value.as_str())
                .map(str::to_owned)
        })
        .collect();
    let next_cursor = match response.pointer("/result/nextCursor") {
        None | Some(serde_json::Value::Null) => None,
        Some(value) => Some(
            value
                .as_str()
                .filter(|cursor| !cursor.is_empty())
                .ok_or_else(|| "active thread list cursor is invalid".to_owned())?
                .to_owned(),
        ),
    };
    Ok((ids, next_cursor))
}

fn parse_thread_metadata(
    session_id: &str,
    response: &serde_json::Value,
) -> Result<ThreadMetadata, String> {
    if response.get("error").is_some() {
        return Err("thread metadata unavailable".to_owned());
    }
    let thread = response
        .pointer("/result/thread")
        .ok_or_else(|| "thread metadata unavailable".to_owned())?;
    if thread.get("id").and_then(|value| value.as_str()) != Some(session_id) {
        return Err("thread metadata id mismatch".to_owned());
    }
    if thread.get("ephemeral").and_then(|value| value.as_bool()) != Some(false) {
        return Err("thread persistence is not verified".to_owned());
    }
    let title = thread
        .get("name")
        .and_then(|value| value.as_str())
        .and_then(sanitize_title);
    let preview = thread
        .get("preview")
        .and_then(|value| value.as_str())
        .and_then(sanitize_title);
    let status_type = thread
        .pointer("/status/type")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let active_flags = thread
        .pointer("/status/activeFlags")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let has_active_flag = |expected: &str| {
        active_flags
            .iter()
            .any(|flag| flag.as_str() == Some(expected))
    };
    let waiting_on_approval = has_active_flag("waitingOnApproval");
    let waiting_on_user_input = has_active_flag("waitingOnUserInput");
    let plan_implementation_pending = has_active_flag("planImplementation")
        || thread
            .pointer("/status/planImplementation/status")
            .and_then(|value| value.as_str())
            .is_some_and(|status| !matches!(status, "completed" | "done"));
    let has_unread_turn = thread
        .get("hasUnreadTurn")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    Ok(ThreadMetadata {
        thread_id: Some(session_id.to_owned()),
        title,
        preview,
        status_type,
        waiting_on_approval,
        waiting_on_user_input,
        plan_implementation_pending,
        has_unread_turn,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION_ID: &str = "019fcc02-569f-7790-92ff-209aaca4275c";

    #[test]
    fn exact_persisted_thread_is_a_verified_route() {
        let metadata = parse_thread_metadata(
            SESSION_ID,
            &serde_json::json!({
                "result": {
                    "thread": {
                        "id": SESSION_ID,
                        "name": "主体任务",
                        "ephemeral": false,
                        "status": { "type": "idle" }
                    }
                }
            }),
        )
        .unwrap();

        assert_eq!(metadata.thread_id.as_deref(), Some(SESSION_ID));
        assert_eq!(metadata.title.as_deref(), Some("主体任务"));
    }

    #[test]
    fn missing_mismatched_and_ephemeral_threads_are_not_routes() {
        let missing = serde_json::json!({ "error": { "message": "not found" } });
        assert!(parse_thread_metadata(SESSION_ID, &missing).is_err());

        let mismatched = serde_json::json!({
            "result": {
                "thread": {
                    "id": "019fcc02-569f-7790-92ff-209aaca4275d",
                    "ephemeral": false
                }
            }
        });
        assert!(parse_thread_metadata(SESSION_ID, &mismatched).is_err());

        let ephemeral = serde_json::json!({
            "result": { "thread": { "id": SESSION_ID, "ephemeral": true } }
        });
        assert!(parse_thread_metadata(SESSION_ID, &ephemeral).is_err());

        let missing_ephemeral = serde_json::json!({
            "result": { "thread": { "id": SESSION_ID } }
        });
        assert!(parse_thread_metadata(SESSION_ID, &missing_ephemeral).is_err());

        let invalid_ephemeral = serde_json::json!({
            "result": { "thread": { "id": SESSION_ID, "ephemeral": "false" } }
        });
        assert!(parse_thread_metadata(SESSION_ID, &invalid_ephemeral).is_err());
    }

    #[test]
    fn active_thread_pages_keep_only_persisted_ids_and_require_valid_cursors() {
        let (ids, cursor) = parse_thread_list_page(&serde_json::json!({
            "result": {
                "data": [
                    { "id": SESSION_ID, "ephemeral": false },
                    { "id": "019fcc02-569f-7790-92ff-209aaca4275d", "ephemeral": true }
                ],
                "nextCursor": "next-page"
            }
        }))
        .unwrap();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(SESSION_ID));
        assert_eq!(cursor.as_deref(), Some("next-page"));

        let invalid = serde_json::json!({
            "result": { "data": [], "nextCursor": 7 }
        });
        assert!(parse_thread_list_page(&invalid).is_err());
    }
}
