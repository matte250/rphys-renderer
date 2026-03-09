//! Pixel-level blending helpers that write directly into a [`Frame`]'s
//! `pixels` byte slice.
//!
//! All operations perform a straight-alpha "over" composite.  For typical VFX
//! use (semi-transparent particles over a fully-opaque background) this is
//! visually indistinguishable from premultiplied alpha compositing.
//!
//! Coordinate system: `(0, 0)` is the **top-left** pixel, `x` increases
//! right, `y` increases down — matching the renderer's pixel convention.

use rphys_scene::Color;

// ── blend_pixel ───────────────────────────────────────────────────────────────

/// Alpha-blend a single RGBA source pixel over the existing pixel at `(x, y)`.
///
/// `alpha` is in the range `[0.0, 1.0]`.  Out-of-bounds coordinates are
/// silently ignored (no panic).
///
/// The underlying pixel format is RGBA with 4 bytes per pixel in row-major
/// order, which matches the layout produced by `tiny-skia`.
#[allow(clippy::too_many_arguments)]
#[inline]
pub fn blend_pixel(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    r: u8,
    g: u8,
    b: u8,
    alpha: f32,
) {
    // Bounds check.
    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.001 {
        return;
    }

    let idx = (y as usize * width as usize + x as usize) * 4;
    // Guard against any buffer-size mismatches.
    if idx + 3 >= pixels.len() {
        return;
    }

    // Straight-alpha "over" blend:
    //   out_c = src_c * alpha + dst_c * (1 - alpha)
    let inv = 1.0 - alpha;
    pixels[idx] = (r as f32 * alpha + pixels[idx] as f32 * inv) as u8;
    pixels[idx + 1] = (g as f32 * alpha + pixels[idx + 1] as f32 * inv) as u8;
    pixels[idx + 2] = (b as f32 * alpha + pixels[idx + 2] as f32 * inv) as u8;
    pixels[idx + 3] = (255.0_f32 * alpha + pixels[idx + 3] as f32 * inv) as u8;
}

// ── draw_dot ──────────────────────────────────────────────────────────────────

/// Draw a filled, soft-edged circle centred at `(cx, cy)` with the given
/// `radius` in pixels.
///
/// The circle has a hard core and a soft anti-aliased edge.  The alpha at
/// each pixel is `alpha * (1 - (dist / radius)²)`, which produces a gentle
/// falloff at the boundary while keeping the centre fully opaque.
///
/// Pixels outside `[0, width) × [0, height)` are silently skipped.
#[allow(clippy::too_many_arguments)]
pub fn draw_dot(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    radius: f32,
    color: Color,
    alpha: f32,
) {
    if radius <= 0.0 || alpha <= 0.001 {
        return;
    }

    let r2 = radius * radius;
    let x0 = (cx - radius).floor() as i32;
    let y0 = (cy - radius).floor() as i32;
    let x1 = (cx + radius).ceil() as i32;
    let y1 = (cy + radius).ceil() as i32;

    for py in y0..=y1 {
        for px in x0..=x1 {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let dist2 = dx * dx + dy * dy;
            if dist2 <= r2 {
                // Smooth edge: fade out quadratically near the boundary.
                let edge_alpha = (1.0 - dist2 / r2).max(0.0);
                blend_pixel(
                    pixels,
                    width,
                    height,
                    px,
                    py,
                    color.r,
                    color.g,
                    color.b,
                    alpha * edge_alpha,
                );
            }
        }
    }
}

// ── draw_glow ─────────────────────────────────────────────────────────────────

