#![cfg_attr(feature = "editor", allow(dead_code, unused_imports))]

#[cfg(feature = "editor")]
mod editor;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WebviewWindow, WindowEvent,
};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const API_ADDRESS: &str = "127.0.0.1:48193";
const MAX_BODY_BYTES: usize = 16 * 1024;
const MAX_SESSIONS: usize = 32;
const MAX_EVENT_IDS_PER_SESSION: usize = 64;
const SESSION_TITLE_MAX_CHARS: usize = 64;
const MAX_EVENT_FUTURE_SKEW_MS: u64 = 60_000;
const MAX_METADATA_ATTEMPTS_PER_SESSION: u8 = 3;
const APP_SERVER_TIMEOUT: Duration = Duration::from_secs(5);
const SUPPORTED_VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "webm", "m4v"];
const VALID_STATES: [&str; 9] = [
    "idle",
    "running-right",
    "running-left",
    "waving",
    "jumping",
    "failed",
    "waiting",
    "running",
    "review",
];

type VideoCatalog = HashMap<String, Vec<String>>;

#[derive(Clone, Debug, Serialize)]
struct StateSnapshot {
    state: String,
    previous_state: String,
    revision: u64,
    source: String,
    updated_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
struct PlaybackSnapshot {
    ready: bool,
    playing: bool,
    decoder_start_count: u32,
    source_switch_count: u32,
    current_video: Option<String>,
    last_event: String,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct WindowFlags {
    visible: bool,
    click_through: bool,
    always_on_top: bool,
}

#[derive(Clone, Debug, Serialize)]
struct HttpStatus {
    address: String,
    bound: bool,
    error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SessionActivity {
    id: String,
    state: String,
    previous_state: String,
    revision: u64,
    title: String,
    activity: String,
    source: String,
    updated_at_ms: u64,
    priority: u8,
    unread: bool,
    deep_link: String,
}

#[derive(Clone, Debug, Serialize)]
struct SessionsSnapshot {
    revision: u64,
    selected_session_id: Option<String>,
    session_count: usize,
    sessions: Vec<SessionActivity>,
}

#[derive(Clone, Debug)]
struct SessionRecord {
    id: String,
    state: String,
    previous_state: String,
    revision: u64,
    title: String,
    activity: String,
    source: String,
    updated_at_ms: u64,
    last_emitted_at_ms: u64,
    ended_at_ms: Option<u64>,
    metadata_attempts: u8,
    unread: bool,
    recent_event_ids: VecDeque<String>,
}

#[derive(Clone, Debug)]
struct SessionRegistry {
    revision: u64,
    selected_session_id: Option<String>,
    records: HashMap<String, SessionRecord>,
}

#[derive(Clone, Debug)]
struct RuntimeSnapshot {
    state: StateSnapshot,
    sessions: SessionRegistry,
    playback: PlaybackSnapshot,
    window: WindowFlags,
    http: HttpStatus,
}

struct RuntimeData {
    snapshot: Mutex<RuntimeSnapshot>,
    video_path: String,
    video_catalog_root: String,
    video_catalog: VideoCatalog,
    decoder_session_id: String,
    api_token: Option<String>,
    ttl_sender: SyncSender<TtlJob>,
    metadata_inflight: Mutex<HashSet<String>>,
    quitting: AtomicBool,
}

struct TtlJob {
    deadline: Instant,
    expected_revision: u64,
    restore_state: String,
}

#[derive(Clone, Serialize)]
struct Bootstrap {
    state: StateSnapshot,
    sessions: SessionsSnapshot,
    video_path: String,
    video_catalog: VideoCatalog,
    decoder_session_id: String,
    endpoint: String,
    click_through: bool,
    always_on_top: bool,
}

#[derive(Clone, Serialize)]
struct VisibilityPayload {
    visible: bool,
}

#[derive(Debug, Deserialize)]
struct SetStateRequest {
    state: String,
    ttl_ms: Option<u64>,
    source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SetWindowRequest {
    visible: Option<bool>,
    click_through: Option<bool>,
    always_on_top: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CodexEventRequest {
    schema_version: u8,
    event_id: String,
    session_id: String,
    turn_id: Option<String>,
    hook_event_name: String,
    state: String,
    activity: String,
    emitted_at_ms: u64,
}

#[derive(Clone, Debug)]
struct ThreadMetadata {
    title: Option<String>,
    status_type: Option<String>,
    waiting_on_approval: bool,
}

#[tauri::command]
fn get_bootstrap(runtime: State<'_, Arc<RuntimeData>>) -> Result<Bootstrap, String> {
    let snapshot = runtime
        .snapshot
        .lock()
        .map_err(|_| "runtime lock poisoned")?;
    Ok(Bootstrap {
        state: snapshot.state.clone(),
        sessions: sessions_snapshot(&snapshot.sessions),
        video_path: runtime.video_path.clone(),
        video_catalog: runtime.video_catalog.clone(),
        decoder_session_id: runtime.decoder_session_id.clone(),
        endpoint: format!("http://{API_ADDRESS}"),
        click_through: snapshot.window.click_through,
        always_on_top: snapshot.window.always_on_top,
    })
}

#[tauri::command]
fn set_pet_state(
    app: AppHandle,
    runtime: State<'_, Arc<RuntimeData>>,
    state: String,
    ttl_ms: Option<u64>,
) -> Result<StateSnapshot, String> {
    transition_state(&app, runtime.inner().clone(), &state, "frontend", ttl_ms)
}

#[tauri::command]
fn open_codex_session(
    app: AppHandle,
    runtime: State<'_, Arc<RuntimeData>>,
    session_id: String,
) -> Result<(), String> {
    if !is_session_id(&session_id) {
        return Err("invalid session id".to_owned());
    }

    {
        let snapshot = runtime
            .snapshot
            .lock()
            .map_err(|_| "runtime lock poisoned")?;
        if !snapshot.sessions.records.contains_key(&session_id) {
            return Err("unknown session".to_owned());
        }
    }

    let deep_link = format!("codex://threads/{session_id}");
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or_else(|| "Windows system root unavailable".to_owned())?;
    let explorer = system_root.join("explorer.exe");
    if !explorer.is_file() {
        return Err("Windows Explorer unavailable".to_owned());
    }
    let mut command = Command::new(explorer);
    command.arg(deep_link);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .spawn()
        .map_err(|_| "failed to open Codex session".to_owned())?;

    let (state, sessions, state_changed) = {
        let mut snapshot = runtime
            .snapshot
            .lock()
            .map_err(|_| "runtime lock poisoned")?;
        let record = snapshot
            .sessions
            .records
            .get_mut(&session_id)
            .ok_or_else(|| "unknown session".to_owned())?;
        record.unread = false;
        if record.state == "review" {
            record.previous_state = record.state.clone();
            record.state = "idle".to_owned();
            record.activity = "idle".to_owned();
            record.revision += 1;
            record.updated_at_ms = now_ms();
        }
        snapshot.sessions.revision += 1;
        let state_changed = recompute_aggregate(&mut snapshot, "session-open");
        (
            snapshot.state.clone(),
            sessions_snapshot(&snapshot.sessions),
            state_changed,
        )
    };

    if state_changed {
        let _ = app.emit("pet-state", state);
    }
    let _ = app.emit("pet-sessions", sessions);
    Ok(())
}

#[tauri::command]
fn report_media_event(
    runtime: State<'_, Arc<RuntimeData>>,
    event: String,
    detail: Option<String>,
) -> Result<(), String> {
    let event = sanitize_label(&event, 40);
    let detail = detail.map(|value| sanitize_label(&value, 180));
    let mut snapshot = runtime
        .snapshot
        .lock()
        .map_err(|_| "runtime lock poisoned")?;
    snapshot.playback.last_event = event.clone();

    match event.as_str() {
        "decoder-started" if !snapshot.playback.ready => {
            snapshot.playback.ready = true;
            snapshot.playback.decoder_start_count += 1;
        }
        "source-switched" => {
            snapshot.playback.source_switch_count += 1;
            snapshot.playback.current_video = detail;
            snapshot.playback.playing = false;
            snapshot.playback.error = None;
        }
        "media-ready" => snapshot.playback.error = None,
        "playing" => {
            snapshot.playback.playing = true;
            snapshot.playback.error = None;
        }
        "paused" => snapshot.playback.playing = false,
        "error" | "bootstrap-error" | "play-rejected" => {
            snapshot.playback.error = detail;
        }
        _ => {}
    }
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn is_valid_state(state: &str) -> bool {
    VALID_STATES.contains(&state)
}

fn sanitize_label(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect()
}

fn sanitize_source(value: Option<String>) -> String {
    let source = value.unwrap_or_else(|| "loopback".to_owned());
    let sanitized: String = source
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
        .take(48)
        .collect();
    if sanitized.is_empty() {
        "loopback".to_owned()
    } else {
        sanitized
    }
}

fn is_session_id(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }
    value.chars().enumerate().all(|(index, ch)| match index {
        8 | 13 | 18 | 23 => ch == '-',
        _ => ch.is_ascii_hexdigit(),
    })
}

fn is_event_id(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn is_valid_turn_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn is_valid_hook_event(value: &str) -> bool {
    matches!(
        value,
        "SessionStart"
            | "SessionEnd"
            | "UserPromptSubmit"
            | "PreToolUse"
            | "PermissionRequest"
            | "PostToolUse"
            | "PreCompact"
            | "PostCompact"
            | "SubagentStart"
            | "SubagentStop"
            | "Stop"
    )
}

fn is_valid_activity(value: &str) -> bool {
    matches!(
        value,
        "starting"
            | "working"
            | "command"
            | "reading"
            | "editing"
            | "searching"
            | "waiting"
            | "recovering"
            | "ready"
            | "subagent"
            | "idle"
    )
}

fn state_for_hook(event: &str, requested: &str) -> Option<&'static str> {
    match event {
        "SessionStart" | "SessionEnd" => Some("idle"),
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PreCompact" | "PostCompact"
        | "SubagentStop" => Some("running"),
        "PermissionRequest" => Some("waiting"),
        "SubagentStart" if matches!(requested, "running-left" | "running-right") => {
            Some(if requested == "running-left" {
                "running-left"
            } else {
                "running-right"
            })
        }
        "SubagentStart" => Some("running"),
        "Stop" => Some("review"),
        _ => None,
    }
}

fn state_priority(state: &str) -> u8 {
    match state {
        "waiting" => 0,
        "failed" => 1,
        "review" => 2,
        "running" | "running-left" | "running-right" | "waving" | "jumping" => 3,
        _ => 4,
    }
}

fn fallback_session_title(session_id: &str) -> String {
    let suffix: String = session_id.chars().take(8).collect();
    format!("Codex Session {suffix}")
}

fn sanitize_title(value: &str) -> Option<String> {
    let title: String = value
        .chars()
        .filter(|ch| {
            !ch.is_control()
                && !matches!(
                    *ch as u32,
                    0x200B..=0x200F | 0x202A..=0x202E | 0x2060..=0x2069
                )
        })
        .take(SESSION_TITLE_MAX_CHARS)
        .collect::<String>()
        .trim()
        .to_owned();
    (!title.is_empty()).then_some(title)
}

fn session_activity(record: &SessionRecord) -> SessionActivity {
    SessionActivity {
        id: record.id.clone(),
        state: record.state.clone(),
        previous_state: record.previous_state.clone(),
        revision: record.revision,
        title: record.title.clone(),
        activity: record.activity.clone(),
        source: record.source.clone(),
        updated_at_ms: record.updated_at_ms,
        priority: state_priority(&record.state),
        unread: record.unread,
        deep_link: format!("codex://threads/{}", record.id),
    }
}

fn sessions_snapshot(registry: &SessionRegistry) -> SessionsSnapshot {
    let mut sessions: Vec<_> = registry.records.values().map(session_activity).collect();
    sessions.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.id.cmp(&right.id))
    });
    SessionsSnapshot {
        revision: registry.revision,
        selected_session_id: registry.selected_session_id.clone(),
        session_count: sessions.len(),
        sessions,
    }
}

fn select_session(registry: &SessionRegistry) -> Option<&SessionRecord> {
    registry.records.values().min_by(|left, right| {
        state_priority(&left.state)
            .cmp(&state_priority(&right.state))
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.id.cmp(&right.id))
    })
}

fn recompute_aggregate(snapshot: &mut RuntimeSnapshot, source: &str) -> bool {
    let selected =
        select_session(&snapshot.sessions).map(|record| (record.id.clone(), record.state.clone()));
    let selected_id = selected.as_ref().map(|(id, _)| id.clone());
    let next_state = selected
        .map(|(_, state)| state)
        .unwrap_or_else(|| "idle".to_owned());
    let selection_changed = snapshot.sessions.selected_session_id != selected_id;
    snapshot.sessions.selected_session_id = selected_id;

    if !selection_changed && snapshot.state.state == next_state {
        return false;
    }

    let previous = snapshot.state.state.clone();
    snapshot.state = StateSnapshot {
        state: next_state,
        previous_state: previous,
        revision: snapshot.state.revision + 1,
        source: source.to_owned(),
        updated_at_ms: now_ms(),
    };
    true
}

fn validate_codex_event(payload: &CodexEventRequest) -> Result<&'static str, String> {
    if payload.schema_version != 1 {
        return Err("unsupported schema version".to_owned());
    }
    if !is_session_id(&payload.session_id) {
        return Err("invalid session id".to_owned());
    }
    if !is_event_id(&payload.event_id) {
        return Err("invalid event id".to_owned());
    }
    if payload
        .turn_id
        .as_deref()
        .is_some_and(|turn_id| !is_valid_turn_id(turn_id))
    {
        return Err("invalid turn id".to_owned());
    }
    if !is_valid_hook_event(&payload.hook_event_name) {
        return Err("unsupported hook event".to_owned());
    }
    if !is_valid_state(&payload.state) {
        return Err("invalid state".to_owned());
    }
    if !is_valid_activity(&payload.activity) {
        return Err("invalid activity".to_owned());
    }
    if payload.emitted_at_ms > now_ms().saturating_add(MAX_EVENT_FUTURE_SKEW_MS) {
        return Err("event timestamp is too far in the future".to_owned());
    }
    state_for_hook(&payload.hook_event_name, &payload.state)
        .ok_or_else(|| "invalid hook state mapping".to_owned())
}

