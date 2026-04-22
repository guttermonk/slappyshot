use std::io::Write;
use std::sync::Arc;

use ab_glyph::{FontRef, PxScale};
use image::{Rgba, RgbaImage};
use imageproc::drawing::{
    draw_filled_circle_mut, draw_filled_ellipse_mut, draw_filled_rect_mut,
    draw_hollow_ellipse_mut, draw_hollow_rect_mut, draw_line_segment_mut, draw_text_mut,
};
use imageproc::rect::Rect as IRect;

use crate::tools::{Annotation, HighlightAnnotation};

// Use the notosans crate to provide font bytes at compile time
static FONT_BYTES: &[u8] = notosans::REGULAR_TTF;

pub fn render_to_image(
    source: &RgbaImage,
    annotations: &[Annotation],
    _corner_roundness: f32,
) -> RgbaImage {
    let mut canvas = source.clone();
    for ann in annotations {
        draw_annotation(&mut canvas, ann);
    }
    canvas
}

fn draw_annotation(img: &mut RgbaImage, ann: &Annotation) {
    match ann {
        Annotation::Line { start, end, style } => {
            let color = to_rgba(style.color);
            let w = style
                .size
                .to_line_width(style.annotation_size_factor)
                .round()
                .max(1.0) as u8;
            draw_thick_line(img, *start, *end, w, color);
        }
        Annotation::Arrow { start, end, style } => {
            draw_arrow_on_image(img, *start, *end, style);
        }
        Annotation::Rectangle {
            top_left,
            size,
            style,
            ..
        } => {
            let (tl, sz) = normalize_rect(*top_left, *size);
            let x = tl.x.round() as i32;
            let y = tl.y.round() as i32;
            let w = sz.x.round().max(1.0) as u32;
            let h = sz.y.round().max(1.0) as u32;
            let color = to_rgba(style.color);
            let lw = style
                .size
                .to_line_width(style.annotation_size_factor)
                .round()
                .max(1.0) as i32;
            if style.fill {
                draw_filled_rect_mut(img, IRect::at(x, y).of_size(w, h), color);
            } else {
                for i in 0..lw {
                    let iw = i as u32 * 2;
                    if w > iw && h > iw {
                        draw_hollow_rect_mut(
                            img,
                            IRect::at(x + i, y + i).of_size(w - iw, h - iw),
                            color,
                        );
                    }
                }
            }
        }
        Annotation::Ellipse {
            center,
            radii,
            style,
        } => {
            let cx = center.x.round() as i32;
            let cy = center.y.round() as i32;
            let rx = radii.x.abs().round().max(1.0) as i32;
            let ry = radii.y.abs().round().max(1.0) as i32;
            let color = to_rgba(style.color);
            if style.fill {
                draw_filled_ellipse_mut(img, (cx, cy), rx, ry, color);
            } else {
                draw_hollow_ellipse_mut(img, (cx, cy), rx, ry, color);
            }
        }
        Annotation::Brush {
            start,
            points,
            style,
        } => {
            let color = to_rgba(style.color);
            let w = style
                .size
                .to_line_width(style.annotation_size_factor)
                .round()
                .max(1.0) as u8;
            if points.is_empty() {
                return;
            }
            let mut prev = *start;
            for &delta in points {
                let next = egui::Pos2::new(start.x + delta.x, start.y + delta.y);
                draw_thick_line(img, prev, next, w, color);
                prev = next;
            }
        }
        Annotation::Text {
            pos,
            content,
            style,
        } => {
            if let Ok(font) = FontRef::try_from_slice(FONT_BYTES) {
                let font_size =
                    style.size.to_text_size(style.annotation_size_factor) as f32;
                let scale = PxScale::from(font_size);
                let color = to_rgba(style.color);
                let x = pos.x.round() as i32;
                let mut y = pos.y.round() as i32;
                for line in content.lines() {
                    draw_text_mut(img, color, x, y, scale, &font, line);
                    y += (font_size * 1.2) as i32;
                }
            }
        }
        Annotation::Marker { pos, number, style } => {
            let cx = pos.x.round() as i32;
            let cy = pos.y.round() as i32;
            let font_size = style.size.to_marker_size(style.annotation_size_factor);
            let r = (font_size * 0.8).round() as i32;
            let color = to_rgba(style.color);
            draw_filled_circle_mut(img, (cx, cy), r, color);

            if let Ok(font) = FontRef::try_from_slice(FONT_BYTES) {
                let scale = PxScale::from(font_size * 0.8);
                let text = number.to_string();
                let tx = cx - (font_size * 0.25 * text.len() as f32) as i32;
                let ty = cy - (font_size * 0.4) as i32;
                let luma = (0.2126 * style.color.r as f32
                    + 0.7152 * style.color.g as f32
                    + 0.0722 * style.color.b as f32)
                    / 255.0;
                let text_color = if luma > 0.5 {
                    Rgba([0, 0, 0, 255])
                } else {
                    Rgba([255, 255, 255, 255])
                };
                draw_text_mut(img, text_color, tx, ty, scale, &font, &text);
            }
        }
        Annotation::Blur {
            top_left,
            blur_w,
            blur_h,
            blurred_pixels,
            ..
        } => {
            let x0 = top_left.x.round().max(0.0) as u32;
            let y0 = top_left.y.round().max(0.0) as u32;
            for dy in 0..*blur_h {
                for dx in 0..*blur_w {
                    let px = x0 + dx;
                    let py = y0 + dy;
                    if px < img.width() && py < img.height() {
                        let i = ((dy * blur_w + dx) * 4) as usize;
                        if i + 3 < blurred_pixels.len() {
                            img.put_pixel(
                                px,
                                py,
                                Rgba([
                                    blurred_pixels[i],
                                    blurred_pixels[i + 1],
                                    blurred_pixels[i + 2],
                                    blurred_pixels[i + 3],
                                ]),
                            );
                        }
                    }
                }
            }
        }
        Annotation::Highlight { kind, style } => {
            let alpha = 100u8;
            match kind {
                HighlightAnnotation::Block { top_left, size } => {
                    let (tl, sz) = normalize_rect(*top_left, *size);
                    let x = tl.x.round() as i32;
                    let y = tl.y.round() as i32;
                    let w = sz.x.round().max(1.0) as u32;
                    let h = sz.y.round().max(1.0) as u32;
                    let color = Rgba([style.color.r, style.color.g, style.color.b, alpha]);
                    blend_rect(img, x, y, w, h, color);
                }
                HighlightAnnotation::Freehand { start, points } => {
                    let lw = style
                        .size
                        .to_highlight_width(style.annotation_size_factor)
                        .round()
                        .max(1.0) as u8;
                    let color = Rgba([style.color.r, style.color.g, style.color.b, alpha]);
                    if points.is_empty() {
                        return;
                    }
                    let mut prev = *start;
                    for &delta in points {
                        let next = egui::Pos2::new(start.x + delta.x, start.y + delta.y);
                        blend_thick_line(img, prev, next, lw, color);
                        prev = next;
                    }
                }
            }
        }
    }
}

