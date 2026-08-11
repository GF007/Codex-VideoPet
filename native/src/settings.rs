use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs::{self, OpenOptions},
    io::Write,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS},
        Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH},
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteKeyValueW, RegGetValueW, RegSetValueExW, HKEY,
            HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
            RRF_RT_REG_SZ,
        },
    },
};

const SETTINGS_SCHEMA_VERSION: u8 = 1;
const SETTINGS_FILE: &str = "settings.json";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "CodexPet";
const OWNER_ID: &str = "codex-pet-overlay";

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowSizePreset {
    Small,
    #[default]
    Medium,
    Large,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u8,
    pub setup_complete: bool,
    pub startup_enabled: bool,
    pub last_hook_verified_at_ms: Option<u64>,
    #[serde(default)]
    pub window_size_preset: WindowSizePreset,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            setup_complete: false,
            startup_enabled: false,
            last_hook_verified_at_ms: None,
            window_size_preset: WindowSizePreset::Medium,
        }
    }
}

pub fn load() -> Result<Settings, String> {
    let path = settings_path()?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default())
        }
        Err(_) => return Err("unable to read Codex Pet settings".to_owned()),
    };
    let settings: Settings =
        serde_json::from_slice(&bytes).map_err(|_| "Codex Pet settings are invalid".to_owned())?;
    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        return Err("Codex Pet settings schema is unsupported".to_owned());
    }
    Ok(settings)
}

pub fn save(settings: &Settings) -> Result<(), String> {
    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        return Err("Codex Pet settings schema is unsupported".to_owned());
    }
    let path = settings_path()?;
    let bytes = serde_json::to_vec_pretty(settings)
        .map_err(|_| "unable to serialize Codex Pet settings".to_owned())?;
    atomic_write(&path, &bytes)
}

pub fn mark_setup_incomplete() -> Result<(), String> {
    let mut settings = load().unwrap_or_default();
    settings.setup_complete = false;
    settings.last_hook_verified_at_ms = None;
    save(&settings)
}

pub fn set_window_size_preset(preset: WindowSizePreset) -> Result<(), String> {
    let mut settings = load()?;
    settings.window_size_preset = preset;
    save(&settings)
}

pub fn set_startup_enabled(enabled: bool) -> Result<(), String> {
    if enabled {
        let executable =
            std::env::current_exe().map_err(|_| "unable to locate Codex Pet".to_owned())?;
        let command = startup_command(&executable);
        if startup_command_readback()?
            .as_deref()
            .is_some_and(|existing| !is_owned_startup_command(existing))
        {
            return Err("Windows startup value CodexPet belongs to another application".to_owned());
        }
        set_run_value(&command)?;
        if startup_command_readback()?.as_deref() != Some(command.as_str()) {
            return Err("Windows startup registration failed readback".to_owned());
        }
    } else {
        match startup_command_readback()? {
            Some(existing) if is_owned_startup_command(&existing) => delete_run_value()?,
            Some(_) => return Ok(()),
            None => {}
        }
        if startup_command_readback()?
            .as_deref()
            .is_some_and(is_owned_startup_command)
        {
            return Err("Windows startup registration could not be removed".to_owned());
        }
    }
    Ok(())
}

pub fn startup_enabled_readback() -> Result<bool, String> {
    Ok(startup_command_readback()?
        .as_deref()
        .is_some_and(is_owned_startup_command))
}

pub fn startup_command_readback() -> Result<Option<String>, String> {
    let subkey = wide(RUN_KEY);
    let value = wide(RUN_VALUE);
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS || size == 0 || size > 32 * 1024 {
        return Err("unable to read Windows startup registration".to_owned());
    }
    let mut bytes = vec![0u8; size as usize];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(bytes.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return Err("unable to read Windows startup registration".to_owned());
    }
    let units =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u16>(), size as usize / 2) };
    let end = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16(&units[..end])
        .map(Some)
        .map_err(|_| "Windows startup registration is invalid".to_owned())
}

fn set_run_value(command: &str) -> Result<(), String> {
    let subkey = wide(RUN_KEY);
    let value = wide(RUN_VALUE);
    let mut key = HKEY::default();
    let status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_QUERY_VALUE | KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if status != ERROR_SUCCESS {
        return Err("unable to open Windows startup registration".to_owned());
    }
    let wide_command = wide(command);
    let bytes = unsafe {
        std::slice::from_raw_parts(
            wide_command.as_ptr().cast::<u8>(),
            wide_command.len() * std::mem::size_of::<u16>(),
        )
    };
    let status = unsafe { RegSetValueExW(key, PCWSTR(value.as_ptr()), None, REG_SZ, Some(bytes)) };
    let _ = unsafe { RegCloseKey(key) };
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err("unable to write Windows startup registration".to_owned())
    }
}

