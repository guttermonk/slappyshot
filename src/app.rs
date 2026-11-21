use egui::{Color32, CornerRadius, Pos2, Rect, Stroke, Vec2};
use image::RgbaImage;
use std::time::Instant;

use crate::config::{Action, Config, apply_theme};
use crate::render::{compute_blur_pixels, copy_to_clipboard, render_to_image};
use crate::screen::Screen;
use crate::style::{Color, Size, Style};
use crate::tools::{
    ActiveDrawing, Annotation, CropRect, CropState, HighlightAnnotation, HighlighterKind, Smoother,
    ToolType, tool_from_string,
};

/// Window geometry, in logical points -- the unit egui and eframe size
/// windows in. Shared with main.rs so the size it builds the window at and the
/// one App::apply_initial_window_size would correct it to cannot drift apart.
///
/// Vertical room for the toolbar strip above the canvas, so a window sized to
/// the image does not then cover part of it.
pub const TOOLBAR_RESERVE_POINTS: f32 = 80.0;
/// Small captures still need a window big enough to hold the toolbars.
///
/// The width is load bearing rather than a comfort figure, which is why it is
/// 700 and not the 400 it used to be. The toolbars measure their own content
/// once laid out and raise the window's minimum width to fit it -- 648 points
/// on this machine -- and ANY gap between the size the window was created at
/// and a minimum imposed afterwards shows up as a resize on every compositor
/// reconfigure. Measured: created at 400 against a toolbar minimum of 648,
/// every configure during a drag alternated 400 -> 648 -> 400, which is what
/// the window flickering under the cursor was.
///
/// Starting wider than the toolbars will ever ask for closes that gap before
/// it can open. 700 against a measured 648 is about 8% of headroom, enough
/// for a different font or another button. If the toolbars ever genuinely
/// need more, the flicker returns and this is the number to raise.
pub const MIN_WINDOW_W_POINTS: f32 = 700.0;
pub const MIN_WINDOW_H_POINTS: f32 = 300.0;

/// How many frames apply_initial_window_size will keep asking for its size
/// before accepting the compositor's answer. Generous enough to outlast the
/// first configure round-trip, short enough that it is over long before a
/// human could have resized the window themselves.
const INITIAL_SIZE_MAX_ATTEMPTS: u32 = 60;

/// How many of those frames may be spent waiting for the surface to meet an
/// output and report a real scale factor. Measured at 2 on Hyprland; this
/// leaves room without risking the whole budget on a compositor that never
/// reports one.
const INITIAL_SIZE_SCALE_GRACE: u32 = 20;

/// How much of the monitor a window may take before it is in the way, when
/// the usable area could not be had any other way.
///
/// A window clamped to the monitor exactly is still too big, because a panel
/// takes a bite out of the screen that the window knows nothing about: the
/// toplevel protocol hands a client the MONITOR size and never the work area.
/// A full-screen capture therefore opened a full-screen-tall window, which the
/// compositor centred in the usable area, leaving the toolbars tucked under
/// the bar -- toolbars being at the top, that is the half that hurts.
///
/// This is a heuristic and not a measurement, which is the honest way to say
/// it, and a heuristic tuned on one machine is a machine-shaped fix: 5% is 43
/// points of the 864 on the display it was measured on and 48 of the 960 on
/// the next one, which clears a 30-point bar and a 38-point bar by luck as
/// much as by design. screen::probe is the measurement, and where it answers
/// -- any compositor with the layer shell -- this is not consulted at all.
const MAX_MONITOR_FRACTION: f32 = 0.95;

/// Breathing room beside the toolbar buttons when they set the window's
/// minimum width.
const TOOLBAR_MIN_WIDTH_PADDING: f32 = 20.0;

/// How far a recomputed minimum size must move before it is worth sending
/// again. These come from laid-out widgets and carry sub-pixel noise; a whole
/// point is below anything a person can see and well above the jitter.
const MIN_SIZE_EPSILON: f32 = 1.0;

/// Zoom limits, as a fraction of actual size -- 5% to 5000%. Applied through
/// App::clamp_zoom rather than to `zoom` directly, because `zoom` is measured
/// in points per pixel and these are measured against the image.
const MIN_ZOOM_SCALE: f32 = 0.05;
const MAX_ZOOM_SCALE: f32 = 50.0;

/// Space left around the image when it is fitted to the window, so a fitted
/// capture does not sit edge to edge with the toolbars.
const FIT_MARGIN_POINTS: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
enum PointerDrag {
    None,
    Panning,
    MovingAnnotation(usize),
}

pub struct App {
    source_image: RgbaImage,
    base_texture: Option<egui::TextureHandle>,
    /// Points of canvas per pixel of image. NOT a percentage: a zoom of 1.0 is
    /// actual size only on a scale 1.0 display. See App::unit_zoom.
    zoom: f32,
    /// While set, `zoom` and `pan` are recomputed from the canvas every frame
    /// so the whole image stays visible. Cleared by any manual zoom.
    fit_to_window: bool,
    /// The zoom the user has asked for, as a fraction of actual size. The
    /// persistent half of the zoom state; `zoom` is derived from it each frame
    /// by refresh_zoom. Ignored while fit_to_window is set.
    requested_scale: f32,
    /// The display's scale factor, refreshed each frame. Cached because the
    /// zoom helpers are called from places that have no Context to hand, and
    /// because reading it in two places in one frame risks reading two
    /// different values.
    pixels_per_point: f32,
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
    initialized: bool,
    // Deferred actions that need ctx
    pending_copy: bool,
    pending_save: bool,
    top_toolbar_content_width: f32,
    bottom_toolbar_content_width: f32,
    toolbar_min_width_applied: bool,
    /// What apply_initial_window_size asked for, kept because a viewport
    /// command does not show up in screen_rect until the next frame.
    initial_size_request: Option<Vec2>,
    /// Frames spent asking for that size. See apply_initial_window_size: one
    /// request is not enough, and an unbounded number would fight the user.
    initial_size_attempts: u32,
    /// A floor held over the window while it is being sized, because a floor
    /// is the only thing this compositor stack honours -- see
    /// apply_initial_window_size. Dropped once the size lands, so the window
    /// stays freely resizable afterwards.
    forced_min: Option<Vec2>,
    /// The last minimum actually sent, so an unchanged one is not re-sent on
    /// every frame. See send_min_inner_size.
    last_min_sent: Option<Vec2>,
    pointer_drag: PointerDrag,
    key_flash: Option<(ToolType, Instant)>,
    show_info: bool,
    /// What the compositor said about the screen before the window was built,
    /// if it was asked and answered. Set means main.rs already sized the
    /// window from a measurement, which is the only moment the size can be
    /// set at all -- see apply_initial_window_size.
    screen: Option<Screen>,
}