fn normalize_rect(top_left: egui::Pos2, size: egui::Vec2) -> (egui::Pos2, egui::Vec2) {
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
    (
        egui::Pos2::new(x, y),
        egui::Vec2::new(size.x.abs(), size.y.abs()),
    )
}

fn to_rgba(color: crate::style::Color) -> Rgba<u8> {
    Rgba([color.r, color.g, color.b, color.a])
}

fn draw_thick_line(
    img: &mut RgbaImage,
    start: egui::Pos2,
    end: egui::Pos2,
    width: u8,
    color: Rgba<u8>,
) {
    if width == 0 {
        return;
    }
    let hw = (width as f32 / 2.0) as i32;
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 0.001 {
        // Draw a single dot
        let cx = start.x.round() as i32;
        let cy = start.y.round() as i32;
        for oy in -hw..=hw {
            for ox in -hw..=hw {
                let px = (cx + ox) as u32;
                let py = (cy + oy) as u32;
                if px < img.width() && py < img.height() {
                    img.put_pixel(px, py, color);
                }
            }
        }
        return;
    }
    let px = -dy / len;
    let py = dx / len;
    for i in -hw..=hw {
        let ox = px * i as f32;
        let oy = py * i as f32;
        draw_line_segment_mut(
            img,
            (start.x + ox, start.y + oy),
            (end.x + ox, end.y + oy),
            color,
        );
    }
}

