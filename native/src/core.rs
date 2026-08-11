use crate::official_pet;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_SESSIONS: usize = 32;
const MAX_EVENT_IDS_PER_SESSION: usize = 64;
const SESSION_TITLE_MAX_CHARS: usize = 64;
const MAX_EVENT_FUTURE_SKEW_MS: u64 = 60_000;
const ACTIVE_ROUTE_LIVE_GRACE_MS: u64 = 15_000;
pub const VALID_STATES: [&str; 9] = [
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateSnapshot {
    pub state: String,
    pub previous_state: String,
    pub revision: u64,
    pub source: String,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionActivity {
    pub id: String,
    pub target_thread_id: Option<String>,
    pub state: String,
    pub previous_state: String,
    pub revision: u64,
    pub title: String,
    pub activity: String,
    pub source: String,
    pub updated_at_ms: u64,
    pub priority: u8,
    pub unread: bool,
    pub expires_at_ms: Option<u64>,
    pub deep_link: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionsSnapshot {
    pub revision: u64,
    pub selected_session_id: Option<String>,
    pub session_count: usize,
    pub sessions: Vec<SessionActivity>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RouteEligibility {
    #[default]
    Unverified,
    Eligible,
    Suppressed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionRecord {
    id: String,
    #[serde(skip)]
    verified_thread_id: Option<String>,
    #[serde(skip)]
    route_eligibility: RouteEligibility,
    #[serde(skip)]
    live_event_observed_at_ms: Option<u64>,
    state: String,
    previous_state: String,
    revision: u64,
    title: String,
    #[serde(skip)]
    transient_title: Option<String>,
    activity: String,
    source: String,
    updated_at_ms: u64,
    last_emitted_at_ms: u64,
    ended_at_ms: Option<u64>,
    metadata_attempts: u8,
    unread: bool,
    expires_at_ms: Option<u64>,
    recent_event_ids: VecDeque<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SessionRegistry {
    revision: u64,
    selected_session_id: Option<String>,
    records: HashMap<String, SessionRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CoreState {
    pub state: StateSnapshot,
    sessions: SessionRegistry,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CodexEventRequest {
    pub schema_version: u8,
    pub event_id: String,
    pub session_id: String,
    pub turn_id: Option<String>,
    pub hook_event_name: String,
    pub state: String,
    pub activity: String,
    pub emitted_at_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub struct ThreadMetadata {
    pub thread_id: Option<String>,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub status_type: Option<String>,
    pub waiting_on_approval: bool,
    pub waiting_on_user_input: bool,
    pub plan_implementation_pending: bool,
    pub has_unread_turn: bool,
}

#[derive(Clone, Debug)]
pub struct CoreOutcome {
    pub state: StateSnapshot,
    pub sessions: SessionsSnapshot,
    pub state_changed: bool,
    pub sessions_changed: bool,
    pub metadata_session: Option<String>,
}

#[derive(Clone, Debug)]
pub struct MetadataRequest {
    pub revision: u64,
}

impl Default for CoreState {
    fn default() -> Self {
        Self {
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
        }
    }
}

impl CoreState {
    pub fn sessions_snapshot(&self) -> SessionsSnapshot {
        sessions_snapshot(&self.sessions)
    }

    pub fn selected_session_state(&self) -> String {
        select_session(&self.sessions)
            .map(|record| record.state.clone())
            .unwrap_or_else(|| "idle".to_owned())
    }

    pub fn session_expiry_schedule(&self) -> (u64, Option<u64>) {
        (
            self.sessions.revision,
            self.sessions
                .records
                .values()
                .filter_map(|record| record.expires_at_ms)
                .min(),
        )
    }

    pub fn sessions_revision(&self) -> u64 {
        self.sessions.revision
    }

    pub fn prepare_restored(&mut self) -> Result<(), String> {
        if self.sessions.records.len() > MAX_SESSIONS {
            return Err("persisted session count exceeds limit".to_owned());
        }
        let current_time_ms = now_ms();
        for (session_id, record) in self.sessions.records.iter_mut() {
            if session_id != &record.id || !is_session_id(session_id) {
                return Err("persisted session id is invalid".to_owned());
            }
            if !is_valid_state(&record.state) || !is_valid_state(&record.previous_state) {
                return Err("persisted session state is invalid".to_owned());
            }
            if !is_valid_activity(&record.activity) {
                return Err("persisted session activity is invalid".to_owned());
            }
            if record.recent_event_ids.len() > MAX_EVENT_IDS_PER_SESSION
                || record
                    .recent_event_ids
                    .iter()
                    .any(|event_id| !is_event_id(event_id))
            {
                return Err("persisted event history is invalid".to_owned());
            }
            if record.last_emitted_at_ms > current_time_ms.saturating_add(MAX_EVENT_FUTURE_SKEW_MS)
            {
                return Err("persisted event timestamp is invalid".to_owned());
            }
            record.title = sanitize_title(&record.title)
                .filter(|title| !is_legacy_fallback_session_title(title))
                .unwrap_or_else(|| fallback_session_title(session_id));
            record.transient_title = None;
            record.source = sanitize_source(Some(record.source.clone()));
            record.metadata_attempts = 0;
            record.verified_thread_id = None;
            record.route_eligibility = RouteEligibility::Unverified;
            record.live_event_observed_at_ms = None;
        }

        let selected =
            select_session(&self.sessions).map(|record| (record.id.clone(), record.state.clone()));
        self.sessions.selected_session_id = selected.as_ref().map(|(id, _)| id.clone());
        self.sessions.revision = self.sessions.revision.saturating_add(1);
        let previous = self.state.state.clone();
        self.state = StateSnapshot {
            state: selected
                .map(|(_, state)| state)
                .unwrap_or_else(|| "idle".to_owned()),
            previous_state: previous,
            revision: self.state.revision.saturating_add(1),
            source: "storage:restore".to_owned(),
            updated_at_ms: current_time_ms,
        };
        Ok(())
    }

    pub fn transition(
        &mut self,
        requested: &str,
        source: &str,
    ) -> Result<(StateSnapshot, String), String> {
        if !is_valid_state(requested) {
            return Err(format!("unknown state: {requested}"));
        }
        let previous = self.state.state.clone();
        self.state = StateSnapshot {
            state: requested.to_owned(),
            previous_state: previous.clone(),
            revision: self.state.revision + 1,
            source: sanitize_source(Some(source.to_owned())),
            updated_at_ms: now_ms(),
        };
        Ok((self.state.clone(), previous))
    }

    pub fn restore_if_revision(
        &mut self,
        expected_revision: u64,
        restore_state: String,
        source: &str,
    ) -> Option<StateSnapshot> {
        if self.state.revision != expected_revision {
            return None;
        }
        let current = self.state.state.clone();
        self.state = StateSnapshot {
            state: restore_state,
            previous_state: current,
            revision: self.state.revision + 1,
            source: sanitize_source(Some(source.to_owned())),
            updated_at_ms: now_ms(),
        };
        Some(self.state.clone())
    }

    pub fn apply_event(&mut self, payload: CodexEventRequest) -> Result<CoreOutcome, String> {
        let expected_state = validate_codex_event(&payload)?;
        let event_time_ms = payload.emitted_at_ms;
        let mut metadata_session = None;
        let (state_changed, sessions_changed) = if payload.hook_event_name == "SessionEnd" {
            let updated = if let Some(record) = self.sessions.records.get_mut(&payload.session_id) {
                if !accept_session_event(record, &payload, true) {
                    return Ok(self.outcome(false, false, None));
                }
                record.previous_state = record.state.clone();
                if record.state == "failed" {
                    record.activity = "recovering".to_owned();
                } else if record.unread {
                    record.state = "review".to_owned();
                    record.activity = "ready".to_owned();
                } else {
                    record.state = "idle".to_owned();
                    record.activity = "idle".to_owned();
                }
                record.source = "codex-hook".to_owned();
                record.updated_at_ms = event_time_ms;
                record.expires_at_ms =
                    official_pet::notification_expires_at_ms(&record.state, record.updated_at_ms);
                record.revision += 1;
                true
            } else {
                false
            };
            if updated {
                self.sessions.revision += 1;
            }
            (self.recompute_aggregate("codex-hook"), updated)
        } else {
            let is_new = !self.sessions.records.contains_key(&payload.session_id);
            if is_new {
                evict_oldest_session(&mut self.sessions);
                self.sessions.records.insert(
                    payload.session_id.clone(),
                    SessionRecord {
                        id: payload.session_id.clone(),
                        verified_thread_id: None,
                        route_eligibility: RouteEligibility::Unverified,
                        live_event_observed_at_ms: None,
                        state: "idle".to_owned(),
                        previous_state: "idle".to_owned(),
                        revision: 0,
                        title: fallback_session_title(&payload.session_id),
                        transient_title: None,
                        activity: "starting".to_owned(),
                        source: "codex-hook".to_owned(),
                        updated_at_ms: event_time_ms,
                        last_emitted_at_ms: 0,
                        ended_at_ms: None,
                        metadata_attempts: 0,
                        unread: false,
                        expires_at_ms: None,
                        recent_event_ids: VecDeque::new(),
                    },
                );
                metadata_session = Some(payload.session_id.clone());
            }
            let record = self
                .sessions
                .records
                .get_mut(&payload.session_id)
                .ok_or_else(|| "session registry update failed".to_owned())?;
            if matches!(record.state.as_str(), "review" | "failed")
                && !matches!(
                    payload.hook_event_name.as_str(),
                    "SessionStart" | "UserPromptSubmit"
                )
            {
                return Ok(self.outcome(false, false, None));
            }
            if !accept_session_event(record, &payload, false) {
                return Ok(self.outcome(false, false, None));
            }
            record.previous_state = record.state.clone();
            record.state = expected_state.to_owned();
            record.activity = payload.activity;
            record.source = "codex-hook".to_owned();
            record.updated_at_ms = event_time_ms;
            record.expires_at_ms =
                official_pet::notification_expires_at_ms(expected_state, record.updated_at_ms);
            record.revision += 1;
            record.unread = expected_state == "review";
            record.route_eligibility = RouteEligibility::Eligible;
            record.live_event_observed_at_ms = Some(now_ms());
            let refresh_metadata = matches!(
                payload.hook_event_name.as_str(),
                "SessionStart" | "UserPromptSubmit" | "PermissionRequest" | "Stop"
            );
            if refresh_metadata {
                record.metadata_attempts = 0;
            }
            if refresh_metadata || is_fallback_session_title(&record.title) {
                metadata_session = Some(payload.session_id.clone());
            }
            self.sessions.revision += 1;
            (self.recompute_aggregate("codex-hook"), true)
        };
        Ok(self.outcome(state_changed, sessions_changed, metadata_session))
    }

    pub fn begin_metadata(
        &mut self,
        session_id: &str,
        max_attempts: u8,
    ) -> Option<MetadataRequest> {
        let record = self.sessions.records.get_mut(session_id)?;
        if record.metadata_attempts >= max_attempts {
            return None;
        }
        record.metadata_attempts += 1;
        Some(MetadataRequest {
            revision: record.revision,
        })
    }

    pub fn apply_metadata(
        &mut self,
        session_id: &str,
        requested_revision: u64,
        metadata: ThreadMetadata,
    ) -> CoreOutcome {
        let mut changed = false;
        let mut retry = false;
        let ThreadMetadata {
            thread_id,
            title,
            preview,
            status_type,
            waiting_on_approval,
            waiting_on_user_input,
            plan_implementation_pending,
            has_unread_turn,
        } = metadata;
        if let Some(record) = self.sessions.records.get_mut(session_id) {
            if record.route_eligibility == RouteEligibility::Eligible
                && thread_id.as_deref() == Some(session_id)
                && record.verified_thread_id.as_deref() != Some(session_id)
            {
                record.verified_thread_id = Some(session_id.to_owned());
                changed = true;
            }
            if let Some(title) = title {
                if record.title != title {
                    record.title = title;
                    changed = true;
                }
                if record.transient_title.take().is_some() {
                    changed = true;
                }
            } else if let Some(preview) = preview {
                if record.transient_title.as_deref() != Some(preview.as_str()) {
                    record.transient_title = Some(preview);
                    changed = true;
                }
            }

            let metadata_is_current =
                record.revision == requested_revision && record.ended_at_ms.is_none();
            if !metadata_is_current {
                retry = record.ended_at_ms.is_none();
            } else {
                let official_state = official_pet::session_status_signal(
                    status_type.as_deref(),
                    waiting_on_approval,
                    waiting_on_user_input,
                    plan_implementation_pending,
                    has_unread_turn,
                );
                if let Some(official_state) = official_state {
                    let may_replace_attention = !official_pet::is_attention_state(&record.state)
                        || state_priority(official_state) < state_priority(&record.state);
                    if may_replace_attention && record.state != official_state {
                        record.previous_state = record.state.clone();
                        record.state = official_state.to_owned();
                        record.activity =
                            official_pet::activity_for_state(official_state).to_owned();
                        record.source = "app-server".to_owned();
                        record.revision += 1;
                        record.updated_at_ms = now_ms();
                        record.expires_at_ms = official_pet::notification_expires_at_ms(
                            official_state,
                            record.updated_at_ms,
                        );
                        record.unread = official_state == "review";
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.sessions.revision += 1;
        }
        let state_changed = changed && self.recompute_aggregate("app-server");
        self.outcome(state_changed, changed, retry.then(|| session_id.to_owned()))
    }

    pub fn mark_opened(&mut self, session_id: &str) -> Result<CoreOutcome, String> {
        if !is_session_id(session_id) {
            return Err("invalid session id".to_owned());
        }
        let record = self
            .sessions
            .records
            .get_mut(session_id)
            .ok_or_else(|| "unknown session".to_owned())?;
        if record.route_eligibility != RouteEligibility::Eligible
            || record.verified_thread_id.as_deref() != Some(session_id)
        {
            return Err("session thread is not verified".to_owned());
        }
        record.unread = false;
        if record.state == "review" {
            record.previous_state = record.state.clone();
            record.state = "idle".to_owned();
            record.activity = "idle".to_owned();
            record.expires_at_ms = None;
            record.revision += 1;
            record.updated_at_ms = now_ms();
        }
        self.sessions.revision += 1;
        let state_changed = self.recompute_aggregate("session-open");
        Ok(self.outcome(state_changed, true, None))
    }

    pub fn is_verified_session_route(
        &self,
        session_id: &str,
        thread_id: &str,
        expected_revision: u64,
    ) -> bool {
        self.sessions.records.get(session_id).is_some_and(|record| {
            record.route_eligibility == RouteEligibility::Eligible
                && record.verified_thread_id.as_deref() == Some(thread_id)
                && session_id == thread_id
                && record.revision == expected_revision
        })
    }

    pub fn route_reconciliation_request(&self) -> Vec<(String, u64)> {
        let mut request = self
            .sessions
            .records
            .values()
            .map(|record| (record.id.clone(), record.revision))
            .collect::<Vec<_>>();
        request.sort_by(|left, right| left.0.cmp(&right.0));
        request
    }

    pub fn apply_active_thread_snapshot(
        &mut self,
        request: &[(String, u64)],
        active_thread_ids: &HashSet<String>,
        allow_live_grace: bool,
    ) -> CoreOutcome {
        let checked_at_ms = now_ms();
        let mut changed = false;
        for (session_id, expected_revision) in request {
            let Some(record) = self.sessions.records.get_mut(session_id) else {
                continue;
            };
            if record.revision != *expected_revision {
                continue;
            }
            let recently_observed_live = record.live_event_observed_at_ms.is_some_and(|observed| {
                checked_at_ms.saturating_sub(observed) < ACTIVE_ROUTE_LIVE_GRACE_MS
            });
            let next = if active_thread_ids.contains(session_id)
                || (allow_live_grace && recently_observed_live)
            {
                RouteEligibility::Eligible
            } else {
                RouteEligibility::Suppressed
            };
            if record.route_eligibility == next {
                continue;
            }
            record.route_eligibility = next;
            if next == RouteEligibility::Eligible {
                record.metadata_attempts = 0;
            } else {
                record.verified_thread_id = None;
            }
            record.revision = record.revision.saturating_add(1);
            changed = true;
        }
        if changed {
            self.sessions.revision = self.sessions.revision.saturating_add(1);
        }
        let state_changed = changed && self.recompute_aggregate("route-reconcile");
        self.outcome(state_changed, changed, None)
    }

    pub fn metadata_candidate_session_ids(&self, limit: usize) -> Vec<String> {
        let mut candidates = self
            .sessions
            .records
            .values()
            .filter(|record| {
                record.route_eligibility == RouteEligibility::Eligible
                    && (record.verified_thread_id.is_none()
                        || is_fallback_session_title(&record.title))
            })
            .map(|record| (record.updated_at_ms, record.id.clone()))
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        candidates
            .into_iter()
            .take(limit)
            .map(|(_, session_id)| session_id)
            .collect()
    }

    pub fn metadata_lookup_complete(&self, session_id: &str, max_attempts: u8) -> bool {
        self.sessions.records.get(session_id).is_none_or(|record| {
            record.route_eligibility != RouteEligibility::Eligible
                || record.verified_thread_id.is_some()
                || record.metadata_attempts >= max_attempts
        })
    }

    pub fn expire_notifications_if_revision(
        &mut self,
        expected_revision: u64,
        current_time_ms: u64,
    ) -> Option<CoreOutcome> {
        if self.sessions.revision != expected_revision {
            return None;
        }
        let mut changed = false;
        for record in self.sessions.records.values_mut() {
            if record
                .expires_at_ms
                .is_some_and(|expires_at_ms| current_time_ms >= expires_at_ms)
            {
                record.previous_state = record.state.clone();
                record.state = "idle".to_owned();
                record.activity = "idle".to_owned();
                record.source = "official:expiry".to_owned();
                record.updated_at_ms = current_time_ms;
                record.expires_at_ms = None;
                record.revision += 1;
                changed = true;
            }
        }
        if !changed {
            return None;
        }
        self.sessions.revision += 1;
        let state_changed = self.recompute_aggregate("official:expiry");
        Some(self.outcome(state_changed, true, None))
    }

    fn outcome(
        &self,
        state_changed: bool,
        sessions_changed: bool,
        metadata_session: Option<String>,
    ) -> CoreOutcome {
        CoreOutcome {
            state: self.state.clone(),
            sessions: self.sessions_snapshot(),
            state_changed,
            sessions_changed,
            metadata_session,
        }
    }

    fn recompute_aggregate(&mut self, source: &str) -> bool {
        let selected =
            select_session(&self.sessions).map(|record| (record.id.clone(), record.state.clone()));
        let selected_id = selected.as_ref().map(|(id, _)| id.clone());
        let next_state = selected
            .map(|(_, state)| state)
            .unwrap_or_else(|| "idle".to_owned());
        let selection_changed = self.sessions.selected_session_id != selected_id;
        self.sessions.selected_session_id = selected_id;
        let presentation_is_transient = matches!(
            self.state.source.as_str(),
            "official:first-awake" | "official:interaction"
        );
        if presentation_is_transient && !official_pet::is_attention_state(&next_state) {
            return false;
        }
        if !selection_changed && self.state.state == next_state {
            return false;
        }
        let previous = self.state.state.clone();
        self.state = StateSnapshot {
            state: next_state,
            previous_state: previous,
            revision: self.state.revision + 1,
            source: source.to_owned(),
            updated_at_ms: now_ms(),
        };
        true
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub fn is_valid_state(state: &str) -> bool {
    VALID_STATES.contains(&state)
}

pub fn sanitize_label(value: &str, max_chars: usize) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect()
}

pub fn sanitize_source(value: Option<String>) -> String {
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

pub fn is_session_id(value: &str) -> bool {
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
    official_pet::hook_state(event).filter(|expected| *expected == requested)
}

fn state_priority(state: &str) -> u8 {
    official_pet::state_priority(state)
}

fn fallback_session_title(session_id: &str) -> String {
    let _ = session_id;
    "当前任务".to_owned()
}

fn is_legacy_fallback_session_title(value: &str) -> bool {
    value == "Codex Session"
        || value.strip_prefix("Codex Session ").is_some_and(|suffix| {
            suffix.len() == 8 && suffix.chars().all(|ch| ch.is_ascii_hexdigit())
        })
}

pub fn is_fallback_session_title(value: &str) -> bool {
    value == "当前任务" || is_legacy_fallback_session_title(value)
}

pub fn sanitize_title(value: &str) -> Option<String> {
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
    let target_thread_id = record.verified_thread_id.clone();
    SessionActivity {
        id: record.id.clone(),
        target_thread_id: target_thread_id.clone(),
        state: record.state.clone(),
        previous_state: record.previous_state.clone(),
        revision: record.revision,
        title: record
            .transient_title
            .clone()
            .unwrap_or_else(|| record.title.clone()),
        activity: record.activity.clone(),
        source: record.source.clone(),
        updated_at_ms: record.updated_at_ms,
        priority: state_priority(&record.state),
        unread: record.unread,
        expires_at_ms: record.expires_at_ms,
        deep_link: target_thread_id.map(|thread_id| format!("codex://threads/{thread_id}")),
    }
}

fn sessions_snapshot(registry: &SessionRegistry) -> SessionsSnapshot {
    let mut sessions: Vec<_> = registry
        .records
        .values()
        .filter(|record| is_session_visible(record))
        .map(session_activity)
        .collect();
    sessions.sort_by(|left, right| {
        left.priority
            .cmp(&right.priority)
            .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
            .then_with(|| left.id.cmp(&right.id))
    });
    let selected_session_id = registry
        .selected_session_id
        .as_ref()
        .filter(|session_id| {
            registry
                .records
                .get(*session_id)
                .is_some_and(is_session_visible)
        })
        .cloned();
    SessionsSnapshot {
        revision: registry.revision,
        selected_session_id,
        session_count: sessions.len(),
        sessions,
    }
}

fn is_session_visible(record: &SessionRecord) -> bool {
    if record.route_eligibility != RouteEligibility::Eligible {
        return false;
    }
    let navigable_attention =
        record.verified_thread_id.is_some() && (record.unread || record.state == "review");
    navigable_attention || matches!(record.state.as_str(), "running" | "waiting" | "failed")
}

fn select_session(registry: &SessionRegistry) -> Option<&SessionRecord> {
    registry
        .records
        .values()
        .filter(|record| {
            record.route_eligibility == RouteEligibility::Eligible
                && (record.state != "review" || record.verified_thread_id.is_some())
        })
        .min_by(|left, right| {
            state_priority(&left.state)
                .cmp(&state_priority(&right.state))
                .then_with(|| right.updated_at_ms.cmp(&left.updated_at_ms))
                .then_with(|| left.id.cmp(&right.id))
        })
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
        .min_by_key(|record| (is_session_visible(record), record.updated_at_ms))
        .map(|record| record.id.clone())
    {
        registry.records.remove(&oldest_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_event(event_id: char, hook_event_name: &str, emitted_at_ms: u64) -> CodexEventRequest {
        let state = official_pet::hook_state(hook_event_name).unwrap_or("idle");
        CodexEventRequest {
            schema_version: 1,
            event_id: event_id.to_string().repeat(64),
            session_id: "019fcc02-569f-7790-92ff-209aaca4275c".to_owned(),
            turn_id: Some("turn-1".to_owned()),
            hook_event_name: hook_event_name.to_owned(),
            state: state.to_owned(),
            activity: official_pet::activity_for_state(state).to_owned(),
            emitted_at_ms,
        }
    }

    fn verify_session(core: &mut CoreState, session_id: &str) {
        let revision = core.sessions.records.get(session_id).unwrap().revision;
        core.apply_metadata(
            session_id,
            revision,
            ThreadMetadata {
                thread_id: Some(session_id.to_owned()),
                ..Default::default()
            },
        );
    }

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
    fn visible_projection_hides_idle_but_keeps_active_and_unread_sessions() {
        let mut core = CoreState::default();
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";

        let idle = core
            .apply_event(test_event('a', "SessionStart", 100))
            .unwrap();
        assert_eq!(idle.sessions.session_count, 0);
        assert!(idle.sessions.sessions.is_empty());
        assert_eq!(idle.sessions.selected_session_id, None);
        assert_eq!(core.sessions.records.len(), 1);

        let running = core
            .apply_event(test_event('b', "UserPromptSubmit", 101))
            .unwrap();
        assert_eq!(running.sessions.session_count, 1);
        assert_eq!(running.sessions.sessions[0].title, "当前任务");
        assert!(!running.sessions.sessions[0]
            .title
            .contains(&session_id[..8]));

        let review = core.apply_event(test_event('c', "Stop", 102)).unwrap();
        assert_eq!(review.sessions.session_count, 0);
        assert!(core.mark_opened(session_id).is_err());

        verify_session(&mut core, session_id);
        let verified_review = core.sessions_snapshot();
        assert_eq!(verified_review.sessions[0].state, "review");
        assert!(verified_review.sessions[0].unread);
        assert_eq!(
            verified_review.sessions[0].target_thread_id.as_deref(),
            Some(session_id)
        );
        assert_eq!(
            verified_review.sessions[0].deep_link.as_deref(),
            Some("codex://threads/019fcc02-569f-7790-92ff-209aaca4275c")
        );

        let opened = core.mark_opened(session_id).unwrap();
        assert_eq!(opened.sessions.session_count, 0);
        assert!(opened.sessions.sessions.is_empty());
        assert_eq!(core.sessions.records.len(), 1);
    }

    #[test]
    fn mismatched_metadata_cannot_make_a_session_navigable() {
        let mut core = CoreState::default();
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        let revision = core.sessions.records.get(session_id).unwrap().revision;

        let outcome = core.apply_metadata(
            session_id,
            revision,
            ThreadMetadata {
                thread_id: Some("019fcc02-569f-7790-92ff-209aaca4275d".to_owned()),
                ..Default::default()
            },
        );

        assert_eq!(outcome.sessions.session_count, 0);
        assert!(!core.is_verified_session_route(session_id, session_id, revision));
        assert!(core.mark_opened(session_id).is_err());
    }

    #[test]
    fn restored_legacy_id_title_is_migrated_to_neutral_fallback() {
        let mut core = CoreState::default();
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
        core.apply_event(test_event('a', "UserPromptSubmit", 100))
            .unwrap();
        core.sessions.records.get_mut(session_id).unwrap().title =
            "Codex Session 019fcc02".to_owned();

        core.prepare_restored().unwrap();

        assert_eq!(
            core.sessions.records.get(session_id).unwrap().title,
            "当前任务"
        );
        assert!(core.sessions_snapshot().sessions.is_empty());
    }

    #[test]
    fn restored_sessions_stay_hidden_until_the_active_thread_snapshot_confirms_them() {
        let mut core = CoreState::default();
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        verify_session(&mut core, session_id);
        assert_eq!(core.sessions_snapshot().session_count, 1);

        core.prepare_restored().unwrap();
        assert_eq!(core.state.state, "idle");
        assert!(core.sessions_snapshot().sessions.is_empty());
        let request = core.route_reconciliation_request();
        let active = HashSet::from([session_id.to_owned()]);
        core.apply_active_thread_snapshot(&request, &active, true);
        verify_session(&mut core, session_id);

        let sessions = core.sessions_snapshot();
        assert_eq!(sessions.session_count, 1);
        assert_eq!(sessions.sessions[0].state, "review");
        assert!(sessions.sessions[0].unread);
    }

    #[test]
    fn inactive_snapshot_suppresses_a_route_until_a_new_live_event_arrives() {
        let mut core = CoreState::default();
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        verify_session(&mut core, session_id);
        let revision = core.sessions_snapshot().sessions[0].revision;

        let request = core.route_reconciliation_request();
        core.apply_active_thread_snapshot(&request, &HashSet::new(), false);
        assert!(core.sessions_snapshot().sessions.is_empty());
        assert!(!core.is_verified_session_route(session_id, session_id, revision));

        let resumed = CodexEventRequest {
            event_id: "b".repeat(64),
            hook_event_name: "UserPromptSubmit".to_owned(),
            state: "running".to_owned(),
            activity: "working".to_owned(),
            emitted_at_ms: 101,
            ..test_event('b', "UserPromptSubmit", 101)
        };
        let outcome = core.apply_event(resumed).unwrap();
        assert_eq!(outcome.sessions.session_count, 1);
        assert_eq!(outcome.sessions.sessions[0].state, "running");
        assert!(outcome.sessions.sessions[0].target_thread_id.is_none());
    }

    #[test]
    fn metadata_preview_is_display_only_and_thread_name_wins() {
        let mut core = CoreState::default();
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
        core.apply_event(test_event('a', "UserPromptSubmit", 100))
            .unwrap();
        let revision = core.sessions_snapshot().sessions[0].revision;
        let private_preview = "PRIVATE_PREVIEW_MARKER";

        let preview = core.apply_metadata(
            session_id,
            revision,
            ThreadMetadata {
                preview: Some(private_preview.to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(preview.sessions.sessions[0].title, private_preview);
        let persisted = serde_json::to_string(&core).unwrap();
        assert!(!persisted.contains(private_preview));
        assert!(persisted.contains("当前任务"));

        let named = core.apply_metadata(
            session_id,
            revision,
            ThreadMetadata {
                title: Some("可识别任务名称".to_owned()),
                preview: Some(private_preview.to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(named.sessions.sessions[0].title, "可识别任务名称");
        let persisted = serde_json::to_string(&core).unwrap();
        assert!(persisted.contains("可识别任务名称"));
        assert!(!persisted.contains(private_preview));
    }

    #[test]
    fn stale_metadata_can_enrich_title_without_changing_session_state() {
        let mut core = CoreState::default();
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
        core.apply_event(test_event('a', "UserPromptSubmit", 100))
            .unwrap();
        let requested_revision = core.sessions_snapshot().sessions[0].revision;
        core.apply_event(test_event('b', "PostToolUse", 101))
            .unwrap();

        let outcome = core.apply_metadata(
            session_id,
            requested_revision,
            ThreadMetadata {
                title: Some("稳定可识别标题".to_owned()),
                status_type: Some("idle".to_owned()),
                ..Default::default()
            },
        );

        assert_eq!(outcome.sessions.sessions[0].title, "稳定可识别标题");
        assert_eq!(outcome.sessions.sessions[0].state, "running");
        assert_eq!(outcome.metadata_session.as_deref(), Some(session_id));
    }

    #[test]
    fn fresh_running_hook_is_not_downgraded_by_neutral_metadata() {
        for status in ["notLoaded", "idle", "closed", "queued"] {
            let mut core = CoreState::default();
            let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
            core.apply_event(test_event('a', "UserPromptSubmit", 100))
                .unwrap();
            let revision = core.sessions_snapshot().sessions[0].revision;

            let outcome = core.apply_metadata(
                session_id,
                revision,
                ThreadMetadata {
                    status_type: Some(status.to_owned()),
                    ..Default::default()
                },
            );

            assert_eq!(outcome.sessions.sessions[0].state, "running", "{status}");
            assert_eq!(
                outcome.sessions.sessions[0].source, "codex-hook",
                "{status}"
            );
            assert!(!outcome.sessions.sessions[0].unread, "{status}");
            assert!(!outcome.state_changed, "{status}");
        }
    }

    #[test]
    fn session_end_blocks_late_events_but_allows_resume() {
        let mut core = CoreState::default();
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        verify_session(&mut core, "019fcc02-569f-7790-92ff-209aaca4275c");
        core.apply_event(test_event('b', "SessionEnd", 200))
            .unwrap();
        let late = core
            .apply_event(test_event('c', "SessionStart", 150))
            .unwrap();
        assert_eq!(late.state.state, "review");
        let resumed = core
            .apply_event(test_event('d', "SessionStart", 201))
            .unwrap();
        assert_eq!(resumed.state.state, "idle");
    }

    #[test]
    fn metadata_cannot_downgrade_terminal_review() {
        let mut core = CoreState::default();
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        let outcome = core.apply_metadata(
            "019fcc02-569f-7790-92ff-209aaca4275c",
            1,
            ThreadMetadata {
                thread_id: Some("019fcc02-569f-7790-92ff-209aaca4275c".to_owned()),
                title: Some("Verified title".to_owned()),
                status_type: Some("idle".to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(outcome.state.state, "review");
        assert_eq!(outcome.sessions.sessions[0].title, "Verified title");
    }

    #[test]
    fn current_metadata_can_upgrade_review_to_a_higher_priority_failure() {
        let mut core = CoreState::default();
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        let revision = core
            .sessions
            .records
            .get("019fcc02-569f-7790-92ff-209aaca4275c")
            .unwrap()
            .revision;
        let outcome = core.apply_metadata(
            "019fcc02-569f-7790-92ff-209aaca4275c",
            revision,
            ThreadMetadata {
                thread_id: Some("019fcc02-569f-7790-92ff-209aaca4275c".to_owned()),
                status_type: Some("systemError".to_owned()),
                ..Default::default()
            },
        );
        assert_eq!(outcome.state.state, "failed");
    }

    #[test]
    fn delayed_tool_events_cannot_downgrade_terminal_states() {
        let mut review = CoreState::default();
        review.apply_event(test_event('a', "Stop", 100)).unwrap();
        verify_session(&mut review, "019fcc02-569f-7790-92ff-209aaca4275c");
        let delayed_review = review
            .apply_event(test_event('b', "PostToolUse", 101))
            .unwrap();
        assert_eq!(delayed_review.state.state, "review");

        let mut failed = CoreState::default();
        let running = test_event('c', "UserPromptSubmit", 200);
        failed.apply_event(running).unwrap();
        let revision = failed.sessions_snapshot().sessions[0].revision;
        failed.apply_metadata(
            "019fcc02-569f-7790-92ff-209aaca4275c",
            revision,
            ThreadMetadata {
                title: None,
                status_type: Some("systemError".to_owned()),
                waiting_on_approval: false,
                ..Default::default()
            },
        );
        let delayed_failed = failed
            .apply_event(test_event('d', "PostToolUse", 201))
            .unwrap();
        assert_eq!(delayed_failed.state.state, "failed");
        let ended_failed = failed
            .apply_event(test_event('e', "SessionEnd", 202))
            .unwrap();
        assert_eq!(ended_failed.state.state, "failed");
    }

    #[test]
    fn a_new_prompt_explicitly_resumes_a_terminal_session() {
        let mut core = CoreState::default();
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        let mut resumed = test_event('b', "UserPromptSubmit", 101);
        resumed.state = "running".to_owned();
        resumed.activity = "working".to_owned();
        assert_eq!(core.apply_event(resumed).unwrap().state.state, "running");
    }

    #[test]
    fn ignored_terminal_activity_does_not_block_an_earlier_resume_event() {
        let mut core = CoreState::default();
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        core.apply_event(test_event('b', "PostToolUse", 200))
            .unwrap();
        let resumed = core
            .apply_event(test_event('c', "UserPromptSubmit", 150))
            .unwrap();
        assert_eq!(resumed.state.state, "running");
    }

    #[test]
    fn session_activity_does_not_interrupt_first_awake_presentation() {
        let mut core = CoreState::default();
        let (waving, _) = core.transition("waving", "official:first-awake").unwrap();
        let outcome = core
            .apply_event(test_event('a', "UserPromptSubmit", 100))
            .unwrap();
        assert_eq!(outcome.state.state, "waving");
        assert_eq!(outcome.state.revision, waving.revision);
        assert_eq!(core.selected_session_state(), "running");
    }

    #[test]
    fn attention_state_interrupts_first_awake_presentation() {
        let mut core = CoreState::default();
        core.transition("waving", "official:first-awake").unwrap();
        let outcome = core
            .apply_event(test_event('a', "PermissionRequest", 100))
            .unwrap();
        assert_eq!(outcome.state.state, "waiting");
    }

    #[test]
    fn a_new_prompt_after_session_end_clears_the_end_guard() {
        let mut core = CoreState::default();
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        core.apply_event(test_event('b', "SessionEnd", 101))
            .unwrap();
        let mut resumed = test_event('c', "UserPromptSubmit", 102);
        resumed.state = "running".to_owned();
        resumed.activity = "working".to_owned();
        core.apply_event(resumed).unwrap();
        let revision = core.sessions_snapshot().sessions[0].revision;
        let outcome = core.apply_metadata(
            "019fcc02-569f-7790-92ff-209aaca4275c",
            revision,
            ThreadMetadata {
                title: None,
                status_type: None,
                waiting_on_approval: true,
                ..Default::default()
            },
        );
        assert_eq!(outcome.state.state, "waiting");
    }

    #[test]
    fn selection_uses_official_priority() {
        let mut core = CoreState::default();
        let mut running = test_event('a', "UserPromptSubmit", 100);
        running.state = "running".to_owned();
        running.activity = "working".to_owned();
        core.apply_event(running).unwrap();
        let mut waiting = test_event('b', "PermissionRequest", 101);
        waiting.session_id = "019fcc02-569f-7790-92ff-209aaca4275d".to_owned();
        waiting.state = "waiting".to_owned();
        waiting.activity = "waiting".to_owned();
        assert_eq!(core.apply_event(waiting).unwrap().state.state, "waiting");
    }

    #[test]
    fn subagent_hooks_use_running_instead_of_fake_drag_direction() {
        let mut core = CoreState::default();
        let mut event = test_event('a', "SubagentStart", 100);
        event.state = "running".to_owned();
        event.activity = "subagent".to_owned();
        assert_eq!(core.apply_event(event).unwrap().state.state, "running");

        let mut invalid = test_event('b', "SubagentStart", 101);
        invalid.state = "running-left".to_owned();
        invalid.activity = "subagent".to_owned();
        assert!(core.apply_event(invalid).is_err());
    }

    #[test]
    fn attention_notifications_expire_without_marking_review_read() {
        let mut core = CoreState::default();
        core.apply_event(test_event('a', "Stop", 100)).unwrap();
        verify_session(&mut core, "019fcc02-569f-7790-92ff-209aaca4275c");
        let (revision, expires_at_ms) = core.session_expiry_schedule();
        let expires_at_ms = expires_at_ms.expect("review expiry");
        assert!(core
            .expire_notifications_if_revision(revision, expires_at_ms - 1)
            .is_none());

        let expired = core
            .expire_notifications_if_revision(revision, expires_at_ms)
            .expect("expired review notification");
        assert_eq!(expired.state.state, "idle");
        assert_eq!(expired.sessions.sessions[0].state, "idle");
        assert!(expired.sessions.sessions[0].unread);
        assert_eq!(expired.sessions.sessions[0].expires_at_ms, None);
    }
}
