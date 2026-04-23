use crate::style::Style;
use egui::{Pos2, Vec2};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Pointer,
    Crop,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
    Marker,
    Blur,
    Highlight,
    Brush,
    Delete,
}

impl ToolType {
    pub fn label(&self) -> &str {
        match self {
            ToolType::Pointer => "Pointer",
            ToolType::Crop => "Crop",
            ToolType::Line => "Line",
            ToolType::Arrow => "Arrow",
            ToolType::Rectangle => "Rect",
            ToolType::Ellipse => "Ellipse",
            ToolType::Text => "Text",
            ToolType::Marker => "Marker",
            ToolType::Blur => "Blur",
            ToolType::Highlight => "Highlight",
            ToolType::Brush => "Brush",
            ToolType::Delete => "Delete",
        }
    }

    pub fn config_name(&self) -> &str {
        match self {
            ToolType::Pointer => "pointer",
            ToolType::Crop => "crop",
            ToolType::Brush => "brush",
            ToolType::Line => "line",
            ToolType::Arrow => "arrow",
            ToolType::Rectangle => "rectangle",
            ToolType::Ellipse => "ellipse",
            ToolType::Text => "text",
            ToolType::Marker => "marker",
            ToolType::Blur => "blur",
            ToolType::Highlight => "highlight",
            ToolType::Delete => "delete",
        }
    }

    pub fn icon(&self) -> &str {
        match self {
            ToolType::Pointer => "↖",
            ToolType::Crop => "✂",
            ToolType::Brush => "✏",
            ToolType::Line => "∕",
            ToolType::Arrow => "↗",
            ToolType::Rectangle => "□",
            ToolType::Ellipse => "⬭",
            ToolType::Text => "T",
            ToolType::Marker => "#",
            ToolType::Blur => "≈",
            ToolType::Highlight => "H",
            ToolType::Delete => "🗑",
        }
    }

}

impl Default for ToolType {
    fn default() -> Self {
        ToolType::Arrow
    }
}

