use codex_pet_native::{
    runtime::API_ADDRESS,
    video_config::{
        import_video_files, load_current_video_config_at, resolve_video_config,
        save_video_config_at, state_schema, LoadedVideoConfig, VideoConfig, VIDEO_CONFIG_FILENAME,
    },
};
use serde::Serialize;
use serde_json::Value;
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::Duration,
};
use tauri::{Manager, State};

struct EditorSession {
    fallback_video_root: PathBuf,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorBootstrap {
    schema: codex_pet_native::video_config::StateSchema,
    video_root: String,
    config_path: String,
    source: codex_pet_native::video_config::VideoConfigSource,
    fingerprint: String,
    config: VideoConfig,
    resolved_paths: std::collections::BTreeMap<String, Vec<String>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeSync {
    status: &'static str,
    message: String,
    runtime_fingerprint: Option<String>,
    playback_fingerprint: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorSaveResult {
    bootstrap: EditorBootstrap,
    runtime_sync: RuntimeSync,
}

#[tauri::command]
fn editor_bootstrap(session: State<'_, Mutex<EditorSession>>) -> Result<EditorBootstrap, String> {
    let root = resolve_editor_root(&session)?;
    let loaded = load_current_video_config_at(&root).map_err(|error| error.to_string())?;
    Ok(to_bootstrap(loaded))
}

#[tauri::command]
fn editor_save_config(
    config: VideoConfig,
    expected_video_root: String,
    session: State<'_, Mutex<EditorSession>>,
) -> Result<EditorSaveResult, String> {
    let root = require_expected_root(&session, &expected_video_root)?;
    let loaded = save_video_config_at(&root, config).map_err(|error| error.to_string())?;
    let runtime_sync = wait_for_runtime(&loaded.fingerprint, &loaded.root);
    Ok(EditorSaveResult {
        bootstrap: to_bootstrap(loaded),
        runtime_sync,
    })
}

#[tauri::command]
fn editor_import_videos(
    state: String,
    paths: Vec<String>,
    expected_video_root: String,
    session: State<'_, Mutex<EditorSession>>,
) -> Result<EditorSaveResult, String> {
    let root = require_expected_root(&session, &expected_video_root)?;
    let current = load_current_video_config_at(&root).map_err(|error| error.to_string())?;
    let loaded = import_video_files(&root, &state, &paths, current.config)
        .map_err(|error| error.to_string())?;
    let runtime_sync = wait_for_runtime(&loaded.fingerprint, &loaded.root);
    Ok(EditorSaveResult {
        bootstrap: to_bootstrap(loaded),
        runtime_sync,
    })
}

#[tauri::command]
fn editor_runtime_sync(
    expected_fingerprint: String,
    expected_video_root: String,
    session: State<'_, Mutex<EditorSession>>,
) -> RuntimeSync {
    match require_expected_root(&session, &expected_video_root) {
        Ok(root) => wait_for_runtime(&expected_fingerprint, &root),
        Err(message) => RuntimeSync {
            status: "error",
            message,
            runtime_fingerprint: None,
            playback_fingerprint: None,
        },
    }
}

fn to_bootstrap(loaded: LoadedVideoConfig) -> EditorBootstrap {
    EditorBootstrap {
        schema: state_schema(),
        video_root: loaded.root.display().to_string(),
        config_path: loaded.config_path.display().to_string(),
        source: loaded.source,
        fingerprint: loaded.fingerprint,
        config: loaded.config,
        resolved_paths: loaded.resolved_paths,
    }
}

fn wait_for_runtime(expected: &str, expected_root: &Path) -> RuntimeSync {
    let mut last = None;
    for _ in 0..30 {
        match read_runtime_config() {
            Ok(None) => {
                return RuntimeSync {
                    status: "saved",
                    message: "配置已保存；Pet 未运行，将在下次启动时直接读取。".to_owned(),
                    runtime_fingerprint: None,
                    playback_fingerprint: None,
                }
            }
            Ok(Some(value)) => {
                let runtime_root = match runtime_root_from_response(&value) {
                    Ok(root) => root,
                    Err(message) => {
                        return RuntimeSync {
                            status: "error",
                            message,
                            runtime_fingerprint: None,
                            playback_fingerprint: None,
                        }
                    }
                };
                let runtime_fingerprint = json_string(&value, "fingerprint");
                let playback_fingerprint = json_string(&value, "playbackFingerprint");
                if runtime_root != expected_root {
                    return RuntimeSync {
                        status: "error",
                        message: "运行中的 Pet 已切换到另一套资源目录，请重新打开编辑器。"
                            .to_owned(),
                        runtime_fingerprint,
                        playback_fingerprint,
                    };
                }
                let active_players = value
                    .get("activePlayerInstances")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let playback_ready = value
                    .get("playbackReady")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let runtime_error = value.get("error").and_then(Value::as_str);
                let playback_error = value.get("playbackError").and_then(Value::as_str);
                if runtime_fingerprint.as_deref() == Some(expected) {
                    if let Some(error) = runtime_error {
                        return RuntimeSync {
                            status: "error",
                            message: format!("运行时拒绝了配置：{error}"),
                            runtime_fingerprint,
                            playback_fingerprint,
                        };
                    }
                    if let Some(error) = playback_error {
                        return RuntimeSync {
                            status: "error",
                            message: format!("配置已载入，但播放器应用失败：{error}"),
                            runtime_fingerprint,
                            playback_fingerprint,
                        };
                    }
                    if active_players == 0 {
                        return RuntimeSync {
                            status: "loaded",
                            message: "运行时已载入配置；播放器当前已释放，显示 Pet 时会应用。"
                                .to_owned(),
                            runtime_fingerprint,
                            playback_fingerprint,
                        };
                    }
                    if playback_ready && playback_fingerprint.as_deref() == Some(expected) {
                        return RuntimeSync {
                            status: "applied",
                            message: "配置与正在播放的 Pet 已同步。".to_owned(),
                            runtime_fingerprint,
                            playback_fingerprint,
                        };
                    }
                }
                last = Some((runtime_fingerprint, playback_fingerprint));
            }
            Err(error) => {
                return RuntimeSync {
                    status: "error",
                    message: error,
                    runtime_fingerprint: None,
                    playback_fingerprint: None,
                }
            }
        }
        thread::sleep(Duration::from_millis(100));
    }
    let (runtime_fingerprint, playback_fingerprint) = last.unwrap_or_default();
    RuntimeSync {
        status: "pending",
        message: "配置已保存，但运行时播放指纹尚未同步；请检查 Pet 播放状态。".to_owned(),
        runtime_fingerprint,
        playback_fingerprint,
    }
}

fn read_runtime_config() -> Result<Option<Value>, String> {
    let address = API_ADDRESS
        .parse::<SocketAddr>()
        .map_err(|error| format!("invalid runtime address: {error}"))?;
    let mut stream = match TcpStream::connect_timeout(&address, Duration::from_millis(150)) {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::TimedOut
            ) =>
        {
            return Ok(None)
        }
        Err(error) => return Err(format!("unable to query Pet runtime: {error}")),
    };
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .map_err(|error| error.to_string())?;
    stream
        .write_all(b"GET /v1/video-config HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .map_err(|error| error.to_string())?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| error.to_string())?;
    let separator = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "invalid response from Pet runtime".to_owned())?;
    let headers = std::str::from_utf8(&response[..separator])
        .map_err(|_| "invalid response headers from Pet runtime".to_owned())?;
    if !headers.starts_with("HTTP/1.1 200 ") && !headers.starts_with("HTTP/1.0 200 ") {
        return Err("Pet runtime rejected the video config query".to_owned());
    }
    serde_json::from_slice(&response[separator + 4..])
        .map(Some)
        .map_err(|error| format!("invalid runtime config response: {error}"))
}

fn json_string(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn resolve_editor_root(session: &State<'_, Mutex<EditorSession>>) -> Result<PathBuf, String> {
    let fallback = session
        .lock()
        .map_err(|_| "editor session lock poisoned".to_owned())?
        .fallback_video_root
        .clone();
    match read_runtime_config()? {
        Some(value) => runtime_root_from_response(&value),
        None => Ok(fallback),
    }
}

fn require_expected_root(
    session: &State<'_, Mutex<EditorSession>>,
    expected: &str,
) -> Result<PathBuf, String> {
    let active = resolve_editor_root(session)?;
    let expected = PathBuf::from(expected)
        .canonicalize()
        .map_err(|error| format!("编辑器资源目录不可用：{error}"))?;
    if active != expected {
        return Err("运行中的 Pet 配置根已变化，请重新打开编辑器后再保存。".to_owned());
    }
    Ok(active)
}

fn runtime_root_from_response(value: &Value) -> Result<PathBuf, String> {
    if value.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err("Pet runtime did not return a valid video config".to_owned());
    }
    let root = json_string(value, "configRoot")
        .map(PathBuf::from)
        .ok_or_else(|| "Pet runtime response is missing configRoot".to_owned())?;
    let config_path = json_string(value, "configPath")
        .map(PathBuf::from)
        .ok_or_else(|| "Pet runtime response is missing configPath".to_owned())?;
    if !root.is_absolute() || !config_path.is_absolute() {
        return Err("Pet runtime returned a non-absolute video config path".to_owned());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("Pet runtime video root is unavailable: {error}"))?;
    let parent = config_path
        .parent()
        .ok_or_else(|| "Pet runtime configPath has no parent".to_owned())?
        .canonicalize()
        .map_err(|error| format!("Pet runtime config directory is unavailable: {error}"))?;
    if parent != root
        || config_path.file_name().and_then(|value| value.to_str()) != Some(VIDEO_CONFIG_FILENAME)
    {
        return Err("Pet runtime returned inconsistent video config paths".to_owned());
    }
    Ok(root)
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            editor_bootstrap,
            editor_save_config,
            editor_import_videos,
            editor_runtime_sync
        ])
        .setup(|app| {
            let local = resolve_video_config();
            let runtime_root = match read_runtime_config().map_err(|error| error.to_string())? {
                Some(value) => Some(runtime_root_from_response(&value)?),
                None => None,
            };
            let fallback_video_root = match local {
                Ok(loaded) => loaded.root,
                Err(error) => runtime_root.clone().ok_or_else(|| error.to_string())?,
            };
            let active_root = runtime_root.unwrap_or_else(|| fallback_video_root.clone());
            app.asset_protocol_scope()
                .allow_directory(&active_root, true)?;
            if active_root != fallback_video_root {
                app.asset_protocol_scope()
                    .allow_directory(&fallback_video_root, true)?;
            }
            app.manage(Mutex::new(EditorSession {
                fallback_video_root,
            }));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run Codex Pet Video Studio");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_schema_comes_from_native_runtime() {
        let schema = state_schema();
        assert_eq!(schema.states, codex_pet_native::core::VALID_STATES);
    }

    #[test]
    fn runtime_address_is_loopback() {
        let address = API_ADDRESS.parse::<SocketAddr>().unwrap();
        assert!(address.ip().is_loopback());
    }

    #[test]
    fn runtime_root_requires_matching_absolute_config_path() {
        let root = std::env::temp_dir().join(format!(
            "codex-pet-editor-root-{}-{}",
            std::process::id(),
            codex_pet_native::core::now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let valid = serde_json::json!({
            "ok": true,
            "configRoot": root.display().to_string(),
            "configPath": root.join(VIDEO_CONFIG_FILENAME).display().to_string()
        });
        assert_eq!(runtime_root_from_response(&valid).unwrap(), root);

        let invalid = serde_json::json!({
            "ok": true,
            "configRoot": root.display().to_string(),
            "configPath": root.join("other.json").display().to_string()
        });
        assert!(runtime_root_from_response(&invalid).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn runtime_root_rejects_missing_or_relative_values() {
        assert!(runtime_root_from_response(&serde_json::json!({ "ok": true })).is_err());
        assert!(runtime_root_from_response(&serde_json::json!({
            "ok": true,
            "configRoot": "VideoSource/V1",
            "configPath": "VideoSource/V1/video-config.json"
        }))
        .is_err());
    }
}