fn accept_session_event(
    record: &mut SessionRecord,
    payload: &CodexEventRequest,
    is_session_end: bool,
) -> bool {
    if record.recent_event_ids.contains(&payload.event_id)
        || payload.emitted_at_ms < record.last_emitted_at_ms
        || (!is_session_end
            && record
                .ended_at_ms
                .is_some_and(|ended_at_ms| payload.emitted_at_ms <= ended_at_ms))
    {
        return false;
    }

    record.recent_event_ids.push_back(payload.event_id.clone());
    while record.recent_event_ids.len() > MAX_EVENT_IDS_PER_SESSION {
        record.recent_event_ids.pop_front();
    }
    record.last_emitted_at_ms = payload.emitted_at_ms;
    record.ended_at_ms = is_session_end.then_some(payload.emitted_at_ms);
    true
}

fn evict_oldest_session(registry: &mut SessionRegistry) {
    if registry.records.len() < MAX_SESSIONS {
        return;
    }
    if let Some(oldest_id) = registry
        .records
        .values()
        .min_by_key(|record| record.updated_at_ms)
        .map(|record| record.id.clone())
    {
        registry.records.remove(&oldest_id);
    }
}

fn apply_codex_event(
    app: &AppHandle,
    runtime: Arc<RuntimeData>,
    payload: CodexEventRequest,
) -> Result<(StateSnapshot, SessionsSnapshot), String> {
    let expected_state = validate_codex_event(&payload)?;
    let mut should_lookup_metadata = false;
    let (state, sessions, state_changed, sessions_changed) = {
        let mut snapshot = runtime
            .snapshot
            .lock()
            .map_err(|_| "runtime lock poisoned")?;

        if payload.hook_event_name == "SessionEnd" {
            let updated =
                if let Some(record) = snapshot.sessions.records.get_mut(&payload.session_id) {
                    if !accept_session_event(record, &payload, true) {
                        return Ok((
                            snapshot.state.clone(),
                            sessions_snapshot(&snapshot.sessions),
                        ));
                    }
                    record.previous_state = record.state.clone();
                    if record.unread {
                        record.state = "review".to_owned();
                        record.activity = "ready".to_owned();
                    } else {
                        record.state = "idle".to_owned();
                        record.activity = "idle".to_owned();
                    }
                    record.source = "codex-hook".to_owned();
                    record.updated_at_ms = now_ms();
                    record.revision += 1;
                    true
                } else {
                    false
                };
            if updated {
                snapshot.sessions.revision += 1;
            }
            let state_changed = recompute_aggregate(&mut snapshot, "codex-hook");
            (
                snapshot.state.clone(),
                sessions_snapshot(&snapshot.sessions),
                state_changed,
                updated,
            )
        } else {
            let is_new = !snapshot.sessions.records.contains_key(&payload.session_id);
            if is_new {
                evict_oldest_session(&mut snapshot.sessions);
                snapshot.sessions.records.insert(
                    payload.session_id.clone(),
                    SessionRecord {
                        id: payload.session_id.clone(),
                        state: "idle".to_owned(),
                        previous_state: "idle".to_owned(),
                        revision: 0,
                        title: fallback_session_title(&payload.session_id),
                        activity: "starting".to_owned(),
                        source: "codex-hook".to_owned(),
                        updated_at_ms: now_ms(),
                        last_emitted_at_ms: 0,
                        ended_at_ms: None,
                        metadata_attempts: 0,
                        unread: false,
                        recent_event_ids: VecDeque::new(),
                    },
                );
                should_lookup_metadata = true;
            }

            let record = snapshot
                .sessions
                .records
                .get_mut(&payload.session_id)
                .ok_or_else(|| "session registry update failed".to_owned())?;

            if !accept_session_event(record, &payload, false) {
                return Ok((
                    snapshot.state.clone(),
                    sessions_snapshot(&snapshot.sessions),
                ));
            }
            record.previous_state = record.state.clone();
            record.state = expected_state.to_owned();
            record.activity = payload.activity;
            record.source = "codex-hook".to_owned();
            record.updated_at_ms = now_ms();
            record.revision += 1;
            record.unread = expected_state == "review";
            should_lookup_metadata |= record.title.starts_with("Codex Session ");

            snapshot.sessions.revision += 1;
            let state_changed = recompute_aggregate(&mut snapshot, "codex-hook");
            (
                snapshot.state.clone(),
                sessions_snapshot(&snapshot.sessions),
                state_changed,
                true,
            )
        }
    };

    if state_changed {
        let _ = app.emit("pet-state", state.clone());
    }
    if sessions_changed {
        let _ = app.emit("pet-sessions", sessions.clone());
    }
    if should_lookup_metadata {
        schedule_metadata_lookup(app.clone(), runtime, payload.session_id);
    }
    Ok((state, sessions))
}

