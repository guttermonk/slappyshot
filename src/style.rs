use egui::Color32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub color: Color,
    pub size: Size,
    pub fill: bool,
    pub annotation_size_factor: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: Color::orange(),
            size: Size::Medium,
            fill: false,
            annotation_size_factor: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Default for Color {
    fn default() -> Self {
        Color::orange()
    }
}

impl Color {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub fn orange() -> Self {
        Self::new(240, 147, 43, 255)
    }
    pub fn to_egui(self) -> Color32 {
        Color32::from_rgba_unmultiplied(self.r, self.g, self.b, self.a)
    }

    /// Parse `#rrggbb` or `#rrggbbaa`, as the palette and theme settings are
    /// written. Used for the remembered colour in state.toml, which is stored
    /// in the same spelling the user would write in config.toml.
    pub fn from_hex(s: &str) -> Option<Self> {
        // to_srgba_unmultiplied and not to_array: Color32 stores its channels
        // premultiplied, so reading them back raw darkens anything with an
        // alpha below 255.
        let [r, g, b, a] = crate::config::parse_hex_color(s)?.to_srgba_unmultiplied();
        Some(Self::new(r, g, b, a))
    }

    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }
}

#[derive(
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Clone,
    Copy,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Small = 0,
    #[default]
    Medium = 1,
    Large = 2,
}

impl Size {
    pub fn label(self) -> &'static str {
        match self {
            Size::Small => "Small",
            Size::Medium => "Medium",
            Size::Large => "Large",
        }
    }

    pub fn to_text_size(self, size_factor: f32) -> i32 {
        match self {
            Size::Small => (16.0 * size_factor) as i32,
            Size::Medium => (24.0 * size_factor) as i32,
            Size::Large => (40.0 * size_factor) as i32,
        }
    }

    pub fn to_marker_size(self, size_factor: f32) -> f32 {
        match self {
            Size::Small => 18.0 * size_factor,
            Size::Medium => 27.0 * size_factor,
            Size::Large => 48.0 * size_factor,
        }
    }

    pub fn to_line_width(self, size_factor: f32) -> f32 {
        match self {
            Size::Small => 3.0 * size_factor,
            Size::Medium => 5.0 * size_factor,
            Size::Large => 7.0 * size_factor,
        }
    }

    pub fn to_arrow_head_length(self, size_factor: f32) -> f32 {
        match self {
            Size::Small => 15.0 * size_factor,
            Size::Medium => 30.0 * size_factor,
            Size::Large => 60.0 * size_factor,
        }
    }

    pub fn to_blur_factor(self, size_factor: f32) -> f32 {
        match self {
            Size::Small => 10.0 * size_factor,
            Size::Medium => 20.0 * size_factor,
            Size::Large => 30.0 * size_factor,
        }
    }

    pub fn to_highlight_width(self, size_factor: f32) -> f32 {
        match self {
            Size::Small => 15.0 * size_factor,
            Size::Medium => 30.0 * size_factor,
            Size::Large => 45.0 * size_factor,
        }
    }
}