fn delete_run_value() -> Result<(), String> {
    let subkey = wide(RUN_KEY);
    let value = wide(RUN_VALUE);
    let status = unsafe {
        RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value.as_ptr()),
        )
    };
    if status == ERROR_SUCCESS || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err("unable to remove Windows startup registration".to_owned())
    }
}

fn settings_path() -> Result<PathBuf, String> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|root| root.join("com.local.codex-pet-overlay").join(SETTINGS_FILE))
        .ok_or_else(|| "LOCALAPPDATA is unavailable".to_owned())
}

fn startup_command(executable: &Path) -> String {
    format!(
        "\"{}\" --login-start --owner {OWNER_ID}",
        executable.display()
    )
}

fn is_owned_startup_command(command: &str) -> bool {
    let current_suffix = format!(" --login-start --owner {OWNER_ID}");
    let path = command
        .strip_suffix(&current_suffix)
        .or_else(|| command.strip_suffix(" --login-start"));
    let Some(path) = path
        .and_then(|value| value.strip_prefix('"'))
        .and_then(|value| value.strip_suffix('"'))
    else {
        return false;
    };
    if path.contains('"') {
        return false;
    }
    matches!(
        path.rsplit(['/', '\\'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "codexpet.exe" | "codex-pet-native.exe"
    )
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Codex Pet settings path is invalid".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|_| "unable to create Codex Pet settings directory".to_owned())?;
    let temporary = parent.join(format!(
        ".settings-{}-{}.tmp",
        std::process::id(),
        crate::core::now_ms()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| "unable to create Codex Pet settings".to_owned())?;
        file.write_all(bytes)
            .map_err(|_| "unable to write Codex Pet settings".to_owned())?;
        file.sync_all()
            .map_err(|_| "unable to flush Codex Pet settings".to_owned())?;
        drop(file);
        let temporary_wide = wide_os(temporary.as_os_str());
        let path_wide = wide_os(path.as_os_str());
        unsafe {
            MoveFileExW(
                PCWSTR(temporary_wide.as_ptr()),
                PCWSTR(path_wide.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|_| "unable to commit Codex Pet settings".to_owned())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn wide(value: &str) -> Vec<u16> {
    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(Some(0))
        .collect()
}

fn wide_os(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn startup_command_quotes_paths_and_uses_login_mode() {
        assert_eq!(
            startup_command(Path::new(r"C:\Local Apps\CodexPet\CodexPet.exe")),
            r#""C:\Local Apps\CodexPet\CodexPet.exe" --login-start --owner codex-pet-overlay"#
        );
    }

    #[test]
    fn startup_ownership_requires_the_product_marker_or_exact_legacy_shape() {
        assert!(is_owned_startup_command(
            r#""C:\Apps\CodexPet.exe" --login-start --owner codex-pet-overlay"#
        ));
        assert!(is_owned_startup_command(
            r#""C:\Apps\CodexPet.exe" --login-start"#
        ));
        assert!(!is_owned_startup_command(
            r#""C:\Apps\Other.exe" --login-start --owner codex-pet-overlay"#
        ));
        assert!(!is_owned_startup_command(
            r#""C:\Apps\CodexPet.exe" --login-start --owner someone-else"#
        ));
    }

    #[test]
    fn settings_roundtrip_is_strict_and_versioned() {
        let settings = Settings {
            setup_complete: true,
            startup_enabled: true,
            last_hook_verified_at_ms: Some(42),
            window_size_preset: WindowSizePreset::Large,
            ..Settings::default()
        };
        let bytes = serde_json::to_vec(&settings).unwrap();
        assert_eq!(
            serde_json::from_slice::<Settings>(&bytes).unwrap(),
            settings
        );
        let mut value: Value = serde_json::from_slice(&bytes).unwrap();
        value["unknown"] = Value::Bool(true);
        assert!(serde_json::from_value::<Settings>(value).is_err());
    }

    #[test]
    fn legacy_settings_default_to_the_medium_window_size() {
        let legacy = serde_json::json!({
            "schema_version": 1,
            "setup_complete": true,
            "startup_enabled": false,
            "last_hook_verified_at_ms": null
        });
        let settings: Settings = serde_json::from_value(legacy).unwrap();
        assert_eq!(settings.window_size_preset, WindowSizePreset::Medium);
    }
}