fn emit_session_refresh(app: &AppHandle, runtime: &RuntimeData) {
    let Ok(snapshot) = runtime.snapshot.lock() else {
        return;
    };
    let _ = app.emit("pet-state", snapshot.state.clone());
    let _ = app.emit("pet-sessions", sessions_snapshot(&snapshot.sessions));
}

fn command_app_server() -> Result<Command, String> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or_else(|| "Windows system root unavailable".to_owned())?;
    let command_path = system_root.join("System32").join("cmd.exe");
    let codex_path = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("npm").join("codex.cmd"))
        .filter(|path| path.is_file())
        .ok_or_else(|| "Codex CLI wrapper unavailable".to_owned())?;
    if !command_path.is_file() {
        return Err("Windows command processor unavailable".to_owned());
    }

    let command_line = format!("\"{}\" app-server --stdio", codex_path.display());
    let mut command = Command::new(command_path);
    command.args(["/D", "/S", "/C", &command_line]);
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
        self.0.as_mut().expect("child guard is always populated")
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

fn fetch_thread_metadata(session_id: &str) -> Result<ThreadMetadata, String> {
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
    let read = serde_json::json!({
        "method": "thread/read",
        "id": 2,
        "params": { "threadId": session_id, "includeTurns": false }
    });
    writeln!(stdin, "{initialized}").map_err(|_| "app server write failed".to_owned())?;
    writeln!(stdin, "{read}").map_err(|_| "app server write failed".to_owned())?;
    stdin
        .flush()
        .map_err(|_| "app server write failed".to_owned())?;
    let response = recv_json_id(&receiver, 2)?;
    drop(stdin);
    child.stop();

    let thread = response
        .pointer("/result/thread")
        .ok_or_else(|| "thread metadata unavailable".to_owned())?;
    if thread.get("id").and_then(|value| value.as_str()) != Some(session_id) {
        return Err("thread metadata id mismatch".to_owned());
    }
    let title = thread
        .get("name")
        .and_then(|value| value.as_str())
        .and_then(sanitize_title);
    let status_type = thread
        .pointer("/status/type")
        .and_then(|value| value.as_str())
        .map(str::to_owned);
    let waiting_on_approval = thread
        .pointer("/status/activeFlags")
        .and_then(|value| value.as_array())
        .is_some_and(|flags| {
            flags
                .iter()
                .any(|flag| flag.as_str() == Some("waitingOnApproval"))
        });
    Ok(ThreadMetadata {
        title,
        status_type,
        waiting_on_approval,
    })
}

