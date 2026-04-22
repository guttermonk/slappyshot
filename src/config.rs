use std::collections::HashMap;
use serde::{Deserialize, Serialize};
use egui::Color32;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub color_palette: ColorPaletteConfig,
    #[serde(default)]
    pub keybinds: KeybindsConfig,
    #[serde(default)]
    pub font: FontConfig,
    #[serde(default)]
    pub theme: ThemeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default)]
    pub fullscreen: bool,
    #[serde(default)]
    pub floating_hack: bool,
    #[serde(default)]
    pub early_exit: bool,
    #[serde(default = "default_corner_roundness")]
    pub corner_roundness: f32,
    #[serde(default = "default_initial_tool")]
    pub initial_tool: String,
    #[serde(default)]
    pub copy_command: Option<String>,
    #[serde(default = "default_annotation_size_factor")]
    pub annotation_size_factor: f32,
    #[serde(default)]
    pub save_after_copy: bool,
    #[serde(default)]
    pub auto_copy: bool,
    #[serde(default)]
    pub output_filename: Option<String>,
    #[serde(default = "default_actions_on_enter")]
    pub actions_on_enter: Vec<Action>,
    #[serde(default = "default_actions_on_escape")]
    pub actions_on_escape: Vec<Action>,
    #[serde(default)]
    pub actions_on_right_click: Vec<Action>,
    #[serde(default)]
    pub default_hide_toolbars: bool,
    #[serde(default)]
    pub default_fill_shapes: bool,
    #[serde(default = "default_primary_highlighter")]
    pub primary_highlighter: String,
    #[serde(default = "default_brush_smooth_history_size")]
    pub brush_smooth_history_size: usize,
    #[serde(default = "default_zoom_factor")]
    pub zoom_factor: f32,
    #[serde(default = "default_pan_step_size")]
    pub pan_step_size: f32,
    #[serde(default)]
    pub no_window_decoration: bool,
    #[serde(default)]
    pub disable_notifications: bool,
}

