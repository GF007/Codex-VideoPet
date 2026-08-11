use crate::core::{CodexEventRequest, CoreState};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows::{
    core::PCWSTR,
    Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
};

const STORAGE_SCHEMA_VERSION: u8 = 1;
const SNAPSHOT_FILE: &str = "session-state.json";
const SNAPSHOT_BACKUP_FILE: &str = "session-state.bak.json";
const EVENTS_DIRECTORY: &str = "events";
const MAX_EVENT_FILE_BYTES: u64 = 16 * 1024;

#[derive(Clone, Debug)]
pub struct SessionStore {
    root: Option<PathBuf>,
}

pub struct RecoveryResult {
    pub core: CoreState,
    pub recovered_events: usize,
    pub error: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredCore {
    schema_version: u8,
    core: CoreState,
}

impl SessionStore {
    pub fn local() -> Self {
        let root = std::env::var_os("LOCALAPPDATA").map(|value| {
            PathBuf::from(value)
                .join("com.local.codex-pet-overlay")
                .join("session-data")
        });
        Self { root }
    }

    #[cfg(test)]
    pub fn at(root: PathBuf) -> Self {
        Self { root: Some(root) }
    }

    #[cfg(test)]
    pub fn disabled() -> Self {
        Self { root: None }
    }

    pub fn recover(&self) -> RecoveryResult {
        let Some(root) = self.root.as_ref() else {
            return RecoveryResult {
                core: CoreState::default(),
                recovered_events: 0,
                error: None,
            };
        };

        let (mut core, had_snapshot, mut errors) = match load_snapshot(root) {
            Ok(Some(core)) => (core, true, Vec::new()),
            Ok(None) => (CoreState::default(), false, Vec::new()),
            Err(error) => (CoreState::default(), false, vec![error]),
        };
        if had_snapshot {
            if let Err(error) = core.prepare_restored() {
                errors.push(error);
                core = CoreState::default();
            }
        }

        let (pending, pending_errors) = load_pending_events(root);
        errors.extend(pending_errors);
        let mut applied_paths = Vec::new();
        for (path, event) in pending {
            match core.apply_event(event) {
                Ok(_) => applied_paths.push(path),
                Err(error) => errors.push(format!("pending event rejected: {error}")),
            }
        }
        if !applied_paths.is_empty() {
            if let Err(error) = core.prepare_restored() {
                errors.push(error);
            }
        }

        if had_snapshot || !applied_paths.is_empty() {
            match self.persist(&core) {
                Ok(()) => {
                    for path in &applied_paths {
                        let _ = fs::remove_file(path);
                    }
                }
                Err(error) => errors.push(error),
            }
        }

        RecoveryResult {
            core,
            recovered_events: applied_paths.len(),
            error: (!errors.is_empty()).then(|| errors.join("; ")),
        }
    }

    pub fn persist(&self, core: &CoreState) -> Result<(), String> {
        let Some(root) = self.root.as_ref() else {
            return Ok(());
        };
        fs::create_dir_all(root).map_err(|_| "unable to create session storage".to_owned())?;
        let bytes = serde_json::to_vec(&StoredCore {
            schema_version: STORAGE_SCHEMA_VERSION,
            core: core.clone(),
        })
        .map_err(|_| "unable to serialize session state".to_owned())?;
        let path = root.join(SNAPSHOT_FILE);
        let backup = root.join(SNAPSHOT_BACKUP_FILE);
        write_with_backup(&path, &backup, &bytes)
    }

    pub fn drain_pending_events<F>(&self, mut apply: F) -> Result<usize, String>
    where
        F: FnMut(CodexEventRequest) -> Result<(), String>,
    {
        let Some(root) = self.root.as_ref() else {
            return Ok(0);
        };
        let (pending, errors) = load_pending_events(root);
        let mut applied = 0usize;
        for (path, event) in pending {
            apply(event)?;
            match fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err("unable to acknowledge pending event".to_owned()),
            }
            applied += 1;
        }
        if errors.is_empty() {
            Ok(applied)
        } else {
            Err(format!(
                "{}; {applied} valid pending event(s) were still applied",
                errors.join("; ")
            ))
        }
    }
}

fn load_snapshot(root: &Path) -> Result<Option<CoreState>, String> {
    let primary = root.join(SNAPSHOT_FILE);
    let backup = root.join(SNAPSHOT_BACKUP_FILE);
    match read_snapshot(&primary) {
        Ok(Some(core)) => Ok(Some(core)),
        Ok(None) => read_snapshot(&backup),
        Err(primary_error) => read_snapshot(&backup).map_err(|_| primary_error),
    }
}

