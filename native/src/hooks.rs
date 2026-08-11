use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
        Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
        System::Threading::{CreateMutexW, WaitForSingleObject},
    },
};

use crate::{core::now_ms, hook_bridge::owner_id};

pub const HOOK_EVENTS: [&str; 11] = [
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Stop",
];

const MATCHER_EVENTS: [&str; 7] = [
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
];

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookInstallResult {
    pub hooks_path: String,
    pub installed_count: usize,
    pub removed_count: usize,
    pub preserved_count: usize,
    pub backup_path: Option<String>,
    pub changed: bool,
    pub trust_required: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct HookMutationStats {
    removed: usize,
    preserved: usize,
}

struct HookMutationLock(HANDLE);

impl HookMutationLock {
    fn acquire() -> Result<Self, String> {
        let handle = unsafe { CreateMutexW(None, false, w!("Local\\CodexPetHooksMutation-v1")) }
            .map_err(|_| "unable to create Hook configuration lock".to_owned())?;
        if unsafe { WaitForSingleObject(handle, 10_000) } != WAIT_OBJECT_0 {
            let _ = unsafe { CloseHandle(handle) };
            return Err("timed out waiting for Hook configuration lock".to_owned());
        }
        Ok(Self(handle))
    }
}

impl Drop for HookMutationLock {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

pub fn install_current() -> Result<HookInstallResult, String> {
    mutate_current(true)
}

pub fn remove_current() -> Result<HookInstallResult, String> {
    mutate_current(false)
}

fn mutate_current(install: bool) -> Result<HookInstallResult, String> {
    let _lock = HookMutationLock::acquire()?;
    let executable =
        std::env::current_exe().map_err(|_| "unable to locate Codex Pet".to_owned())?;
    let path = hooks_path()?;
    mutate_file(&path, &executable, install)
}

fn hooks_path() -> Result<PathBuf, String> {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .map(|root| root.join(".codex").join("hooks.json"))
        .ok_or_else(|| "USERPROFILE is unavailable".to_owned())
}

fn mutate_file(path: &Path, executable: &Path, install: bool) -> Result<HookInstallResult, String> {
    if !install && !path.exists() {
        return Ok(HookInstallResult {
            hooks_path: path.display().to_string(),
            installed_count: 0,
            removed_count: 0,
            preserved_count: 0,
            backup_path: None,
            changed: false,
            trust_required: false,
        });
    }
    let original = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => b"{\"hooks\":{}}".to_vec(),
        Err(_) => return Err("unable to read Codex hooks configuration".to_owned()),
    };
    let mut value: Value = serde_json::from_slice(&original)
        .map_err(|_| "Codex hooks configuration is invalid JSON".to_owned())?;
    let before = value.clone();
    let command = hook_command(executable, false);
    let command_windows = hook_command(executable, true);
    let stats = mutate_document(&mut value, &command, &command_windows, install)?;
    let changed = value != before;
    let backup_path = if changed {
        let bytes = serde_json::to_vec_pretty(&value)
            .map_err(|_| "unable to serialize Codex hooks configuration".to_owned())?;
        write_with_backup(path, &original, &bytes)?
    } else {
        None
    };
    Ok(HookInstallResult {
        hooks_path: path.display().to_string(),
        installed_count: if install { HOOK_EVENTS.len() } else { 0 },
        removed_count: stats.removed,
        preserved_count: stats.preserved,
        backup_path: backup_path.map(|path| path.display().to_string()),
        changed,
        trust_required: install,
    })
}

fn hook_command(executable: &Path, windows: bool) -> String {
    let mut path = executable.display().to_string();
    if !windows {
        path = path.replace('\\', "/");
    }
    format!("\"{path}\" --hook --owner {}", owner_id())
}

fn mutate_document(
    value: &mut Value,
    command: &str,
    command_windows: &str,
    install: bool,
) -> Result<HookMutationStats, String> {
    let mut candidate = value.clone();
    let stats = mutate_document_inner(&mut candidate, command, command_windows, install)?;
    *value = candidate;
    Ok(stats)
}

fn mutate_document_inner(
    value: &mut Value,
    command: &str,
    command_windows: &str,
    install: bool,
) -> Result<HookMutationStats, String> {
    let root = value
        .as_object_mut()
        .ok_or_else(|| "Codex hooks configuration root must be an object".to_owned())?;
    if !install && !root.contains_key("hooks") {
        return Ok(HookMutationStats::default());
    }
    if install && !root.contains_key("hooks") {
        root.insert("hooks".to_owned(), Value::Object(Map::new()));
    }
    let events = root
        .get_mut("hooks")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| "Codex hooks field must be an object".to_owned())?;
    let mut stats = HookMutationStats::default();
    for event in HOOK_EVENTS {
        if install {
            let groups = events
                .entry(event.to_owned())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or_else(|| format!("Codex hook event {event} must be an array"))?;
            remove_owned_hooks(groups, command, command_windows, &mut stats)?;
            groups.push(owned_group(event, command, command_windows));
        } else if let Some(groups) = events.get_mut(event) {
            let groups = groups
                .as_array_mut()
                .ok_or_else(|| format!("Codex hook event {event} must be an array"))?;
            remove_owned_hooks(groups, command, command_windows, &mut stats)?;
            if groups.is_empty() {
                events.remove(event);
            }
        }
    }
    Ok(stats)
}

