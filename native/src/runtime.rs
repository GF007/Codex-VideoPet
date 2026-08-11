use crate::{
    core::{
        now_ms, sanitize_source, CodexEventRequest, CoreOutcome, CoreState, SessionsSnapshot,
        StateSnapshot, MAX_SESSIONS,
    },
    metadata,
    mpv::{PlayerController, PlayerEvent},
    storage::SessionStore,
    theme::{load_ui_theme_at, LoadedUiTheme, UiTheme, UI_PALETTE_FILENAME},
    video_config::{load_video_config_at, select_clip, LoadedVideoConfig, VideoConfigSource},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const API_ADDRESS: &str = "127.0.0.1:48193";
pub const WM_APP_STATE: u32 = WM_APP + 1;
pub const WM_APP_WINDOW: u32 = WM_APP + 2;
pub const WM_APP_SHOW: u32 = WM_APP + 3;
pub const WM_APP_TRAY: u32 = WM_APP + 4;
pub const WM_APP_EXIT: u32 = WM_APP + 5;
pub const MAX_METADATA_ATTEMPTS_PER_SESSION: u8 = 3;
const ACTIVE_ROUTE_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
const OUTBOX_DRAIN_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, Serialize)]
pub struct PlaybackSnapshot {
    pub ready: bool,
    pub playing: bool,
    pub decoder_start_count: u32,
    pub source_switch_count: u32,
    pub crossfade_count: u32,
    pub crossfade_active: bool,
    pub crossfade_duration_ms: u64,
    pub current_video: Option<String>,
    pub current_asset: Option<String>,
    pub loop_start_ms: Option<u64>,
    pub loop_end_ms: Option<u64>,
    pub video_config_revision: u64,
    pub video_config_fingerprint: String,
    pub last_event: String,
    pub error: Option<String>,
    pub active_player_instances: u32,
    pub player_release_count: u32,
    pub hwdec_current: String,
    pub mpv_version: String,
    pub mpv_api: String,
    pub mpv_dll: String,
    pub mpv_configuration: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct WindowFlags {
    pub visible: bool,
    pub click_through: bool,
    pub always_on_top: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct HttpStatus {
    pub address: String,
    pub bound: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct RuntimeSnapshot {
    pub core: CoreState,
    pub playback: PlaybackSnapshot,
    pub window: WindowFlags,
    pub http: HttpStatus,
    pub ui_error: Option<String>,
    pub session_error: Option<String>,
    pub session_storage_error: Option<String>,
    pub hook_event_count: u64,
    pub last_hook_event_at_ms: Option<u64>,
    pub recovered_event_count: u64,
}

#[derive(Clone, Debug)]
pub struct UiSnapshot {
    pub state: StateSnapshot,
    pub sessions: SessionsSnapshot,
    pub playback: PlaybackSnapshot,
    pub window: WindowFlags,
    pub session_error: Option<String>,
    pub theme: UiTheme,
}

#[derive(Debug, Deserialize)]
pub struct SetStateRequest {
    pub state: String,
    pub ttl_ms: Option<u64>,
    pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SetWindowRequest {
    pub visible: Option<bool>,
    pub click_through: Option<bool>,
    pub always_on_top: Option<bool>,
}

struct TtlJob {
    deadline: Instant,
    expected_revision: u64,
    expected_sessions_revision: u64,
    restore_state: String,
}

struct SessionExpiryJob {
    deadline: Option<Instant>,
    expected_revision: u64,
}

#[derive(Clone)]
struct MediaSelection {
    active_state: Option<String>,
    current_file: Option<String>,
    current_path: Option<String>,
    last_by_state: HashMap<String, String>,
    generation: u64,
    random_state: u64,
}

#[derive(Clone, Copy)]
enum VideoSelectionReason {
    StateChange,
    ConfigReload,
    PlaybackEnded { expected_generation: u64 },
}

pub struct RuntimeData {
    pub snapshot: Mutex<RuntimeSnapshot>,
    pub video_catalog_root: String,
    pub decoder_session_id: String,
    pub api_token: Option<String>,
    ttl_sender: Sender<TtlJob>,
    session_expiry_sender: Sender<SessionExpiryJob>,
    session_store: SessionStore,
    metadata_inflight: Mutex<HashSet<String>>,
    active_route_gate: Mutex<()>,
    state_gate: Mutex<()>,
    media: Mutex<MediaSelection>,
    video_config: Mutex<LoadedVideoConfig>,
    video_config_revision: AtomicU64,
    video_config_error: Mutex<Option<String>>,
    ui_theme: Mutex<LoadedUiTheme>,
    ui_theme_revision: AtomicU64,
    ui_theme_error: Mutex<Option<String>>,
    player: Mutex<Option<PlayerController>>,
    video_hwnd: AtomicIsize,
    info_hwnd: AtomicIsize,
    show_on_next_hook: AtomicBool,
    pub shutdown: AtomicBool,
}

impl RuntimeData {
    pub fn new(video_config: LoadedVideoConfig) -> Arc<Self> {
        Self::new_with_store(video_config, SessionStore::local())
    }

    pub(crate) fn new_with_store(
        video_config: LoadedVideoConfig,
        session_store: SessionStore,
    ) -> Arc<Self> {
        let (ttl_sender, ttl_receiver) = mpsc::channel();
        let (session_expiry_sender, session_expiry_receiver) = mpsc::channel();
        let recovery = session_store.recover();
        let video_catalog_root = video_config.root.display().to_string();
        let video_config_fingerprint = video_config.fingerprint.clone();
        let (ui_theme, ui_theme_error) = match load_ui_theme_at(&video_config.root) {
            Ok(theme) => (theme, None),
            Err(error) => (LoadedUiTheme::fallback(), Some(error)),
        };
        let runtime = Arc::new(Self {
            snapshot: Mutex::new(RuntimeSnapshot {
                core: recovery.core,
                playback: PlaybackSnapshot {
                    ready: false,
                    playing: false,
                    decoder_start_count: 0,
                    source_switch_count: 0,
                    crossfade_count: 0,
                    crossfade_active: false,
                    crossfade_duration_ms: crate::mpv::CROSSFADE_DURATION_MS,
                    current_video: None,
                    current_asset: None,
                    loop_start_ms: None,
                    loop_end_ms: None,
                    video_config_revision: 1,
                    video_config_fingerprint: video_config_fingerprint.clone(),
                    last_event: "startup".to_owned(),
                    error: None,
                    active_player_instances: 0,
                    player_release_count: 0,
                    hwdec_current: "unavailable".to_owned(),
                    mpv_version: "unavailable".to_owned(),
                    mpv_api: "unavailable".to_owned(),
                    mpv_dll: "unavailable".to_owned(),
                    mpv_configuration: "unavailable".to_owned(),
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
                ui_error: None,
                session_error: None,
                session_storage_error: recovery
                    .error
                    .map(|error| crate::core::sanitize_label(&error, 180)),
                hook_event_count: 0,
                last_hook_event_at_ms: None,
                recovered_event_count: recovery.recovered_events as u64,
            }),
            video_catalog_root,
            decoder_session_id: format!("{}-{}", std::process::id(), now_ms()),
            api_token: std::env::var("CODEX_PET_TOKEN")
                .ok()
                .filter(|value| !value.is_empty()),
            ttl_sender,
            session_expiry_sender,
            session_store,
            metadata_inflight: Mutex::new(HashSet::new()),
            active_route_gate: Mutex::new(()),
            state_gate: Mutex::new(()),
            media: Mutex::new(MediaSelection {
                active_state: None,
                current_file: None,
                current_path: None,
                last_by_state: HashMap::new(),
                generation: 0,
                random_state: now_ms() ^ (std::process::id() as u64),
            }),
            video_config: Mutex::new(video_config),
            video_config_revision: AtomicU64::new(1),
            video_config_error: Mutex::new(None),
            ui_theme: Mutex::new(ui_theme),
            ui_theme_revision: AtomicU64::new(1),
            ui_theme_error: Mutex::new(ui_theme_error),
            player: Mutex::new(None),
            video_hwnd: AtomicIsize::new(0),
            info_hwnd: AtomicIsize::new(0),
            show_on_next_hook: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
        });
        start_ttl_worker(runtime.clone(), ttl_receiver);
        start_session_expiry_worker(runtime.clone(), session_expiry_receiver);
        start_video_config_watcher(runtime.clone());
        start_outbox_worker(runtime.clone());
        runtime.schedule_session_expiry();
        start_active_route_worker(runtime.clone());
        runtime
    }

    pub fn register_windows(&self, video_hwnd: isize, info_hwnd: isize) {
        self.video_hwnd.store(video_hwnd, Ordering::Release);
        self.info_hwnd.store(info_hwnd, Ordering::Release);
    }

    pub fn start_hidden_until_hook(&self) {
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.window.visible = false;
        }
        self.show_on_next_hook.store(true, Ordering::Release);
    }

    pub fn attach_player(
        self: &Arc<Self>,
        release_after: Duration,
        composition_width: i32,
        composition_height: i32,
    ) -> Result<(), String> {
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        let video_hwnd = self.video_hwnd.load(Ordering::Acquire);
        if video_hwnd == 0 {
            return Err("video window is unavailable".to_owned());
        }
        let (event_sender, event_receiver) = mpsc::channel();
        let controller = PlayerController::start(
            video_hwnd,
            composition_width,
            composition_height,
            release_after,
            event_sender,
        )?;
        *self
            .player
            .lock()
            .map_err(|_| "player lock poisoned".to_owned())? = Some(controller);
        start_player_event_worker(self.clone(), event_receiver);
        self.ensure_video_for_current_state(true)?;
        Ok(())
    }

    pub fn drain_outbox_once(self: &Arc<Self>) -> Result<usize, String> {
        if self.is_shutting_down() {
            return Ok(0);
        }
        let store = self.session_store.clone();
        match store.drain_pending_events(|event| self.apply_codex_event(event).map(|_| ())) {
            Ok(count) => {
                self.set_storage_error(None);
                Ok(count)
            }
            Err(error) => {
                self.set_storage_error(Some(error.clone()));
                Err(error)
            }
        }
    }

    pub fn ui_snapshot(&self) -> Option<UiSnapshot> {
        let snapshot = self.snapshot.lock().ok()?;
        let theme = self.ui_theme.lock().ok()?.theme;
        Some(UiSnapshot {
            state: snapshot.core.state.clone(),
            sessions: snapshot.core.sessions_snapshot(),
            playback: snapshot.playback.clone(),
            window: snapshot.window.clone(),
            session_error: snapshot.session_error.clone(),
            theme,
        })
    }

    pub fn selected_base_state(&self) -> Result<String, String> {
        self.snapshot
            .lock()
            .map(|snapshot| snapshot.core.selected_session_state())
            .map_err(|_| "runtime lock poisoned".to_owned())
    }

    pub fn transition_state(
        &self,
        requested: &str,
        source: &str,
        ttl_ms: Option<u64>,
    ) -> Result<StateSnapshot, String> {
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        let (state, restore_state, sessions_revision) = {
            let mut snapshot = self.snapshot.lock().map_err(|_| "runtime lock poisoned")?;
            let (state, restore_state) = snapshot.core.transition(requested, source)?;
            (state, restore_state, snapshot.core.sessions_revision())
        };
        self.ensure_video_for_current_state(false)?;
        self.notify_state();
        if let Some(ttl) = ttl_ms.filter(|ttl| *ttl > 0) {
            let _ = self.ttl_sender.send(TtlJob {
                deadline: Instant::now() + Duration::from_millis(ttl.clamp(50, 30_000)),
                expected_revision: state.revision,
                expected_sessions_revision: sessions_revision,
                restore_state,
            });
        }
        Ok(state)
    }

    pub fn apply_codex_event(
        self: &Arc<Self>,
        payload: CodexEventRequest,
    ) -> Result<CoreOutcome, String> {
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        let event_time_ms = payload.emitted_at_ms;
        let (outcome, persisted_core) = {
            let mut snapshot = self.snapshot.lock().map_err(|_| "runtime lock poisoned")?;
            let outcome = snapshot.core.apply_event(payload)?;
            snapshot.hook_event_count = snapshot.hook_event_count.saturating_add(1);
            snapshot.last_hook_event_at_ms = Some(event_time_ms);
            (outcome, snapshot.core.clone())
        };
        if let Err(error) = self.session_store.persist(&persisted_core) {
            self.set_storage_error(Some(error.clone()));
            return Err(error);
        }
        self.set_storage_error(None);
        if self.show_on_next_hook.swap(false, Ordering::AcqRel) {
            if let Ok(mut snapshot) = self.snapshot.lock() {
                snapshot.window.visible = true;
            }
            self.post_video(WM_APP_WINDOW);
        }
        self.publish_outcome(&outcome);
        if let Some(session_id) = outcome.metadata_session.clone() {
            self.schedule_metadata_lookup(session_id);
        }
        self.schedule_session_expiry();
        Ok(outcome)
    }

    pub fn mark_session_opened(&self, session_id: &str) -> Result<CoreOutcome, String> {
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        self.mark_session_opened_locked(session_id)
    }

    fn mark_session_opened_locked(&self, session_id: &str) -> Result<CoreOutcome, String> {
        let (outcome, persisted_core) = {
            let mut snapshot = self.snapshot.lock().map_err(|_| "runtime lock poisoned")?;
            let outcome = snapshot.core.mark_opened(session_id)?;
            (outcome, snapshot.core.clone())
        };
        if let Err(error) = self.session_store.persist(&persisted_core) {
            self.set_storage_error(Some(error.clone()));
            return Err(error);
        }
        self.set_storage_error(None);
        self.publish_outcome(&outcome);
        self.schedule_session_expiry();
        Ok(outcome)
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutdown.load(Ordering::Acquire)
    }

    pub fn is_verified_session_route(
        &self,
        session_id: &str,
        thread_id: &str,
        expected_revision: u64,
    ) -> bool {
        self.snapshot
            .lock()
            .map(|snapshot| {
                snapshot
                    .core
                    .is_verified_session_route(session_id, thread_id, expected_revision)
            })
            .unwrap_or(false)
    }

    pub fn refresh_active_routes(self: &Arc<Self>, allow_live_grace: bool) -> Result<(), String> {
        let _refresh = self
            .active_route_gate
            .lock()
            .map_err(|_| "route refresh gate poisoned".to_owned())?;
        if self.is_shutting_down() {
            return Err("Codex Pet 正在退出".to_owned());
        }
        let request = self
            .snapshot
            .lock()
            .map_err(|_| "runtime lock poisoned".to_owned())?
            .core
            .route_reconciliation_request();
        if request.is_empty() {
            return Ok(());
        }
        let active_thread_ids = metadata::fetch_active_thread_ids()?;
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        if self.is_shutting_down() {
            return Err("Codex Pet 正在退出".to_owned());
        }
        let (outcome, persisted_core, metadata_sessions) = {
            let mut snapshot = self.snapshot.lock().map_err(|_| "runtime lock poisoned")?;
            let outcome = snapshot.core.apply_active_thread_snapshot(
                &request,
                &active_thread_ids,
                allow_live_grace,
            );
            let metadata_sessions = snapshot.core.metadata_candidate_session_ids(MAX_SESSIONS);
            (outcome, snapshot.core.clone(), metadata_sessions)
        };
        if let Err(error) = self.session_store.persist(&persisted_core) {
            self.set_storage_error(Some(error.clone()));
            return Err(error);
        }
        self.set_storage_error(None);
        self.publish_outcome(&outcome);
        self.schedule_session_expiry();
        drop(_gate);
        if !metadata_sessions.is_empty() {
            start_metadata_restore_worker(self.clone(), metadata_sessions);
        }
        Ok(())
    }

    pub fn submit_verified_session_route<F>(
        &self,
        session_id: &str,
        thread_id: &str,
        expected_revision: u64,
        launcher: F,
    ) -> Result<(), String>
    where
        F: FnOnce(&str) -> Result<(), String>,
    {
        if self.is_shutting_down() {
            return Err("Codex Pet 正在退出".to_owned());
        }
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        let verified = self
            .snapshot
            .lock()
            .map(|snapshot| {
                snapshot
                    .core
                    .is_verified_session_route(session_id, thread_id, expected_revision)
            })
            .unwrap_or(false);
        if !verified || self.is_shutting_down() {
            return Err("主体任务映射已更新".to_owned());
        }
        launcher(&format!("codex://threads/{thread_id}"))?;
        self.mark_session_opened_locked(session_id).map(|_| ())
    }

    pub fn restore_state_if_revision(
        &self,
        expected_revision: u64,
        restore_state: String,
        expected_sessions_revision: u64,
        source: &str,
    ) -> Result<bool, String> {
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        let restored = {
            let mut snapshot = self
                .snapshot
                .lock()
                .map_err(|_| "runtime lock poisoned".to_owned())?;
            let restore_state = if snapshot.core.sessions_revision() != expected_sessions_revision {
                snapshot.core.selected_session_state()
            } else {
                restore_state
            };
            snapshot
                .core
                .restore_if_revision(expected_revision, restore_state, source)
        };
        if restored.is_some() {
            self.ensure_video_for_current_state(false)?;
            self.notify_state();
        }
        Ok(restored.is_some())
    }

    pub fn update_window(&self, request: SetWindowRequest) -> Result<WindowFlags, String> {
        let flags = {
            let mut snapshot = self.snapshot.lock().map_err(|_| "runtime lock poisoned")?;
            if let Some(value) = request.visible {
                snapshot.window.visible = value;
            }
            if let Some(value) = request.click_through {
                snapshot.window.click_through = value;
            }
            if let Some(value) = request.always_on_top {
                snapshot.window.always_on_top = value;
            }
            snapshot.window.clone()
        };
        self.post_video(WM_APP_WINDOW);
        self.notify_state();
        Ok(flags)
    }

    pub fn apply_window_visibility_to_player(&self, visible: bool) {
        if let Ok(player) = self.player.lock() {
            if let Some(player) = player.as_ref() {
                player.set_visible(visible);
            }
        }
    }

    pub fn resize_player(&self, width: i32, height: i32) -> Result<(), String> {
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        let player = self
            .player
            .lock()
            .map_err(|_| "player lock poisoned".to_owned())?;
        player
            .as_ref()
            .ok_or_else(|| "video player is unavailable".to_owned())?
            .resize(width, height)
    }

    pub fn diagnostics_json(&self) -> serde_json::Value {
        let Ok(video_config) = self.video_config.lock() else {
            return serde_json::json!({ "ok": false, "error": "video config lock poisoned" });
        };
        let Ok(snapshot) = self.snapshot.lock() else {
            return serde_json::json!({ "ok": false, "error": "runtime lock poisoned" });
        };
        let video_catalog = video_config
            .resolved_paths
            .iter()
            .map(|(state, paths)| (state.clone(), paths.clone()))
            .collect::<HashMap<_, _>>();
        let video_config_error = self
            .video_config_error
            .lock()
            .ok()
            .and_then(|value| value.clone());
        let ui_theme = self.ui_theme.lock().ok();
        let ui_theme_error = self
            .ui_theme_error
            .lock()
            .ok()
            .and_then(|value| value.clone());
        serde_json::json!({
            "ok": true,
            "state": snapshot.core.state,
            "sessions": snapshot.core.sessions_snapshot(),
            "playback": snapshot.playback,
            "window": snapshot.window,
            "http": snapshot.http,
            "videoPath": snapshot.playback.current_video,
            "videoCatalogRoot": self.video_catalog_root,
            "videoCatalog": video_catalog,
            "videoConfig": video_config.config,
            "videoConfigSource": video_config.source,
            "videoConfigRevision": self.video_config_revision.load(Ordering::Acquire),
            "videoConfigFingerprint": video_config.fingerprint,
            "videoConfigError": video_config_error,
            "uiTheme": ui_theme.as_ref().map(|theme| theme.theme),
            "uiThemeSource": ui_theme.as_ref().map(|theme| theme.source_file.clone()),
            "uiThemeSourceSha256": ui_theme.as_ref().map(|theme| theme.source_sha256.clone()),
            "uiThemeFingerprint": ui_theme.as_ref().map(|theme| theme.fingerprint.clone()),
            "uiThemeRevision": self.ui_theme_revision.load(Ordering::Acquire),
            "uiThemeError": ui_theme_error,
            "decoderSessionId": self.decoder_session_id,
            "validStates": crate::core::VALID_STATES,
            "renderer": "win32-libmpv-d2d-dwrite",
            "petDriver": crate::official_pet::DRIVER_VERSION,
            "uiError": snapshot.ui_error,
            "sessionError": snapshot.session_error,
            "sessionStorageError": snapshot.session_storage_error,
            "hookEventCount": snapshot.hook_event_count,
            "lastHookEventAtMs": snapshot.last_hook_event_at_ms,
            "recoveredEventCount": snapshot.recovered_event_count
        })
    }

    pub fn video_config_json(&self) -> serde_json::Value {
        let Ok(video_config) = self.video_config.lock() else {
            return serde_json::json!({ "ok": false, "error": "video config unavailable" });
        };
        let (
            current_state,
            playback_fingerprint,
            playback_ready,
            playback_error,
            active_player_instances,
        ) = self
            .snapshot
            .lock()
            .map(|snapshot| {
                (
                    Some(snapshot.core.state.state.clone()),
                    snapshot.playback.video_config_fingerprint.clone(),
                    snapshot.playback.ready,
                    snapshot.playback.error.clone(),
                    snapshot.playback.active_player_instances,
                )
            })
            .unwrap_or((None, String::new(), false, None, 0));
        let media = self.media.lock().ok();
        let error = self
            .video_config_error
            .lock()
            .ok()
            .and_then(|value| value.clone());
        serde_json::json!({
            "ok": true,
            "schema": crate::video_config::state_schema(),
            "config": video_config.config,
            "configRoot": video_config.root.display().to_string(),
            "configPath": video_config.config_path.display().to_string(),
            "source": video_config.source,
            "revision": self.video_config_revision.load(Ordering::Acquire),
            "fingerprint": video_config.fingerprint,
            "currentState": current_state,
            "selectedAsset": media.as_ref().and_then(|media| media.current_file.clone()),
            "playbackFingerprint": playback_fingerprint,
            "playbackReady": playback_ready,
            "playbackError": playback_error,
            "activePlayerInstances": active_player_instances,
            "error": error
        })
    }

    pub fn sessions_json(&self) -> serde_json::Value {
        let Ok(snapshot) = self.snapshot.lock() else {
            return serde_json::json!({ "ok": false, "error": "runtime unavailable" });
        };
        let sessions = snapshot.core.sessions_snapshot();
        serde_json::json!({
            "ok": true,
            "revision": sessions.revision,
            "selected_session_id": sessions.selected_session_id,
            "session_count": sessions.session_count,
            "sessions": sessions.sessions
        })
    }

    pub fn set_http_status(&self, bound: bool, error: Option<String>) {
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.http.bound = bound;
            snapshot.http.error = error;
        }
        self.notify_state();
    }

    pub fn set_ui_error(&self, error: Option<String>) {
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.ui_error = error.map(|value| crate::core::sanitize_label(&value, 180));
        }
    }

    pub fn set_session_error(&self, error: Option<String>) {
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.session_error = error.map(|value| crate::core::sanitize_label(&value, 120));
        }
        self.notify_state();
    }

    fn set_storage_error(&self, error: Option<String>) {
        if let Ok(mut snapshot) = self.snapshot.lock() {
            snapshot.session_storage_error =
                error.map(|value| crate::core::sanitize_label(&value, 180));
        }
    }

    fn set_video_config_error(&self, error: Option<String>) {
        let changed = self
            .video_config_error
            .lock()
            .map(|mut current| {
                if *current == error {
                    false
                } else {
                    *current = error;
                    true
                }
            })
            .unwrap_or(false);
        if changed {
            self.notify_state();
        }
    }

    fn set_ui_theme_error(&self, error: Option<String>) {
        let error = error.map(|value| crate::core::sanitize_label(&value, 180));
        let changed = self
            .ui_theme_error
            .lock()
            .map(|mut current| {
                if *current == error {
                    false
                } else {
                    *current = error;
                    true
                }
            })
            .unwrap_or(false);
        if changed {
            self.notify_state();
        }
    }

    fn apply_ui_theme(&self, next: LoadedUiTheme) -> bool {
        let changed = self
            .ui_theme
            .lock()
            .map(|mut current| {
                if current.fingerprint == next.fingerprint {
                    false
                } else {
                    *current = next;
                    true
                }
            })
            .unwrap_or(false);
        self.set_ui_theme_error(None);
        if changed {
            self.ui_theme_revision.fetch_add(1, Ordering::AcqRel);
            self.notify_state();
        }
        changed
    }

    fn apply_video_config(&self, next: LoadedVideoConfig) -> Result<bool, String> {
        let _gate = self
            .state_gate
            .lock()
            .map_err(|_| "state gate poisoned".to_owned())?;
        let previous = {
            let mut current = self
                .video_config
                .lock()
                .map_err(|_| "video config lock poisoned".to_owned())?;
            if current.fingerprint == next.fingerprint {
                None
            } else {
                let previous = current.clone();
                *current = next;
                Some(previous)
            }
        };
        let Some(previous) = previous else {
            self.set_video_config_error(None);
            return Ok(false);
        };
        let previous_revision = self.video_config_revision.fetch_add(1, Ordering::AcqRel);
        if let Err(error) = self.ensure_video_for_current_state(true) {
            if let Ok(mut current) = self.video_config.lock() {
                *current = previous;
            }
            self.video_config_revision
                .store(previous_revision, Ordering::Release);
            self.set_video_config_error(Some(error.clone()));
            return Err(error);
        }
        self.set_video_config_error(None);
        self.notify_state();
        Ok(true)
    }

    pub fn stop(&self) {
        let _gate = self.state_gate.lock().ok();
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut player) = self.player.lock() {
            if let Some(mut player) = player.take() {
                player.shutdown();
            }
        }
        let _ = self.ttl_sender.send(TtlJob {
            deadline: Instant::now(),
            expected_revision: u64::MAX,
            expected_sessions_revision: u64::MAX,
            restore_state: "idle".to_owned(),
        });
        let _ = self.session_expiry_sender.send(SessionExpiryJob {
            deadline: None,
            expected_revision: u64::MAX,
        });
    }