fn schedule_metadata_lookup(app: AppHandle, runtime: Arc<RuntimeData>, session_id: String) {
    let should_start = runtime
        .metadata_inflight
        .lock()
        .map(|mut inflight| inflight.insert(session_id.clone()))
        .unwrap_or(false);
    if !should_start {
        return;
    }

    let requested_revision = runtime.snapshot.lock().ok().and_then(|mut snapshot| {
        let record = snapshot.sessions.records.get_mut(&session_id)?;
        if record.metadata_attempts >= MAX_METADATA_ATTEMPTS_PER_SESSION {
            return None;
        }
        record.metadata_attempts += 1;
        Some(record.revision)
    });
    let Some(requested_revision) = requested_revision else {
        if let Ok(mut inflight) = runtime.metadata_inflight.lock() {
            inflight.remove(&session_id);
        }
        return;
    };

    let runtime_for_thread = runtime.clone();
    let session_for_cleanup = session_id.clone();
    if thread::Builder::new()
        .name("codex-pet-metadata".to_owned())
        .spawn(move || {
            if let Ok(metadata) = fetch_thread_metadata(&session_id) {
                let mut changed = false;
                let mut state_changed = false;
                if let Ok(mut snapshot) = runtime_for_thread.snapshot.lock() {
                    if let Some(record) = snapshot.sessions.records.get_mut(&session_id) {
                        if let Some(title) = metadata.title {
                            if record.title != title {
                                record.title = title;
                                changed = true;
                            }
                        }
                        let state_is_current = record.revision == requested_revision
                            && record.ended_at_ms.is_none()
                            && record.state != "review";
                        if state_is_current
                            && metadata.waiting_on_approval
                            && record.state != "waiting"
                        {
                            record.previous_state = record.state.clone();
                            record.state = "waiting".to_owned();
                            record.activity = "waiting".to_owned();
                            record.source = "app-server".to_owned();
                            record.revision += 1;
                            record.updated_at_ms = now_ms();
                            changed = true;
                        } else if state_is_current
                            && metadata.status_type.as_deref() == Some("systemError")
                            && record.state != "failed"
                        {
                            record.previous_state = record.state.clone();
                            record.state = "failed".to_owned();
                            record.activity = "recovering".to_owned();
                            record.source = "app-server".to_owned();
                            record.revision += 1;
                            record.updated_at_ms = now_ms();
                            changed = true;
                        }
                    }
                    if changed {
                        snapshot.sessions.revision += 1;
                        state_changed = recompute_aggregate(&mut snapshot, "app-server");
                    }
                }
                if changed || state_changed {
                    emit_session_refresh(&app, &runtime_for_thread);
                }
            }
            if let Ok(mut inflight) = runtime_for_thread.metadata_inflight.lock() {
                inflight.remove(&session_id);
            }
        })
        .is_err()
    {
        if let Ok(mut inflight) = runtime.metadata_inflight.lock() {
            inflight.remove(&session_for_cleanup);
        }
    }
}