pub fn tool_from_string(s: &str) -> Option<ToolType> {
    match s.to_lowercase().as_str() {
        "pointer" => Some(ToolType::Pointer),
        "crop" => Some(ToolType::Crop),
        "line" => Some(ToolType::Line),
        "arrow" => Some(ToolType::Arrow),
        "rectangle" | "rect" => Some(ToolType::Rectangle),
        "ellipse" => Some(ToolType::Ellipse),
        "text" => Some(ToolType::Text),
        "marker" => Some(ToolType::Marker),
        "blur" => Some(ToolType::Blur),
        "highlight" => Some(ToolType::Highlight),
        "brush" => Some(ToolType::Brush),
        "delete" => Some(ToolType::Delete),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HighlighterKind {
    Block,
    Freehand,
}

impl Default for HighlighterKind {
    fn default() -> Self {
        HighlighterKind::Block
    }
}

#[derive(Debug, Clone)]
pub enum HighlightAnnotation {
    Block { top_left: Pos2, size: Vec2 },
    Freehand { start: Pos2, points: Vec<Vec2> },
}

pub enum Annotation {
    Arrow {
        start: Pos2,
        end: Pos2,
        style: Style,
    },
    Line {
        start: Pos2,
        end: Pos2,
        style: Style,
    },
    Rectangle {
        top_left: Pos2,
        size: Vec2,
        style: Style,
        corner_roundness: f32,
    },
    Ellipse {
        center: Pos2,
        radii: Vec2,
        style: Style,
    },
    Brush {
        start: Pos2,
        points: Vec<Vec2>,
        style: Style,
    },
    Text {
        pos: Pos2,
        content: String,
        style: Style,
    },
    Marker {
        pos: Pos2,
        number: u16,
        style: Style,
    },
    Blur {
        top_left: Pos2,
        size: Vec2,
        blurred_pixels: Arc<Vec<u8>>,
        blur_w: u32,
        blur_h: u32,
        texture: Option<egui::TextureHandle>,
    },
    Highlight {
        kind: HighlightAnnotation,
        style: Style,
    },
}

impl Clone for Annotation {
    fn clone(&self) -> Self {
        match self {
            Annotation::Arrow { start, end, style } => Annotation::Arrow {
                start: *start,
                end: *end,
                style: *style,
            },
            Annotation::Line { start, end, style } => Annotation::Line {
                start: *start,
                end: *end,
                style: *style,
            },
            Annotation::Rectangle {
                top_left,
                size,
                style,
                corner_roundness,
            } => Annotation::Rectangle {
                top_left: *top_left,
                size: *size,
                style: *style,
                corner_roundness: *corner_roundness,
            },
            Annotation::Ellipse {
                center,
                radii,
                style,
            } => Annotation::Ellipse {
                center: *center,
                radii: *radii,
                style: *style,
            },
            Annotation::Brush {
                start,
                points,
                style,
            } => Annotation::Brush {
                start: *start,
                points: points.clone(),
                style: *style,
            },
            Annotation::Text {
                pos,
                content,
                style,
            } => Annotation::Text {
                pos: *pos,
                content: content.clone(),
                style: *style,
            },
            Annotation::Marker { pos, number, style } => Annotation::Marker {
                pos: *pos,
                number: *number,
                style: *style,
            },
            Annotation::Blur {
                top_left,
                size,
                blurred_pixels,
                blur_w,
                blur_h,
                ..
            } => Annotation::Blur {
                top_left: *top_left,
                size: *size,
                blurred_pixels: Arc::clone(blurred_pixels),
                blur_w: *blur_w,
                blur_h: *blur_h,
                texture: None, // will re-upload on next draw
            },
            Annotation::Highlight { kind, style } => Annotation::Highlight {
                kind: kind.clone(),
                style: *style,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub enum ActiveDrawing {
    None,
    Arrow {
        start: Pos2,
    },
    Line {
        start: Pos2,
    },
    Rectangle {
        start: Pos2,
        top_left: Pos2,
        size: Vec2,
    },
    Ellipse {
        start: Pos2,
        center: Pos2,
        radii: Vec2,
    },
    Brush {
        start: Pos2,
        points: Vec<Vec2>,
        smoother: Smoother,
    },
    Blur {
        top_left: Pos2,
        size: Vec2,
    },
    Highlight {
        kind: HighlighterKind,
        start: Pos2,
        points: Vec<Vec2>,
        block_size: Vec2,
    },
    Text {
        pos: Pos2,
        content: String,
    },
}

#[derive(Debug, Clone)]
pub struct Smoother {
    history: Vec<Vec2>,
    smoothed_point: Option<Vec2>,
    max_history: usize,
    last_update: Option<Instant>,
}

impl Smoother {
    pub fn new(max_history: usize) -> Self {
        Self {
            history: Vec::with_capacity(max_history + 1),
            smoothed_point: Option::None,
            max_history,
            last_update: Option::None,
        }
    }

    pub fn update(&mut self, raw: Vec2) -> Vec2 {
        if self.max_history == 0 {
            return raw;
        }
        // Add to history
        if self.history.len() >= self.max_history {
            self.history.remove(0);
        }
        self.history.push(raw);

        // Compute averaged raw input
        let n = self.history.len() as f32;
        let sum = self.history.iter().fold(Vec2::ZERO, |acc, p| acc + *p);
        let averaged_raw = sum / n;

        // Estimate speed
        let dt = if let Some(last_update) = self.last_update {
            let now = Instant::now();
            let elapsed = now.duration_since(last_update).as_secs_f32();
            self.last_update = Some(now);
            elapsed
        } else {
            self.last_update = Some(Instant::now());
            0.0
        };
        let last = *self.history.last().unwrap_or(&raw);
        let first = *self.history.first().unwrap_or(&raw);
        let distance = (last - first).length();
        let total_dt = (dt * self.history.len() as f32).clamp(0.001, 1.0);
        let speed = distance / total_dt;

        let alpha = Self::compute_alpha(speed);

        let smoothed = if let Some(prev) = self.smoothed_point {
            Vec2::new(
                alpha * averaged_raw.x + (1.0 - alpha) * prev.x,
                alpha * averaged_raw.y + (1.0 - alpha) * prev.y,
            )
        } else {
            averaged_raw
        };

        self.smoothed_point = Some(smoothed);
        smoothed
    }

    fn compute_alpha(speed: f32) -> f32 {
        let min_alpha = 0.05f32;
        let max_alpha = 0.5f32;
        let clamped_speed = speed.clamp(0.01, 500.0);
        let norm = (clamped_speed / 500.0).sqrt();
        min_alpha + (max_alpha - min_alpha) * norm
    }
}

#[derive(Debug, Clone, Default)]
pub struct CropState {
    pub rect: Option<CropRect>,
}

#[derive(Debug, Clone)]
pub struct CropRect {
    pub pos: Pos2,
    pub size: Vec2,
    pub active: bool,
}