impl App {
    pub fn new(source_image: RgbaImage, config: Config, screen: Option<Screen>) -> Self {
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
            fit_to_window: config.general.zoom_to_fit,
            requested_scale: 1.0,
            pixels_per_point: 1.0,
            pan: Vec2::ZERO,
            canvas_rect: Rect::NOTHING,
            annotations: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            active_drawing: ActiveDrawing::None,
            active_tool: initial_tool,
            crop_state: CropState::default(),
            marker_counter: 1,
            style,
            primary_highlighter,
            drag_start: Option::None,
            config,
            toolbars_visible,
            initialized: false,
            pending_copy: false,
            pending_save: false,

            top_toolbar_content_width: 9999.0,
            bottom_toolbar_content_width: 9999.0,
            toolbar_min_width_applied: false,
            initial_size_request: None,
            initial_size_attempts: 0,
            forced_min: None,
            last_min_sent: None,
            pointer_drag: PointerDrag::None,
            key_flash: None,
            show_info: false,
            screen,
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

    fn clamp_to_image(&self, p: Pos2) -> Pos2 {
        let w = self.source_image.width() as f32;
        let h = self.source_image.height() as f32;
        Pos2::new(p.x.clamp(0.0, w), p.y.clamp(0.0, h))
    }

    fn is_inside_image(&self, screen_pos: Pos2) -> bool {
        self.image_rect().contains(screen_pos)
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
            .unwrap_or(1);
    }

    fn switch_tool(&mut self, tool: ToolType) {
        if tool == ToolType::Marker && self.active_tool == ToolType::Marker {
            self.marker_counter = 1;
        }
        self.active_drawing = ActiveDrawing::None;
        self.drag_start = None;
        self.active_tool = tool;
    }

    /// The ONE place that sets the window's minimum size.
    ///
    /// Two callers used to send this independently -- the sizing code with the
    /// size it wanted, the toolbar code with the width its buttons need -- and
    /// they alternated, each seeing the other's value as a change and
    /// re-sending its own. On the wire that is a window being given two
    /// different minimums every frame, which is what the flicker was:
    ///
    ///   set_min_size(512, 464)   <- sizing
    ///   set_min_size(648, 464)   <- toolbars
    ///   set_min_size(512, 464)   <- sizing again, next frame
    ///   set_min_size(648, 464)
    ///
    /// So both constraints are collected here and the larger of each axis
    /// wins. A minimum is a floor; two floors mean the higher one, not the
    /// most recently mentioned one.
    ///
    /// The epsilon is because these are measured from laid-out widgets and
    /// carry sub-pixel noise. Comparing floats exactly would let a drift of
    /// a thousandth of a point re-send the constraint on every frame, which
    /// is the same failure wearing a smaller number.
    fn update_min_inner_size(&mut self, ctx: &egui::Context) {
        let mut min = egui::vec2(MIN_WINDOW_W_POINTS, MIN_WINDOW_H_POINTS);

        // 9999.0 is the "not measured yet" sentinel the toolbar widths start
        // at; it must not become a minimum.
        let toolbar_w = self
            .top_toolbar_content_width
            .max(self.bottom_toolbar_content_width)
            + TOOLBAR_MIN_WIDTH_PADDING;
        if toolbar_w < 9999.0 {
            min.x = min.x.max(toolbar_w);
        }

        if let Some(forced) = self.forced_min {
            min = min.max(forced);
        }

        if let Some(last) = self.last_min_sent
            && (last.x - min.x).abs() < MIN_SIZE_EPSILON
            && (last.y - min.y).abs() < MIN_SIZE_EPSILON
        {
            return;
        }
        self.last_min_sent = Some(min);
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min));
    }

    /// Correct the window's size from the frame that first knows the display
    /// scale -- for the compositors that could not be asked beforehand.
    ///
    /// This runs only when screen::probe came back empty, because a window
    /// whose size was measured before it was built has nothing to correct:
    /// main.rs had the scale and the usable area and used both. Where the
    /// probe does fail -- GNOME, X11, no layer shell -- main.rs falls back to
    /// treating the image's PIXELS as POINTS, which is right only at scale
    /// 1.0, and this is where that gets found out and fixed.
    ///
    /// Clamped to the monitor, because the floor in main.rs has never had a
    /// matching ceiling: a full-screen capture asks for a window larger than
    /// the screen it was taken from, at ANY scale. The image is not shrunk to
    /// fit -- zoom stays 1:1 and the existing pan (drag, or the arrow keys
    /// with the pointer tool) reaches the rest of it.
    ///
    /// Clamped to MAX_MONITOR_FRACTION of the monitor rather than to the
    /// monitor itself, because the monitor is not the usable area and this
    /// path has no way to learn the difference. See the constant.
    ///
    /// Asked for repeatedly, and that is the part that took a measurement to
    /// learn. A single request sent on the first frame is simply lost: the
    /// Wayland surface has not been configured yet, so there is nothing to
    /// resize. So it re-asks until the window is within its bounds, and gives
    /// up after INITIAL_SIZE_MAX_ATTEMPTS. Both halves matter: without the
    /// retry the request never lands, and without the cap a compositor that
    /// declines the size -- tiled, maximised, snapped, or simply Hyprland --
    /// would be fought every frame for as long as the window is open,
    /// including after the user has resized it by hand.
    fn apply_initial_window_size(&mut self, ctx: &egui::Context) {
        if self.initial_size_attempts >= INITIAL_SIZE_MAX_ATTEMPTS
            || self.config.general.fullscreen
            || self.screen.is_some()
        {
            return;
        }

        let ppp = ctx.pixels_per_point();
        // Spelled out rather than `!(ppp > 0.0)`, which was the same test
        // wearing a disguise clippy objects to: both reject a NaN, and that
        // is the point of it. Dividing the image size by a NaN would poison
        // every number downstream of here.
        if !ppp.is_finite() || ppp <= 0.0 {
            return; // Not known yet. Try again next frame.
        }

        // Wait for the surface to meet an output before committing to a size.
        //
        // This is the trap this function was written to avoid and then fell
        // into anyway. On the first frames egui reports a PLACEHOLDER
        // pixels_per_point of 1.0 and no monitor; the real scale arrives a
        // couple of frames later (1.25 on the first frame with a monitor, in
        // the trace that found this). Computing the size from that placeholder
        // and caching it reproduces exactly the pixels-as-points mistake in
        // main.rs -- the correction and the bug agreeing on the wrong answer.
        //
        // monitor_size is the signal rather than ppp itself, because a 1.0
        // scale is perfectly legitimate and indistinguishable from the
        // placeholder by value alone. A monitor means the surface has been
        // configured, which is when the scale becomes real.
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        if monitor.is_none() && self.initial_size_attempts < INITIAL_SIZE_SCALE_GRACE {
            self.initial_size_attempts += 1;
            ctx.request_repaint();
            return;
        }

        let want = *self.initial_size_request.get_or_insert_with(|| {
            // Points, from pixels.
            let mut size = egui::vec2(
                self.source_image.width() as f32 / ppp,
                self.source_image.height() as f32 / ppp + TOOLBAR_RESERVE_POINTS,
            );

            // Still None after the grace period: a window of the right size
            // unclamped beats a correctly clamped one that never arrives.
            if let Some(monitor) = monitor {
                size = size.min(monitor * MAX_MONITOR_FRACTION);
            }

            size.max(egui::vec2(MIN_WINDOW_W_POINTS, MIN_WINDOW_H_POINTS))
        });

        // Done when the window is big enough AND not too big, which are two
        // different questions with two different answers.
        //
        // Big enough is measured against `want` and not for equality with it.
        // The mechanism is a floor, so "at least" is the whole of what it can
        // achieve, and a window may legitimately be WIDER than asked -- the
        // toolbars have a minimum width of their own and it is routinely
        // larger than a small capture. Testing for equality instead left every
        // small capture spinning out all 60 attempts against a width it was
        // never going to get, and then holding the floor below for the life of
        // the window, which turned into a resize fight with the compositor
        // whenever the window was dragged.
        //
        // Not too big is measured against the monitor clamp, and leaving it
        // out was the bug that broke a second laptop: a full-screen capture
        // opens at the monitor's own size, which is already past `want`, so
        // "big enough" read as satisfied on the first frame and the window
        // kept every point of the bar's area it was sitting under.
        //
        // The 1.0 of slack on each is for a compositor's rounding, not for the
        // window being a bit off: a point under is still under.
        let have = ctx.input(|i| i.viewport_rect()).size();
        // Floored at the window minimum, so the ceiling and the floor cannot
        // contradict each other on a screen too small to hold the toolbars.
        let ceiling = monitor
            .map(|monitor| {
                (monitor * MAX_MONITOR_FRACTION)
                    .max(egui::vec2(MIN_WINDOW_W_POINTS, MIN_WINDOW_H_POINTS))
            })
            .unwrap_or(egui::Vec2::INFINITY);
        if have.x + 1.0 >= want.x
            && have.y + 1.0 >= want.y
            && have.x <= ceiling.x + 1.0
            && have.y <= ceiling.y + 1.0
        {
            self.initial_size_attempts = INITIAL_SIZE_MAX_ATTEMPTS;
            // Let go of the floor now that the size has landed, so the window
            // can still be made smaller by hand afterwards.
            self.forced_min = None;
            return;
        }

        self.initial_size_attempts += 1;
        if self.initial_size_attempts >= INITIAL_SIZE_MAX_ATTEMPTS {
            // Out of attempts. Drop the floor rather than leaving it standing:
            // a size that could not be reached in a second is not going to be
            // reached by holding a constraint over the window forever, and the
            // constraint is what the user would feel.
            self.forced_min = None;
            return;
        }
        // The floor, not the size request, is what actually moves this window.
        //
        // ViewportCommand::InnerSize and MaxInnerSize are both ignored here --
        // measured, 60 sends over 60 frames with screen_rect never moving.
        // MinInnerSize is the only one with any effect.
        //
        // Its effect is not purely "grow", which is worth stating because the
        // obvious model says it should be. With the window sitting at the
        // monitor clamp, a floor BELOW the current size brings it down to
        // match: a full-screen capture measures 1536x864 with
        // MAX_MONITOR_FRACTION at 1.0 and 1459x821 at 0.95. That is the case
        // this still earns its keep for, now that main.rs asks for the size it
        // actually wants -- a capture as large as the screen is the one that
        // cannot be requested small enough up front.
        //
        // Recorded rather than sent. update_min_inner_size owns the actual
        // command and runs later in the frame, so this and the toolbar's
        // width reach the compositor as one combined floor instead of two
        // competing ones.
        self.forced_min = Some(want);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(want));
        // egui only redraws on demand, and nothing else here guarantees
        // another frame -- without this the retry would wait for the user to
        // move the mouse.
        ctx.request_repaint();
    }

    /// The `zoom` that shows the image at actual size -- one image pixel to one
    /// physical display pixel. This is what "100%" means.
    ///
    /// It is not 1.0, and that was the zoom-in nobody asked for. `zoom`
    /// multiplies POINTS, which is what the painter draws in, while the image
    /// is measured in PIXELS; the window is sized image_pixels / scale points
    /// (see apply_initial_window_size) precisely so the image fills it at
    /// actual size. Drawing it at one point per pixel therefore magnified it
    /// by the display's scale factor and overflowed the window it had just
    /// been sized to fit -- on this display, 25% too big with the edges cut
    /// off. At scale 1.0 the two units coincide and nothing looks wrong, which
    /// is the "sometimes" in it.
    fn unit_zoom(&self) -> f32 {
        let ppp = self.pixels_per_point;
        if ppp.is_finite() && ppp > 0.0 {
            1.0 / ppp
        } else {
            1.0
        }
    }

    /// Zoom that shows the whole image inside the canvas.
    ///
    /// Capped at actual size, so fitting only ever shrinks. A capture smaller
    /// than the window stays at 100% rather than being blown up to fill it:
    /// these are screenshots, and magnified pixels are not a better look at
    /// them.
    fn fit_zoom(&self) -> f32 {
        let unit = self.unit_zoom();
        let w = self.source_image.width() as f32;
        let h = self.source_image.height() as f32;
        if !self.canvas_rect.is_finite() || w <= 0.0 || h <= 0.0 {
            return unit;
        }
        let avail = self.canvas_rect.size() - Vec2::splat(2.0 * FIT_MARGIN_POINTS);
        if avail.x <= 0.0 || avail.y <= 0.0 {
            return unit;
        }
        let fit = (avail.x / w).min(avail.y / h).min(unit);
        self.clamp_zoom(fit)
    }

    /// Hold `zoom` between MIN_ZOOM_SCALE and MAX_ZOOM_SCALE of actual size.
    fn clamp_zoom(&self, zoom: f32) -> f32 {
        let unit = self.unit_zoom();
        zoom.clamp(MIN_ZOOM_SCALE * unit, MAX_ZOOM_SCALE * unit)
    }

    /// `zoom` as a fraction of actual size -- 1.0 is 100%, which is what the
    /// toolbar shows and what the limits are expressed in.
    fn zoom_scale(&self) -> f32 {
        self.zoom / self.unit_zoom()
    }

    /// Bring `zoom` into line with the scale factor and the window, once per
    /// frame, before anything reads it.
    ///
    /// Both of its inputs move underneath it. The scale factor is a
    /// placeholder 1.0 on the first frames and becomes real a frame or two
    /// later (see apply_initial_window_size), and the window is resized
    /// through startup and by hand afterwards. So the stored percentage is the
    /// state that persists and `zoom` is derived from it every frame, rather
    /// than `zoom` being set once from numbers that were not final.
    fn refresh_zoom(&mut self) {
        if self.fit_to_window {
            self.zoom = self.fit_zoom();
            // Nothing off-screen to pan to when the whole image is visible.
            self.pan = Vec2::ZERO;
        } else {
            self.zoom = self.clamp_zoom(self.requested_scale * self.unit_zoom());
        }
    }

    /// Show the image at actual size, centred.
    fn reset_zoom(&mut self) {
        self.fit_to_window = false;
        self.requested_scale = 1.0;
        self.zoom = self.unit_zoom();
        self.pan = Vec2::ZERO;
    }

    fn zoom_step(&mut self, factor: f32) {
        // A deliberate zoom leaves fit mode: otherwise refresh_zoom would
        // recompute the fitted zoom next frame and undo it.
        self.fit_to_window = false;
        if self.canvas_rect.is_finite() {
            let center = self.canvas_rect.center();
            let img_pos = self.screen_to_image(center);
            self.zoom = self.clamp_zoom(self.zoom * factor);
            let new_screen = self.image_to_screen(img_pos);
            self.pan += center.to_vec2() - new_screen.to_vec2();
        } else {
            self.zoom = self.clamp_zoom(self.zoom * factor);
        }
        self.requested_scale = self.zoom_scale();
    }

    fn copy_to_clipboard_action(&self) {
        let rendered = render_to_image(
            &self.source_image,
            &self.annotations,
            self.config.general.corner_roundness,
        );
        match copy_to_clipboard(&rendered, self.config.general.copy_command.as_deref()) {
            Ok(()) => {
                let _ = notify_rust::Notification::new()
                    .summary("slappyshot")
                    .body("Copied to clipboard")
                    .icon("slappyshot")
                    .show();
            }
            Err(e) => {
                eprintln!("Copy to clipboard failed: {}", e);
            }
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
                let _ = notify_rust::Notification::new()
                    .summary("slappyshot")
                    .body(&format!("Saved: {}", filename))
                    .icon("slappyshot")
                    .show();
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
            // The image just changed size, so whatever the view was framing is
            // gone either way: back to actual size, or refitted next frame.
            self.pan = Vec2::ZERO;
            if !self.fit_to_window {
                self.requested_scale = 1.0;
                self.zoom = self.unit_zoom();
            }
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
            ToolType::Pointer | ToolType::Text | ToolType::Marker | ToolType::Delete => {}
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
        if self.active_tool == ToolType::Crop
            && let Some(ref mut crop) = self.crop_state.rect
            && crop.active
        {
            crop.size = raw_end - crop.pos;
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

        if self.active_tool == ToolType::Crop
            && let Some(ref mut crop) = self.crop_state.rect
        {
            crop.active = false;
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
            ToolType::Delete => {
                let threshold = 8.0 / self.zoom;
                let hit_idx = self
                    .annotations
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, ann)| annotation_hit_test(ann, img_pos, threshold))
                    .map(|(idx, _)| idx);
                if let Some(idx) = hit_idx {
                    self.undo_stack.push(self.annotations.clone());
                    self.redo_stack.clear();
                    self.annotations.remove(idx);
                }
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

            // Arrow-key pan when the Pointer tool is active. View-scroll
            // convention: right arrow scrolls the view right (content shifts
            // left), matching browsers / document viewers. OS typematic
            // repeat drives continuous panning while held.
            if self.active_tool == ToolType::Pointer {
                let (pan_left, pan_right, pan_up, pan_down) = ctx.input(|i| {
                    (
                        i.key_pressed(egui::Key::ArrowLeft),
                        i.key_pressed(egui::Key::ArrowRight),
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                    )
                });
                let step = self.config.general.pan_step_size;
                if pan_left {
                    self.pan.x += step;
                }
                if pan_right {
                    self.pan.x -= step;
                }
                if pan_up {
                    self.pan.y += step;
                }
                if pan_down {
                    self.pan.y -= step;
                }
            }

            // Tool shortcuts and zoom shortcuts
            let zoom_factor = self.config.general.zoom_factor;
            for s in &key_events {
                if let Some(c) = s.chars().next() {
                    match c {
                        '=' | '+' => self.zoom_step(zoom_factor),
                        '-' => self.zoom_step(1.0 / zoom_factor),
                        '0' => self.reset_zoom(),
                        'F' => self.fit_to_window = true,
                        '1'..='8' => {
                            let idx = (c as u8 - b'1') as usize;
                            let palette = self.config.palette_colors();
                            if let Some(&(r, g, b, a)) = palette.get(idx) {
                                self.style.color = Color { r, g, b, a };
                            }
                        }
                        'S' => self.style.size = Size::Small,
                        'M' => self.style.size = Size::Medium,
                        'L' => self.style.size = Size::Large,
                        '[' => {
                            self.style.size = match self.style.size {
                                Size::Small => Size::Large,
                                Size::Medium => Size::Small,
                                Size::Large => Size::Medium,
                            };
                        }
                        ']' => {
                            self.style.size = match self.style.size {
                                Size::Small => Size::Medium,
                                Size::Medium => Size::Large,
                                Size::Large => Size::Small,
                            };
                        }
                        'f' => self.style.fill = !self.style.fill,
                        'c' => self.pending_copy = true,
                        's' => self.pending_save = true,
                        'z' => self.undo(),
                        'y' => self.redo(),
                        'q' => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        _ => {
                            let keybinds = self.config.keybind_map();
                            if let Some(tool_name) = keybinds.get(&c)
                                && let Some(tool) = tool_from_string(tool_name)
                            {
                                self.switch_tool(tool);
                                self.key_flash = Some((tool, Instant::now()));
                            }
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
                    let rounding = CornerRadius::same(
                        (self.config.general.corner_roundness * zoom)
                            .round()
                            .clamp(0.0, 255.0) as u8,
                    );
                    if self.style.fill {
                        painter.rect_filled(r, rounding, color);
                    } else {
                        painter.rect_stroke(r, rounding, stroke, egui::StrokeKind::Inside);
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
                        .map(&i2s)
                        .collect();
                    painter.add(egui::Shape::line(screen_points, stroke));
                }
            }
            ActiveDrawing::Blur { top_left, size } => {
                if size.length() > 0.5 {
                    let s = i2s(*top_left);
                    let e = i2s(*top_left + *size);
                    let r = Rect::from_two_pos(s, e);
                    painter.rect_stroke(
                        r,
                        CornerRadius::ZERO,
                        Stroke::new(1.0_f32, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
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
                            painter.rect_filled(r, CornerRadius::ZERO, hi_color);
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
                                    .map(i2s)
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
                    CornerRadius::ZERO,
                    dim,
                );
            }
            // Bottom strip
            if crop_rect.bottom() < canvas.bottom() {
                painter.rect_filled(
                    Rect::from_min_max(egui::pos2(canvas.min.x, crop_rect.bottom()), canvas.max),
                    CornerRadius::ZERO,
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
                    CornerRadius::ZERO,
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
                    CornerRadius::ZERO,
                    dim,
                );
            }

            // Border
            painter.rect_stroke(
                crop_rect,
                CornerRadius::ZERO,
                Stroke::new(2.0_f32, Color32::WHITE),
                egui::StrokeKind::Inside,
            );

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
            self.fit_to_window = false;
            self.zoom = self.clamp_zoom(self.zoom * factor);
            self.requested_scale = self.zoom_scale();
            let new_screen = self.image_to_screen(img_pos);
            self.pan += hover.to_vec2() - new_screen.to_vec2();
        }

        // Pan with middle button
        if response.dragged_by(egui::PointerButton::Middle) {
            self.pan += response.drag_delta();
        }

        let modifiers = ctx.input(|i| i.modifiers);

        if self.active_tool == ToolType::Pointer {
            // Pointer tool: pan canvas or move annotation with primary drag
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
                && self.is_inside_image(origin)
            {
                let img_pos = self.screen_to_image(origin);
                let threshold = 8.0 / self.zoom;
                let hit_idx = self
                    .annotations
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, ann)| annotation_hit_test(ann, img_pos, threshold))
                    .map(|(idx, _)| idx);
                if let Some(idx) = hit_idx {
                    self.undo_stack.push(self.annotations.clone());
                    self.redo_stack.clear();
                    self.pointer_drag = PointerDrag::MovingAnnotation(idx);
                } else {
                    self.pointer_drag = PointerDrag::Panning;
                }
            }
            if response.dragged_by(egui::PointerButton::Primary) {
                match self.pointer_drag {
                    PointerDrag::Panning => {
                        self.pan += response.drag_delta();
                    }
                    PointerDrag::MovingAnnotation(idx) => {
                        let delta = response.drag_delta() / self.zoom;
                        if idx < self.annotations.len() {
                            shift_annotation(&mut self.annotations[idx], delta.x, delta.y);
                        }
                    }
                    PointerDrag::None => {}
                }
            }
            if response.drag_stopped_by(egui::PointerButton::Primary) {
                self.pointer_drag = PointerDrag::None;
            }
        } else {
            // Primary button drag start
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(origin) = ctx.input(|i| i.pointer.press_origin())
                && self.is_inside_image(origin)
            {
                let img_pos = self.clamp_to_image(self.screen_to_image(origin));
                self.drag_start = Some(img_pos);
                self.on_drag_begin(img_pos, modifiers);
            }

            // Primary drag update
            if response.dragged_by(egui::PointerButton::Primary)
                && let (Some(start), Some(hover)) =
                    (self.drag_start, ctx.input(|i| i.pointer.hover_pos()))
            {
                let current = self.clamp_to_image(self.screen_to_image(hover));
                let delta_vec = current.to_vec2() - start.to_vec2();
                let delta_pos = Pos2::new(delta_vec.x, delta_vec.y);
                self.on_drag_update(start, delta_pos, modifiers);
            }

            // Primary drag end
            let drag_released = response.drag_stopped_by(egui::PointerButton::Primary);
            if drag_released && let Some(start) = self.drag_start.take() {
                let hover = ctx
                    .input(|i| i.pointer.hover_pos())
                    .map(|p| self.clamp_to_image(self.screen_to_image(p)))
                    .unwrap_or(start);
                let delta_vec = hover.to_vec2() - start.to_vec2();
                let delta_pos = Pos2::new(delta_vec.x, delta_vec.y);
                self.on_drag_end(start, delta_pos, modifiers);
            }

            // Click (single press-release without drag)
            if response.clicked_by(egui::PointerButton::Primary)
                && let Some(pos) = ctx.input(|i| i.pointer.interact_pos())
                && self.is_inside_image(pos)
            {
                let img_pos = self.clamp_to_image(self.screen_to_image(pos));
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
    // egui 0.37 replaced `update(&mut self, ctx, frame)` with
    // `ui(&mut self, ui, frame)`: the framework hands the app a root Ui now
    // rather than a bare Context, because panels are laid out inside a Ui
    // instead of against the Context.
    //
    // The body still works almost entirely in terms of the Context, so it is
    // taken back out of the Ui here rather than rewriting every call. Cloned
    // because the panels want `&mut ui` while `ctx` is still in use, and a
    // borrow would keep `ui` frozen. A Context is a handle around an Arc, so
    // this is a refcount bump rather than a copy of any UI state.
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();

        // Ensure texture is uploaded on first frame
        if !self.initialized {
            self.initialized = true;
            self.upload_base_texture(ctx);
            apply_theme(ctx, &self.config.theme);
        }

        // Before anything reads `zoom`. Every zoom figure is relative to this,
        // and on the first frames it is a placeholder 1.0 that the compositor
        // replaces a frame or two later -- which is exactly why fit mode
        // recomputes rather than deciding once.
        let ppp = ctx.pixels_per_point();
        if ppp.is_finite() && ppp > 0.0 {
            self.pixels_per_point = ppp;
        }
        // Ahead of the toolbars, which print the zoom percentage: the canvas
        // is not measured until the central panel runs, so fitting only there
        // would leave the readout a frame behind. The central panel fits again
        // against the fresh rect.
        self.refresh_zoom();

        self.apply_initial_window_size(ctx);

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

        // show_collapsible replaced show_animated in egui 0.37, and it takes
        // the expanded flag by &mut so a panel can collapse itself when its
        // handle is dragged. It cannot be `&mut self.toolbars_visible`,
        // because the closures below borrow all of `self` -- hence a local,
        // copied back once both panels have had their say.
        let mut toolbars_visible = self.toolbars_visible;

        // Top toolbar
        egui::Panel::top("tools")
            .frame(
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin {
                    top: 3,
                    bottom: 4,
                    left: 0,
                    right: 0,
                }),
            )
            .show_collapsible(ui, &mut toolbars_visible, |ui| {
                let available = ui.available_width();
                let leading = ((available - self.top_toolbar_content_width) / 2.0).max(0.0);
                let r = ui.horizontal(|ui| {
                    ui.add_space(leading);
                    let keybind_map = self.config.keybind_map();
                    let tool_key_map: std::collections::HashMap<String, char> =
                        keybind_map.into_iter().map(|(k, v)| (v, k)).collect();
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
                        ToolType::Delete,
                    ] {
                        let selected = self.active_tool == tool;
                        let tooltip = if let Some(&key) = tool_key_map.get(tool.config_name()) {
                            format!("{} ({})", tool.label(), key)
                        } else {
                            tool.label().to_string()
                        };
                        let resp = ui
                            .add_sized(
                                [30.0, 30.0],
                                egui::Button::selectable(
                                    selected,
                                    egui::RichText::new(tool.icon()).size(20.0),
                                ),
                            )
                            .on_hover_text(tooltip);
                        // Draw a momentary white border when activated by keyboard
                        if let Some((flash_tool, flash_time)) = self.key_flash
                            && flash_tool == tool
                            && flash_time.elapsed().as_millis() < 150
                        {
                            ui.painter().rect_stroke(
                                resp.rect,
                                CornerRadius::same(2),
                                Stroke::new(2.0_f32, Color32::WHITE),
                                egui::StrokeKind::Inside,
                            );
                            ui.ctx().request_repaint();
                        }
                        if resp.clicked() {
                            self.switch_tool(tool);
                        }
                    }
                    ui.separator();
                    if ui
                        .add_sized(
                            [30.0, 30.0],
                            egui::Button::new(egui::RichText::new("↩").size(20.0)),
                        )
                        .on_hover_text("Undo (z)")
                        .clicked()
                    {
                        self.undo();
                    }
                    if ui
                        .add_sized(
                            [30.0, 30.0],
                            egui::Button::new(egui::RichText::new("↪").size(20.0)),
                        )
                        .on_hover_text("Redo (y)")
                        .clicked()
                    {
                        self.redo();
                    }
                    ui.separator();
                    if ui
                        .add_sized(
                            [30.0, 30.0],
                            egui::Button::new(egui::RichText::new("\u{F018F}").size(20.0)),
                        )
                        .on_hover_text("Copy (c)")
                        .clicked()
                    {
                        self.pending_copy = true;
                    }
                    if ui
                        .add_sized(
                            [30.0, 30.0],
                            egui::Button::new(egui::RichText::new("\u{F0C7}").size(20.0)),
                        )
                        .on_hover_text("Save (s)")
                        .clicked()
                    {
                        self.pending_save = true;
                    }
                });
                self.top_toolbar_content_width = r.response.rect.width() - leading;
            });

        // Bottom toolbar
        egui::Panel::bottom("style_bar")
            .frame(
                egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin {
                    top: 4,
                    bottom: 4,
                    left: 0,
                    right: 0,
                }),
            )
            .show_collapsible(ui, &mut toolbars_visible, |ui| {
                let available = ui.available_width();
                let leading = ((available - self.bottom_toolbar_content_width) / 2.0).max(0.0);
                let r = ui.horizontal(|ui| {
                    ui.add_space(leading);
                    ui.spacing_mut().interact_size.y = 22.0;
                    for (size, label) in
                        &[(Size::Small, "S"), (Size::Medium, "M"), (Size::Large, "L")]
                    {
                        if ui
                            .add_sized(
                                [22.0, 22.0],
                                egui::Button::selectable(self.style.size == *size, *label),
                            )
                            .on_hover_text(format!("{} (Shift+{} | [ / ])", size.label(), label))
                            .clicked()
                        {
                            self.style.size = *size;
                        }
                    }
                    ui.separator();
                    ui.toggle_value(&mut self.style.fill, "Fill")
                        .on_hover_text("Fill (f)");
                    ui.separator();
                    let palette = self.config.palette_colors();
                    for (idx, &(r, g, b, a)) in palette.iter().enumerate() {
                        let c = Color32::from_rgba_unmultiplied(r, g, b, a);
                        let selected = self.style.color == Color { r, g, b, a };
                        let (resp, painter) =
                            ui.allocate_painter(egui::vec2(22.0, 22.0), egui::Sense::click());
                        painter.rect_filled(resp.rect, CornerRadius::same(3), c);
                        if selected {
                            painter.rect_stroke(
                                resp.rect.shrink(2.0),
                                CornerRadius::same(1),
                                Stroke::new(2.0_f32, Color32::WHITE),
                                egui::StrokeKind::Inside,
                            );
                        }
                        // Only the first 8 palette slots have number-key shortcuts ('1'..='8');
                        // any additional custom palette colors are mouse-only.
                        let resp = if idx < 8 {
                            let key = (b'1' + idx as u8) as char;
                            resp.on_hover_text(format!("Color {} ({})", idx + 1, key))
                        } else {
                            resp.on_hover_text(format!("Color {}", idx + 1))
                        };
                        if resp.clicked() {
                            self.style.color = Color { r, g, b, a };
                        }
                    }
                    ui.separator();
                    let zoom_factor = self.config.general.zoom_factor;
                    if ui
                        .add_sized(
                            [22.0, 22.0],
                            egui::Button::new(egui::RichText::new("−").size(16.0)),
                        )
                        .on_hover_text("Zoom Out (-)")
                        .clicked()
                    {
                        self.zoom_step(1.0 / zoom_factor);
                    }
                    ui.label(format!("Zoom: {:.0}%", self.zoom_scale() * 100.0));
                    if ui
                        .add_sized(
                            [22.0, 22.0],
                            egui::Button::new(egui::RichText::new("+").size(16.0)),
                        )
                        .on_hover_text("Zoom In (=)")
                        .clicked()
                    {
                        self.zoom_step(zoom_factor);
                    }
                    if ui.button("1:1").on_hover_text("Actual Size (0)").clicked() {
                        self.reset_zoom();
                    }
                    let mut fit = self.fit_to_window;
                    if ui
                        .toggle_value(&mut fit, "Fit")
                        .on_hover_text("Fit to Window (Shift+F)")
                        .changed()
                    {
                        self.fit_to_window = fit;
                        if !fit {
                            // Leaving fit mode holds the zoom it was showing,
                            // rather than jumping to whatever was asked for
                            // before fit took over.
                            self.requested_scale = self.zoom_scale();
                        }
                    }
                    if ui
                        .add_sized(
                            [22.0, 22.0],
                            egui::Button::new(egui::RichText::new("ℹ").size(14.0)),
                        )
                        .on_hover_text("Keyboard Shortcuts")
                        .clicked()
                    {
                        self.show_info = !self.show_info;
                    }
                });
                self.bottom_toolbar_content_width = r.response.rect.width() - leading;
            });

        // The window's minimum, from every constraint at once. Must run after
        // the toolbars have laid out, since it reads their measured widths.
        self.toolbars_visible = toolbars_visible;

        self.update_min_inner_size(ctx);

        let min_toolbar_w = self
            .top_toolbar_content_width
            .max(self.bottom_toolbar_content_width)
            + TOOLBAR_MIN_WIDTH_PADDING;
        if min_toolbar_w < 9999.0 && !self.toolbar_min_width_applied {
            self.toolbar_min_width_applied = true;
            // Measured against the size apply_initial_window_size ASKED
            // for, not against screen_rect, whenever a request is
            // outstanding. Both run in the same frame and this one runs
            // second, but a viewport command does not land until the next
            // frame -- so screen_rect here still reports the provisional
            // size from main.rs. Widening a window on the strength of a
            // measurement that is about to be replaced gets the decision
            // backwards: a small capture correctly sized down to the 400pt
            // minimum would be judged "wide enough" against the stale
            // value and never widened for the toolbar.
            let current = self
                .initial_size_request
                .unwrap_or_else(|| ctx.input(|i| i.viewport_rect()).size());
            if current.x < min_toolbar_w {
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    min_toolbar_w,
                    current.y,
                )));
            }
        }

        // Info window (separate OS window)
        if self.show_info {
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of("info_window"),
                egui::ViewportBuilder::default()
                    .with_title("Keyboard Shortcuts")
                    .with_inner_size([420.0, 615.0])
                    .with_resizable(false)
                    .with_minimize_button(false)
                    .with_maximize_button(false),
                |ctx, _class| {
                    if ctx.input(|i| {
                        i.viewport().close_requested()
                            || i.key_pressed(egui::Key::Escape)
                            || i.events
                                .iter()
                                .any(|e| matches!(e, egui::Event::Text(s) if s == "q"))
                    }) {
                        self.show_info = false;
                    }
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin {
                                left: 16,
                                right: 8,
                                top: 8,
                                bottom: 8,
                            }),
                        )
                        .show(ctx, |ui| {
                            egui::Grid::new("info_grid")
                                .num_columns(2)
                                .spacing([16.0, 4.0])
                                .striped(true)
                                .show(ui, |ui| {
                                    let header = |ui: &mut egui::Ui, text: &str| {
                                        ui.strong(text);
                                        ui.end_row();
                                    };
                                    let row = |ui: &mut egui::Ui, shortcut: &str, action: &str| {
                                        ui.monospace(shortcut);
                                        ui.label(action);
                                        ui.end_row();
                                    };

                                    header(ui, "Navigation");
                                    row(ui, "Scroll wheel", "Zoom in / out (centered on cursor)");
                                    row(ui, "= / +", "Zoom in");
                                    row(ui, "-", "Zoom out");
                                    row(ui, "0", "Actual size (100%)");
                                    row(ui, "Shift+F", "Fit image to window");
                                    row(ui, "Middle drag", "Pan");
                                    row(ui, "Pointer + drag", "Pan canvas");
                                    row(ui, "Pointer + drag annotation", "Move annotation");
                                    row(ui, "Arrow keys (Pointer tool)", "Pan canvas");

                                    ui.end_row();
                                    header(ui, "Tool Modifiers");
                                    row(ui, "Shift + Arrow / Line", "Snap to 15° angle increments");
                                    row(
                                        ui,
                                        "Shift + Rect / Ellipse",
                                        "Constrain to square / circle",
                                    );

                                    ui.end_row();
                                    header(ui, "Style");
                                    row(ui, "1 – 8", "Pick palette color");
                                    row(ui, "Shift+S / M / L", "Set size Small / Medium / Large");
                                    row(ui, "[ / ]", "Cycle size smaller / bigger (wraps)");
                                    row(ui, "f", "Toggle fill");

                                    ui.end_row();
                                    header(ui, "Actions");
                                    row(ui, "z / Ctrl+Z", "Undo");
                                    row(ui, "y / Ctrl+Y", "Redo");
                                    row(ui, "c / Ctrl+C", "Copy to clipboard");
                                    row(ui, "s / Ctrl+S", "Save to file");
                                    row(ui, "Ctrl+T", "Toggle toolbars");
                                    row(ui, "q", "Quit");
                                    row(ui, "Enter", "Configurable (default: copy & exit)");
                                    row(ui, "Escape", "Configurable (default: exit)");

                                    ui.end_row();
                                    header(ui, "Tips");
                                    row(
                                        ui,
                                        "m (on Marker tool)",
                                        "Press again to reset counter to 1",
                                    );
                                    row(ui, "d (Delete tool)", "Click an annotation to remove it");
                                });
                        });
                },
            );
        }

        // Central panel
        let frame = egui::Frame::NONE.fill(Color32::from_rgb(30, 30, 35));
        egui::CentralPanel::default().frame(frame).show(ui, |ui| {
            self.canvas_rect = ui.available_rect_before_wrap();
            // Again, now that the canvas has been measured: the call before
            // the panels was working from last frame's rect, which is a frame
            // stale while the window is being resized.
            self.refresh_zoom();

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

                // Consume Enter (without Shift) before TextEdit so it doesn't insert a newline.
                // Shift+Enter is left unconsumed so TextEdit handles it as a newline.
                let enter_pressed = ctx.input_mut(|i| {
                    if i.key_pressed(egui::Key::Enter) && !i.modifiers.shift {
                        i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                        true
                    } else {
                        false
                    }
                });
                let escape_pressed = ctx.input(|i| i.key_pressed(egui::Key::Escape));

                // Id::unique, not IdSalt::new: egui 0.37 split the old
                // Id::new in two, and this is a root id for a top-level Area
                // rather than a salt unique within some parent. Same hashing,
                // so the id is the same across frames -- which matters,
                // because the Area's position and focus are stored under it.
                egui::Area::new(egui::Id::unique("text_input"))
                    .fixed_pos(screen_pos)
                    .show(ctx, |ui| {
                        let resp = ui.add(
                            egui::TextEdit::multiline(&mut local_content)
                                .font(egui::FontId::proportional(font_size))
                                .desired_width(300.0)
                                // `.frame(true)` was here. In egui 0.37 frame()
                                // takes a Frame rather than a bool, and true
                                // only ever meant "the theme's frame" -- which
                                // is what an unset frame resolves to now. The
                                // call said nothing, so it is gone rather than
                                // spelled out again.
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

                if enter_pressed {
                    // Commit on Enter
                    if !local_content.is_empty() {
                        let ann = Annotation::Text {
                            pos,
                            content: local_content,
                            style: self.style,
                        };
                        self.commit_annotation(ann);
                    }
                    self.active_drawing = ActiveDrawing::None;
                } else if escape_pressed {
                    // Cancel on Escape
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
            let rounding =
                CornerRadius::same((*corner_roundness * zoom).round().clamp(0.0, 255.0) as u8);
            let lw = style.size.to_line_width(factor) * zoom;
            let color = style.color.to_egui();
            if style.fill {
                painter.rect_filled(r, rounding, color);
            } else {
                painter.rect_stroke(
                    r,
                    rounding,
                    Stroke::new(lw, color),
                    egui::StrokeKind::Inside,
                );
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
                    .map(i2s)
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
            painter.circle_stroke(sp, r, Stroke::new(1.5_f32, Color32::WHITE));
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
                    painter.rect_filled(r, CornerRadius::ZERO, hi_color);
                }
                HighlightAnnotation::Freehand { start, points } => {
                    if !points.is_empty() {
                        let screen_points: Vec<Pos2> = std::iter::once(*start)
                            .chain(
                                points
                                    .iter()
                                    .map(|&delta| Pos2::new(start.x + delta.x, start.y + delta.y)),
                            )
                            .map(i2s)
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

fn point_to_segment_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let ap = p - a;
    let len_sq = ab.x * ab.x + ab.y * ab.y;
    if len_sq < 1e-6 {
        return ap.length();
    }
    let t = ((ap.x * ab.x + ap.y * ab.y) / len_sq).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn annotation_hit_test(ann: &Annotation, pos: Pos2, threshold: f32) -> bool {
    match ann {
        Annotation::Arrow { start, end, .. } | Annotation::Line { start, end, .. } => {
            point_to_segment_dist(pos, *start, *end) < threshold
        }
        Annotation::Rectangle { top_left, size, .. } => {
            Rect::from_two_pos(*top_left, *top_left + *size)
                .expand(threshold)
                .contains(pos)
        }
        Annotation::Ellipse { center, radii, .. } => {
            let dx = (pos.x - center.x) / (radii.x + threshold);
            let dy = (pos.y - center.y) / (radii.y + threshold);
            dx * dx + dy * dy <= 1.0
        }
        Annotation::Brush { start, points, .. } => {
            if pos.distance(*start) < threshold {
                return true;
            }
            points
                .iter()
                .any(|&d| pos.distance(Pos2::new(start.x + d.x, start.y + d.y)) < threshold)
        }
        Annotation::Text { pos: ann_pos, .. } | Annotation::Marker { pos: ann_pos, .. } => {
            pos.distance(*ann_pos) < threshold * 3.0
        }
        Annotation::Blur { top_left, size, .. } => Rect::from_two_pos(*top_left, *top_left + *size)
            .expand(threshold)
            .contains(pos),
        Annotation::Highlight { kind, .. } => match kind {
            HighlightAnnotation::Block { top_left, size } => {
                Rect::from_two_pos(*top_left, *top_left + *size)
                    .expand(threshold)
                    .contains(pos)
            }
            HighlightAnnotation::Freehand { start, points } => {
                if pos.distance(*start) < threshold {
                    return true;
                }
                points
                    .iter()
                    .any(|&d| pos.distance(Pos2::new(start.x + d.x, start.y + d.y)) < threshold)
            }
        },
    }
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