fn read_snapshot(path: &Path) -> Result<Option<CoreState>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("unable to read persisted session state".to_owned()),
    };
    let stored: StoredCore = serde_json::from_slice(&bytes)
        .map_err(|_| "persisted session state is invalid".to_owned())?;
    if stored.schema_version != STORAGE_SCHEMA_VERSION {
        return Err("persisted session schema is unsupported".to_owned());
    }
    Ok(Some(stored.core))
}

fn load_pending_events(root: &Path) -> (Vec<(PathBuf, CodexEventRequest)>, Vec<String>) {
    let directory = root.join(EVENTS_DIRECTORY);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (Vec::new(), Vec::new())
        }
        Err(_) => return (Vec::new(), vec!["unable to read pending events".to_owned()]),
    };
    let mut events = Vec::new();
    let mut errors = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let length = entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        if length == 0 || length > MAX_EVENT_FILE_BYTES {
            errors.push("pending event file has an invalid size".to_owned());
            if let Err(error) = quarantine_pending_event(&directory, &path) {
                errors.push(error);
            }
            continue;
        }
        let event = fs::read(&path)
            .map_err(|_| "unable to read pending event".to_owned())
            .and_then(|bytes| {
                serde_json::from_slice::<CodexEventRequest>(&bytes)
                    .map_err(|_| "pending event is invalid".to_owned())
            });
        match event {
            Ok(event) => events.push((path, event)),
            Err(error) => {
                errors.push(error);
                if let Err(error) = quarantine_pending_event(&directory, &path) {
                    errors.push(error);
                }
            }
        }
    }
    events.sort_by(|(_, left), (_, right)| {
        left.emitted_at_ms
            .cmp(&right.emitted_at_ms)
            .then_with(|| left.event_id.cmp(&right.event_id))
    });
    (events, errors)
}

fn quarantine_pending_event(directory: &Path, path: &Path) -> Result<(), String> {
    let quarantine = directory.join("quarantine");
    fs::create_dir_all(&quarantine)
        .map_err(|_| "unable to create pending event quarantine".to_owned())?;
    let name = path
        .file_name()
        .ok_or_else(|| "pending event path is invalid".to_owned())?;
    let target = quarantine.join(format!(
        "{}-{}-{}",
        crate::core::now_ms(),
        std::process::id(),
        name.to_string_lossy()
    ));
    fs::rename(path, target).map_err(|_| "unable to quarantine invalid pending event".to_owned())
}

