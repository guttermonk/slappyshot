use egui::{Color32, Pos2, Rect, Rounding, Stroke, Vec2};
use image::RgbaImage;

use crate::config::{Action, Config, apply_theme};
use crate::render::{compute_blur_pixels, copy_to_clipboard, render_to_image};
use crate::style::{Color, Size, Style};
use crate::tools::{
    ActiveDrawing, Annotation, CropRect, CropState, HighlightAnnotation, HighlighterKind, Smoother,
    ToolType, tool_from_string,
};

pub struct App {
    source_image: RgbaImage,
    base_texture: Option<egui::TextureHandle>,
    zoom: f32,
    pan: Vec2,
    canvas_rect: Rect,
    annotations: Vec<Annotation>,
    undo_stack: Vec<Vec<Annotation>>,
    redo_stack: Vec<Vec<Annotation>>,
    active_drawing: ActiveDrawing,
    active_tool: ToolType,
    crop_state: CropState,
    marker_counter: u16,
    style: Style,
    primary_highlighter: HighlighterKind,
    drag_start: Option<Pos2>,
    config: Config,
    toolbars_visible: bool,
    last_saved_path: Option<String>,
    initialized: bool,
    // Deferred actions that need ctx
    pending_copy: bool,
    pending_save: bool,
    top_toolbar_content_width: f32,
    bottom_toolbar_content_width: f32,
    toolbar_min_width_applied: bool,
}

impl App {
    pub fn new(source_image: RgbaImage, config: Config) -> Self {
        let style = Style {
            color: {
                let palette = config.palette_colors();
                let &(r, g, b, a) = palette.first().unwrap_or(&(240, 147, 43, 255));
                Color { r, g, b, a }
            },
            size: Size::Medium,
            fill: config.general.default_fill_shapes,
            annotation_size_factor: config.general.annotation_size_factor,
        };

        let initial_tool = tool_from_string(&config.general.initial_tool).unwrap_or_default();

        let primary_highlighter = if config.general.primary_highlighter == "freehand" {
            HighlighterKind::Freehand
        } else {
            HighlighterKind::Block
        };

        let toolbars_visible = !config.general.default_hide_toolbars;

        Self {
            source_image,
            base_texture: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            canvas_rect: Rect::NOTHING,
            annotations: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            active_drawing: ActiveDrawing::None,
            active_tool: initial_tool,
            crop_state: CropState::default(),
            marker_counter: 0,
            style,
            primary_highlighter,
            drag_start: Option::None,
            config,
            toolbars_visible,
            last_saved_path: None,
            initialized: false,
            pending_copy: false,
            pending_save: false,

            top_toolbar_content_width: 9999.0,
            bottom_toolbar_content_width: 9999.0,
            toolbar_min_width_applied: false,
        }
    }

    fn upload_base_texture(&mut self, ctx: &egui::Context) {
        let color_img = rgba_image_to_egui(&self.source_image);
        self.base_texture = Some(ctx.load_texture("base", color_img, egui::TextureOptions::LINEAR));
    }

    fn img_center(&self) -> Vec2 {
        Vec2::new(
            self.source_image.width() as f32 / 2.0,
            self.source_image.height() as f32 / 2.0,
        )
    }

    fn image_to_screen(&self, p: Pos2) -> Pos2 {
        let img_c = self.img_center();
        let canvas_c = self.canvas_rect.center();
        let offset = (p.to_vec2() - img_c) * self.zoom + self.pan;
        canvas_c + offset
    }

    fn screen_to_image(&self, p: Pos2) -> Pos2 {
        let img_c = self.img_center();
        let canvas_c = self.canvas_rect.center();
        (img_c + (p.to_vec2() - canvas_c.to_vec2() - self.pan) / self.zoom).to_pos2()
    }

    fn image_rect(&self) -> Rect {
        let w = self.source_image.width() as f32 * self.zoom;
        let h = self.source_image.height() as f32 * self.zoom;
        let center = self.canvas_rect.center() + self.pan;
        Rect::from_center_size(center, egui::vec2(w, h))
    }

