//! Internal text rasterizer backed by `fontdue`.
//!
//! [`TextRenderer`] is private to `rphys-overlay`. All public text drawing
//! goes through [`crate::OverlayRenderer`].

use fontdue::{Font, FontSettings};
use rphys_renderer::Frame;
use rphys_scene::Color;

/// Roboto Bold embedded at compile time. ~293 KB — acceptable for a CLI tool.
static FONT_BYTES: &[u8] = include_bytes!("fonts/Roboto-Bold.ttf");

// ── TextRenderer ──────────────────────────────────────────────────────────────

/// Rasterises glyphs from the bundled Roboto Bold font and composites them
/// into a [`Frame`] buffer.
pub(crate) struct TextRenderer {
    font: Font,
}

impl TextRenderer {
    /// Initialise the renderer by parsing the embedded font bytes.
    ///
    /// # Panics
    ///
    /// Panics if the embedded font bytes are malformed. Since they are baked
    /// into the binary at compile time this should never happen in practice.
    pub(crate) fn new() -> Self {
        let font = Font::from_bytes(FONT_BYTES, FontSettings::default())
            .expect("embedded Roboto-Bold font is always valid");
        Self { font }
    }

    /// Rasterise `text` and alpha-blend it into `frame`.
    ///
    /// `(x, y)` is the **top-left** corner of the text line in pixel space.
    /// Individual glyphs are positioned relative to the line baseline, which
    /// is derived from the font's ascent metric.
    ///
    /// `alpha` multiplies the glyph coverage mask — use `1.0` for fully
    /// opaque text, lower values for drop shadows or dimmed text.
    ///
    /// Pixels outside the frame bounds are silently clipped.
    //
    // The 7 parameters (plus &self) are required by the overlay API contract;
    // grouping into a struct would add indirection for an internal function.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_text(
        &self,
        frame: &mut Frame,
        text: &str,
        x: i32,
        y: i32,
        size: f32,
        color: Color,
        alpha: f32,
    ) {
        // Derive the baseline from the ascent so `y` means "top of line".
        let ascent = self
            .font
            .horizontal_line_metrics(size)
            .map(|m| m.ascent)
            .unwrap_or(size * 0.78);
        let baseline = y + ascent as i32;

        let frame_w = frame.width as i32;
        let frame_h = frame.height as i32;

        let mut cursor_x = x;

        for ch in text.chars() {
            let (metrics, bitmap) = self.font.rasterize(ch, size);

            if metrics.width > 0 && metrics.height > 0 {
                let glyph_left = cursor_x + metrics.xmin;
                let glyph_top = baseline - (metrics.ymin + metrics.height as i32);

                for row in 0..metrics.height {
                    let py = glyph_top + row as i32;
                    if py < 0 || py >= frame_h {
                        continue;
                    }

                    for col in 0..metrics.width {
                        let px = glyph_left + col as i32;
                        if px < 0 || px >= frame_w {
                            continue;
                        }

                        let coverage = bitmap[row * metrics.width + col];
                        if coverage == 0 {
                            continue;
                        }

                        let glyph_alpha = (coverage as f32 / 255.0) * alpha;
                        blit_pixel(frame, px as u32, py as u32, color, glyph_alpha);
                    }
                }
            }

            // Advance the cursor. Fall back to half of `size` for glyphs with
            // zero advance (e.g. unsupported emoji in a Latin font).
            let advance = metrics.advance_width;
            cursor_x += if advance > 0.5 {
                advance as i32
            } else {
                (size * 0.5) as i32
            };
        }
    }

    /// Estimate the pixel bounding box of `text` at `size` px.
    ///
    /// Returns `(width, height)`.
    pub(crate) fn measure(&self, text: &str, size: f32) -> (u32, u32) {
        let height = self
            .font
            .horizontal_line_metrics(size)
            .map(|m| (m.ascent - m.descent) as u32)
            .unwrap_or(size as u32);

        let width: f32 = text
            .chars()
            .map(|ch| self.font.metrics(ch, size).advance_width)
            .sum();

        (width as u32, height)
    }
}

// ── Pixel compositing helpers ─────────────────────────────────────────────────

/// Alpha-blend `color` at opacity `alpha` onto the pixel at `(px, py)`.
///
/// Uses standard over-compositing (straight alpha):
/// ```text
/// out = src * alpha + dst * (1 - alpha)
/// ```
///
/// # Panics
///
/// Panics if `px >= frame.width` or `py >= frame.height`. Callers must
/// perform bounds checks before calling this function.
pub(crate) fn blit_pixel(frame: &mut Frame, px: u32, py: u32, color: Color, alpha: f32) {
    let idx = ((py * frame.width + px) * 4) as usize;
    let dst = &mut frame.pixels[idx..idx + 4];
    let inv = 1.0 - alpha;

    dst[0] = (color.r as f32 * alpha + dst[0] as f32 * inv).min(255.0) as u8;
    dst[1] = (color.g as f32 * alpha + dst[1] as f32 * inv).min(255.0) as u8;
    dst[2] = (color.b as f32 * alpha + dst[2] as f32 * inv).min(255.0) as u8;
    dst[3] = (dst[3] as f32 + color.a as f32 * alpha).min(255.0) as u8;
}