fn draw_arrow_on_image(
    img: &mut RgbaImage,
    start: egui::Pos2,
    end: egui::Pos2,
    style: &crate::style::Style,
) {
    let color = to_rgba(style.color);
    let lw = style
        .size
        .to_line_width(style.annotation_size_factor)
        .round()
        .max(1.0) as u8;

    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return;
    }

    let dir_x = dx / len;
    let dir_y = dy / len;
    let head_len = style.size.to_arrow_head_length(style.annotation_size_factor);
    let half_angle = 60.0f32.to_radians() / 2.0;

    // Draw tail line
    draw_thick_line(img, start, end, lw, color);

    // Arrowhead wing 1: rotate -dir by +half_angle
    let cos_a = half_angle.cos();
    let sin_a = half_angle.sin();
    let h1x = (-dir_x) * cos_a + (-dir_y) * (-sin_a);
    let h1y = (-dir_x) * sin_a + (-dir_y) * cos_a;
    let h1 = egui::Pos2::new(end.x + h1x * head_len, end.y + h1y * head_len);

    // Arrowhead wing 2: rotate -dir by -half_angle
    let h2x = (-dir_x) * cos_a + (-dir_y) * sin_a;
    let h2y = (-dir_x) * (-sin_a) + (-dir_y) * cos_a;
    let h2 = egui::Pos2::new(end.x + h2x * head_len, end.y + h2y * head_len);

    draw_thick_line(img, end, h1, lw, color);
    draw_thick_line(img, end, h2, lw, color);
}

fn blend_rect(img: &mut RgbaImage, x: i32, y: i32, w: u32, h: u32, color: Rgba<u8>) {
    let x0 = x.max(0) as u32;
    let y0 = y.max(0) as u32;
    let x1 = (x + w as i32).max(0).min(img.width() as i32) as u32;
    let y1 = (y + h as i32).max(0).min(img.height() as i32) as u32;
    let a = color[3] as f32 / 255.0;
    for py in y0..y1 {
        for px in x0..x1 {
            let orig = *img.get_pixel(px, py);
            let blended = Rgba([
                (color[0] as f32 * a + orig[0] as f32 * (1.0 - a)) as u8,
                (color[1] as f32 * a + orig[1] as f32 * (1.0 - a)) as u8,
                (color[2] as f32 * a + orig[2] as f32 * (1.0 - a)) as u8,
                255,
            ]);
            img.put_pixel(px, py, blended);
        }
    }
}

fn blend_thick_line(
    img: &mut RgbaImage,
    start: egui::Pos2,
    end: egui::Pos2,
    width: u8,
    color: Rgba<u8>,
) {
    let hw = (width as f32 / 2.0).ceil() as i32;
    let dx = end.x - start.x;
    let dy = end.y - start.y;
    let len = (dx * dx + dy * dy).sqrt();
    let steps = len.ceil() as usize;
    for i in 0..=steps {
        let t = if steps == 0 {
            0.0f32
        } else {
            i as f32 / steps as f32
        };
        let cx = (start.x + dx * t).round() as i32;
        let cy = (start.y + dy * t).round() as i32;
        blend_rect(img, cx - hw, cy - hw, width as u32, width as u32, color);
    }
}

pub fn copy_to_clipboard(img: &RgbaImage, copy_command: Option<&str>) -> anyhow::Result<()> {
    if let Some(cmd) = copy_command {
        let mut png_bytes: Vec<u8> = Vec::new();
        {
            let mut cursor = std::io::Cursor::new(&mut png_bytes);
            img.write_to(&mut cursor, image::ImageFormat::Png)?;
        }
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        if let Some(stdin) = child.stdin.as_mut() {
            stdin.write_all(&png_bytes)?;
        }
        child.wait()?;
    } else {
        let mut clipboard = arboard::Clipboard::new()?;
        clipboard.set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: std::borrow::Cow::Borrowed(img.as_raw()),
        })?;
    }
    Ok(())
}

// Make Arc<Vec<u8>> available for use in blur computation
pub fn compute_blur_pixels(
    source: &RgbaImage,
    top_left: egui::Pos2,
    size: egui::Vec2,
    sigma: f32,
) -> (Arc<Vec<u8>>, u32, u32) {
    let (tl, sz) = normalize_rect(top_left, size);
    let x = tl.x.round().max(0.0) as u32;
    let y = tl.y.round().max(0.0) as u32;
    let w = (sz.x.round() as u32)
        .clamp(1, source.width().saturating_sub(x).max(1));
    let h = (sz.y.round() as u32)
        .clamp(1, source.height().saturating_sub(y).max(1));
    let sub = image::imageops::crop_imm(source, x, y, w, h).to_image();
    let blurred = image::imageops::blur(&sub, sigma);
    (Arc::new(blurred.into_raw()), w, h)
}