fn transition_state(
    app: &AppHandle,
    runtime: Arc<RuntimeData>,
    requested: &str,
    source: &str,
    ttl_ms: Option<u64>,
) -> Result<StateSnapshot, String> {
    if !is_valid_state(requested) {
        return Err(format!("unknown state: {requested}"));
    }

    let (snapshot, restore_state) = {
        let mut inner = runtime
            .snapshot
            .lock()
            .map_err(|_| "runtime lock poisoned")?;
        let previous = inner.state.state.clone();
        inner.state = StateSnapshot {
            state: requested.to_owned(),
            previous_state: previous.clone(),
            revision: inner.state.revision + 1,
            source: sanitize_source(Some(source.to_owned())),
            updated_at_ms: now_ms(),
        };
        (inner.state.clone(), previous)
    };

    let _ = app.emit("pet-state", snapshot.clone());

    if let Some(ttl) = ttl_ms.filter(|ttl| *ttl > 0) {
        let _ = runtime.ttl_sender.try_send(TtlJob {
            deadline: Instant::now() + Duration::from_millis(ttl.clamp(50, 30_000)),
            expected_revision: snapshot.revision,
            restore_state,
        });
    }

    Ok(snapshot)
}

fn run_ttl_worker(app: AppHandle, runtime: Arc<RuntimeData>, receiver: Receiver<TtlJob>) {
    while let Ok(mut pending) = receiver.recv() {
        loop {
            let remaining = pending.deadline.saturating_duration_since(Instant::now());
            match receiver.recv_timeout(remaining) {
                Ok(newer) => pending = newer,
                Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    let restored = {
                        let Ok(mut inner) = runtime.snapshot.lock() else {
                            break;
                        };
                        if inner.state.revision != pending.expected_revision {
                            break;
                        }
                        let current = inner.state.state.clone();
                        inner.state = StateSnapshot {
                            state: pending.restore_state,
                            previous_state: current,
                            revision: inner.state.revision + 1,
                            source: "ttl".to_owned(),
                            updated_at_ms: now_ms(),
                        };
                        inner.state.clone()
                    };
                    let _ = app.emit("pet-state", restored);
                    break;
                }
            }
        }
    }
}

fn video_state_from_stem(stem: &str) -> Option<&'static str> {
    let normalized = stem.to_ascii_lowercase();
    VALID_STATES
        .iter()
        .copied()
        .filter(|state| normalized.starts_with(state))
        .max_by_key(|state| state.len())
}

fn is_supported_video(path: &Path) -> bool {
    path.extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| {
            SUPPORTED_VIDEO_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
        })
}

fn collect_video_files(root: &Path, output: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_dir() {
            collect_video_files(&path, output)?;
        } else if file_type.is_file() && is_supported_video(&path) {
            output.push(path);
        }
    }
    Ok(())
}

fn scan_video_catalog(root: &Path) -> Result<VideoCatalog, Box<dyn std::error::Error>> {
    let mut catalog = VALID_STATES
        .iter()
        .map(|state| ((*state).to_owned(), Vec::new()))
        .collect::<HashMap<_, _>>();
    let mut files = Vec::new();
    collect_video_files(root, &mut files)?;
    files.sort();

    for path in files {
        let directory_state = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            .and_then(|value| {
                VALID_STATES
                    .iter()
                    .copied()
                    .find(|state| value.eq_ignore_ascii_case(state))
            });
        let filename_state = path
            .file_stem()
            .and_then(|value| value.to_str())
            .and_then(video_state_from_stem);
        if let Some(state) = directory_state.or(filename_state) {
            catalog
                .get_mut(state)
                .expect("all valid states are initialized")
                .push(path.canonicalize()?.to_string_lossy().into_owned());
        }
    }

    if catalog.get("running").is_some_and(Vec::is_empty) {
        let mut running = catalog.get("running-left").cloned().unwrap_or_default();
        running.extend(catalog.get("running-right").cloned().unwrap_or_default());
        running.sort();
        catalog.insert("running".to_owned(), running);
    }

    let missing = VALID_STATES
        .iter()
        .filter(|state| catalog.get(**state).is_none_or(Vec::is_empty))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("video catalog is missing states: {}", missing.join(", ")).into());
    }
    Ok(catalog)
}

fn resolve_video_catalog(
    app: &AppHandle,
) -> Result<(PathBuf, VideoCatalog), Box<dyn std::error::Error>> {
    let configured_root = std::env::var_os("CODEX_PET_VIDEO_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("project root unavailable")?
        .join("VideoSource")
        .join("V1");
    let local_data_root = app
        .path()
        .app_local_data_dir()?
        .join("VideoSource")
        .join("V1");
    let executable_root = app.path().resource_dir()?.join("VideoSource").join("V1");

    let mut roots = Vec::new();
    if let Some(root) = configured_root {
        roots.push(root);
    }
    roots.extend([source_root, local_data_root, executable_root]);

    let mut checked = Vec::new();
    for root in roots {
        checked.push(root.display().to_string());
        if !root.is_dir() {
            continue;
        }
        if let Ok(catalog) = scan_video_catalog(&root) {
            return Ok((root.canonicalize()?, catalog));
        }
    }

    Err(format!(
        "video catalog is missing or incomplete; checked: {}",
        checked.join("; ")
    )
    .into())
}

fn diagnostics_json(runtime: &RuntimeData) -> serde_json::Value {
    let Ok(snapshot) = runtime.snapshot.lock() else {
        return serde_json::json!({ "ok": false, "error": "runtime lock poisoned" });
    };
    serde_json::json!({
        "ok": true,
        "state": snapshot.state,
        "sessions": sessions_snapshot(&snapshot.sessions),
        "playback": snapshot.playback,
        "window": snapshot.window,
        "http": snapshot.http,
        "videoPath": runtime.video_path,
        "videoCatalogRoot": runtime.video_catalog_root,
        "videoCatalog": runtime.video_catalog,
        "decoderSessionId": runtime.decoder_session_id,
        "validStates": VALID_STATES,
    })
}

fn json_response(status: u16, value: serde_json::Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let header = Header::from_bytes("Content-Type", "application/json; charset=utf-8")
        .expect("static content-type header");
    Response::from_data(serde_json::to_vec(&value).unwrap_or_else(|_| b"{}".to_vec()))
        .with_status_code(StatusCode(status))
        .with_header(header)
}

fn read_json_body<T: DeserializeOwned>(request: &mut Request) -> Result<T, String> {
    let length = request
        .body_length()
        .ok_or_else(|| "content-length is required".to_owned())?;
    if length > MAX_BODY_BYTES {
        return Err("request body too large".to_owned());
    }
    let mut body = String::new();
    request
        .as_reader()
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_string(&mut body)
        .map_err(|_| "failed to read request body".to_owned())?;
    if body.len() > MAX_BODY_BYTES {
        return Err("request body too large".to_owned());
    }
    serde_json::from_str(&body).map_err(|_| "invalid JSON body".to_owned())
}

fn is_authorized(request: &Request, runtime: &RuntimeData) -> bool {
    let Some(expected) = runtime.api_token.as_deref() else {
        return true;
    };
    let expected = format!("Bearer {expected}");
    request.headers().iter().any(|header| {
        header.field.equiv("Authorization") && header.value.as_str() == expected.as_str()
    })
}

fn apply_window_request(
    app: &AppHandle,
    runtime: &RuntimeData,
    payload: SetWindowRequest,
) -> Result<WindowFlags, String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "main window is unavailable".to_owned())?;

    if let Some(always_on_top) = payload.always_on_top {
        window
            .set_always_on_top(always_on_top)
            .map_err(|error| sanitize_label(&error.to_string(), 160))?;
        let mut snapshot = runtime
            .snapshot
            .lock()
            .map_err(|_| "runtime lock poisoned")?;
        snapshot.window.always_on_top = always_on_top;
    }

    if let Some(click_through) = payload.click_through {
        window
            .set_ignore_cursor_events(click_through)
            .map_err(|error| sanitize_label(&error.to_string(), 160))?;
        let mut snapshot = runtime
            .snapshot
            .lock()
            .map_err(|_| "runtime lock poisoned")?;
        snapshot.window.click_through = click_through;
    }

    if let Some(visible) = payload.visible {
        set_visible(&window, runtime, visible);
    }

    runtime
        .snapshot
        .lock()
        .map(|snapshot| snapshot.window.clone())
        .map_err(|_| "runtime lock poisoned".to_owned())
}

