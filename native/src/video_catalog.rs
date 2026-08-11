use crate::core::VALID_STATES;
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

const SUPPORTED_VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "webm", "m4v"];
pub type VideoCatalog = HashMap<String, Vec<String>>;

pub fn video_state_from_stem(stem: &str) -> Option<&'static str> {
    let normalized = stem.to_ascii_lowercase();
    VALID_STATES
        .iter()
        .copied()
        .filter(|state| normalized.starts_with(state))
        .max_by_key(|state| state.len())
}

pub fn is_supported_video(path: &Path) -> bool {
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

pub fn scan_video_catalog(root: &Path) -> Result<VideoCatalog, Box<dyn std::error::Error>> {
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

pub fn resolve_video_catalog() -> Result<(PathBuf, VideoCatalog), Box<dyn std::error::Error>> {
    let roots = video_root_candidates()?;
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

pub fn video_root_candidates() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let configured_root = std::env::var_os("CODEX_PET_VIDEO_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute());
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("project root unavailable")?;
    let source_root = project_root.join("VideoSource").join("V1");
    let local_data_root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.to_path_buf())
        .join("com.local.codex-pet-overlay")
        .join("VideoSource")
        .join("V1");
    let executable_root = std::env::current_exe()?
        .parent()
        .ok_or("executable directory unavailable")?
        .join("VideoSource")
        .join("V1");

    let mut roots = Vec::new();
    if let Some(root) = configured_root {
        roots.push(root);
    }
    roots.extend([executable_root, source_root, local_data_root]);
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::now_ms;

    #[test]
    fn maps_numbered_video_names_without_hardcoded_lists() {
        assert_eq!(video_state_from_stem("idle"), Some("idle"));
        assert_eq!(video_state_from_stem("idle1"), Some("idle"));
        assert_eq!(video_state_from_stem("idle_1"), Some("idle"));
        assert_eq!(video_state_from_stem("running-left2"), Some("running-left"));
        assert_eq!(
            video_state_from_stem("running-right99"),
            Some("running-right")
        );
        assert_eq!(video_state_from_stem("running_custom"), Some("running"));
        assert_eq!(video_state_from_stem("unknown"), None);
    }

    #[test]
    fn catalog_keeps_duplicate_content_as_separate_candidates() {
        let root = std::env::temp_dir().join(format!(
            "codex-pet-native-video-scan-{}-{}",
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
}