fn write_with_backup(path: &Path, backup: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "session storage path is invalid".to_owned())?;
    fs::create_dir_all(parent).map_err(|_| "unable to create session storage".to_owned())?;
    let temporary = parent.join(format!(
        ".session-state-{}-{}.tmp",
        std::process::id(),
        crate::core::now_ms()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| "unable to create session snapshot".to_owned())?;
        file.write_all(bytes)
            .map_err(|_| "unable to write session snapshot".to_owned())?;
        file.sync_all()
            .map_err(|_| "unable to flush session snapshot".to_owned())?;
        drop(file);
        if path.exists() {
            fs::copy(path, backup).map_err(|_| "unable to back up session snapshot".to_owned())?;
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
        .map_err(|_| "unable to replace session snapshot".to_owned())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::official_pet;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "codex-pet-storage-{name}-{}-{}",
            std::process::id(),
            crate::core::now_ms()
        ))
    }

    fn event(event_id: char, event_name: &str, emitted_at_ms: u64) -> CodexEventRequest {
        let state = official_pet::hook_state(event_name).expect("known hook");
        CodexEventRequest {
            schema_version: 1,
            event_id: event_id.to_string().repeat(64),
            session_id: "019fcc02-569f-7790-92ff-209aaca4275c".to_owned(),
            turn_id: Some("turn-1".to_owned()),
            hook_event_name: event_name.to_owned(),
            state: state.to_owned(),
            activity: official_pet::activity_for_state(state).to_owned(),
            emitted_at_ms,
        }
    }

    #[test]
    fn snapshot_round_trip_requires_route_revalidation_before_showing_review() {
        let root = test_root("snapshot");
        let store = SessionStore::at(root.clone());
        let mut core = CoreState::default();
        core.apply_event(event('a', "Stop", crate::core::now_ms()))
            .unwrap();
        let session_id = "019fcc02-569f-7790-92ff-209aaca4275c";
        let revision = core.begin_metadata(session_id, 3).unwrap().revision;
        core.apply_metadata(
            session_id,
            revision,
            crate::core::ThreadMetadata {
                thread_id: Some(session_id.to_owned()),
                ..Default::default()
            },
        );
        store.persist(&core).unwrap();

        let mut recovered = store.recover();
        assert_eq!(recovered.core.state.state, "idle");
        assert!(recovered.core.sessions_snapshot().sessions.is_empty());
        let request = recovered.core.route_reconciliation_request();
        recovered.core.apply_active_thread_snapshot(
            &request,
            &std::collections::HashSet::from([session_id.to_owned()]),
            true,
        );
        let revision = recovered
            .core
            .begin_metadata(session_id, 3)
            .unwrap()
            .revision;
        recovered.core.apply_metadata(
            session_id,
            revision,
            crate::core::ThreadMetadata {
                thread_id: Some(session_id.to_owned()),
                ..Default::default()
            },
        );
        assert!(recovered.core.sessions_snapshot().sessions[0].unread);
        assert_eq!(
            recovered.core.sessions_snapshot().sessions[0]
                .target_thread_id
                .as_deref(),
            Some(session_id)
        );
        assert_eq!(recovered.recovered_events, 0);
        assert!(recovered.error.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn legacy_snapshot_without_verified_thread_recovers_fail_closed() {
        let root = test_root("legacy-route");
        let store = SessionStore::at(root.clone());
        let mut core = CoreState::default();
        core.apply_event(event('c', "Stop", crate::core::now_ms()))
            .unwrap();
        store.persist(&core).unwrap();

        let bytes = fs::read(root.join(SNAPSHOT_FILE)).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("verified_thread_id"));
        let recovered = store.recover();
        assert!(recovered.error.is_none());
        assert!(recovered.core.sessions_snapshot().sessions.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pending_event_is_replayed_once_and_removed_after_snapshot() {
        let root = test_root("pending");
        let store = SessionStore::at(root.clone());
        let events = root.join(EVENTS_DIRECTORY);
        fs::create_dir_all(&events).unwrap();
        let pending = event('b', "UserPromptSubmit", crate::core::now_ms());
        fs::write(
            events.join(format!("{}.json", pending.event_id)),
            serde_json::to_vec(&pending).unwrap(),
        )
        .unwrap();

        let recovered = store.recover();
        assert_eq!(recovered.core.state.state, "idle");
        assert!(recovered.core.sessions_snapshot().sessions.is_empty());
        assert_eq!(recovered.core.route_reconciliation_request().len(), 1);
        assert_eq!(recovered.recovered_events, 1);
        assert_eq!(fs::read_dir(&events).unwrap().count(), 0);
        let second = store.recover();
        assert_eq!(second.core.route_reconciliation_request()[0].1, 1);
        assert!(second.core.sessions_snapshot().sessions.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn running_drain_keeps_an_event_until_the_callback_succeeds() {
        let root = test_root("running-drain");
        let events = root.join(EVENTS_DIRECTORY);
        fs::create_dir_all(&events).unwrap();
        let payload = event('f', "UserPromptSubmit", crate::core::now_ms());
        let path = events.join("pending.json");
        fs::write(&path, serde_json::to_vec(&payload).unwrap()).unwrap();
        let store = SessionStore::at(root.clone());

        assert!(store
            .drain_pending_events(|_| Err("persist failed".to_owned()))
            .is_err());
        assert!(path.exists());

        let mut observed = Vec::new();
        assert_eq!(
            store
                .drain_pending_events(|event| {
                    observed.push(event.event_id);
                    Ok(())
                })
                .unwrap(),
            1
        );
        assert_eq!(observed, vec![payload.event_id]);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_pending_event_is_quarantined_without_blocking_valid_events() {
        let root = test_root("quarantine");
        let events = root.join(EVENTS_DIRECTORY);
        fs::create_dir_all(&events).unwrap();
        fs::write(events.join("broken.json"), b"{not-json").unwrap();
        let payload = event('e', "UserPromptSubmit", crate::core::now_ms());
        let valid = events.join("valid.json");
        fs::write(&valid, serde_json::to_vec(&payload).unwrap()).unwrap();
        let store = SessionStore::at(root.clone());
        let mut observed = Vec::new();

        let error = store
            .drain_pending_events(|event| {
                observed.push(event.event_id);
                Ok(())
            })
            .unwrap_err();

        assert!(error.contains("still applied"));
        assert_eq!(observed, vec![payload.event_id]);
        assert!(!valid.exists());
        assert_eq!(fs::read_dir(events.join("quarantine")).unwrap().count(), 1);
        let _ = fs::remove_dir_all(root);
    }
}