fn handle_http(mut request: Request, app: &AppHandle, runtime: Arc<RuntimeData>) {
    let method = request.method().clone();
    let path = request.url().split('?').next().unwrap_or_default();

    let response = if method == Method::Post && !is_authorized(&request, &runtime) {
        json_response(
            401,
            serde_json::json!({ "ok": false, "error": "unauthorized" }),
        )
    } else {
        match (method, path) {
            (Method::Get, "/health") => json_response(200, serde_json::json!({ "ok": true })),
            (Method::Get, "/v1/state") => json_response(200, diagnostics_json(&runtime)),
            (Method::Get, "/v1/sessions") => match runtime.snapshot.lock() {
                Ok(snapshot) => {
                    let sessions = sessions_snapshot(&snapshot.sessions);
                    json_response(
                        200,
                        serde_json::json!({
                            "ok": true,
                            "revision": sessions.revision,
                            "selected_session_id": sessions.selected_session_id,
                            "session_count": sessions.session_count,
                            "sessions": sessions.sessions
                        }),
                    )
                }
                Err(_) => json_response(
                    500,
                    serde_json::json!({ "ok": false, "error": "runtime unavailable" }),
                ),
            },
            (Method::Get, "/v1/states") => {
                json_response(200, serde_json::json!({ "states": VALID_STATES }))
            }
            (Method::Post, "/v1/codex-event") => {
                match read_json_body::<CodexEventRequest>(&mut request) {
                    Ok(payload) => match apply_codex_event(app, runtime.clone(), payload) {
                        Ok((state, sessions)) => json_response(
                            200,
                            serde_json::json!({
                                "ok": true,
                                "state": state,
                                "sessions": sessions
                            }),
                        ),
                        Err(error) => {
                            json_response(422, serde_json::json!({ "ok": false, "error": error }))
                        }
                    },
                    Err(error) => {
                        json_response(400, serde_json::json!({ "ok": false, "error": error }))
                    }
                }
            }
            (Method::Post, "/v1/state") | (Method::Post, "/v1/codex-state") => {
                match read_json_body::<SetStateRequest>(&mut request) {
                    Ok(payload) => {
                        let source = sanitize_source(payload.source);
                        match transition_state(
                            app,
                            runtime,
                            &payload.state,
                            &source,
                            payload.ttl_ms,
                        ) {
                            Ok(snapshot) => json_response(
                                200,
                                serde_json::json!({ "ok": true, "state": snapshot }),
                            ),
                            Err(error) => json_response(
                                422,
                                serde_json::json!({ "ok": false, "error": error }),
                            ),
                        }
                    }
                    Err(error) => {
                        json_response(400, serde_json::json!({ "ok": false, "error": error }))
                    }
                }
            }
            (Method::Post, "/v1/window") => {
                match read_json_body::<SetWindowRequest>(&mut request) {
                    Ok(payload) => match apply_window_request(app, &runtime, payload) {
                        Ok(window) => {
                            json_response(200, serde_json::json!({ "ok": true, "window": window }))
                        }
                        Err(error) => {
                            json_response(500, serde_json::json!({ "ok": false, "error": error }))
                        }
                    },
                    Err(error) => {
                        json_response(400, serde_json::json!({ "ok": false, "error": error }))
                    }
                }
            }
            _ => json_response(
                404,
                serde_json::json!({ "ok": false, "error": "not found" }),
            ),
        }
    };

    let _ = request.respond(response);
}

fn start_http_server(app: AppHandle, runtime: Arc<RuntimeData>) {
    let server = match Server::http(API_ADDRESS) {
        Ok(server) => server,
        Err(error) => {
            if let Ok(mut snapshot) = runtime.snapshot.lock() {
                snapshot.http.error = Some(sanitize_label(&error.to_string(), 160));
            }
            return;
        }
    };

    if let Ok(mut snapshot) = runtime.snapshot.lock() {
        snapshot.http.bound = true;
        snapshot.http.error = None;
    }

    for request in server.incoming_requests() {
        handle_http(request, &app, runtime.clone());
    }
}