    fn publish_outcome(&self, outcome: &CoreOutcome) {
        if outcome.state_changed {
            let _ = self.ensure_video_for_current_state(false);
        }
        if outcome.state_changed || outcome.sessions_changed {
            self.notify_state();
        }
    }

    fn schedule_metadata_lookup(self: &Arc<Self>, session_id: String) {
        if self.is_shutting_down() {
            return;
        }
        let should_start = self
            .metadata_inflight
            .lock()
            .map(|mut inflight| inflight.insert(session_id.clone()))
            .unwrap_or(false);
        if !should_start {
            return;
        }
        let requested_revision = self.snapshot.lock().ok().and_then(|mut snapshot| {
            snapshot
                .core
                .begin_metadata(&session_id, MAX_METADATA_ATTEMPTS_PER_SESSION)
                .map(|request| request.revision)
        });
        let Some(requested_revision) = requested_revision else {
            if let Ok(mut inflight) = self.metadata_inflight.lock() {
                inflight.remove(&session_id);
            }
            return;
        };
        let runtime = self.clone();
        let cleanup_id = session_id.clone();
        if thread::Builder::new()
            .name("codex-pet-metadata".to_owned())
            .spawn(move || {
                let mut retry_id = None;
                let mut retry_after_error = false;
                let result = if runtime.is_shutting_down() {
                    Err("Codex Pet 正在退出".to_owned())
                } else {
                    metadata::fetch_thread_metadata(&session_id)
                };
                match result {
                    Ok(result) => {
                        if !runtime.is_shutting_down() {
                            if let Ok(_gate) = runtime.state_gate.lock() {
                                let outcome_and_core = runtime
                                    .snapshot
                                    .lock()
                                    .map(|mut snapshot| {
                                        let outcome = snapshot.core.apply_metadata(
                                            &session_id,
                                            requested_revision,
                                            result,
                                        );
                                        (outcome, snapshot.core.clone())
                                    })
                                    .ok();
                                if let Some((outcome, persisted_core)) = outcome_and_core {
                                    if let Err(error) =
                                        runtime.session_store.persist(&persisted_core)
                                    {
                                        runtime.set_storage_error(Some(error));
                                    } else {
                                        runtime.set_storage_error(None);
                                    }
                                    retry_id = outcome.metadata_session.clone();
                                    runtime.publish_outcome(&outcome);
                                    runtime.schedule_session_expiry();
                                }
                            }
                        }
                    }
                    Err(_) => {
                        if !runtime.is_shutting_down() {
                            retry_id = Some(session_id.clone());
                            retry_after_error = true;
                        }
                    }
                }
                if let Ok(mut inflight) = runtime.metadata_inflight.lock() {
                    inflight.remove(&session_id);
                }
                if let Some(retry_id) = retry_id {
                    if retry_after_error {
                        thread::sleep(Duration::from_millis(250));
                    }
                    if !runtime.is_shutting_down() {
                        runtime.schedule_metadata_lookup(retry_id);
                    }
                }
            })
            .is_err()
        {
            if let Ok(mut inflight) = self.metadata_inflight.lock() {
                inflight.remove(&cleanup_id);
            }
        }
    }

