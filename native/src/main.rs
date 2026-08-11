#![windows_subsystem = "windows"]

use codex_pet_native::{
    hook_bridge, hooks,
    runtime::RuntimeData,
    settings, setup,
    ui::{self, SingleInstance},
    video_config::{load_video_config_at, resolve_video_config},
};
use std::{ffi::OsStr, os::windows::ffi::OsStrExt, path::PathBuf, time::Duration};
use windows::{
    core::{w, PCWSTR},
    Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK},
};

fn main() {
    let mode = std::env::args_os().nth(1);
    match mode.as_deref() {
        Some(argument) if argument == OsStr::new("--hook") => {
            hook_bridge::run_from_stdin();
            return;
        }
        Some(argument) if argument == OsStr::new("--hook-self-test") => {
            exit_for_result(hook_bridge::self_test());
            return;
        }
        Some(argument) if argument == OsStr::new("--install-hooks") => {
            exit_for_json(hooks::install_current());
            return;
        }
        Some(argument) if argument == OsStr::new("--remove-hooks") => {
            exit_for_json(remove_integration());
            return;
        }
        Some(argument) if argument == OsStr::new("--repair-integration") => {
            exit_for_json(repair_integration());
            return;
        }
        Some(argument) if argument == OsStr::new("--setup") => {
            if let Err(error) = setup::run() {
                show_error(&error);
                std::process::exit(2);
            }
            return;
        }
        Some(argument) if argument == OsStr::new("--shutdown") => {
            exit_for_result(ui::request_existing_exit(Duration::from_secs(5)).map(|_| ()));
            return;
        }
        Some(argument) if argument == OsStr::new("--validate-video-config") => {
            exit_for_result(validate_video_config_cli());
            return;
        }
        _ => {}
    }
    if mode.is_none()
        && settings::load()
            .map(|settings| !settings.setup_complete)
            .unwrap_or(true)
    {
        if let Err(error) = setup::run() {
            show_error(&error);
            std::process::exit(2);
        }
        return;
    }
    let login_start = mode
        .as_deref()
        .is_some_and(|argument| argument == OsStr::new("--login-start"));
    if let Err(error) = run(login_start) {
        show_error(&error);
        std::process::exit(2);
    }
}

fn remove_integration() -> Result<hooks::HookInstallResult, String> {
    let _ = ui::request_existing_exit(Duration::from_secs(5))?;
    let report = hooks::remove_current()?;
    settings::set_startup_enabled(false)?;
    settings::mark_setup_incomplete()?;
    Ok(report)
}

fn repair_integration() -> Result<hooks::HookInstallResult, String> {
    settings::mark_setup_incomplete()?;
    hooks::install_current()
}

fn exit_for_result(result: Result<(), String>) {
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(2);
    }
}

fn exit_for_json<T: serde::Serialize>(result: Result<T, String>) {
    match result.and_then(|value| {
        serde_json::to_string(&value).map_err(|_| "unable to serialize command result".to_owned())
    }) {
        Ok(value) => println!("{value}"),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}

fn validate_video_config_cli() -> Result<(), String> {
    let root = std::env::var_os("CODEX_PET_VIDEO_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "CODEX_PET_VIDEO_DIR is required for validation".to_owned())?;
    if !root.is_absolute() {
        return Err("CODEX_PET_VIDEO_DIR must be absolute for validation".to_owned());
    }
    load_video_config_at(&root)
        .map(|_| ())
        .map_err(|error| format!("video config validation failed: {error}"))
}

fn run(login_start: bool) -> Result<(), String> {
    let Some(_single_instance) = SingleInstance::acquire()? else {
        return Ok(());
    };
    let video_config = resolve_video_config().map_err(|error| error.to_string())?;
    let runtime = RuntimeData::new(video_config);
    if login_start {
        runtime.start_hidden_until_hook();
    }
    let http = codex_pet_native::http::start(runtime.clone())?;
    let result = ui::run(runtime.clone());
    runtime.stop();
    let _ = http.join();
    result
}

fn show_error(error: &str) {
    let text = OsStr::new(error)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    unsafe {
        let _ = MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            w!("Codex Pet Overlay 启动失败"),
            MB_OK | MB_ICONERROR,
        );
    }
}