/// Fill a rectangle with a solid color, alpha-blending into existing pixels.
pub(crate) fn fill_rect(frame: &mut Frame, x: i32, y: i32, w: i32, h: i32, color: Color) {
    let fw = frame.width as i32;
    let fh = frame.height as i32;

    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + w).min(fw);
    let y1 = (y + h).min(fh);

    if x0 >= x1 || y0 >= y1 {
        return;
    }

    let src_alpha = color.a as f32 / 255.0;
    for py in y0..y1 {
        for px in x0..x1 {
            blit_pixel(frame, px as u32, py as u32, color, src_alpha);
        }
    }
}

/// Draw a horizontal line at pixel row `py`, spanning `x0..x1`.
///
/// `thickness` controls how many pixel rows the line occupies.
pub(crate) fn draw_hline(
    frame: &mut Frame,
    py: i32,
    x0: i32,
    x1: i32,
    color: Color,
    thickness: i32,
) {
    let fw = frame.width as i32;
    let fh = frame.height as i32;
    let px0 = x0.max(0);
    let px1 = x1.min(fw);
    if px0 >= px1 {
        return;
    }
    let src_alpha = color.a as f32 / 255.0;

    for dy in 0..thickness {
        let row = py + dy;
        if row < 0 || row >= fh {
            continue;
        }
        for px in px0..px1 {
            blit_pixel(frame, px as u32, row as u32, color, src_alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_renderer_initialises() {
        let _tr = TextRenderer::new(); // must not panic
    }

    #[test]
    fn measure_returns_nonzero_for_ascii() {
        let tr = TextRenderer::new();
        let (w, h) = tr.measure("Hello", 18.0);
        assert!(w > 0, "measured width should be nonzero for ASCII text");
        assert!(h > 0, "measured height should be nonzero");
    }

    #[test]
    fn draw_text_clips_at_negative_offset() {
        // Draw text at x=-1000, y=-1000 — should not panic
        let tr = TextRenderer::new();
        let mut frame = Frame::new(100, 100);
        tr.draw_text(&mut frame, "Hello", -1000, -1000, 18.0, Color::WHITE, 1.0);
    }

    #[test]
    fn draw_text_clips_at_far_right_offset() {
        // Draw text far off-screen to the right — should not panic
        let tr = TextRenderer::new();
        let mut frame = Frame::new(100, 100);
        tr.draw_text(&mut frame, "Hello", 99_000, 0, 18.0, Color::WHITE, 1.0);
    }

    #[test]
    fn blit_pixel_blends_opaque_color() {
        let mut frame = Frame::new(4, 4);
        // Set background to black (already zeros)
        blit_pixel(&mut frame, 2, 2, Color::rgb(255, 0, 0), 1.0);
        let idx = ((2 * 4 + 2) * 4) as usize;
        assert_eq!(frame.pixels[idx], 255); // R
        assert_eq!(frame.pixels[idx + 1], 0); // G
        assert_eq!(frame.pixels[idx + 2], 0); // B
    }

    #[test]
    fn fill_rect_writes_pixels() {
        let mut frame = Frame::new(50, 50);
        fill_rect(&mut frame, 10, 10, 20, 20, Color::rgba(0, 0, 200, 200));
        // Center of rect should be non-zero
        let idx = ((15 * 50 + 15) * 4) as usize;
        assert!(frame.pixels[idx + 2] > 0, "blue channel should be written");
    }

    #[test]
    fn fill_rect_clips_correctly() {
        let mut frame = Frame::new(20, 20);
        // Rect mostly off-screen — should not panic and should not write OOB
        fill_rect(&mut frame, 15, 15, 100, 100, Color::rgba(255, 0, 0, 255));
        // In-bounds portion should be written
        let idx = ((16 * 20 + 16) * 4) as usize;
        assert!(frame.pixels[idx] > 0, "in-bounds pixel should be written");
    }

    #[test]
    fn draw_text_writes_pixels_for_text() {
        let tr = TextRenderer::new();
        let mut frame = Frame::new(200, 60);
        tr.draw_text(&mut frame, "Hi", 5, 5, 18.0, Color::WHITE, 1.0);
        // At least one pixel should be non-zero
        let any_nonzero = frame.pixels.iter().any(|&b| b > 0);
        assert!(any_nonzero, "drawing text should modify frame pixels");
    }
}