fn default_corner_roundness() -> f32 {
    12.0
}
fn default_initial_tool() -> String {
    "arrow".to_string()
}
fn default_annotation_size_factor() -> f32 {
    1.0
}
fn default_actions_on_enter() -> Vec<Action> {
    vec![Action::SaveToClipboard, Action::Exit]
}
fn default_actions_on_escape() -> Vec<Action> {
    vec![Action::Exit]
}
fn default_primary_highlighter() -> String {
    "block".to_string()
}
fn default_brush_smooth_history_size() -> usize {
    5
}
fn default_zoom_factor() -> f32 {
    1.1
}
fn default_pan_step_size() -> f32 {
    50.0
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            fullscreen: false,
            floating_hack: false,
            early_exit: false,
            corner_roundness: default_corner_roundness(),
            initial_tool: default_initial_tool(),
            copy_command: None,
            annotation_size_factor: default_annotation_size_factor(),
            save_after_copy: false,
            auto_copy: false,
            output_filename: None,
            actions_on_enter: default_actions_on_enter(),
            actions_on_escape: default_actions_on_escape(),
            actions_on_right_click: vec![],
            default_hide_toolbars: false,
            default_fill_shapes: false,
            primary_highlighter: default_primary_highlighter(),
            brush_smooth_history_size: default_brush_smooth_history_size(),
            zoom_factor: default_zoom_factor(),
            pan_step_size: default_pan_step_size(),
            no_window_decoration: false,
            disable_notifications: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ColorPaletteConfig {
    #[serde(default)]
    pub palette: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KeybindsConfig {
    pub pointer: Option<String>,
    pub crop: Option<String>,
    pub brush: Option<String>,
    pub line: Option<String>,
    pub arrow: Option<String>,
    pub rectangle: Option<String>,
    pub ellipse: Option<String>,
    pub text: Option<String>,
    pub marker: Option<String>,
    pub blur: Option<String>,
    pub highlight: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FontConfig {
    pub family: Option<String>,
    pub style: Option<String>,
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThemeConfig {
    pub panel_fill: Option<String>,
    pub text_color: Option<String>,
    pub button_fill: Option<String>,
    pub button_active: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    SaveToClipboard,
    SaveToFile,
    SaveToFileAs,
    CopyFilepathToClipboard,
    Exit,
}

impl Config {
    pub fn load(config_path: Option<&str>) -> Config {
        let path = if let Some(p) = config_path {
            std::path::PathBuf::from(p)
        } else {
            let xdg_dirs = xdg::BaseDirectories::with_prefix("slappyshot");
            xdg_dirs
                .find_config_file("config.toml")
                .unwrap_or_else(|| {
                    let home = std::env::var("HOME").unwrap_or_default();
                    std::path::PathBuf::from(format!(
                        "{}/.config/slappyshot/config.toml",
                        home
                    ))
                })
        };

        if path.exists() {
            match std::fs::read_to_string(&path) {
                Ok(content) => match toml::from_str::<Config>(&content) {
                    Ok(cfg) => return cfg,
                    Err(e) => {
                        eprintln!("Warning: failed to parse config: {}", e);
                    }
                },
                Err(e) => {
                    eprintln!("Warning: failed to read config: {}", e);
                }
            }
        }

        Config::default()
    }

    pub fn keybind_map(&self) -> HashMap<char, String> {
        let mut map = HashMap::new();
        // defaults
        let defaults: &[(&str, char)] = &[
            ("pointer", 'p'),
            ("crop", 'c'),
            ("brush", 'b'),
            ("line", 'i'),
            ("arrow", 'z'),
            ("rectangle", 'r'),
            ("ellipse", 'e'),
            ("text", 't'),
            ("marker", 'm'),
            ("blur", 'u'),
            ("highlight", 'g'),
        ];
        for (tool, default_key) in defaults {
            let key = match *tool {
                "pointer" => self
                    .keybinds
                    .pointer
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "crop" => self
                    .keybinds
                    .crop
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "brush" => self
                    .keybinds
                    .brush
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "line" => self
                    .keybinds
                    .line
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "arrow" => self
                    .keybinds
                    .arrow
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "rectangle" => self
                    .keybinds
                    .rectangle
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "ellipse" => self
                    .keybinds
                    .ellipse
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "text" => self
                    .keybinds
                    .text
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "marker" => self
                    .keybinds
                    .marker
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "blur" => self
                    .keybinds
                    .blur
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                "highlight" => self
                    .keybinds
                    .highlight
                    .as_ref()
                    .and_then(|s| s.chars().next())
                    .unwrap_or(*default_key),
                _ => *default_key,
            };
            map.insert(key, tool.to_string());
        }
        map
    }

    pub fn palette_colors(&self) -> Vec<(u8, u8, u8, u8)> {
        if !self.color_palette.palette.is_empty() {
            let mut result = Vec::new();
            for hex in &self.color_palette.palette {
                if let Some(c) = parse_hex_color(hex) {
                    let arr = c.to_array();
                    result.push((arr[0], arr[1], arr[2], arr[3]));
                }
            }
            if !result.is_empty() {
                return result;
            }
        }
        // default palette
        vec![
            (240, 147, 43, 255), // orange
            (235, 77, 75, 255),  // red
            (106, 176, 76, 255), // green
            (34, 166, 179, 255), // blue
            (19, 15, 64, 255),   // cove
            (200, 37, 184, 255), // pink
        ]
    }
}

pub fn parse_hex_color(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some(Color32::from_rgb(r, g, b))
        }
        8 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            let a = u8::from_str_radix(&s[6..8], 16).ok()?;
            Some(Color32::from_rgba_unmultiplied(r, g, b, a))
        }
        _ => None,
    }
}

pub fn apply_theme(ctx: &egui::Context, theme: &ThemeConfig) {
    let mut visuals = egui::Visuals::dark();

    if let Some(ref hex) = theme.panel_fill {
        if let Some(color) = parse_hex_color(hex) {
            visuals.panel_fill = color;
            visuals.window_fill = color;
        }
    }
    if let Some(ref hex) = theme.text_color {
        if let Some(color) = parse_hex_color(hex) {
            visuals.override_text_color = Some(color);
        }
    }
    if let Some(ref hex) = theme.button_fill {
        if let Some(color) = parse_hex_color(hex) {
            visuals.widgets.inactive.weak_bg_fill = color;
            visuals.widgets.inactive.bg_fill = color;
        }
    }
    if let Some(ref hex) = theme.button_active {
        if let Some(color) = parse_hex_color(hex) {
            visuals.widgets.active.bg_fill = color;
            visuals.widgets.active.weak_bg_fill = color;
            visuals.selection.bg_fill = color;
        }
    }

    ctx.set_visuals(visuals);
}
