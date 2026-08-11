pub const DRIVER_VERSION: &str = "codex-desktop-26.730.8199";
pub const FIRST_AWAKE_DURATION_MS: u64 = 8_000;
pub const FAILED_NOTIFICATION_DURATION_MS: u64 = 60 * 60 * 1_000;
pub const WAITING_NOTIFICATION_DURATION_MS: u64 = 24 * 60 * 60 * 1_000;
pub const REVIEW_NOTIFICATION_DURATION_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
pub const DRAG_THRESHOLD_PX: i32 = 4;

pub fn hook_state(event: &str) -> Option<&'static str> {
    match event {
        "SessionStart" | "SessionEnd" => Some("idle"),
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" | "PreCompact" | "PostCompact"
        | "SubagentStart" | "SubagentStop" => Some("running"),
        "PermissionRequest" => Some("waiting"),
        "Stop" => Some("review"),
        _ => None,
    }
}

pub fn session_status_state(
    status_type: Option<&str>,
    waiting_on_approval: bool,
    waiting_on_user_input: bool,
    plan_implementation_pending: bool,
    has_unread_turn: bool,
) -> &'static str {
    session_status_signal(
        status_type,
        waiting_on_approval,
        waiting_on_user_input,
        plan_implementation_pending,
        has_unread_turn,
    )
    .unwrap_or("idle")
}

pub fn session_status_signal(
    status_type: Option<&str>,
    waiting_on_approval: bool,
    waiting_on_user_input: bool,
    plan_implementation_pending: bool,
    has_unread_turn: bool,
) -> Option<&'static str> {
    if waiting_on_approval || waiting_on_user_input || plan_implementation_pending {
        Some("waiting")
    } else if matches!(status_type, Some("systemError" | "failed")) {
        Some("failed")
    } else if matches!(status_type, Some("active" | "inProgress" | "running")) {
        Some("running")
    } else if has_unread_turn {
        Some("review")
    } else {
        None
    }
}

pub fn notification_expires_at_ms(state: &str, updated_at_ms: u64) -> Option<u64> {
    let duration = match state {
        "failed" => FAILED_NOTIFICATION_DURATION_MS,
        "waiting" => WAITING_NOTIFICATION_DURATION_MS,
        "review" => REVIEW_NOTIFICATION_DURATION_MS,
        _ => return None,
    };
    Some(updated_at_ms.saturating_add(duration))
}

pub fn state_priority(state: &str) -> u8 {
    match state {
        "waiting" => 0,
        "failed" => 1,
        "review" => 2,
        "running" | "running-left" | "running-right" | "waving" | "jumping" => 3,
        _ => 4,
    }
}

pub fn activity_for_state(state: &str) -> &'static str {
    match state {
        "waiting" => "waiting",
        "failed" => "recovering",
        "review" => "ready",
        "running" | "running-left" | "running-right" => "working",
        _ => "idle",
    }
}

pub fn is_attention_state(state: &str) -> bool {
    matches!(state, "waiting" | "failed" | "review")
}

pub fn drag_state(delta_x: i32) -> Option<&'static str> {
    if delta_x >= DRAG_THRESHOLD_PX {
        Some("running-right")
    } else if delta_x <= -DRAG_THRESHOLD_PX {
        Some("running-left")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_states_match_the_official_base_vocabulary() {
        assert_eq!(hook_state("SubagentStart"), Some("running"));
        assert_eq!(hook_state("PermissionRequest"), Some("waiting"));
        assert_eq!(hook_state("Stop"), Some("review"));
        assert_eq!(hook_state("unknown"), None);
    }

    #[test]
    fn app_server_status_uses_official_precedence() {
        assert_eq!(
            session_status_state(Some("systemError"), true, false, false, true),
            "waiting"
        );
        assert_eq!(
            session_status_state(Some("systemError"), false, false, false, true),
            "failed"
        );
        assert_eq!(
            session_status_state(Some("active"), false, false, false, true),
            "running"
        );
        assert_eq!(
            session_status_state(None, false, false, false, true),
            "review"
        );
    }

    #[test]
    fn neutral_app_server_status_has_no_state_signal() {
        for status in [
            Some("notLoaded"),
            Some("idle"),
            Some("closed"),
            Some("queued"),
            None,
        ] {
            assert_eq!(
                session_status_signal(status, false, false, false, false),
                None
            );
        }
        assert_eq!(
            session_status_signal(Some("notLoaded"), false, false, false, true),
            Some("review")
        );
    }

    #[test]
    fn attention_expirations_match_codex_desktop_26_730_8199() {
        assert_eq!(
            notification_expires_at_ms("failed", 10),
            Some(10 + FAILED_NOTIFICATION_DURATION_MS)
        );
        assert_eq!(
            notification_expires_at_ms("waiting", 10),
            Some(10 + WAITING_NOTIFICATION_DURATION_MS)
        );
        assert_eq!(
            notification_expires_at_ms("review", 10),
            Some(10 + REVIEW_NOTIFICATION_DURATION_MS)
        );
        assert_eq!(notification_expires_at_ms("running", 10), None);
    }

    #[test]
    fn drag_direction_ignores_small_pointer_jitter() {
        assert_eq!(drag_state(3), None);
        assert_eq!(drag_state(-3), None);
        assert_eq!(drag_state(4), Some("running-right"));
        assert_eq!(drag_state(-4), Some("running-left"));
    }
}