fn remove_owned_hooks(
    groups: &mut Vec<Value>,
    command: &str,
    command_windows: &str,
    stats: &mut HookMutationStats,
) -> Result<(), String> {
    let mut next_groups = Vec::with_capacity(groups.len());
    for mut group in groups.drain(..) {
        let object = group
            .as_object_mut()
            .ok_or_else(|| "Codex hook group must be an object".to_owned())?;
        let hooks = object
            .get_mut("hooks")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| "Codex hook group hooks must be an array".to_owned())?;
        let mut retained = Vec::with_capacity(hooks.len());
        for hook in hooks.drain(..) {
            if is_owned_hook(&hook, command, command_windows) || is_legacy_hook(&hook) {
                stats.removed += 1;
            } else {
                stats.preserved += 1;
                retained.push(hook);
            }
        }
        *hooks = retained;
        if !hooks.is_empty() || object.keys().any(|key| key != "hooks" && key != "matcher") {
            next_groups.push(group);
        }
    }
    *groups = next_groups;
    Ok(())
}

fn is_owned_hook(hook: &Value, command: &str, command_windows: &str) -> bool {
    let Some(object) = hook.as_object() else {
        return false;
    };
    let exact_current = object.get("command").and_then(Value::as_str) == Some(command)
        && object.get("commandWindows").and_then(Value::as_str) == Some(command_windows);
    let marked_current = object
        .get("command")
        .and_then(Value::as_str)
        .is_some_and(is_marked_native_command)
        && object
            .get("commandWindows")
            .and_then(Value::as_str)
            .is_some_and(is_marked_native_command);
    object.get("type").and_then(Value::as_str) == Some("command")
        && object.get("timeout").and_then(Value::as_u64) == Some(1)
        && (exact_current || marked_current)
}

fn is_marked_native_command(command: &str) -> bool {
    let suffix = format!(" --hook --owner {}", owner_id());
    let Some(quoted_path) = command.strip_suffix(&suffix) else {
        return false;
    };
    let Some(path) = quoted_path
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        return false;
    };
    if path.contains('"') {
        return false;
    }
    let leaf = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    matches!(leaf.as_str(), "codexpet.exe" | "codex-pet-native.exe")
}

fn is_legacy_hook(hook: &Value) -> bool {
    let Some(object) = hook.as_object() else {
        return false;
    };
    if object.get("type").and_then(Value::as_str) != Some("command")
        || object.get("timeout").and_then(Value::as_u64) != Some(1)
    {
        return false;
    }
    let Some(posix) = object.get("command").and_then(Value::as_str) else {
        return false;
    };
    let Some(windows) = object.get("commandWindows").and_then(Value::as_str) else {
        return false;
    };
    let Some(posix_path) = legacy_script_path(posix, "node") else {
        return false;
    };
    let Some(windows_path) = legacy_script_path(windows, "node.exe") else {
        return false;
    };
    let Ok(posix_path) = fs::canonicalize(posix_path) else {
        return false;
    };
    let Ok(windows_path) = fs::canonicalize(windows_path) else {
        return false;
    };
    if posix_path != windows_path {
        return false;
    }
    let Some(root) = windows_path.parent().and_then(Path::parent) else {
        return false;
    };
    fs::read(root.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| value.get("name").and_then(Value::as_str).map(str::to_owned))
        .as_deref()
        == Some("codex-pet-overlay")
}