    fn schedule_session_expiry(&self) {
        let Some((expected_revision, expires_at_ms)) = self
            .snapshot
            .lock()
            .ok()
            .map(|snapshot| snapshot.core.session_expiry_schedule())
        else {
            return;
        };
        let deadline = expires_at_ms.map(|expires_at_ms| {
            Instant::now() + Duration::from_millis(expires_at_ms.saturating_sub(now_ms()).max(1))
        });
        let _ = self.session_expiry_sender.send(SessionExpiryJob {
            deadline,
            expected_revision,
        });
    }

    fn ensure_video_for_current_state(&self, force: bool) -> Result<(), String> {
        let reason = if force {
            VideoSelectionReason::ConfigReload
        } else {
            VideoSelectionReason::StateChange
        };
        self.select_video_for_current_state(reason).map(|_| ())
    }

    fn advance_video_for_current_state(&self, expected_generation: u64) -> Result<bool, String> {
        self.select_video_for_current_state(VideoSelectionReason::PlaybackEnded {
            expected_generation,
        })
    }

    fn select_video_for_current_state(&self, reason: VideoSelectionReason) -> Result<bool, String> {
        let state = self
            .snapshot
            .lock()
            .map_err(|_| "runtime lock poisoned".to_owned())?
            .core
            .state
            .state
            .clone();
        let video_config = self
            .video_config
            .lock()
            .map_err(|_| "video config lock poisoned".to_owned())?
            .clone();
        let state_config = video_config
            .config
            .states
            .get(&state)
            .ok_or_else(|| format!("state {state} has no video config"))?;
        let resolved_paths = video_config
            .resolved_paths
            .get(&state)
            .ok_or_else(|| format!("state {state} has no resolved videos"))?;
        let mut media = self
            .media
            .lock()
            .map_err(|_| "media lock poisoned".to_owned())?;
        let same_active_state =
            media.active_state.as_deref() == Some(&state) && media.current_path.is_some();
        match reason {
            VideoSelectionReason::StateChange if same_active_state => return Ok(false),
            VideoSelectionReason::PlaybackEnded {
                expected_generation,
            } if !same_active_state || media.generation != expected_generation => return Ok(false),
            _ => {}
        }
        let preserved_file = match reason {
            VideoSelectionReason::ConfigReload if same_active_state => {
                media.current_file.as_deref().and_then(|current| {
                    state_config
                        .clips
                        .iter()
                        .find(|clip| clip.enabled && clip.file.eq_ignore_ascii_case(current))
                })
            }
            _ => None,
        };
        let previous_media = media.clone();
        let previous = media.last_by_state.get(&state).cloned();
        media.random_state = xorshift(media.random_state);
        let clip = preserved_file
            .or_else(|| select_clip(state_config, previous.as_deref(), media.random_state))
            .ok_or_else(|| format!("state {state} has no enabled video"))?;
        let index = state_config
            .clips
            .iter()
            .position(|candidate| std::ptr::eq(candidate, clip))
            .ok_or_else(|| format!("state {state} selection is invalid"))?;
        let path = resolved_paths
            .get(index)
            .cloned()
            .ok_or_else(|| format!("state {state} video path is unresolved"))?;
        let file = clip.file.clone();
        let loop_range = clip.loop_range.clone();
        let transition_policy = state_config.transition_policy;
        media.generation += 1;
        let generation = media.generation;
        media.active_state = Some(state.clone());
        media.current_file = Some(file.clone());
        media.current_path = Some(path.clone());
        media.last_by_state.insert(state.clone(), file);
        let player = self
            .player
            .lock()
            .map_err(|_| "player lock poisoned".to_owned())?;
        if let Some(player) = player.as_ref() {
            let at_boundary = matches!(reason, VideoSelectionReason::PlaybackEnded { .. });
            let force_interrupt = matches!(reason, VideoSelectionReason::ConfigReload)
                || crate::official_pet::is_attention_state(&state);
            if let Err(error) = player.load(
                path,
                generation,
                loop_range,
                at_boundary,
                transition_policy,
                force_interrupt,
            ) {
                *media = previous_media;
                return Err(error);
            }
        }
        Ok(true)
    }