    fn commit_annotation(&mut self, ann: Annotation) {
        self.undo_stack.push(self.annotations.clone());
        self.redo_stack.clear();
        self.annotations.push(ann);
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.annotations.clone());
            self.annotations = prev;
            self.sync_marker_counter();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.annotations.clone());
            self.annotations = next;
            self.sync_marker_counter();
        }
    }

    fn sync_marker_counter(&mut self) {
        self.marker_counter = self
            .annotations
            .iter()
            .filter_map(|a| {
                if let Annotation::Marker { number, .. } = a {
                    Some(*number)
                } else {
                    None
                }
            })
            .max()
            .map(|n| n + 1)
            .unwrap_or(0);
    }

    fn switch_tool(&mut self, tool: ToolType) {
        // Cancel any active drawing
        self.active_drawing = ActiveDrawing::None;
        self.drag_start = None;
        self.active_tool = tool;
    }

    fn copy_to_clipboard_action(&self) {
        let rendered = render_to_image(
            &self.source_image,
            &self.annotations,
            self.config.general.corner_roundness,
        );
        if let Err(e) = copy_to_clipboard(&rendered, self.config.general.copy_command.as_deref()) {
            eprintln!("Copy to clipboard failed: {}", e);
        }
        if self.config.general.save_after_copy {
            // We can't call save here (needs &mut self), just note it
        }
    }

    fn save_to_file(&mut self) {
        let filename = if let Some(ref tmpl) = self.config.general.output_filename {
            let expanded = shellexpand_tilde(tmpl);
            chrono::Local::now().format(&expanded).to_string()
        } else {
            format!(
                "/tmp/slappyshot-{}.png",
                chrono::Local::now().format("%Y%m%d-%H%M%S")
            )
        };
        let rendered = render_to_image(
            &self.source_image,
            &self.annotations,
            self.config.general.corner_roundness,
        );
        match rendered.save(&filename) {
            Ok(()) => {
                self.last_saved_path = Some(filename);
            }
            Err(e) => {
                eprintln!("Failed to save file: {}", e);
            }
        }
    }

    fn execute_actions(&mut self, actions: Vec<Action>) {
        for action in &actions {
            match action {
                Action::Exit => {
                    std::process::exit(0);
                }
                Action::SaveToFile => {
                    self.pending_save = true;
                }
                Action::SaveToClipboard => {
                    self.pending_copy = true;
                }
                Action::SaveToFileAs => {
                    self.pending_save = true;
                }
                Action::CopyFilepathToClipboard => {
                    // Will be handled with ctx in the main update loop
                }
            }
        }
    }

    fn confirm_crop(&mut self, ctx: &egui::Context) {
        if let Some(crop) = self.crop_state.rect.clone() {
            let (tl, sz) = normalize_rect(crop.pos, crop.size);
            let x = tl.x.round().max(0.0) as u32;
            let y = tl.y.round().max(0.0) as u32;
            let w = (sz.x.round() as u32)
                .min(self.source_image.width().saturating_sub(x))
                .max(1);
            let h = (sz.y.round() as u32)
                .min(self.source_image.height().saturating_sub(y))
                .max(1);

            let cropped = image::imageops::crop_imm(&self.source_image, x, y, w, h).to_image();

            let dx = -(tl.x);
            let dy = -(tl.y);
            for ann in &mut self.annotations {
                shift_annotation(ann, dx, dy);
            }

            self.source_image = cropped;
            let color_img = rgba_image_to_egui(&self.source_image);
            self.base_texture =
                Some(ctx.load_texture("base", color_img, egui::TextureOptions::LINEAR));

            self.crop_state = CropState::default();
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
        }
    }

    fn on_drag_begin(&mut self, img_pos: Pos2, modifiers: egui::Modifiers) {
        match self.active_tool {
            ToolType::Arrow => {
                self.active_drawing = ActiveDrawing::Arrow { start: img_pos };
            }
            ToolType::Line => {
                self.active_drawing = ActiveDrawing::Line { start: img_pos };
            }
            ToolType::Rectangle => {
                self.active_drawing = ActiveDrawing::Rectangle {
                    start: img_pos,
                    top_left: img_pos,
                    size: Vec2::ZERO,
                };
            }
            ToolType::Ellipse => {
                self.active_drawing = ActiveDrawing::Ellipse {
                    start: img_pos,
                    center: img_pos,
                    radii: Vec2::ZERO,
                };
            }
            ToolType::Brush => {
                let max_history = self.config.general.brush_smooth_history_size;
                self.active_drawing = ActiveDrawing::Brush {
                    start: img_pos,
                    points: Vec::new(),
                    smoother: Smoother::new(max_history),
                };
            }
            ToolType::Blur => {
                self.active_drawing = ActiveDrawing::Blur {
                    top_left: img_pos,
                    size: Vec2::ZERO,
                };
            }
            ToolType::Highlight => {
                self.active_drawing = ActiveDrawing::Highlight {
                    kind: self.primary_highlighter,
                    start: img_pos,
                    points: Vec::new(),
                    block_size: Vec2::ZERO,
                };
            }
            ToolType::Crop => {
                self.crop_state.rect = Some(CropRect {
                    pos: img_pos,
                    size: Vec2::ZERO,
                    active: true,
                });
            }
            ToolType::Pointer | ToolType::Text | ToolType::Marker => {}
        }
        let _ = modifiers;
    }

    fn on_drag_update(&mut self, start: Pos2, delta: Pos2, modifiers: egui::Modifiers) {
        let raw_end = start + delta.to_vec2();
        match &mut self.active_drawing {
            ActiveDrawing::Arrow { start: s } => {
                let s = *s;
                let end = if modifiers.shift {
                    snap_to_angle(s, raw_end)
                } else {
                    raw_end
                };
                self.active_drawing = ActiveDrawing::Arrow { start: s };
                // We'll use start + end for preview in draw_active_drawing
                // Store the end in a preview via a different mechanism
                // Actually let's store end in the enum directly
                // We need to redesign slightly to carry end in Arrow preview
                // For now, we'll compute from stored data in draw_active_drawing
                // using drag_start and current pointer
                let _ = end;
            }
            ActiveDrawing::Line { start: s } => {
                let _s = *s;
                let _ = modifiers.shift;
            }
            ActiveDrawing::Rectangle {
                start: s,
                top_left,
                size,
            } => {
                let s = *s;
                let mut raw_size = raw_end - s;
                if modifiers.shift {
                    let side = raw_size.x.abs().max(raw_size.y.abs());
                    raw_size = Vec2::new(side * raw_size.x.signum(), side * raw_size.y.signum());
                }
                *top_left = s;
                *size = raw_size;
            }
            ActiveDrawing::Ellipse {
                start: s,
                center,
                radii,
            } => {
                let s = *s;
                let mut raw_size = raw_end - s;
                if modifiers.shift {
                    let side = raw_size.x.abs().max(raw_size.y.abs());
                    raw_size = Vec2::new(side * raw_size.x.signum(), side * raw_size.y.signum());
                }
                *center = s + raw_size / 2.0;
                *radii = Vec2::new(raw_size.x.abs() / 2.0, raw_size.y.abs() / 2.0);
            }
            ActiveDrawing::Brush {
                start: s,
                points,
                smoother,
            } => {
                let s = *s;
                let raw_delta = raw_end.to_vec2() - s.to_vec2();
                let smoothed = smoother.update(raw_delta);
                points.push(smoothed);
            }
            ActiveDrawing::Blur { top_left, size } => {
                *top_left = start;
                *size = raw_end - start;
            }
            ActiveDrawing::Highlight {
                kind,
                start: s,
                points,
                block_size,
            } => {
                let s = *s;
                match kind {
                    HighlighterKind::Block => {
                        *block_size = raw_end - s;
                    }
                    HighlighterKind::Freehand => {
                        let raw_delta = raw_end.to_vec2() - s.to_vec2();
                        points.push(raw_delta);
                    }
                }
            }
            ActiveDrawing::None | ActiveDrawing::Text { .. } => {}
        }

        // Also update crop
        if self.active_tool == ToolType::Crop {
            if let Some(ref mut crop) = self.crop_state.rect {
                if crop.active {
                    crop.size = raw_end - crop.pos;
                }
            }
        }
    }

    fn on_drag_end(&mut self, start: Pos2, delta: Pos2, modifiers: egui::Modifiers) {
        let raw_end = start + delta.to_vec2();

        match self.active_drawing.clone() {
            ActiveDrawing::Arrow { start: s } => {
                let end = if modifiers.shift {
                    snap_to_angle(s, raw_end)
                } else {
                    raw_end
                };
                let ann = Annotation::Arrow {
                    start: s,
                    end,
                    style: self.style,
                };
                self.commit_annotation(ann);
                self.active_drawing = ActiveDrawing::None;
            }
            ActiveDrawing::Line { start: s } => {
                let end = if modifiers.shift {
                    snap_to_angle(s, raw_end)
                } else {
                    raw_end
                };
                let ann = Annotation::Line {
                    start: s,
                    end,
                    style: self.style,
                };
                self.commit_annotation(ann);
                self.active_drawing = ActiveDrawing::None;
            }
            ActiveDrawing::Rectangle { start: s, .. } => {
                let mut sz = raw_end - s;
                if modifiers.shift {
                    let side = sz.x.abs().max(sz.y.abs());
                    sz = Vec2::new(side * sz.x.signum(), side * sz.y.signum());
                }
                if sz.length() > 1.0 {
                    let ann = Annotation::Rectangle {
                        top_left: s,
                        size: sz,
                        style: self.style,
                        corner_roundness: self.config.general.corner_roundness,
                    };
                    self.commit_annotation(ann);
                }
                self.active_drawing = ActiveDrawing::None;
            }
            ActiveDrawing::Ellipse { start: s, .. } => {
                let mut sz = raw_end - s;
                if modifiers.shift {
                    let side = sz.x.abs().max(sz.y.abs());
                    sz = Vec2::new(side * sz.x.signum(), side * sz.y.signum());
                }
                if sz.length() > 1.0 {
                    let center = s + sz / 2.0;
                    let radii = Vec2::new(sz.x.abs() / 2.0, sz.y.abs() / 2.0);
                    let ann = Annotation::Ellipse {
                        center,
                        radii,
                        style: self.style,
                    };
                    self.commit_annotation(ann);
                }
                self.active_drawing = ActiveDrawing::None;
            }
            ActiveDrawing::Brush {
                start: s, points, ..
            } => {
                if !points.is_empty() {
                    let ann = Annotation::Brush {
                        start: s,
                        points,
                        style: self.style,
                    };
                    self.commit_annotation(ann);
                }
                self.active_drawing = ActiveDrawing::None;
            }
            ActiveDrawing::Blur { top_left, size } => {
                if size.length() > 1.0 {
                    let sigma = self
                        .style
                        .size
                        .to_blur_factor(self.style.annotation_size_factor);
                    let (pixels, bw, bh) =
                        compute_blur_pixels(&self.source_image, top_left, size, sigma);
                    let (ntl, _) = normalize_rect(top_left, size);
                    let ann = Annotation::Blur {
                        top_left: ntl,
                        size: Vec2::new(size.x.abs(), size.y.abs()),
                        blurred_pixels: pixels,
                        blur_w: bw,
                        blur_h: bh,
                        texture: None,
                    };
                    self.commit_annotation(ann);
                }
                self.active_drawing = ActiveDrawing::None;
            }
            ActiveDrawing::Highlight {
                kind,
                start: s,
                points,
                block_size: _,
            } => {
                match kind {
                    HighlighterKind::Block => {
                        let mut sz = raw_end - s;
                        if modifiers.shift {
                            let side = sz.x.abs().max(sz.y.abs());
                            sz = Vec2::new(side * sz.x.signum(), side * sz.y.signum());
                        }
                        if sz.length() > 1.0 {
                            let ann = Annotation::Highlight {
                                kind: HighlightAnnotation::Block {
                                    top_left: s,
                                    size: sz,
                                },
                                style: self.style,
                            };
                            self.commit_annotation(ann);
                        }
                    }
                    HighlighterKind::Freehand => {
                        if !points.is_empty() {
                            let ann = Annotation::Highlight {
                                kind: HighlightAnnotation::Freehand { start: s, points },
                                style: self.style,
                            };
                            self.commit_annotation(ann);
                        }
                    }
                }
                self.active_drawing = ActiveDrawing::None;
            }
            ActiveDrawing::None | ActiveDrawing::Text { .. } => {}
        }

        if self.active_tool == ToolType::Crop {
            if let Some(ref mut crop) = self.crop_state.rect {
                crop.active = false;
            }
        }
    }

    fn on_click(&mut self, img_pos: Pos2, _modifiers: egui::Modifiers) {
        match self.active_tool {
            ToolType::Marker => {
                let ann = Annotation::Marker {
                    pos: img_pos,
                    number: self.marker_counter,
                    style: self.style,
                };
                self.marker_counter += 1;
                self.commit_annotation(ann);
            }
            ToolType::Text => {
                self.active_drawing = ActiveDrawing::Text {
                    pos: img_pos,
                    content: String::new(),
                };
            }
            _ => {}
        }
    }

    fn handle_keyboard(&mut self, ctx: &egui::Context) {
        let is_text_active = matches!(self.active_drawing, ActiveDrawing::Text { .. });

        let (ctrl_z, ctrl_y, ctrl_c, ctrl_s, ctrl_t, escape, enter, key_events) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Z) && i.modifiers.ctrl,
                i.key_pressed(egui::Key::Y) && i.modifiers.ctrl,
                i.key_pressed(egui::Key::C) && i.modifiers.ctrl,
                i.key_pressed(egui::Key::S) && i.modifiers.ctrl,
                i.key_pressed(egui::Key::T) && i.modifiers.ctrl,
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter) && i.modifiers.ctrl,
                i.events
                    .iter()
                    .filter_map(|ev| {
                        if let egui::Event::Text(s) = ev {
                            Some(s.clone())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>(),
            )
        });

        if !is_text_active {
            if ctrl_z {
                self.undo();
            }
            if ctrl_y {
                self.redo();
            }
            if ctrl_c {
                self.pending_copy = true;
            }
            if ctrl_s {
                self.pending_save = true;
            }
            if ctrl_t {
                self.toolbars_visible = !self.toolbars_visible;
            }
            if escape {
                if !matches!(self.active_drawing, ActiveDrawing::None) {
                    self.active_drawing = ActiveDrawing::None;
                    self.drag_start = None;
                } else {
                    let actions = self.config.general.actions_on_escape.clone();
                    self.execute_actions(actions);
                }
            }
            if enter {
                let actions = self.config.general.actions_on_enter.clone();
                self.execute_actions(actions);
            }

            // Tool shortcuts
            for s in &key_events {
                if let Some(c) = s.chars().next() {
                    let keybinds = self.config.keybind_map();
                    if let Some(tool_name) = keybinds.get(&c) {
                        if let Some(tool) = tool_from_string(tool_name) {
                            self.switch_tool(tool);
                        }
                    }
                }
            }
        }
    }


    fn draw_active_preview(&self, painter: &egui::Painter, current_cursor: Option<Pos2>) {
        let zoom = self.zoom;
        let pan = self.pan;
        let canvas_rect = self.canvas_rect;
        let img_center = self.img_center();

        let i2s = |p: Pos2| -> Pos2 {
            let canvas_c = canvas_rect.center();
            canvas_c + (p.to_vec2() - img_center) * zoom + pan
        };

        let color = self.style.color.to_egui();
        let factor = self.style.annotation_size_factor;
        let lw = self.style.size.to_line_width(factor) * zoom;
        let stroke = Stroke::new(lw, color);

        match &self.active_drawing {
            ActiveDrawing::None => {}
            ActiveDrawing::Text { .. } => {} // handled with Area widget
            ActiveDrawing::Arrow { start } => {
                if let Some(cursor) = current_cursor {
                    let s = i2s(*start);
                    let e = cursor;
                    draw_arrow_preview(painter, s, e, stroke, self.style.size, factor, zoom);
                }
            }
            ActiveDrawing::Line { start } => {
                if let Some(cursor) = current_cursor {
                    let s = i2s(*start);
                    painter.line_segment([s, cursor], stroke);
                }
            }
            ActiveDrawing::Rectangle { top_left, size, .. } => {
                if size.length() > 0.5 {
                    let s = i2s(*top_left);
                    let e = i2s(*top_left + *size);
                    let r = Rect::from_two_pos(s, e);
                    let rounding = Rounding::same(self.config.general.corner_roundness * zoom);
                    if self.style.fill {
                        painter.rect_filled(r, rounding, color);
                    } else {
                        painter.rect_stroke(r, rounding, stroke);
                    }
                }
            }
            ActiveDrawing::Ellipse { center, radii, .. } => {
                if radii.length() > 0.5 {
                    let sc = i2s(*center);
                    let sr = *radii * zoom;
                    draw_ellipse_preview(painter, sc, sr, color, stroke, self.style.fill);
                }
            }
            ActiveDrawing::Brush { start, points, .. } => {
                if !points.is_empty() {
                    let screen_points: Vec<Pos2> = std::iter::once(*start)
                        .chain(
                            points
                                .iter()
                                .map(|&delta| Pos2::new(start.x + delta.x, start.y + delta.y)),
                        )
                        .map(|p| i2s(p))
                        .collect();
                    painter.add(egui::Shape::line(screen_points, stroke));
                }
            }
            ActiveDrawing::Blur { top_left, size } => {
                if size.length() > 0.5 {
                    let s = i2s(*top_left);
                    let e = i2s(*top_left + *size);
                    let r = Rect::from_two_pos(s, e);
                    painter.rect_stroke(r, Rounding::ZERO, Stroke::new(1.0, Color32::WHITE));
                }
            }
            ActiveDrawing::Highlight {
                kind,
                start,
                points,
                block_size,
            } => {
                let hi_color = Color32::from_rgba_unmultiplied(
                    self.style.color.r,
                    self.style.color.g,
                    self.style.color.b,
                    100,
                );
                match kind {
                    HighlighterKind::Block => {
                        if block_size.length() > 0.5 {
                            let s = i2s(*start);
                            let e = i2s(*start + *block_size);
                            let r = Rect::from_two_pos(s, e);
                            painter.rect_filled(r, Rounding::ZERO, hi_color);
                        }
                    }
                    HighlighterKind::Freehand => {
                        if !points.is_empty() {
                            let hw = self.style.size.to_highlight_width(factor) * zoom;
                            let hi_stroke = Stroke::new(hw, hi_color);
                            let screen_points: Vec<Pos2> =
                                std::iter::once(*start)
                                    .chain(points.iter().map(|&delta| {
                                        Pos2::new(start.x + delta.x, start.y + delta.y)
                                    }))
                                    .map(|p| i2s(p))
                                    .collect();
                            painter.add(egui::Shape::line(screen_points, hi_stroke));
                        }
                    }
                }
            }
        }
    }

    fn draw_crop_overlay(&self, painter: &egui::Painter) {
        if let Some(crop) = &self.crop_state.rect {
            let tl = self.image_to_screen(crop.pos);
            let br = self.image_to_screen(crop.pos + crop.size);
            let crop_rect = Rect::from_two_pos(tl, br);
            let canvas = self.canvas_rect;
            let dim = Color32::from_black_alpha(128);

            // Top strip
            if crop_rect.top() > canvas.top() {
                painter.rect_filled(
                    Rect::from_min_max(canvas.min, egui::pos2(canvas.max.x, crop_rect.top())),
                    Rounding::ZERO,
                    dim,
                );
            }
            // Bottom strip
            if crop_rect.bottom() < canvas.bottom() {
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(canvas.min.x, crop_rect.bottom()), canvas.max),
                    Rounding::ZERO,
                    dim,
                );
            }
            // Left strip
            let top = crop_rect.top().max(canvas.top());
            let bottom = crop_rect.bottom().min(canvas.bottom());
            if crop_rect.left() > canvas.left() {
                painter.rect_filled(
                    Rect::from_min_max(
                        egui::pos2(canvas.min.x, top),
                        egui::pos2(crop_rect.left(), bottom),
                    ),
                    Rounding::ZERO,
                    dim,
                );
            }
            // Right strip
            if crop_rect.right() < canvas.right() {
                painter.rect_filled(
                    Rect::from_min_max(
                        egui::pos2(crop_rect.right(), top),
                        egui::pos2(canvas.max.x, bottom),
                    ),
                    Rounding::ZERO,
                    dim,
                );
            }

            // Border
            painter.rect_stroke(crop_rect, Rounding::ZERO, Stroke::new(2.0, Color32::WHITE));

            // Confirm instruction
            painter.text(
                crop_rect.center_bottom() + Vec2::new(0.0, 16.0),
                egui::Align2::CENTER_TOP,
                "Press Enter to confirm crop",
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
        }
    }

    fn handle_canvas_input(&mut self, response: &egui::Response, ctx: &egui::Context) {
        // Zoom with scroll
        let scroll = ctx.input(|i| i.smooth_scroll_delta.y);
        if scroll != 0.0 && response.hovered() {
            let factor = if scroll > 0.0 {
                self.config.general.zoom_factor
            } else {
                1.0 / self.config.general.zoom_factor
            };
            let hover = ctx
                .input(|i| i.pointer.hover_pos())
                .unwrap_or(self.canvas_rect.center());
            let img_pos = self.screen_to_image(hover);
            self.zoom *= factor;
            self.zoom = self.zoom.clamp(0.05, 50.0);
            let new_screen = self.image_to_screen(img_pos);
            self.pan += hover.to_vec2() - new_screen.to_vec2();
        }

        // Pan with middle button
        if response.dragged_by(egui::PointerButton::Middle) {
            self.pan += response.drag_delta();
        }

        let modifiers = ctx.input(|i| i.modifiers);

        // Primary button drag start
        if response.drag_started_by(egui::PointerButton::Primary) {
            if let Some(origin) = ctx.input(|i| i.pointer.press_origin()) {
                let img_pos = self.screen_to_image(origin);
                self.drag_start = Some(img_pos);
                self.on_drag_begin(img_pos, modifiers);
            }
        }

        // Primary drag update
        if response.dragged_by(egui::PointerButton::Primary) {
            if let (Some(start), Some(hover)) =
                (self.drag_start, ctx.input(|i| i.pointer.hover_pos()))
            {
                let current = self.screen_to_image(hover);
                let delta_vec = current.to_vec2() - start.to_vec2();
                let delta_pos = Pos2::new(delta_vec.x, delta_vec.y);
                self.on_drag_update(start, delta_pos, modifiers);
            }
        }

        // Primary drag end
        let drag_released = response.drag_stopped_by(egui::PointerButton::Primary);
        if drag_released {
            if let Some(start) = self.drag_start.take() {
                let hover = ctx
                    .input(|i| i.pointer.hover_pos())
                    .map(|p| self.screen_to_image(p))
                    .unwrap_or(start);
                let delta_vec = hover.to_vec2() - start.to_vec2();
                let delta_pos = Pos2::new(delta_vec.x, delta_vec.y);
                self.on_drag_end(start, delta_pos, modifiers);
            }
        }

        // Click (single press-release without drag)
        if response.clicked_by(egui::PointerButton::Primary) {
            if let Some(pos) = ctx.input(|i| i.pointer.interact_pos()) {
                let img_pos = self.screen_to_image(pos);
                self.on_click(img_pos, modifiers);
            }
        }

        // Right click
        if response.secondary_clicked() {
            let actions = self.config.general.actions_on_right_click.clone();
            self.execute_actions(actions);
        }

        // Crop confirm with Enter
        if self.active_tool == ToolType::Crop {
            let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
            if enter && self.crop_state.rect.is_some() {
                self.confirm_crop(ctx);
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Ensure texture is uploaded on first frame
        if !self.initialized {
            self.initialized = true;
            self.upload_base_texture(ctx);
            apply_theme(ctx, &self.config.theme);
        }

        // Handle deferred actions
        if self.pending_copy {
            self.pending_copy = false;
            self.copy_to_clipboard_action();
            if self.config.general.save_after_copy {
                self.save_to_file();
            }
        }
        if self.pending_save {
            self.pending_save = false;
            self.save_to_file();
        }

        self.handle_keyboard(ctx);

        // Top toolbar
        egui::TopBottomPanel::top("tools")
            .frame(
                egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin {
                    top: 3.0,
                    bottom: 4.0,
                    left: 0.0,
                    right: 0.0,
                }),
            )
            .show_animated(ctx, self.toolbars_visible, |ui| {
                let available = ui.available_width();
                let leading = ((available - self.top_toolbar_content_width) / 2.0).max(0.0);
                let r = ui.horizontal(|ui| {
                    ui.add_space(leading);
                    for &tool in &[
                        ToolType::Pointer,
                        ToolType::Crop,
                        ToolType::Brush,
                        ToolType::Line,
                        ToolType::Arrow,
                        ToolType::Rectangle,
                        ToolType::Ellipse,
                        ToolType::Text,
                        ToolType::Marker,
                        ToolType::Blur,
                        ToolType::Highlight,
                    ] {
                        let selected = self.active_tool == tool;
                        if ui
                            .add_sized(
                                [30.0, 30.0],
                                egui::SelectableLabel::new(selected, egui::RichText::new(tool.icon()).size(20.0)),
                            )
                            .on_hover_text(tool.label())
                            .clicked()
                        {
                            self.switch_tool(tool);
                        }
                    }
                    ui.separator();
                    if ui
                        .add_sized([30.0, 30.0], egui::Button::new(egui::RichText::new("↩").size(20.0)))
                        .on_hover_text("Undo")
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui
                        .add_sized([30.0, 30.0], egui::Button::new(egui::RichText::new("↪").size(20.0)))
                        .on_hover_text("Redo")
                        .clicked()
                    {
                        self.redo();
                    }
                    ui.separator();
                    if ui
                        .add_sized([30.0, 30.0], egui::Button::new(egui::RichText::new("⎘").size(20.0)))
                        .on_hover_text("Copy")
                        .clicked()
                    {
                        self.pending_copy = true;
                    }
                    if ui
                        .add_sized([30.0, 30.0], egui::Button::new(egui::RichText::new("💾").size(20.0)))
                        .on_hover_text("Save")
                        .clicked()
                    {
                        self.pending_save = true;
                    }
                    if let Some(ref path) = self.last_saved_path.clone() {
                        ui.label(format!("Saved: {}", path));
                    }
                });
                self.top_toolbar_content_width = r.response.rect.width() - leading;
            });

        // Bottom toolbar
        egui::TopBottomPanel::bottom("style_bar")
            .frame(
                egui::Frame::side_top_panel(&ctx.style()).inner_margin(egui::Margin {
                    top: 4.0,
                    bottom: 4.0,
                    left: 0.0,
                    right: 0.0,
                }),
            )
            .show_animated(ctx, self.toolbars_visible, |ui| {
                let available = ui.available_width();
                let leading = ((available - self.bottom_toolbar_content_width) / 2.0).max(0.0);
                let r = ui.horizontal(|ui| {
                    ui.add_space(leading);
                    ui.spacing_mut().interact_size.y = 22.0;
                    let palette = self.config.palette_colors();
                    for &(r, g, b, a) in &palette {
                        let c = Color32::from_rgba_unmultiplied(r, g, b, a);
                        let selected = self.style.color == Color { r, g, b, a };
                        let (resp, painter) =
                            ui.allocate_painter(egui::vec2(22.0, 22.0), egui::Sense::click());
                        painter.rect_filled(resp.rect, Rounding::same(3.0), c);
                        if selected {
                            painter.rect_stroke(
                                resp.rect.shrink(2.0),
                                Rounding::same(1.0),
                                Stroke::new(2.0, Color32::WHITE),
                            );
                        }
                        if resp.clicked() {
                            self.style.color = Color { r, g, b, a };
                        }
                    }
                    ui.separator();
                    for (size, label) in
                        &[(Size::Small, "S"), (Size::Medium, "M"), (Size::Large, "L")]
                    {
                        if ui
                            .add_sized(
                                [22.0, 22.0],
                                egui::SelectableLabel::new(self.style.size == *size, *label),
                            )
                            .clicked()
                        {
                            self.style.size = *size;
                        }
                    }
                    ui.separator();
                    ui.toggle_value(&mut self.style.fill, "Fill");
                    ui.separator();
                    ui.label(format!("Zoom: {:.0}%", self.zoom * 100.0));
                    if ui.button("Reset").clicked() {
                        self.zoom = 1.0;
                        self.pan = Vec2::ZERO;
                    }
                });
                self.bottom_toolbar_content_width = r.response.rect.width() - leading;
            });

        // Enforce minimum window width to fit the toolbar
        let min_toolbar_w = self.top_toolbar_content_width.max(self.bottom_toolbar_content_width);
        if min_toolbar_w < 9999.0 {
            ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(min_toolbar_w, 300.0)));
            if !self.toolbar_min_width_applied {
                self.toolbar_min_width_applied = true;
                let current_w = ctx.screen_rect().width();
                if current_w < min_toolbar_w {
                    let current_h = ctx.screen_rect().height();
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(min_toolbar_w, current_h)));
                }
            }
        }

        // Central panel
        let frame = egui::Frame::none().fill(Color32::from_rgb(30, 30, 35));
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            self.canvas_rect = ui.available_rect_before_wrap();

            let (response, painter) =
                ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());

            // Draw base image
            if let Some(texture) = &self.base_texture {
                let img_rect = self.image_rect();
                painter.image(
                    texture.id(),
                    img_rect,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }

            // Draw committed annotations
            let zoom = self.zoom;
            let pan = self.pan;
            let canvas_rect = self.canvas_rect;
            let img_center = self.img_center();
            let factor = self.style.annotation_size_factor;

            let i2s = |p: Pos2| -> Pos2 {
                let canvas_c = canvas_rect.center();
                canvas_c + (p.to_vec2() - img_center) * zoom + pan
            };

            for ann in self.annotations.iter_mut() {
                draw_one_annotation(ann, &painter, ctx, &i2s, zoom, factor);
            }

            // Draw active drawing preview
            let cursor = ctx.input(|i| i.pointer.hover_pos());
            self.draw_active_preview(&painter, cursor);

            // Draw crop overlay
            if self.active_tool == ToolType::Crop {
                self.draw_crop_overlay(&painter);
            }

            self.handle_canvas_input(&response, ctx);
        });

        // Text editing overlay
        if self.active_tool == ToolType::Text {
            // Extract data without holding a borrow
            let text_data = if let ActiveDrawing::Text { pos, ref content } = self.active_drawing {
                Some((pos, content.clone()))
            } else {
                None
            };

            if let Some((pos, mut local_content)) = text_data {
                let screen_pos = self.image_to_screen(pos);
                let font_size =
                    self.style
                        .size
                        .to_text_size(self.style.annotation_size_factor) as f32
                        * self.zoom;
                let color = self.style.color.to_egui();

                egui::Area::new(egui::Id::new("text_input"))
                    .fixed_pos(screen_pos)
                    .show(ctx, |ui| {
                        let resp = ui.add(
                            egui::TextEdit::multiline(&mut local_content)
                                .font(egui::FontId::proportional(font_size))
                                .desired_width(300.0)
                                .frame(true)
                                .text_color(color),
                        );
                        resp.request_focus();
                    });

                // Update content back
                if let ActiveDrawing::Text {
                    ref mut content, ..
                } = self.active_drawing
                {
                    *content = local_content.clone();
                }

                // Commit on Escape
                let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
                if escape {
                    if !local_content.is_empty() {
                        let ann = Annotation::Text {
                            pos,
                            content: local_content,
                            style: self.style,
                        };
                        self.commit_annotation(ann);
                    }
                    self.active_drawing = ActiveDrawing::None;
                }
            }
        }

        // Auto-copy on first frame if configured
        if self.config.general.auto_copy && self.annotations.is_empty() {
            // don't auto-copy on empty - it will be triggered after annotation
        }
    }
}

