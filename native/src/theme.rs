use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Component, Path},
};

pub const UI_PALETTE_FILENAME: &str = "ui-palette.json";
const UI_PALETTE_SCHEMA_VERSION: u32 = 1;
const UI_PALETTE_ALGORITHM: &str = "oklab-kmeans-v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct UiColor {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl UiColor {
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct UiTheme {
    pub base: UiColor,
    pub accent: UiColor,
    pub light: UiColor,
}

impl Default for UiTheme {
    fn default() -> Self {
        Self {
            base: UiColor::new(9, 9, 17),
            accent: UiColor::new(110, 72, 221),
            light: UiColor::new(240, 247, 251),
        }
    }
}

#[derive(Clone, Debug)]
pub struct LoadedUiTheme {
    pub theme: UiTheme,
    pub source_file: String,
    pub source_sha256: String,
    pub fingerprint: String,
}

impl LoadedUiTheme {
    pub fn fallback() -> Self {
        Self {
            theme: UiTheme::default(),
            source_file: "fallback".to_owned(),
            source_sha256: String::new(),
            fingerprint: "fallback-oklab-kmeans-v1".to_owned(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteFile {
    schema_version: u32,
    algorithm: String,
    source: PaletteSource,
    colors: PaletteColors,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteSource {
    file: String,
    sha256: String,
    frame_index: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaletteColors {
    base: String,
    accent: String,
    light: String,
}

pub fn load_ui_theme_at(root: &Path) -> Result<LoadedUiTheme, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("ui theme root unavailable: {error}"))?;
    let palette_path = root.join(UI_PALETTE_FILENAME);
    let bytes = fs::read(&palette_path)
        .map_err(|error| format!("{} unavailable: {error}", UI_PALETTE_FILENAME))?;
    let bytes = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
    let palette: PaletteFile = serde_json::from_slice(bytes)
        .map_err(|error| format!("{} is invalid: {error}", UI_PALETTE_FILENAME))?;
    validate_source(&root, &palette.source)?;
    if palette.schema_version != UI_PALETTE_SCHEMA_VERSION {
        return Err("unsupported ui palette schema version".to_owned());
    }
    if palette.algorithm != UI_PALETTE_ALGORITHM {
        return Err("unsupported ui palette algorithm".to_owned());
    }
    if palette.source.frame_index != 0 {
        return Err("ui palette must use idle frame zero".to_owned());
    }

    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(LoadedUiTheme {
        theme: UiTheme {
            base: parse_hex_color(&palette.colors.base)?,
            accent: parse_hex_color(&palette.colors.accent)?,
            light: parse_hex_color(&palette.colors.light)?,
        },
        source_file: palette.source.file,
        source_sha256: palette.source.sha256,
        fingerprint: format!("{:016x}", hasher.finish()),
    })
}

fn validate_source(root: &Path, source: &PaletteSource) -> Result<(), String> {
    let relative = Path::new(&source.file);
    if relative.is_absolute()
        || relative.components().next().is_none()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("ui palette source path is invalid".to_owned());
    }
    let source_path = root.join(relative);
    if !source_path.is_file() {
        return Err("ui palette source video is missing".to_owned());
    }
    if source.sha256.len() != 64 || !source.sha256.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err("ui palette source hash is invalid".to_owned());
    }
    Ok(())
}

fn parse_hex_color(value: &str) -> Result<UiColor, String> {
    let bytes = value.as_bytes();
    if bytes.len() != 7 || bytes[0] != b'#' {
        return Err("ui palette color must use #RRGGBB".to_owned());
    }
    let parse = |start| {
        u8::from_str_radix(&value[start..start + 2], 16)
            .map_err(|_| "ui palette color must use #RRGGBB".to_owned())
    };
    Ok(UiColor::new(parse(1)?, parse(3)?, parse(5)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bounded_hex_colors() {
        assert_eq!(parse_hex_color("#090911").unwrap(), UiColor::new(9, 9, 17));
        assert!(parse_hex_color("090911").is_err());
        assert!(parse_hex_color("#xyzxyz").is_err());
    }

    #[test]
    fn fallback_matches_the_checked_in_idle_visual_direction() {
        let theme = UiTheme::default();
        assert_eq!(theme.base, UiColor::new(9, 9, 17));
        assert_eq!(theme.accent, UiColor::new(110, 72, 221));
        assert_eq!(theme.light, UiColor::new(240, 247, 251));
    }

    #[test]
    fn checked_in_palette_matches_an_existing_idle_source() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("VideoSource")
            .join("V1");
        let loaded = load_ui_theme_at(&root).unwrap();
        assert_eq!(loaded.source_file, "idle.mp4");
        assert_ne!(loaded.theme.accent, loaded.theme.base);
        assert_eq!(loaded.source_sha256.len(), 64);
    }
}