    fn apply_player_event(&self, event: PlayerEvent) {
        let Ok(_gate) = self.state_gate.lock() else {
            return;
        };
        if let PlayerEvent::MediaEnding { generation } | PlayerEvent::MediaEnded { generation } =
            &event
        {
            let result = self.advance_video_for_current_state(*generation);
            match result {
                Ok(false) => return,
                Ok(true) => {
                    if let Ok(mut snapshot) = self.snapshot.lock() {
                        snapshot.playback.ready = false;
                        snapshot.playback.playing = false;
                        snapshot.playback.last_event =
                            if matches!(event, PlayerEvent::MediaEnding { .. }) {
                                "media-ending"
                            } else {
                                "media-ended"
                            }
                            .to_owned();
                        snapshot.playback.error = None;
                    }
                }
                Err(error) => {
                    if let Ok(mut snapshot) = self.snapshot.lock() {
                        snapshot.playback.error = Some(crate::core::sanitize_label(&error, 180));
                        snapshot.playback.last_event = "media-error".to_owned();
                        snapshot.playback.ready = false;
                        snapshot.playback.playing = false;
                        snapshot.playback.video_config_fingerprint.clear();
                    }
                }
            }
            self.notify_state();
            return;
        }
        let (current_generation, selected_state, selected_file) = self
            .media
            .lock()
            .map(|media| {
                (
                    Some(media.generation),
                    media.active_state.clone(),
                    media.current_file.clone(),
                )
            })
            .unwrap_or((None, None, None));
        let (selected_loop, config_fingerprint) = self
            .video_config
            .lock()
            .map(|config| {
                let loop_range = selected_state
                    .as_deref()
                    .and_then(|state| config.config.states.get(state))
                    .and_then(|state| {
                        let file = selected_file.as_deref()?;
                        state
                            .clips
                            .iter()
                            .find(|clip| clip.file.eq_ignore_ascii_case(file))
                    })
                    .and_then(|clip| clip.loop_range.clone());
                (loop_range, config.fingerprint.clone())
            })
            .unwrap_or((None, String::new()));
        if let Ok(mut snapshot) = self.snapshot.lock() {
            match event {
                PlayerEvent::LibraryReady {
                    dll_path,
                    api_major,
                    api_minor,
                    version,
                    configuration,
                } => {
                    snapshot.playback.mpv_dll = dll_path;
                    snapshot.playback.mpv_api = format!("{api_major}.{api_minor}");
                    snapshot.playback.mpv_version = version;
                    snapshot.playback.mpv_configuration = configuration;
                    snapshot.playback.last_event = "library-ready".to_owned();
                    snapshot.playback.error = None;
                }
                PlayerEvent::HandleCount(count) => {
                    if count > snapshot.playback.active_player_instances {
                        snapshot.playback.decoder_start_count +=
                            count - snapshot.playback.active_player_instances;
                    }
                    snapshot.playback.active_player_instances = count;
                    snapshot.playback.last_event = "handle-count".to_owned();
                }
                PlayerEvent::SourceSwitched {
                    generation,
                    filename,
                } => {
                    snapshot.playback.source_switch_count += 1;
                    if current_generation == Some(generation) {
                        snapshot.playback.current_video = Some(filename);
                        snapshot.playback.current_asset = None;
                        snapshot.playback.loop_start_ms = None;
                        snapshot.playback.loop_end_ms = None;
                        snapshot.playback.ready = false;
                        snapshot.playback.playing = false;
                        snapshot.playback.last_event = "source-switched".to_owned();
                        snapshot.playback.error = None;
                    }
                }
                PlayerEvent::MediaReady {
                    generation,
                    filename,
                    hwdec,
                } => {
                    if current_generation == Some(generation) {
                        snapshot.playback.ready = true;
                        snapshot.playback.current_video = Some(filename);
                        snapshot.playback.current_asset = selected_file.clone();
                        snapshot.playback.loop_start_ms =
                            selected_loop.as_ref().map(|range| range.start_ms);
                        snapshot.playback.loop_end_ms =
                            selected_loop.as_ref().map(|range| range.end_ms);
                        snapshot.playback.video_config_revision =
                            self.video_config_revision.load(Ordering::Acquire);
                        snapshot.playback.video_config_fingerprint = config_fingerprint.clone();
                        snapshot.playback.hwdec_current = hwdec;
                        snapshot.playback.last_event = "media-ready".to_owned();
                        snapshot.playback.error = None;
                    }
                }
                PlayerEvent::MediaEnded { .. } | PlayerEvent::MediaEnding { .. } => {
                    unreachable!("media end handled before snapshot update")
                }
                PlayerEvent::CrossfadeStarted { generation } => {
                    if current_generation == Some(generation) {
                        snapshot.playback.crossfade_active = true;
                        snapshot.playback.last_event = "crossfade-started".to_owned();
                    }
                }
                PlayerEvent::CrossfadeCompleted { generation } => {
                    if current_generation == Some(generation) {
                        snapshot.playback.crossfade_active = false;
                        snapshot.playback.crossfade_count += 1;
                        snapshot.playback.last_event = "crossfade-completed".to_owned();
                    }
                }
                PlayerEvent::Playing(playing) => {
                    snapshot.playback.playing = playing;
                    snapshot.playback.last_event =
                        if playing { "playing" } else { "paused" }.to_owned();
                }
                PlayerEvent::Released => {
                    snapshot.playback.ready = false;
                    snapshot.playback.playing = false;
                    snapshot.playback.player_release_count += 1;
                    snapshot.playback.crossfade_active = false;
                    snapshot.playback.last_event = "player-released".to_owned();
                }
                PlayerEvent::MediaError {
                    generation,
                    message,
                } => {
                    if current_generation == Some(generation) {
                        snapshot.playback.error = Some(crate::core::sanitize_label(&message, 180));
                        snapshot.playback.last_event = "media-error".to_owned();
                        snapshot.playback.ready = false;
                        snapshot.playback.playing = false;
                        snapshot.playback.video_config_fingerprint.clear();
                    }
                }
                PlayerEvent::Error(error) => {
                    snapshot.playback.error = Some(crate::core::sanitize_label(&error, 180));
                    snapshot.playback.last_event = "error".to_owned();
                    snapshot.playback.ready = false;
                    snapshot.playback.playing = false;
                    snapshot.playback.video_config_fingerprint.clear();
                }
            }
        }
        self.notify_state();
    }