// -------- standalone drawing helpers --------

fn draw_one_annotation(
    ann: &mut Annotation,
    painter: &egui::Painter,
    ctx: &egui::Context,
    i2s: &impl Fn(Pos2) -> Pos2,
    zoom: f32,
    factor: f32,
) {
    match ann {
        Annotation::Arrow { start, end, style } => {
            let s = i2s(*start);
            let e = i2s(*end);
            let lw = style.size.to_line_width(factor) * zoom;
            let stroke = Stroke::new(lw, style.color.to_egui());
            draw_arrow_preview(painter, s, e, stroke, style.size, factor, zoom);
        }
        Annotation::Line { start, end, style } => {
            let s = i2s(*start);
            let e = i2s(*end);
            let lw = style.size.to_line_width(factor) * zoom;
            painter.line_segment([s, e], Stroke::new(lw, style.color.to_egui()));
        }
        Annotation::Rectangle {
            top_left,
            size,
            style,
            corner_roundness,
        } => {
            let s = i2s(*top_left);
            let e = i2s(*top_left + *size);
            let r = Rect::from_two_pos(s, e);
            let rounding = Rounding::same(*corner_roundness * zoom);
            let lw = style.size.to_line_width(factor) * zoom;
            let color = style.color.to_egui();
            if style.fill {
                painter.rect_filled(r, rounding, color);
            } else {
                painter.rect_stroke(r, rounding, Stroke::new(lw, color));
            }
        }
        Annotation::Ellipse {
            center,
            radii,
            style,
        } => {
            let sc = i2s(*center);
            let sr = *radii * zoom;
            let color = style.color.to_egui();
            let lw = style.size.to_line_width(factor) * zoom;
            draw_ellipse_preview(painter, sc, sr, color, Stroke::new(lw, color), style.fill);
        }
        Annotation::Brush {
            start,
            points,
            style,
        } => {
            if !points.is_empty() {
                let lw = style.size.to_line_width(factor) * zoom;
                let color = style.color.to_egui();
                let screen_points: Vec<Pos2> = std::iter::once(*start)
                    .chain(
                        points
                            .iter()
                            .map(|&delta| Pos2::new(start.x + delta.x, start.y + delta.y)),
                    )
                    .map(|p| i2s(p))
                    .collect();
                painter.add(egui::Shape::line(screen_points, Stroke::new(lw, color)));
            }
        }
        Annotation::Text {
            pos,
            content,
            style,
        } => {
            let sp = i2s(*pos);
            let font_size = style.size.to_text_size(factor) as f32 * zoom;
            let color = style.color.to_egui();
            let mut y = sp.y;
            for line in content.lines() {
                painter.text(
                    Pos2::new(sp.x, y),
                    egui::Align2::LEFT_TOP,
                    line,
                    egui::FontId::proportional(font_size),
                    color,
                );
                y += font_size * 1.2;
            }
        }
        Annotation::Marker { pos, number, style } => {
            let sp = i2s(*pos);
            let font_size = style.size.to_marker_size(factor) * zoom;
            let r = font_size * 0.8;
            let color = style.color.to_egui();
            painter.circle_filled(sp, r, color);
            painter.circle_stroke(sp, r, Stroke::new(1.5, Color32::WHITE));
            let luma = (0.2126 * style.color.r as f32
                + 0.7152 * style.color.g as f32
                + 0.0722 * style.color.b as f32)
                / 255.0;
            let text_color = if luma > 0.5 {
                Color32::BLACK
            } else {
                Color32::WHITE
            };
            painter.text(
                sp,
                egui::Align2::CENTER_CENTER,
                number.to_string(),
                egui::FontId::proportional(font_size * 0.9),
                text_color,
            );
        }
        Annotation::Blur {
            top_left,
            size,
            blurred_pixels,
            blur_w,
            blur_h,
            texture,
        } => {
            // Upload texture lazily
            if texture.is_none() {
                let color_img = egui::ColorImage::from_rgba_unmultiplied(
                    [*blur_w as usize, *blur_h as usize],
                    blurred_pixels,
                );
                *texture = Some(ctx.load_texture("blur", color_img, egui::TextureOptions::LINEAR));
            }
            if let Some(tex) = texture {
                let s = i2s(*top_left);
                let e = i2s(*top_left + *size);
                let r = Rect::from_two_pos(s, e);
                painter.image(
                    tex.id(),
                    r,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        Annotation::Highlight { kind, style } => {
            let hi_color =
                Color32::from_rgba_unmultiplied(style.color.r, style.color.g, style.color.b, 100);
            let hw = style.size.to_highlight_width(factor) * zoom;
            match kind {
                HighlightAnnotation::Block { top_left, size } => {
                    let s = i2s(*top_left);
                    let e = i2s(*top_left + *size);
                    let r = Rect::from_two_pos(s, e);
                    painter.rect_filled(r, Rounding::ZERO, hi_color);
                }
                HighlightAnnotation::Freehand { start, points } => {
                    if !points.is_empty() {
                        let screen_points: Vec<Pos2> = std::iter::once(*start)
                            .chain(
                                points
                                    .iter()
                                    .map(|&delta| Pos2::new(start.x + delta.x, start.y + delta.y)),
                            )
                            .map(|p| i2s(p))
                            .collect();
                        painter.add(egui::Shape::line(screen_points, Stroke::new(hw, hi_color)));
                    }
                }
            }
        }
    }
}

fn draw_arrow_preview(
    painter: &egui::Painter,
    start: Pos2,
    end: Pos2,
    stroke: Stroke,
    size: crate::style::Size,
    factor: f32,
    zoom: f32,
) {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }
    let dir_x = dx / len;
    let dir_y = dy / len;
    let head_len = size.to_arrow_head_length(factor) * zoom;
    let half_angle = 60.0f32.to_radians() / 2.0;
    let cos_a = half_angle.cos();
    let sin_a = half_angle.sin();

    painter.line_segment([start, end], stroke);

    // Wing 1
    let h1x = (-dir_x) * cos_a + (-dir_y) * (-sin_a);
    let h1y = (-dir_x) * sin_a + (-dir_y) * cos_a;
    let h1 = Pos2::new(end.x + h1x * head_len, end.y + h1y * head_len);

    // Wing 2
    let h2x = (-dir_x) * cos_a + (-dir_y) * sin_a;
    let h2y = (-dir_x) * (-sin_a) + (-dir_y) * cos_a;
    let h2 = Pos2::new(end.x + h2x * head_len, end.y + h2y * head_len);

    painter.line_segment([end, h1], stroke);
    painter.line_segment([end, h2], stroke);
}

fn draw_ellipse_preview(
    painter: &egui::Painter,
    center: Pos2,
    radii: Vec2,
    fill_color: Color32,
    stroke: Stroke,
    fill: bool,
) {
    let n = 64usize;
    let points: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = (i as f32 / n as f32) * std::f32::consts::TAU;
            Pos2::new(center.x + radii.x * t.cos(), center.y + radii.y * t.sin())
        })
        .collect();

    if fill {
        // Use a filled polygon
        let verts: Vec<egui::epaint::Vertex> = points
            .iter()
            .map(|&p| egui::epaint::Vertex {
                pos: p,
                uv: egui::epaint::WHITE_UV,
                color: fill_color,
            })
            .collect();
        if verts.len() >= 3 {
            // Build triangle fan from center
            let center_v = egui::epaint::Vertex {
                pos: center,
                uv: egui::epaint::WHITE_UV,
                color: fill_color,
            };
            let mut all_verts = vec![center_v];
            all_verts.extend_from_slice(&verts);
            let mut indices = Vec::new();
            for i in 1..all_verts.len() - 1 {
                indices.push(0u32);
                indices.push(i as u32);
                indices.push((i + 1) as u32);
            }
            let mesh = egui::epaint::Mesh {
                indices,
                vertices: all_verts,
                texture_id: egui::TextureId::default(),
            };
            painter.add(egui::Shape::mesh(mesh));
        }
    } else {
        painter.add(egui::Shape::line(points, stroke));
    }
}