fn legacy_script_path(command: &str, executable: &str) -> Option<PathBuf> {
    let prefix = format!("{executable} \"");
    let path = command
        .strip_prefix(&prefix)
        .and_then(|value| value.strip_suffix('"'))?;
    if path.contains('"') {
        return None;
    }
    let normalized = path.replace('/', "\\");
    normalized
        .to_ascii_lowercase()
        .ends_with("\\scripts\\codex-pet-hook.mjs")
        .then(|| PathBuf::from(normalized))
}

fn owned_group(event: &str, command: &str, command_windows: &str) -> Value {
    let mut group = Map::new();
    if MATCHER_EVENTS.contains(&event) {
        group.insert("matcher".to_owned(), Value::String(String::new()));
    }
    group.insert(
        "hooks".to_owned(),
        Value::Array(vec![json!({
            "type": "command",
            "command": command,
            "commandWindows": command_windows,
            "timeout": 1
        })]),
    );
    Value::Object(group)
}

fn write_with_backup(
    path: &Path,
    original: &[u8],
    bytes: &[u8],
) -> Result<Option<PathBuf>, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Codex hooks path is invalid".to_owned())?;
    fs::create_dir_all(parent).map_err(|_| "unable to create Codex config directory".to_owned())?;
    let suffix = format!("{}-{}", now_ms(), std::process::id());
    let temporary = parent.join(format!(".hooks-{suffix}.tmp"));
    let backup = parent.join(format!("hooks.json.bak.{suffix}"));
    let had_original = path.exists();
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| "unable to create Codex hooks temporary file".to_owned())?;
        file.write_all(bytes)
            .map_err(|_| "unable to write Codex hooks temporary file".to_owned())?;
        file.sync_all()
            .map_err(|_| "unable to flush Codex hooks temporary file".to_owned())?;
        drop(file);
        let _: Value = serde_json::from_slice(
            &fs::read(&temporary)
                .map_err(|_| "unable to read back Codex hooks temporary file".to_owned())?,
        )
        .map_err(|_| "Codex hooks temporary file failed validation".to_owned())?;
        if had_original {
            let current = fs::read(path)
                .map_err(|_| "Codex hooks configuration changed during update".to_owned())?;
            if current != original {
                return Err("Codex hooks configuration changed during update; retry".to_owned());
            }
            fs::write(&backup, original)
                .map_err(|_| "unable to back up Codex hooks configuration".to_owned())?;
        }
        let temporary_wide = wide_path(&temporary);
        let path_wide = wide_path(path);
        unsafe {
            MoveFileExW(
                PCWSTR(temporary_wide.as_ptr()),
                PCWSTR(path_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|_| "unable to commit Codex hooks configuration".to_owned())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.map(|_| had_original.then_some(backup))
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands() -> (String, String) {
        let path = Path::new(r"C:\Program Files\Codex Pet\CodexPet.exe");
        (hook_command(path, false), hook_command(path, true))
    }

    #[test]
    fn install_is_idempotent_and_preserves_unknown_hooks() {
        let (command, command_windows) = commands();
        let user_hook = json!({"type":"command","command":"user","custom":true});
        let mut value = json!({
            "customRoot": 7,
            "hooks": {
                "Stop": [{"matcher":"user","customGroup":true,"hooks":[user_hook.clone()]}],
                "CustomEvent": [{"hooks":[{"type":"command","command":"custom"}]}]
            }
        });
        mutate_document(&mut value, &command, &command_windows, true).unwrap();
        let once = value.clone();
        mutate_document(&mut value, &command, &command_windows, true).unwrap();
        assert_eq!(value, once);
        assert_eq!(value["customRoot"], 7);
        assert_eq!(value["hooks"]["Stop"][0]["hooks"][0], user_hook);
        assert!(value["hooks"]["CustomEvent"].is_array());
        for event in HOOK_EVENTS {
            let groups = value["hooks"][event].as_array().unwrap();
            let owned = groups
                .iter()
                .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
                .filter(|hook| is_owned_hook(hook, &command, &command_windows))
                .count();
            assert_eq!(owned, 1, "{event}");
        }
    }

    #[test]
    fn uninstall_removes_only_owned_nested_hooks() {
        let (command, command_windows) = commands();
        let user_hook = json!({"type":"command","command":"user"});
        let mut value = json!({"hooks":{"Stop":[{
            "matcher":"keep",
            "hooks":[owned_group("Stop", &command, &command_windows)["hooks"][0].clone(), user_hook.clone()]
        }]}});
        mutate_document(&mut value, &command, &command_windows, false).unwrap();
        assert_eq!(value["hooks"]["Stop"][0]["matcher"], "keep");
        assert_eq!(value["hooks"]["Stop"][0]["hooks"], json!([user_hook]));
    }

    #[test]
    fn unproven_legacy_node_hooks_are_preserved() {
        let (command, command_windows) = commands();
        let legacy = json!({
            "type":"command",
            "command":"node \"G:/Old/CodexPetTest/scripts/codex-pet-hook.mjs\"",
            "commandWindows":"node.exe \"G:\\Old\\CodexPetTest\\scripts\\codex-pet-hook.mjs\"",
            "timeout":1
        });
        let similar = json!({
            "type":"command",
            "command":"node \"G:/Else/codex-pet-hook.mjs\" --extra",
            "commandWindows":"node.exe \"G:\\Else\\codex-pet-hook.mjs\" --extra",
            "timeout":1
        });
        let mut value = json!({"hooks":{"Stop":[{"hooks":[legacy,similar.clone()]}]}});
        mutate_document(&mut value, &command, &command_windows, true).unwrap();
        let retained = value["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
        assert_eq!(retained.len(), 2);
        assert_eq!(retained[1], similar);
    }

    #[test]
    fn legacy_hook_migrates_only_from_a_verified_product_root() {
        let (command, command_windows) = commands();
        let root = std::env::temp_dir().join(format!(
            "codex-pet-hook-owner-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let scripts = root.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(
            root.join("package.json"),
            br#"{"name":"codex-pet-overlay"}"#,
        )
        .unwrap();
        let legacy_path = scripts.join("codex-pet-hook.mjs");
        fs::write(&legacy_path, b"// verified legacy entry").unwrap();
        let windows_path = legacy_path.display().to_string();
        let posix_path = windows_path.replace('\\', "/");
        let legacy = json!({
            "type":"command",
            "command":format!("node \"{posix_path}\""),
            "commandWindows":format!("node.exe \"{windows_path}\""),
            "timeout":1
        });
        let mut value = json!({"hooks":{"Stop":[{"hooks":[legacy]}]}});
        mutate_document(&mut value, &command, &command_windows, true).unwrap();
        let hooks = value["hooks"]["Stop"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
            .collect::<Vec<_>>();
        assert_eq!(hooks.len(), 1);
        assert!(is_owned_hook(hooks[0], &command, &command_windows));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_known_event_fails_without_mutating_the_document() {
        let (command, command_windows) = commands();
        let mut value = json!({"hooks":{"Stop":"invalid"}});
        let before = value.clone();
        assert!(mutate_document(&mut value, &command, &command_windows, true).is_err());
        assert_eq!(value, before);
    }

    #[test]
    fn moving_the_native_executable_replaces_the_owned_command_without_duplication() {
        let old = Path::new(r"C:\Old\CodexPet.exe");
        let new = Path::new(r"C:\Users\Me\AppData\Local\Programs\CodexPet\CodexPet.exe");
        let mut value = json!({"hooks":{}});
        mutate_document(
            &mut value,
            &hook_command(old, false),
            &hook_command(old, true),
            true,
        )
        .unwrap();
        let new_command = hook_command(new, false);
        let new_windows = hook_command(new, true);
        mutate_document(&mut value, &new_command, &new_windows, true).unwrap();
        for event in HOOK_EVENTS {
            let hooks = value["hooks"][event]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|group| group["hooks"].as_array().into_iter().flatten())
                .collect::<Vec<_>>();
            assert_eq!(hooks.len(), 1, "{event}");
            assert_eq!(hooks[0]["command"], new_command);
        }
    }

    #[test]
    fn removing_from_a_document_without_hooks_is_a_noop() {
        let (command, command_windows) = commands();
        let mut value = json!({"custom":true});
        let before = value.clone();
        mutate_document(&mut value, &command, &command_windows, false).unwrap();
        assert_eq!(value, before);
    }
}