fn set_visible(window: &WebviewWindow, runtime: &RuntimeData, visible: bool) {
    if visible {
        let _ = window.show();
    } else {
        let _ = window.hide();
    }
    if let Ok(mut snapshot) = runtime.snapshot.lock() {
        snapshot.window.visible = visible;
    }
    let _ = window
        .app_handle()
        .emit("pet-visibility", VisibilityPayload { visible });
}

fn toggle_visible(app: &AppHandle, runtime: &RuntimeData) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    let visible = window.is_visible().unwrap_or(false);
    set_visible(&window, runtime, !visible);
}

fn build_tray(
    app: &AppHandle,
    runtime: Arc<RuntimeData>,
) -> Result<(), Box<dyn std::error::Error>> {
    let visibility_item = MenuItem::with_id(app, "visibility", "显示 / 隐藏", true, None::<&str>)?;
    let click_item =
        CheckMenuItem::with_id(app, "click-through", "鼠标穿透", true, false, None::<&str>)?;
    let top_item =
        CheckMenuItem::with_id(app, "always-on-top", "始终置顶", true, true, None::<&str>)?;
    let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&visibility_item, &click_item, &top_item, &quit_item])?;

    let click_item_for_event = click_item.clone();
    let top_item_for_event = top_item.clone();
    let runtime_for_menu = runtime.clone();
    let runtime_for_tray = runtime.clone();

    let mut builder = TrayIconBuilder::with_id("codex-pet")
        .tooltip("Codex Pet Overlay")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| match event.id.as_ref() {
            "visibility" => toggle_visible(app, &runtime_for_menu),
            "click-through" => {
                let Some(window) = app.get_webview_window("main") else {
                    return;
                };
                let next = runtime_for_menu
                    .snapshot
                    .lock()
                    .map(|snapshot| !snapshot.window.click_through)
                    .unwrap_or(false);
                if window.set_ignore_cursor_events(next).is_ok() {
                    let _ = click_item_for_event.set_checked(next);
                    if let Ok(mut snapshot) = runtime_for_menu.snapshot.lock() {
                        snapshot.window.click_through = next;
                    }
                }
            }
            "always-on-top" => {
                let Some(window) = app.get_webview_window("main") else {
                    return;
                };
                let next = runtime_for_menu
                    .snapshot
                    .lock()
                    .map(|snapshot| !snapshot.window.always_on_top)
                    .unwrap_or(true);
                if window.set_always_on_top(next).is_ok() {
                    let _ = top_item_for_event.set_checked(next);
                    if let Ok(mut snapshot) = runtime_for_menu.snapshot.lock() {
                        snapshot.window.always_on_top = next;
                    }
                }
            }
            "quit" => {
                runtime_for_menu.quitting.store(true, Ordering::SeqCst);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(move |tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                toggle_visible(tray.app_handle(), &runtime_for_tray);
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[cfg(not(feature = "editor"))]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                if let Some(runtime) = app.try_state::<Arc<RuntimeData>>() {
                    set_visible(&window, runtime.inner().as_ref(), true);
                } else {
                    let _ = window.show();
                }
                let _ = window.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::POSITION
                        | tauri_plugin_window_state::StateFlags::SIZE,
                )
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            set_pet_state,
            open_codex_session,
            report_media_event
        ])
        .setup(|app| {
            let (video_catalog_root, video_catalog) = resolve_video_catalog(app.handle())?;
            app.asset_protocol_scope()
                .allow_directory(&video_catalog_root, true)?;
            let video_path = video_catalog
                .get("idle")
                .and_then(|videos| videos.first())
                .map(PathBuf::from)
                .ok_or("idle video is missing")?;
            let decoder_session_id = format!("{}-{}", std::process::id(), now_ms());
            let api_token = std::env::var("CODEX_PET_TOKEN")
                .ok()
                .filter(|token| !token.is_empty());
            let (ttl_sender, ttl_receiver) = mpsc::sync_channel(64);
            let runtime = Arc::new(RuntimeData {
                snapshot: Mutex::new(RuntimeSnapshot {
                    state: StateSnapshot {
                        state: "idle".to_owned(),
                        previous_state: "idle".to_owned(),
                        revision: 0,
                        source: "startup".to_owned(),
                        updated_at_ms: now_ms(),
                    },
                    sessions: SessionRegistry {
                        revision: 0,
                        selected_session_id: None,
                        records: HashMap::new(),
                    },
                    playback: PlaybackSnapshot {
                        ready: false,
                        playing: false,
                        decoder_start_count: 0,
                        source_switch_count: 0,
                        current_video: None,
                        last_event: "startup".to_owned(),
                        error: None,
                    },
                    window: WindowFlags {
                        visible: true,
                        click_through: false,
                        always_on_top: true,
                    },
                    http: HttpStatus {
                        address: API_ADDRESS.to_owned(),
                        bound: false,
                        error: None,
                    },
                }),
                video_path: video_path.to_string_lossy().into_owned(),
                video_catalog_root: video_catalog_root.to_string_lossy().into_owned(),
                video_catalog,
                decoder_session_id,
                api_token,
                ttl_sender,
                metadata_inflight: Mutex::new(HashSet::new()),
                quitting: AtomicBool::new(false),
            });

            app.manage(runtime.clone());
            build_tray(app.handle(), runtime.clone())?;

            let app_for_ttl = app.handle().clone();
            let runtime_for_ttl = runtime.clone();
            thread::Builder::new()
                .name("codex-pet-ttl".to_owned())
                .spawn(move || run_ttl_worker(app_for_ttl, runtime_for_ttl, ttl_receiver))?;

            let app_for_http = app.handle().clone();
            let runtime_for_http = runtime.clone();
            thread::Builder::new()
                .name("codex-pet-loopback".to_owned())
                .spawn(move || start_http_server(app_for_http, runtime_for_http))?;

            if let Some(window) = app.get_webview_window("main") {
                let runtime_for_close = runtime.clone();
                let window_for_close = window.clone();
                window.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        if !runtime_for_close.quitting.load(Ordering::SeqCst) {
                            api.prevent_close();
                            set_visible(&window_for_close, &runtime_for_close, false);
                        }
                    }
                });
                window.show()?;
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Codex Pet Overlay");
}