/// Draw a radial glow centred at `(cx, cy)` with the given outer `radius`.
///
/// Unlike `draw_dot`, the glow is brightest at the **edge** of the ball and
/// fades to transparent toward the outer radius.  This creates a soft halo
/// effect suitable for boost flashes.
///
/// Alpha at each pixel: `alpha * (1 - dist / radius)²`.
#[allow(clippy::too_many_arguments)]
pub fn draw_glow(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    cx: f32,
    cy: f32,
    radius: f32,
    color: Color,
    alpha: f32,
) {
    if radius <= 0.0 || alpha <= 0.001 {
        return;
    }

    let x0 = (cx - radius).floor() as i32;
    let y0 = (cy - radius).floor() as i32;
    let x1 = (cx + radius).ceil() as i32;
    let y1 = (cy + radius).ceil() as i32;

    for py in y0..=y1 {
        for px in x0..=x1 {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist <= radius {
                // Quadratic falloff: bright near center, fades to edge.
                let t = 1.0 - dist / radius;
                let pixel_alpha = alpha * t * t;
                blend_pixel(
                    pixels,
                    width,
                    height,
                    px,
                    py,
                    color.r,
                    color.g,
                    color.b,
                    pixel_alpha,
                );
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_pixels(w: u32, h: u32) -> Vec<u8> {
        vec![0u8; (w * h * 4) as usize]
    }

    #[test]
    fn test_blend_pixel_basic() {
        let mut px = blank_pixels(4, 4);
        blend_pixel(&mut px, 4, 4, 1, 1, 255, 0, 0, 1.0);
        let idx = (1 * 4 + 1) * 4;
        assert_eq!(px[idx], 255, "R channel should be 255");
        assert_eq!(px[idx + 1], 0, "G channel should be 0");
        assert_eq!(px[idx + 2], 0, "B channel should be 0");
    }

    #[test]
    fn test_blend_pixel_out_of_bounds_no_panic() {
        let mut px = blank_pixels(4, 4);
        // None of these should panic.
        blend_pixel(&mut px, 4, 4, -1, 0, 255, 0, 0, 1.0);
        blend_pixel(&mut px, 4, 4, 4, 0, 255, 0, 0, 1.0);
        blend_pixel(&mut px, 4, 4, 0, -1, 255, 0, 0, 1.0);
        blend_pixel(&mut px, 4, 4, 0, 4, 255, 0, 0, 1.0);
        // Buffer should remain all zeros.
        assert!(px.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_draw_dot_writes_pixels() {
        let (w, h) = (32u32, 32u32);
        let mut px = blank_pixels(w, h);
        let color = Color::rgb(0, 255, 0);
        draw_dot(&mut px, w, h, 16.0, 16.0, 4.0, color, 1.0);

        // Center pixel should be green.
        let idx = (16 * w as usize + 16) * 4;
        assert!(
            px[idx + 1] > 0,
            "green channel should be non-zero at center"
        );
    }

    #[test]
    fn test_draw_dot_stays_in_bounds() {
        let (w, h) = (8u32, 8u32);
        let mut px = blank_pixels(w, h);
        let color = Color::rgb(255, 255, 255);
        // Draw a dot partially outside the frame — should not panic.
        draw_dot(&mut px, w, h, -2.0, -2.0, 4.0, color, 1.0);
        draw_dot(&mut px, w, h, 10.0, 10.0, 4.0, color, 1.0);
        // No assertion needed — the test passes if it does not panic.
    }

    #[test]
    fn test_draw_glow_writes_pixels() {
        let (w, h) = (32u32, 32u32);
        let mut px = blank_pixels(w, h);
        let color = Color::rgb(255, 165, 0);
        draw_glow(&mut px, w, h, 16.0, 16.0, 8.0, color, 1.0);

        // At least some pixel near the center should be affected.
        let idx = (16 * w as usize + 16) * 4;
        assert!(
            px[idx] > 0 || px[idx + 3] > 0,
            "glow should write at least one non-zero channel near center"
        );
    }

    #[test]
    fn test_blend_pixel_partial_alpha() {
        let mut px = blank_pixels(2, 2);
        // Start with a fully white background at (0,0).
        px[0] = 200;
        px[1] = 200;
        px[2] = 200;
        px[3] = 255;
        // Blend a 50% red over it.
        blend_pixel(&mut px, 2, 2, 0, 0, 255, 0, 0, 0.5);
        // Output red should be between 100 and 255 (mix of 255 and 200).
        assert!(px[0] > 100 && px[0] <= 255, "R after blend: {}", px[0]);
        // Green should be reduced (0 * 0.5 + 200 * 0.5 = 100).
        assert!(px[1] > 50 && px[1] < 200, "G after blend: {}", px[1]);
    }
}