    pub fn notify_state(&self) {
        let hwnd = self.info_hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            let _ = unsafe {
                PostMessageW(
                    Some(windows::Win32::Foundation::HWND(hwnd as *mut _)),
                    WM_APP_STATE,
                    Default::default(),
                    Default::default(),
                )
            };
        }
    }

    pub fn post_video(&self, message: u32) {
        let hwnd = self.video_hwnd.load(Ordering::Acquire);
        if hwnd != 0 {
            let _ = unsafe {
                PostMessageW(
                    Some(windows::Win32::Foundation::HWND(hwnd as *mut _)),
                    message,
                    Default::default(),
                    Default::default(),
                )
            };
        }
    }
}

fn start_player_event_worker(runtime: Arc<RuntimeData>, receiver: Receiver<PlayerEvent>) {
    let _ = thread::Builder::new()
        .name("codex-pet-player-events".to_owned())
        .spawn(move || {
            while !runtime.shutdown.load(Ordering::Acquire) {
                match receiver.recv_timeout(Duration::from_millis(250)) {
                    Ok(event) => runtime.apply_player_event(event),
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
}

fn start_outbox_worker(runtime: Arc<RuntimeData>) {
    let _ = thread::Builder::new()
        .name("codex-pet-outbox".to_owned())
        .spawn(move || {
            while !runtime.is_shutting_down() {
                let _ = runtime.drain_outbox_once();
                let wait_until = Instant::now() + OUTBOX_DRAIN_INTERVAL;
                while !runtime.is_shutting_down() && Instant::now() < wait_until {
                    thread::sleep(Duration::from_millis(50));
                }
            }
        });
}

fn start_video_config_watcher(runtime: Arc<RuntimeData>) {
    let root = PathBuf::from(&runtime.video_catalog_root);
    if !root.is_dir() {
        return;
    }
    let _ = thread::Builder::new()
        .name("codex-pet-video-config".to_owned())
        .spawn(move || {
            let config_path = root.join(crate::video_config::VIDEO_CONFIG_FILENAME);
            let palette_path = root.join(UI_PALETTE_FILENAME);
            while !runtime.shutdown.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(250));
                if !config_path.is_file() {
                    let configured = runtime
                        .video_config
                        .lock()
                        .map(|config| config.source == VideoConfigSource::Configured)
                        .unwrap_or(false);
                    if configured {
                        runtime.set_video_config_error(Some(
                            "video-config.json was removed; the last valid config remains active"
                                .to_owned(),
                        ));
                    }
                    continue;
                }
                match load_video_config_at(&root) {
                    Ok(config) => {
                        let _ = runtime.apply_video_config(config);
                    }
                    Err(error) => runtime.set_video_config_error(Some(
                        crate::core::sanitize_label(&error.to_string(), 180),
                    )),
                }
                if !palette_path.is_file() {
                    runtime.set_ui_theme_error(Some(format!(
                        "{UI_PALETTE_FILENAME} was removed; the last valid theme remains active"
                    )));
                    continue;
                }
                match load_ui_theme_at(&root) {
                    Ok(theme) => {
                        runtime.apply_ui_theme(theme);
                    }
                    Err(error) => runtime.set_ui_theme_error(Some(error)),
                }
            }
        });
}

fn start_ttl_worker(runtime: Arc<RuntimeData>, receiver: Receiver<TtlJob>) {
    let _ = thread::Builder::new()
        .name("codex-pet-ttl".to_owned())
        .spawn(move || {
            while !runtime.shutdown.load(Ordering::Acquire) {
                let Ok(mut pending) = receiver.recv() else {
                    return;
                };
                if runtime.shutdown.load(Ordering::Acquire) {
                    return;
                }
                loop {
                    let remaining = pending.deadline.saturating_duration_since(Instant::now());
                    match receiver.recv_timeout(remaining) {
                        Ok(newer) => pending = newer,
                        Err(RecvTimeoutError::Disconnected) => return,
                        Err(RecvTimeoutError::Timeout) => {
                            if let Ok(_gate) = runtime.state_gate.lock() {
                                let restored =
                                    runtime.snapshot.lock().ok().and_then(|mut snapshot| {
                                        let restore_state = if snapshot.core.sessions_revision()
                                            != pending.expected_sessions_revision
                                        {
                                            snapshot.core.selected_session_state()
                                        } else {
                                            pending.restore_state
                                        };
                                        snapshot.core.restore_if_revision(
                                            pending.expected_revision,
                                            restore_state,
                                            "ttl",
                                        )
                                    });
                                if restored.is_some() {
                                    let _ = runtime.ensure_video_for_current_state(false);
                                    runtime.notify_state();
                                }
                            }
                            break;
                        }
                    }
                }
            }
        });
}

fn start_metadata_restore_worker(runtime: Arc<RuntimeData>, session_ids: Vec<String>) {
    if session_ids.is_empty() {
        return;
    }
    let _ = thread::Builder::new()
        .name("codex-pet-metadata-restore".to_owned())
        .spawn(move || {
            for session_id in session_ids {
                if runtime.is_shutting_down() {
                    return;
                }
                runtime.schedule_metadata_lookup(session_id.clone());
                while !runtime.is_shutting_down() {
                    let complete = runtime
                        .snapshot
                        .lock()
                        .map(|snapshot| {
                            snapshot.core.metadata_lookup_complete(
                                &session_id,
                                MAX_METADATA_ATTEMPTS_PER_SESSION,
                            )
                        })
                        .unwrap_or(true);
                    if complete {
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }
        });
}

fn start_active_route_worker(runtime: Arc<RuntimeData>) {
    let _ = thread::Builder::new()
        .name("codex-pet-active-routes".to_owned())
        .spawn(move || loop {
            if runtime.is_shutting_down() {
                return;
            }
            let _ = runtime.refresh_active_routes(true);
            let wait_until = Instant::now() + ACTIVE_ROUTE_REFRESH_INTERVAL;
            while !runtime.is_shutting_down() && Instant::now() < wait_until {
                thread::sleep(Duration::from_millis(100));
            }
        });
}

fn start_session_expiry_worker(runtime: Arc<RuntimeData>, receiver: Receiver<SessionExpiryJob>) {
    let _ = thread::Builder::new()
        .name("codex-pet-session-expiry".to_owned())
        .spawn(move || {
            let mut pending: Option<SessionExpiryJob> = None;
            while !runtime.shutdown.load(Ordering::Acquire) {
                let received = if let Some(job) = pending.as_ref() {
                    let remaining = job
                        .deadline
                        .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                        .unwrap_or_default();
                    receiver.recv_timeout(remaining)
                } else {
                    match receiver.recv() {
                        Ok(job) => {
                            pending = job.deadline.map(|_| job);
                            continue;
                        }
                        Err(_) => return,
                    }
                };
                match received {
                    Ok(job) => pending = job.deadline.map(|_| job),
                    Err(RecvTimeoutError::Disconnected) => return,
                    Err(RecvTimeoutError::Timeout) if pending.is_some() => {
                        let job = pending.take().expect("pending expiry job");
                        if let Ok(_gate) = runtime.state_gate.lock() {
                            let outcome_and_core =
                                runtime.snapshot.lock().ok().and_then(|mut snapshot| {
                                    let outcome = snapshot.core.expire_notifications_if_revision(
                                        job.expected_revision,
                                        now_ms(),
                                    )?;
                                    Some((outcome, snapshot.core.clone()))
                                });
                            if let Some((outcome, persisted_core)) = outcome_and_core {
                                if let Err(error) = runtime.session_store.persist(&persisted_core) {
                                    runtime.set_storage_error(Some(error));
                                } else {
                                    runtime.set_storage_error(None);
                                }
                                runtime.publish_outcome(&outcome);
                            }
                            runtime.schedule_session_expiry();
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                }
            }
        });
}

fn xorshift(mut value: u64) -> u64 {
    if value == 0 {
        value = 0x9e3779b97f4a7c15;
    }
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    value
}

pub fn safe_source(request: SetStateRequest) -> (String, String, Option<u64>) {
    (
        request.state,
        sanitize_source(request.source),
        request.ttl_ms,
    )
}

pub fn video_leaf(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("video")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(event_id: char, hook_event_name: &str, emitted_at_ms: u64) -> CodexEventRequest {
        let state = crate::official_pet::hook_state(hook_event_name).unwrap();
        CodexEventRequest {
            schema_version: 1,
            event_id: event_id.to_string().repeat(64),
            session_id: "019fcc02-569f-7790-92ff-209aaca4275c".to_owned(),
            turn_id: None,
            hook_event_name: hook_event_name.to_owned(),
            state: state.to_owned(),
            activity: crate::official_pet::activity_for_state(state).to_owned(),
            emitted_at_ms,
        }
    }

    fn test_runtime() -> Arc<RuntimeData> {
        let states = crate::core::VALID_STATES
            .iter()
            .map(|state| {
                (
                    (*state).to_owned(),
                    crate::video_config::StateVideoConfig {
                        selection: crate::video_config::SelectionMode::WeightedRandom,
                        transition_policy: crate::video_config::TransitionPolicy::Immediate,
                        clips: vec![crate::video_config::VideoClipConfig {
                            file: format!("{state}.mp4"),
                            enabled: true,
                            weight: 1,
                            loop_range: None,
                        }],
                    },
                )
            })
            .collect();
        let config = crate::video_config::VideoConfig {
            schema_version: crate::video_config::VIDEO_CONFIG_SCHEMA_VERSION,
            states,
        };
        let root = PathBuf::from("test-catalog");
        let resolved_paths = crate::core::VALID_STATES
            .iter()
            .map(|state| ((*state).to_owned(), vec![format!("{state}.mp4")]))
            .collect();
        RuntimeData::new_with_store(
            LoadedVideoConfig {
                root: root.clone(),
                config_path: root.join(crate::video_config::VIDEO_CONFIG_FILENAME),
                source: VideoConfigSource::LegacyScan,
                fingerprint: crate::video_config::config_fingerprint(&config),
                config,
                resolved_paths,
            },
            SessionStore::disabled(),
        )
    }

    fn add_idle_clip(runtime: &RuntimeData, file: &str) {
        let mut config = runtime.video_config.lock().unwrap();
        config.config.states.get_mut("idle").unwrap().clips.push(
            crate::video_config::VideoClipConfig {
                file: file.to_owned(),
                enabled: true,
                weight: 1,
                loop_range: None,
            },
        );
        config
            .resolved_paths
            .get_mut("idle")
            .unwrap()
            .push(file.to_owned());
        config.fingerprint = crate::video_config::config_fingerprint(&config.config);
    }

    #[test]
    fn xorshift_never_stays_zero() {
        assert_ne!(xorshift(0), 0);
    }

    #[test]
    fn player_metrics_allow_two_decoders_only_during_a_crossfade() {
        let runtime = test_runtime();
        runtime.apply_player_event(PlayerEvent::HandleCount(1));
        runtime.apply_player_event(PlayerEvent::HandleCount(2));
        runtime.apply_player_event(PlayerEvent::CrossfadeStarted { generation: 0 });
        {
            let snapshot = runtime.snapshot.lock().unwrap();
            assert_eq!(snapshot.playback.active_player_instances, 2);
            assert_eq!(snapshot.playback.decoder_start_count, 2);
            assert!(snapshot.playback.crossfade_active);
        }

        runtime.apply_player_event(PlayerEvent::HandleCount(1));
        runtime.apply_player_event(PlayerEvent::CrossfadeCompleted { generation: 0 });
        let snapshot = runtime.snapshot.lock().unwrap();
        assert_eq!(snapshot.playback.active_player_instances, 1);
        assert_eq!(snapshot.playback.crossfade_count, 1);
        assert!(!snapshot.playback.crossfade_active);
        drop(snapshot);
        runtime.stop();
    }

    #[test]
    fn completed_media_selects_another_clip_without_changing_logical_state() {
        let runtime = test_runtime();
        add_idle_clip(&runtime, "idle2.mp4");
        runtime.ensure_video_for_current_state(true).unwrap();
        let state_before = runtime.snapshot.lock().unwrap().core.state.clone();
        let (generation_before, file_before) = {
            let media = runtime.media.lock().unwrap();
            (media.generation, media.current_file.clone().unwrap())
        };

        runtime.apply_player_event(PlayerEvent::MediaEnded {
            generation: generation_before,
        });

        let state_after = runtime.snapshot.lock().unwrap().core.state.clone();
        let media = runtime.media.lock().unwrap();
        assert_eq!(state_after.state, state_before.state);
        assert_eq!(state_after.revision, state_before.revision);
        assert_eq!(media.generation, generation_before + 1);
        assert_ne!(media.current_file.as_deref(), Some(file_before.as_str()));
        drop(media);
        assert_eq!(
            runtime.snapshot.lock().unwrap().playback.last_event,
            "media-ended"
        );
        runtime.stop();
    }

    #[test]
    fn completed_media_replays_the_only_enabled_clip() {
        let runtime = test_runtime();
        runtime.ensure_video_for_current_state(true).unwrap();
        let (generation_before, file_before) = {
            let media = runtime.media.lock().unwrap();
            (media.generation, media.current_file.clone())
        };

        runtime.apply_player_event(PlayerEvent::MediaEnded {
            generation: generation_before,
        });

        let media = runtime.media.lock().unwrap();
        assert_eq!(media.generation, generation_before + 1);
        assert_eq!(media.current_file, file_before);
        drop(media);
        runtime.stop();
    }

    #[test]
    fn stale_media_completion_cannot_advance_a_new_state() {
        let runtime = test_runtime();
        runtime.ensure_video_for_current_state(true).unwrap();
        let stale_generation = runtime.media.lock().unwrap().generation;
        runtime
            .transition_state("running", "stale-media-test", None)
            .unwrap();
        let generation_before = runtime.media.lock().unwrap().generation;

        runtime.apply_player_event(PlayerEvent::MediaEnded {
            generation: stale_generation,
        });

        let media = runtime.media.lock().unwrap();
        assert_eq!(media.active_state.as_deref(), Some("running"));
        assert_eq!(media.generation, generation_before);
        drop(media);
        runtime.stop();
    }

    #[test]
    fn concurrent_transitions_leave_media_aligned_with_core_state() {
        let runtime = test_runtime();
        let first = runtime.clone();
        let second = runtime.clone();
        let left = thread::spawn(move || {
            for _ in 0..100 {
                first
                    .transition_state("running", "race-test-a", None)
                    .unwrap();
            }
        });
        let right = thread::spawn(move || {
            for _ in 0..100 {
                second
                    .transition_state("waiting", "race-test-b", None)
                    .unwrap();
            }
        });
        left.join().unwrap();
        right.join().unwrap();
        let state = runtime.snapshot.lock().unwrap().core.state.state.clone();
        let media_state = runtime.media.lock().unwrap().active_state.clone();
        assert_eq!(media_state.as_deref(), Some(state.as_str()));
        runtime.stop();
    }

    #[test]
    fn transient_restore_uses_new_session_base_after_session_revision_changes() {
        let runtime = test_runtime();
        runtime
            .snapshot
            .lock()
            .unwrap()
            .core
            .apply_event(event('a', "SessionStart", 100))
            .unwrap();
        let sessions_revision = runtime.snapshot.lock().unwrap().core.sessions_revision();
        let transient = runtime
            .transition_state("waving", "official:first-awake", None)
            .unwrap();
        runtime
            .snapshot
            .lock()
            .unwrap()
            .core
            .apply_event(event('b', "UserPromptSubmit", 101))
            .unwrap();
        assert!(runtime
            .restore_state_if_revision(
                transient.revision,
                "idle".to_owned(),
                sessions_revision,
                "test",
            )
            .unwrap());
        assert_eq!(runtime.snapshot.lock().unwrap().core.state.state, "running");
        runtime.stop();
    }

    #[test]
    fn config_reload_reapplies_media_without_mutating_logical_state() {
        let runtime = test_runtime();
        runtime.ensure_video_for_current_state(true).unwrap();
        let state_before = runtime.snapshot.lock().unwrap().core.state.clone();
        let generation_before = runtime.media.lock().unwrap().generation;
        let mut next = runtime.video_config.lock().unwrap().clone();
        next.config.states.get_mut("idle").unwrap().clips[0].loop_range =
            Some(crate::video_config::VideoLoop {
                start_ms: 500,
                end_ms: 1500,
            });
        next.fingerprint = crate::video_config::config_fingerprint(&next.config);

        assert!(runtime.apply_video_config(next).unwrap());
        let state_after = runtime.snapshot.lock().unwrap().core.state.clone();
        let media = runtime.media.lock().unwrap();
        assert_eq!(state_after.state, state_before.state);
        assert_eq!(state_after.revision, state_before.revision);
        assert_eq!(media.active_state.as_deref(), Some("idle"));
        assert_eq!(media.current_file.as_deref(), Some("idle.mp4"));
        assert_eq!(media.generation, generation_before + 1);
        drop(media);
        runtime.stop();
    }

    #[test]
    fn media_error_invalidates_applied_playback_fingerprint() {
        let runtime = test_runtime();
        let fingerprint = runtime.video_config.lock().unwrap().fingerprint.clone();
        {
            let mut snapshot = runtime.snapshot.lock().unwrap();
            snapshot.playback.ready = true;
            snapshot.playback.playing = true;
            snapshot.playback.video_config_fingerprint = fingerprint;
        }

        runtime.apply_player_event(PlayerEvent::MediaError {
            generation: 0,
            message: "loop failed".to_owned(),
        });

        let snapshot = runtime.snapshot.lock().unwrap();
        assert!(!snapshot.playback.ready);
        assert!(!snapshot.playback.playing);
        assert!(snapshot.playback.video_config_fingerprint.is_empty());
        assert_eq!(snapshot.playback.last_event, "media-error");
        drop(snapshot);
        assert_eq!(runtime.video_config_json()["playbackError"], "loop failed");
        runtime.stop();
    }

    #[test]
    fn video_config_response_identifies_the_authoritative_root() {
        let runtime = test_runtime();
        let response = runtime.video_config_json();
        assert_eq!(response["configRoot"], "test-catalog");
        assert_eq!(
            response["configPath"],
            PathBuf::from("test-catalog")
                .join(crate::video_config::VIDEO_CONFIG_FILENAME)
                .display()
                .to_string()
        );
        runtime.stop();
    }

    #[test]
    fn applying_a_new_ui_theme_updates_the_next_ui_snapshot_once() {
        let runtime = test_runtime();
        let next = LoadedUiTheme {
            theme: UiTheme {
                base: crate::theme::UiColor::new(1, 2, 3),
                accent: crate::theme::UiColor::new(4, 5, 6),
                light: crate::theme::UiColor::new(240, 241, 242),
            },
            source_file: "idle.mp4".to_owned(),
            source_sha256: "a".repeat(64),
            fingerprint: "test-theme".to_owned(),
        };

        assert!(runtime.apply_ui_theme(next.clone()));
        assert_eq!(runtime.ui_snapshot().unwrap().theme, next.theme);
        assert_eq!(runtime.ui_theme_revision.load(Ordering::Acquire), 2);
        assert!(!runtime.apply_ui_theme(next));
        assert_eq!(runtime.ui_theme_revision.load(Ordering::Acquire), 2);
        runtime.stop();
    }
}