#[cfg(feature = "editor")]
pub fn run() {
    editor::run();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exposes_exactly_nine_states() {
        assert_eq!(VALID_STATES.len(), 9);
        assert!(VALID_STATES.iter().all(|state| is_valid_state(state)));
        assert!(!is_valid_state("prompt-with-secrets"));
    }

    #[test]
    fn source_sanitization_is_bounded() {
        let sanitized = sanitize_source(Some("codex hook\n<private>".repeat(10)));
        assert!(sanitized.len() <= 48);
        assert!(!sanitized.contains('\n'));
        assert!(!sanitized.contains('<'));
    }

    #[test]
    fn validates_session_and_event_ids() {
        assert!(is_session_id("019fcc02-569f-7790-92ff-209aaca4275c"));
        assert!(!is_session_id("probe-session"));
        assert!(is_event_id(&"a".repeat(64)));
        assert!(!is_event_id(&"z".repeat(64)));
    }

    #[test]
    fn maps_numbered_video_names_without_hardcoded_lists() {
        assert_eq!(video_state_from_stem("idle"), Some("idle"));
        assert_eq!(video_state_from_stem("idle1"), Some("idle"));
        assert_eq!(video_state_from_stem("idle3"), Some("idle"));
        assert_eq!(video_state_from_stem("idle_1"), Some("idle"));
        assert_eq!(video_state_from_stem("idle-anything"), Some("idle"));
        assert_eq!(video_state_from_stem("IDLE_Custom"), Some("idle"));
        assert_eq!(video_state_from_stem("running-left2"), Some("running-left"));
        assert_eq!(
            video_state_from_stem("running-left-anything"),
            Some("running-left")
        );
        assert_eq!(
            video_state_from_stem("running-right99"),
            Some("running-right")
        );
        assert_eq!(video_state_from_stem("running"), Some("running"));
        assert_eq!(video_state_from_stem("running_custom"), Some("running"));
        assert_eq!(video_state_from_stem("unknown"), None);
    }

    #[test]
    fn catalog_keeps_duplicate_content_as_separate_candidates() {
        let root = std::env::temp_dir().join(format!(
            "codex-pet-video-scan-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&root).expect("create video scan fixture");
        for name in [
            "idle.mp4",
            "idle1.mp4",
            "idle_1.mp4",
            "running-left.mp4",
            "running-right.mp4",
            "waving.mp4",
            "jumping.mp4",
            "failed.mp4",
            "waiting.mp4",
            "review.mp4",
        ] {
            fs::write(root.join(name), b"same-content").expect("write video scan fixture");
        }

        let catalog = scan_video_catalog(&root).expect("scan video catalog");
        assert_eq!(catalog["idle"].len(), 3);
        assert_eq!(catalog["running"].len(), 2);
        fs::remove_dir_all(&root).expect("remove video scan fixture");
    }

    fn test_event(event_id: char, hook_event_name: &str, emitted_at_ms: u64) -> CodexEventRequest {
        CodexEventRequest {
            schema_version: 1,
            event_id: event_id.to_string().repeat(64),
            session_id: "019fcc02-569f-7790-92ff-209aaca4275c".to_owned(),
            turn_id: Some("turn-1".to_owned()),
            hook_event_name: hook_event_name.to_owned(),
            state: if hook_event_name == "Stop" {
                "review".to_owned()
            } else {
                "idle".to_owned()
            },
            activity: if hook_event_name == "Stop" {
                "ready".to_owned()
            } else {
                "idle".to_owned()
            },
            emitted_at_ms,
        }
    }

    #[test]
    fn session_end_blocks_late_events_but_allows_resume() {
        let mut record = SessionRecord {
            id: "019fcc02-569f-7790-92ff-209aaca4275c".to_owned(),
            state: "review".to_owned(),
            previous_state: "running".to_owned(),
            revision: 2,
            title: "Codex Session 019fcc02".to_owned(),
            activity: "ready".to_owned(),
            source: "test".to_owned(),
            updated_at_ms: 200,
            last_emitted_at_ms: 100,
            ended_at_ms: None,
            metadata_attempts: 0,
            unread: true,
            recent_event_ids: VecDeque::new(),
        };

        let ended = test_event('a', "SessionEnd", 200);
        assert!(accept_session_event(&mut record, &ended, true));
        assert_eq!(record.ended_at_ms, Some(200));
        assert!(!accept_session_event(
            &mut record,
            &test_event('b', "SessionStart", 150),
            false
        ));
        assert!(!accept_session_event(&mut record, &ended, true));
        assert!(accept_session_event(
            &mut record,
            &test_event('c', "SessionStart", 201),
            false
        ));
        assert_eq!(record.ended_at_ms, None);
    }

    #[test]
    fn hook_states_preserve_ready_and_waiting_semantics() {
        assert_eq!(state_for_hook("Stop", "review"), Some("review"));
        assert_eq!(
            state_for_hook("PermissionRequest", "waiting"),
            Some("waiting")
        );
        assert_eq!(state_for_hook("PostToolUse", "running"), Some("running"));
        assert_eq!(state_for_hook("Stop", "idle"), Some("review"));
    }

    #[test]
    fn selection_uses_official_priority_then_recency() {
        let mut records = HashMap::new();
        for (id, state, updated_at_ms) in [
            ("019fcc02-569f-7790-92ff-209aaca4275c", "running", 30),
            ("019fcc02-569f-7790-92ff-209aaca4275d", "review", 20),
            ("019fcc02-569f-7790-92ff-209aaca4275e", "waiting", 10),
        ] {
            records.insert(
                id.to_owned(),
                SessionRecord {
                    id: id.to_owned(),
                    state: state.to_owned(),
                    previous_state: "idle".to_owned(),
                    revision: 1,
                    title: fallback_session_title(id),
                    activity: "working".to_owned(),
                    source: "test".to_owned(),
                    updated_at_ms,
                    last_emitted_at_ms: updated_at_ms,
                    ended_at_ms: None,
                    metadata_attempts: 0,
                    unread: state == "review",
                    recent_event_ids: VecDeque::new(),
                },
            );
        }
        let registry = SessionRegistry {
            revision: 3,
            selected_session_id: None,
            records,
        };
        assert_eq!(
            select_session(&registry).map(|record| record.state.as_str()),
            Some("waiting")
        );
    }
}