fn snap_to_angle(start: Pos2, end: Pos2) -> Pos2 {
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let angle = dy.atan2(dx);
    let snap_deg = 15.0f32.to_radians();
    let snapped = (angle / snap_deg).round() * snap_deg;
    let len = (dx * dx + dy * dy).sqrt();
    Pos2::new(start.x + len * snapped.cos(), start.y + len * snapped.sin())
}

fn normalize_rect(top_left: Pos2, size: Vec2) -> (Pos2, Vec2) {
    let x = if size.x < 0.0 {
        top_left.x + size.x
    } else {
        top_left.x
    };
    let y = if size.y < 0.0 {
        top_left.y + size.y
    } else {
        top_left.y
    };
    (Pos2::new(x, y), Vec2::new(size.x.abs(), size.y.abs()))
}

fn shift_annotation(ann: &mut Annotation, dx: f32, dy: f32) {
    let shift = |p: &mut Pos2| {
        p.x += dx;
        p.y += dy;
    };
    match ann {
        Annotation::Arrow { start, end, .. } => {
            shift(start);
            shift(end);
        }
        Annotation::Line { start, end, .. } => {
            shift(start);
            shift(end);
        }
        Annotation::Rectangle { top_left, .. } => {
            shift(top_left);
        }
        Annotation::Ellipse { center, .. } => {
            shift(center);
        }
        Annotation::Brush { start, .. } => {
            shift(start);
        }
        Annotation::Text { pos, .. } => {
            shift(pos);
        }
        Annotation::Marker { pos, .. } => {
            shift(pos);
        }
        Annotation::Blur { top_left, .. } => {
            shift(top_left);
        }
        Annotation::Highlight { kind, .. } => match kind {
            HighlightAnnotation::Block { top_left, .. } => {
                shift(top_left);
            }
            HighlightAnnotation::Freehand { start, .. } => {
                shift(start);
            }
        },
    }
}

fn rgba_image_to_egui(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    )
}

fn shellexpand_tilde(s: &str) -> String {
    if s.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_default();
        format!("{}{}", home, &s[1..])
    } else {
        s.to_string()
    }
}
