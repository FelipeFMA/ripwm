use std::{collections::HashMap, fs, path::PathBuf};

use image::{DynamicImage, imageops::FilterType};
use serde::Deserialize;
use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            ImportMem, Renderer,
            element::{
                Kind,
                memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement},
            },
        },
    },
    utils::{Physical, Size, Transform},
};

const DEFAULT_CONFIG: &str = r##"# ripwm configuration
#
# Set a wallpaper image:
# wallpaper = ~/Pictures/Wallpaper.png
#
# Or disable the wallpaper:
# wallpaper = off
wallpaper = off

# Border colors in #RRGGBB or #RRGGBBAA format
active_border_color = "#4c7899"
inactive_border_color = "#2f343a"
"##;

#[derive(Debug, Clone)]
pub enum WallpaperSetting {
    Off,
    Path(PathBuf),
}

#[derive(Debug, Clone)]
pub struct RipwmConfig {
    pub wallpaper: WallpaperSetting,
    pub active_border_color: [f32; 4],
    pub inactive_border_color: [f32; 4],
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default = "default_wallpaper")]
    wallpaper: String,
    #[serde(default = "default_active_border_color")]
    active_border_color: String,
    #[serde(default = "default_inactive_border_color")]
    inactive_border_color: String,
}

impl Default for RawConfig {
    fn default() -> Self {
        Self {
            wallpaper: default_wallpaper(),
            active_border_color: default_active_border_color(),
            inactive_border_color: default_inactive_border_color(),
        }
    }
}

fn default_wallpaper() -> String {
    String::from("off")
}

fn default_active_border_color() -> String {
    String::from("#4c7899")
}

fn default_inactive_border_color() -> String {
    String::from("#2f343a")
}

pub fn load_or_create_config() -> RipwmConfig {
    let config_path = config_path();

    if let Some(parent) = config_path.parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        tracing::warn!("Failed to create config directory {}: {err}", parent.display());
    }

    if !config_path.exists()
        && let Err(err) = fs::write(&config_path, DEFAULT_CONFIG)
    {
        tracing::warn!("Failed to write default config {}: {err}", config_path.display());
    }

    let raw = match fs::read_to_string(&config_path) {
        Ok(contents) => {
            let normalized = normalize_wallpaper_values(&contents);
            toml::from_str::<RawConfig>(&normalized).unwrap_or_else(|err| {
                tracing::warn!("Invalid config at {}: {err}", config_path.display());
                RawConfig::default()
            })
        }
        Err(err) => {
            tracing::warn!("Failed to read config {}: {err}", config_path.display());
            RawConfig::default()
        }
    };

    let wallpaper = if raw.wallpaper.trim().eq_ignore_ascii_case("off") {
        WallpaperSetting::Off
    } else {
        WallpaperSetting::Path(expand_home(raw.wallpaper.trim()))
    };

    let active_border_color = parse_color_or_default(
        raw.active_border_color.trim(),
        [0.298_039_23, 0.470_588_24, 0.6, 1.0],
        "active_border_color",
    );
    let inactive_border_color = parse_color_or_default(
        raw.inactive_border_color.trim(),
        [0.184_313_73, 0.203_921_57, 0.227_450_98, 1.0],
        "inactive_border_color",
    );

    RipwmConfig { wallpaper, active_border_color, inactive_border_color }
}

pub(crate) fn config_path() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".config/ripwm/ripwm.toml"),
        None => PathBuf::from(".config/ripwm/ripwm.toml"),
    }
}

fn normalize_wallpaper_values(contents: &str) -> String {
    contents
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("wallpaper") {
                return line.to_string();
            }

            let Some((lhs, rhs)) = line.split_once('=') else {
                return line.to_string();
            };

            if lhs.trim() != "wallpaper" {
                return line.to_string();
            }

            let value = rhs.trim();
            if value.is_empty() || value.starts_with('"') || value.starts_with('\'') {
                return line.to_string();
            }

            format!("wallpaper = \"{}\"", value.replace('"', "\\\""))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn expand_home(raw: &str) -> PathBuf {
    if raw == "~"
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home);
    }

    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }

    PathBuf::from(raw)
}

fn parse_color_or_default(raw: &str, default: [f32; 4], key: &str) -> [f32; 4] {
    match parse_hex_color(raw) {
        Some(color) => color,
        None => {
            tracing::warn!("Invalid color for {key}: {raw}. Falling back to default");
            default
        }
    }
}

fn parse_hex_color(raw: &str) -> Option<[f32; 4]> {
    let value = raw.strip_prefix('#').unwrap_or(raw);

    match value.len() {
        6 => {
            let r = u8::from_str_radix(&value[0..2], 16).ok()?;
            let g = u8::from_str_radix(&value[2..4], 16).ok()?;
            let b = u8::from_str_radix(&value[4..6], 16).ok()?;
            Some([f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0, 1.0])
        }
        8 => {
            let r = u8::from_str_radix(&value[0..2], 16).ok()?;
            let g = u8::from_str_radix(&value[2..4], 16).ok()?;
            let b = u8::from_str_radix(&value[4..6], 16).ok()?;
            let a = u8::from_str_radix(&value[6..8], 16).ok()?;
            Some([
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
                f32::from(a) / 255.0,
            ])
        }
        _ => None,
    }
}

enum WallpaperSource {
    Off,
    Image(DynamicImage),
}

pub struct WallpaperState {
    source: WallpaperSource,
    cached_by_size: HashMap<(i32, i32), MemoryRenderBuffer>,
}

impl WallpaperState {
    pub fn from_config(config: &RipwmConfig) -> Self {
        let source = match &config.wallpaper {
            WallpaperSetting::Off => WallpaperSource::Off,
            WallpaperSetting::Path(path) => match image::open(path) {
                Ok(image) => WallpaperSource::Image(image),
                Err(err) => {
                    tracing::warn!("Failed to load wallpaper {}: {err}", path.display());
                    WallpaperSource::Off
                }
            },
        };

        Self { source, cached_by_size: HashMap::new() }
    }

    pub fn render_element<R>(
        &mut self,
        renderer: &mut R,
        size: Size<i32, Physical>,
    ) -> Option<MemoryRenderBufferRenderElement<R>>
    where
        R: Renderer + ImportMem,
        R::TextureId: Send + Clone + 'static,
    {
        if size.w <= 0 || size.h <= 0 {
            return None;
        }

        let key = (size.w, size.h);
        if !self.cached_by_size.contains_key(&key) {
            let buffer = self.create_buffer(size)?;
            self.cached_by_size.insert(key, buffer);
        }

        let buffer = self.cached_by_size.get(&key)?;

        MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            (0.0, 0.0),
            buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        )
        .ok()
    }

    fn create_buffer(&self, size: Size<i32, Physical>) -> Option<MemoryRenderBuffer> {
        let WallpaperSource::Image(image) = &self.source else {
            return None;
        };

        let width = u32::try_from(size.w).ok()?;
        let height = u32::try_from(size.h).ok()?;

        let resized = image.resize_to_fill(width, height, FilterType::Lanczos3).to_rgba8();

        Some(MemoryRenderBuffer::from_slice(
            resized.as_raw(),
            Fourcc::Abgr8888,
            (size.w, size.h),
            1,
            Transform::Normal,
            None,
        ))
    }
}
