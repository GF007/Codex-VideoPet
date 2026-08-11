use crate::{
    core::{is_valid_state, VALID_STATES},
    official_pet,
    video_catalog::{is_supported_video, scan_video_catalog, video_root_candidates, VideoCatalog},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

pub const VIDEO_CONFIG_SCHEMA_VERSION: u32 = 1;
pub const VIDEO_CONFIG_FILENAME: &str = "video-config.json";
const MAX_WEIGHT: u32 = 1_000_000;
const MAX_LOOP_POINT_MS: u64 = 24 * 60 * 60 * 1_000;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionMode {
    #[default]
    WeightedRandom,
    Fixed,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionPolicy {
    #[default]
    Immediate,
    FinishCurrentClip,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VideoLoop {
    pub start_ms: u64,
    pub end_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VideoClipConfig {
    pub file: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default, rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_range: Option<VideoLoop>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateVideoConfig {
    #[serde(default)]
    pub selection: SelectionMode,
    #[serde(default)]
    pub transition_policy: TransitionPolicy,
    pub clips: Vec<VideoClipConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct VideoConfig {
    pub schema_version: u32,
    pub states: BTreeMap<String, StateVideoConfig>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoConfigSource {
    Configured,
    LegacyScan,
}

#[derive(Clone, Debug)]
pub struct LoadedVideoConfig {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub source: VideoConfigSource,
    pub fingerprint: String,
    pub config: VideoConfig,
    pub resolved_paths: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StateSchema {
    pub driver_version: &'static str,
    pub states: Vec<&'static str>,
}

pub fn state_schema() -> StateSchema {
    StateSchema {
        driver_version: official_pet::DRIVER_VERSION,
        states: VALID_STATES.to_vec(),
    }
}

pub fn resolve_video_config() -> Result<LoadedVideoConfig, Box<dyn std::error::Error>> {
    let mut checked = Vec::new();
    for root in video_root_candidates()? {
        checked.push(root.display().to_string());
        if !root.is_dir() {
            continue;
        }
        if root.join(VIDEO_CONFIG_FILENAME).is_file() {
            return load_video_config_at(&root);
        }
        if let Ok(catalog) = scan_video_catalog(&root) {
            return load_legacy_catalog_at(&root, catalog);
        }
    }
    Err(format!(
        "video config is missing or incomplete; checked: {}",
        checked.join("; ")
    )
    .into())
}

pub fn load_video_config_at(root: &Path) -> Result<LoadedVideoConfig, Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    let config_path = root.join(VIDEO_CONFIG_FILENAME);
    let bytes = fs::read(&config_path)?;
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let mut config: VideoConfig = serde_json::from_slice(bytes)?;
    normalize_config_paths(&mut config)?;
    build_loaded(root, config_path, VideoConfigSource::Configured, config)
}

pub fn load_current_video_config_at(
    root: &Path,
) -> Result<LoadedVideoConfig, Box<dyn std::error::Error>> {
    let config_path = root.join(VIDEO_CONFIG_FILENAME);
    if config_path.is_file() {
        load_video_config_at(root)
    } else {
        let catalog = scan_video_catalog(root)?;
        load_legacy_catalog_at(root, catalog)
    }
}

pub fn save_video_config_at(
    root: &Path,
    mut config: VideoConfig,
) -> Result<LoadedVideoConfig, Box<dyn std::error::Error>> {
    normalize_config_paths(&mut config)?;
    let root = root.canonicalize()?;
    let config_path = root.join(VIDEO_CONFIG_FILENAME);
    let loaded = build_loaded(
        root.clone(),
        config_path.clone(),
        VideoConfigSource::Configured,
        config,
    )?;
    let mut bytes = serde_json::to_vec_pretty(&loaded.config)?;
    bytes.push(b'\n');
    write_replace(&config_path, &bytes)?;

    let readback = load_video_config_at(&root)?;
    if readback.fingerprint != loaded.fingerprint {
        return Err("video config readback fingerprint mismatch".into());
    }
    Ok(readback)
}

pub fn import_video_files(
    root: &Path,
    state: &str,
    sources: &[String],
    mut config: VideoConfig,
) -> Result<LoadedVideoConfig, Box<dyn std::error::Error>> {
    if !is_valid_state(state) {
        return Err(format!("unknown state: {state}").into());
    }
    if sources.is_empty() {
        return Err("no video files were provided".into());
    }
    let root = root.canonicalize()?;
    let destination_dir = root.join(state);
    fs::create_dir_all(&destination_dir)?;
    let state_config = config
        .states
        .get_mut(state)
        .ok_or_else(|| format!("video config is missing state: {state}"))?;

    for source in sources {
        let source = PathBuf::from(source).canonicalize()?;
        if !source.is_file() || !is_supported_video(&source) {
            return Err(format!("unsupported video file: {}", source.display()).into());
        }
        let destination = if source.starts_with(&root) {
            source
        } else {
            let file_name = source
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or("video filename is not valid UTF-8")?;
            let destination = unique_destination(&destination_dir, file_name);
            copy_replace(&source, &destination)?;
            destination.canonicalize()?
        };
        let relative = relative_file(&root, &destination)?;
        if state_config
            .clips
            .iter()
            .any(|clip| clip.file.eq_ignore_ascii_case(&relative))
        {
            continue;
        }
        state_config.clips.push(VideoClipConfig {
            file: relative,
            enabled: true,
            weight: default_weight(),
            loop_range: None,
        });
    }
    save_video_config_at(&root, config)
}

pub fn select_clip<'a>(
    state: &'a StateVideoConfig,
    previous_file: Option<&str>,
    random_value: u64,
) -> Option<&'a VideoClipConfig> {
    let enabled = state
        .clips
        .iter()
        .filter(|clip| clip.enabled)
        .collect::<Vec<_>>();
    if enabled.is_empty() {
        return None;
    }
    if state.selection == SelectionMode::Fixed {
        return enabled.first().copied();
    }
    let mut choices = enabled
        .iter()
        .copied()
        .filter(|clip| {
            enabled.len() == 1
                || previous_file.is_none_or(|previous| !clip.file.eq_ignore_ascii_case(previous))
        })
        .collect::<Vec<_>>();
    if choices.is_empty() {
        choices = enabled;
    }
    let total = choices
        .iter()
        .fold(0_u64, |sum, clip| sum.saturating_add(clip.weight as u64));
    let mut target = random_value % total;
    for clip in choices {
        if target < clip.weight as u64 {
            return Some(clip);
        }
        target -= clip.weight as u64;
    }
    None
}

pub fn config_fingerprint(config: &VideoConfig) -> String {
    let bytes = serde_json::to_vec(config).unwrap_or_default();
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn load_legacy_catalog_at(
    root: &Path,
    catalog: VideoCatalog,
) -> Result<LoadedVideoConfig, Box<dyn std::error::Error>> {
    let root = root.canonicalize()?;
    let config = config_from_catalog(&root, &catalog)?;
    let config_path = root.join(VIDEO_CONFIG_FILENAME);
    build_loaded(root, config_path, VideoConfigSource::LegacyScan, config)
}

fn config_from_catalog(
    root: &Path,
    catalog: &VideoCatalog,
) -> Result<VideoConfig, Box<dyn std::error::Error>> {
    let mut states = BTreeMap::new();
    for state in VALID_STATES {
        let clips = catalog
            .get(state)
            .ok_or_else(|| format!("video catalog is missing state: {state}"))?
            .iter()
            .map(|path| {
                Ok(VideoClipConfig {
                    file: relative_file(root, Path::new(path))?,
                    enabled: true,
                    weight: default_weight(),
                    loop_range: None,
                })
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
        states.insert(
            state.to_owned(),
            StateVideoConfig {
                selection: SelectionMode::WeightedRandom,
                transition_policy: TransitionPolicy::Immediate,
                clips,
            },
        );
    }
    Ok(VideoConfig {
        schema_version: VIDEO_CONFIG_SCHEMA_VERSION,
        states,
    })
}

fn build_loaded(
    root: PathBuf,
    config_path: PathBuf,
    source: VideoConfigSource,
    config: VideoConfig,
) -> Result<LoadedVideoConfig, Box<dyn std::error::Error>> {
    if config.schema_version != VIDEO_CONFIG_SCHEMA_VERSION {
        return Err(format!(
            "unsupported video config schema {}; expected {}",
            config.schema_version, VIDEO_CONFIG_SCHEMA_VERSION
        )
        .into());
    }
    let unknown = config
        .states
        .keys()
        .filter(|state| !is_valid_state(state))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(format!("video config has unknown states: {}", unknown.join(", ")).into());
    }
    let missing = VALID_STATES
        .iter()
        .filter(|state| !config.states.contains_key(**state))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!("video config is missing states: {}", missing.join(", ")).into());
    }

    let canonical_root = root.canonicalize()?;
    let mut resolved_paths = BTreeMap::new();
    for state in VALID_STATES {
        let state_config = config
            .states
            .get(state)
            .expect("all states were checked above");
        if !state_config.clips.iter().any(|clip| clip.enabled) {
            return Err(format!("state {state} has no enabled video").into());
        }
        let mut duplicates = HashSet::new();
        let mut paths = Vec::with_capacity(state_config.clips.len());
        for clip in &state_config.clips {
            validate_clip(state, clip)?;
            let duplicate_key = clip.file.to_ascii_lowercase();
            if !duplicates.insert(duplicate_key) {
                return Err(
                    format!("state {state} contains duplicate video: {}", clip.file).into(),
                );
            }
            let resolved = canonical_root.join(Path::new(&clip.file)).canonicalize()?;
            if !resolved.starts_with(&canonical_root) {
                return Err(
                    format!("video path escapes the configured root: {}", clip.file).into(),
                );
            }
            if !resolved.is_file() || !is_supported_video(&resolved) {
                return Err(format!("video is missing or unsupported: {}", clip.file).into());
            }
            paths.push(resolved.to_string_lossy().into_owned());
        }
        resolved_paths.insert(state.to_owned(), paths);
    }
    let fingerprint = config_fingerprint(&config);
    Ok(LoadedVideoConfig {
        root: canonical_root,
        config_path,
        source,
        fingerprint,
        config,
        resolved_paths,
    })
}

fn validate_clip(state: &str, clip: &VideoClipConfig) -> Result<(), Box<dyn std::error::Error>> {
    if clip.file.trim().is_empty() {
        return Err(format!("state {state} contains an empty video path").into());
    }
    if clip.weight == 0 || clip.weight > MAX_WEIGHT {
        return Err(format!(
            "video {} has invalid weight {}; expected 1..={MAX_WEIGHT}",
            clip.file, clip.weight
        )
        .into());
    }
    if let Some(loop_range) = &clip.loop_range {
        if loop_range.start_ms >= loop_range.end_ms || loop_range.end_ms > MAX_LOOP_POINT_MS {
            return Err(format!(
                "video {} has invalid loop range {}..{} ms",
                clip.file, loop_range.start_ms, loop_range.end_ms
            )
            .into());
        }
    }
    validate_relative_file(&clip.file)
}

fn validate_relative_file(file: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(file);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("video path must be a normalized relative path: {file}").into());
    }
    if !is_supported_video(path) {
        return Err(format!("unsupported video extension: {file}").into());
    }
    Ok(())
}

fn normalize_config_paths(config: &mut VideoConfig) -> Result<(), Box<dyn std::error::Error>> {
    for state in config.states.values_mut() {
        for clip in &mut state.clips {
            clip.file = clip.file.replace('\\', "/");
            validate_relative_file(&clip.file)?;
        }
    }
    Ok(())
}

fn relative_file(root: &Path, path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let path = path.canonicalize()?;
    if !path.starts_with(root) {
        return Err(format!(
            "video path is outside {}: {}",
            root.display(),
            path.display()
        )
        .into());
    }
    let relative = path.strip_prefix(root)?;
    let value = relative.to_string_lossy().replace('\\', "/");
    validate_relative_file(&value)?;
    Ok(value)
}

fn unique_destination(directory: &Path, file_name: &str) -> PathBuf {
    let original = Path::new(file_name);
    let stem = original
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("video");
    let extension = original
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("mp4");
    let first = directory.join(file_name);
    if !first.exists() {
        return first;
    }
    for suffix in 2..10_000 {
        let candidate = directory.join(format!("{stem}-{suffix}.{extension}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    directory.join(format!("{stem}-{}.{}", std::process::id(), extension))
}

fn copy_replace(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let temp = destination.with_extension(format!(
        "{}.tmp-{}",
        destination
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("video"),
        std::process::id()
    ));
    fs::copy(source, &temp)?;
    if destination.exists() {
        fs::remove_file(destination)?;
    }
    fs::rename(temp, destination)?;
    Ok(())
}

fn write_replace(path: &Path, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    let temp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let backup = path.with_extension("json.bak");
    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    if path.exists() {
        fs::rename(path, &backup)?;
    }
    if let Err(error) = fs::rename(&temp, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        return Err(error.into());
    }
    if backup.exists() {
        fs::remove_file(backup)?;
    }
    Ok(())
}

fn default_enabled() -> bool {
    true
}

fn default_weight() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::now_ms;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> (PathBuf, VideoConfig) {
        let root = std::env::temp_dir().join(format!(
            "codex-pet-video-config-{}-{}",
            std::process::id(),
            now_ms().saturating_add(FIXTURE_ID.fetch_add(1, Ordering::Relaxed))
        ));
        fs::create_dir_all(&root).unwrap();
        let mut states = BTreeMap::new();
        for state in VALID_STATES {
            let file = format!("{state}.mp4");
            fs::write(root.join(&file), b"video").unwrap();
            states.insert(
                state.to_owned(),
                StateVideoConfig {
                    selection: SelectionMode::WeightedRandom,
                    transition_policy: TransitionPolicy::Immediate,
                    clips: vec![VideoClipConfig {
                        file,
                        enabled: true,
                        weight: 1,
                        loop_range: None,
                    }],
                },
            );
        }
        (
            root,
            VideoConfig {
                schema_version: VIDEO_CONFIG_SCHEMA_VERSION,
                states,
            },
        )
    }

    #[test]
    fn state_schema_is_derived_from_runtime_states() {
        let schema = state_schema();
        assert_eq!(schema.states, VALID_STATES);
        assert_eq!(schema.driver_version, official_pet::DRIVER_VERSION);
    }

    #[test]
    fn config_roundtrip_is_validated_and_bom_free() {
        let (root, config) = fixture();
        let saved = save_video_config_at(&root, config).unwrap();
        assert_eq!(saved.source, VideoConfigSource::Configured);
        let bytes = fs::read(root.join(VIDEO_CONFIG_FILENAME)).unwrap();
        assert!(!bytes.starts_with(&[0xef, 0xbb, 0xbf]));
        assert_eq!(
            load_video_config_at(&root).unwrap().fingerprint,
            saved.fingerprint
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_parent_paths_and_invalid_loop_ranges() {
        let (root, mut config) = fixture();
        config.states.get_mut("idle").unwrap().clips[0].file = "../idle.mp4".to_owned();
        assert!(save_video_config_at(&root, config).is_err());

        let (other_root, mut config) = fixture();
        config.states.get_mut("idle").unwrap().clips[0].loop_range = Some(VideoLoop {
            start_ms: 1000,
            end_ms: 1000,
        });
        assert!(build_loaded(
            root.clone(),
            root.join(VIDEO_CONFIG_FILENAME),
            VideoConfigSource::Configured,
            config
        )
        .is_err());
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(other_root).unwrap();
    }

    #[test]
    fn weighted_selection_avoids_the_previous_clip() {
        let state = StateVideoConfig {
            selection: SelectionMode::WeightedRandom,
            transition_policy: TransitionPolicy::Immediate,
            clips: vec![
                VideoClipConfig {
                    file: "a.mp4".to_owned(),
                    enabled: true,
                    weight: 100,
                    loop_range: None,
                },
                VideoClipConfig {
                    file: "b.mp4".to_owned(),
                    enabled: true,
                    weight: 1,
                    loop_range: None,
                },
            ],
        };
        assert_eq!(select_clip(&state, Some("a.mp4"), 0).unwrap().file, "b.mp4");
        assert_eq!(select_clip(&state, Some("b.mp4"), 0).unwrap().file, "a.mp4");
    }

    #[test]
    fn checked_in_config_matches_the_runtime_state_schema() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("VideoSource")
            .join("V1");
        let loaded = load_video_config_at(&root).unwrap();
        assert_eq!(loaded.source, VideoConfigSource::Configured);
        assert_eq!(loaded.config.states.len(), VALID_STATES.len());
        assert!(VALID_STATES
            .iter()
            .all(|state| loaded.config.states.contains_key(*state)));
        assert_eq!(
            loaded.config.states["jumping"].transition_policy,
            TransitionPolicy::FinishCurrentClip
        );
        assert_eq!(
            loaded.config.states["waving"].transition_policy,
            TransitionPolicy::FinishCurrentClip
        );
        assert_eq!(
            loaded.config.states["idle"].transition_policy,
            TransitionPolicy::Immediate
        );
    }
}
