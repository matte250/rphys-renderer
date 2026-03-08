//! CPU software renderer using `tiny-skia`.
//!
//! Converts a [`PhysicsState`] snapshot into a raw RGBA [`Frame`] buffer.
//!
//! ## Coordinate system
//!
//! Physics uses a Y-up coordinate system (origin at bottom-left).
//! The renderer flips Y when converting to pixel space:
//!
//! ```text
//! pixel_x = (world_x - camera_origin.x) * scale
//! pixel_y = frame_height - (world_y - camera_origin.y) * scale
//! ```
//!
//! ## Opacity rules
//!
//! - Dynamic / kinematic bodies: rendered at full opacity.
//! - Static bodies: rendered at 80% of their defined alpha.

use rphys_physics::types::{BodyState, PhysicsState};
use rphys_scene::{BodyType, Color, ShapeKind, Vec2};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

// ── Frame ─────────────────────────────────────────────────────────────────────

/// Raw RGBA pixel buffer produced by the renderer.
///
/// Pixels are stored in row-major order, top-left to bottom-right.
/// Each pixel is 4 bytes: `[R, G, B, A]`.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Raw RGBA pixel data (`width × height × 4` bytes).
    pub pixels: Vec<u8>,
}

impl Frame {
    /// Create a new blank (all-zero) frame.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0u8; (width * height * 4) as usize],
        }
    }

    /// Return the RGBA components of the pixel at `(x, y)`.
    ///
    /// `(0, 0)` is the top-left corner.
    ///
    /// # Panics
    ///
    /// Panics if `x >= self.width` or `y >= self.height`.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(x < self.width, "x out of bounds");
        assert!(y < self.height, "y out of bounds");
        let idx = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[idx],
            self.pixels[idx + 1],
            self.pixels[idx + 2],
            self.pixels[idx + 3],
        ]
    }
}

// ── RenderContext ─────────────────────────────────────────────────────────────

/// Configuration for mapping the physics world into pixel space.
#[derive(Debug, Clone)]
pub struct RenderContext {
    /// Output frame width in pixels.
    pub width: u32,
    /// Output frame height in pixels.
    pub height: u32,
    /// World-space position of the pixel (0, 0) — i.e. the top-left corner of
    /// the frame corresponds to this world coordinate.
    pub camera_origin: Vec2,
    /// Pixels per meter. Larger values zoom in.
    pub scale: f32,
    /// Background fill color.
    pub background_color: Color,
}

// ── Renderer trait ────────────────────────────────────────────────────────────

/// A strategy for rendering a physics-world snapshot into a pixel frame.
pub trait Renderer {
    /// Render `state` into a [`Frame`] using the provided context.
    fn render(&self, state: &PhysicsState, ctx: &RenderContext) -> Frame;
}

// ── TinySkiaRenderer ──────────────────────────────────────────────────────────

/// CPU software renderer backed by the `tiny-skia` 2D graphics library.
///
/// Supports circles, rectangles, and convex polygons.
pub struct TinySkiaRenderer;

impl Renderer for TinySkiaRenderer {
    fn render(&self, state: &PhysicsState, ctx: &RenderContext) -> Frame {
        // Guard against zero-size frames.
        let Some(mut pixmap) = Pixmap::new(ctx.width, ctx.height) else {
            return Frame::new(ctx.width, ctx.height);
        };

        // ── background ────────────────────────────────────────────────────────
        let bg = to_skia_color(ctx.background_color, 1.0);
        pixmap.fill(bg);

        // ── bodies ────────────────────────────────────────────────────────────
        for body in &state.bodies {
            if !body.is_alive {
                continue;
            }
            render_body(&mut pixmap, body, ctx);
        }

        // ── extract pixel data ────────────────────────────────────────────────
        // tiny-skia stores pixels as premultiplied RGBA.  For fully-opaque
        // shapes the values match non-premultiplied exactly; for translucent
        // shapes callers that need straight alpha should un-premultiply.
        let raw = pixmap.data().to_vec();
        Frame {
            width: ctx.width,
            height: ctx.height,
            pixels: raw,
        }
    }
}

// ── Internal rendering helpers ────────────────────────────────────────────────

/// Render a single body into `pixmap`.
fn render_body(pixmap: &mut Pixmap, body: &BodyState, ctx: &RenderContext) {
    let alpha_factor = match body.body_type {
        BodyType::Static => 0.8,
        _ => 1.0,
    };

    let fill_color = to_skia_color(body.color, alpha_factor);
    let stroke_color = darken_color(body.color, alpha_factor);

    let (cx, cy) = world_to_pixel(body.position, ctx);

    match &body.shape {
        ShapeKind::Circle { radius } => {
            let radius_px = radius * ctx.scale;
            draw_circle(pixmap, cx, cy, radius_px, fill_color, stroke_color);
        }
        ShapeKind::Rectangle { width, height } => {
            let w_px = width * ctx.scale;
            let h_px = height * ctx.scale;
            draw_rect(
                pixmap,
                cx,
                cy,
                w_px,
                h_px,
                body.rotation,
                fill_color,
                stroke_color,
            );
        }
        ShapeKind::Polygon { vertices } => {
            draw_polygon(
                pixmap,
                body.position,
                body.rotation,
                vertices,
                ctx,
                fill_color,
                stroke_color,
            );
        }
    }
}

/// Draw a filled circle with a 1-px stroke.
fn draw_circle(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    radius: f32,
    fill: tiny_skia::Color,
    stroke_color: tiny_skia::Color,
) {
    let Some(path) = ({
        let mut pb = PathBuilder::new();
        pb.push_circle(cx, cy, radius);
        pb.finish()
    }) else {
        return;
    };

    // Fill
    let mut paint = Paint::default();
    paint.set_color(fill);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    // Stroke
    let mut stroke_paint = Paint::default();
    stroke_paint.set_color(stroke_color);
    stroke_paint.anti_alias = true;
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
}

/// Draw a (possibly rotated) filled rectangle with a 1-px stroke.
///
/// `cx`, `cy` are the pixel-space centre of the rectangle.
/// `rotation` is the CCW angle in radians (physics convention).
///
/// Because the Y-axis is flipped between world and screen, a CCW physics
/// rotation maps to a CW screen rotation — so we convert to degrees and
/// pass the angle as-is to tiny-skia (which uses CW-positive with Y-down).
#[allow(clippy::too_many_arguments)]
fn draw_rect(
    pixmap: &mut Pixmap,
    cx: f32,
    cy: f32,
    w: f32,
    h: f32,
    rotation: f32,
    fill: tiny_skia::Color,
    stroke_color: tiny_skia::Color,
) {
    let hw = w / 2.0;
    let hh = h / 2.0;

    let Some(path) = ({
        let mut pb = PathBuilder::new();
        pb.move_to(-hw, -hh);
        pb.line_to(hw, -hh);
        pb.line_to(hw, hh);
        pb.line_to(-hw, hh);
        pb.close();
        pb.finish()
    }) else {
        return;
    };

    // Rotate in screen space (CW positive with Y-down = physics CCW sign flipped
    // by the Y flip, so both cancel and we use the same angle magnitude).
    let angle_deg = rotation.to_degrees();
    let transform = Transform::from_rotate(angle_deg).post_translate(cx, cy);

    let mut paint = Paint::default();
    paint.set_color(fill);
    paint.anti_alias = true;
    pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);

    let mut stroke_paint = Paint::default();
    stroke_paint.set_color(stroke_color);
    stroke_paint.anti_alias = true;
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &stroke_paint, &stroke, transform, None);
}

/// Draw a filled convex polygon (vertices are local-space offsets) with a
/// 1-px stroke.
fn draw_polygon(
    pixmap: &mut Pixmap,
    body_pos: Vec2,
    rotation: f32,
    vertices: &[Vec2],
    ctx: &RenderContext,
    fill: tiny_skia::Color,
    stroke_color: tiny_skia::Color,
) {
    if vertices.len() < 3 {
        return;
    }

    // Transform each vertex: rotate by body rotation, add body position,
    // then convert to pixel space.
    let cos = rotation.cos();
    let sin = rotation.sin();

    let pixel_verts: Vec<(f32, f32)> = vertices
        .iter()
        .map(|v| {
            // Rotate offset
            let rx = cos * v.x - sin * v.y;
            let ry = sin * v.x + cos * v.y;
            // World position
            let wx = body_pos.x + rx;
            let wy = body_pos.y + ry;
            world_to_pixel(Vec2::new(wx, wy), ctx)
        })
        .collect();

    let Some(path) = ({
        let mut pb = PathBuilder::new();
        let (x0, y0) = pixel_verts[0];
        pb.move_to(x0, y0);
        for &(x, y) in &pixel_verts[1..] {
            pb.line_to(x, y);
        }
        pb.close();
        pb.finish()
    }) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(fill);
    paint.anti_alias = true;
    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );

    let mut stroke_paint = Paint::default();
    stroke_paint.set_color(stroke_color);
    stroke_paint.anti_alias = true;
    let stroke = Stroke {
        width: 1.0,
        ..Stroke::default()
    };
    pixmap.stroke_path(&path, &stroke_paint, &stroke, Transform::identity(), None);
}

// ── Coordinate helpers ────────────────────────────────────────────────────────

/// Convert a world-space position to pixel-space coordinates.
///
/// Applies the camera offset, scale, and Y-axis flip.
fn world_to_pixel(world: Vec2, ctx: &RenderContext) -> (f32, f32) {
    let px = (world.x - ctx.camera_origin.x) * ctx.scale;
    let py = ctx.height as f32 - (world.y - ctx.camera_origin.y) * ctx.scale;
    (px, py)
}

// ── Color helpers ─────────────────────────────────────────────────────────────

/// Convert an [`rphys_scene::Color`] to a `tiny_skia::Color`, applying an
/// alpha multiplier.
fn to_skia_color(c: Color, alpha_factor: f32) -> tiny_skia::Color {
    let a = (c.a as f32 / 255.0) * alpha_factor;
    tiny_skia::Color::from_rgba(
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        a,
    )
    .unwrap_or(tiny_skia::Color::BLACK)
}

/// Return a darker version of the color (30% dimmer) for use as a stroke.
fn darken_color(c: Color, alpha_factor: f32) -> tiny_skia::Color {
    const FACTOR: f32 = 0.7;
    let a = (c.a as f32 / 255.0) * alpha_factor;
    tiny_skia::Color::from_rgba(
        c.r as f32 / 255.0 * FACTOR,
        c.g as f32 / 255.0 * FACTOR,
        c.b as f32 / 255.0 * FACTOR,
        a,
    )
    .unwrap_or(tiny_skia::Color::BLACK)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_physics::types::{BodyId, BodyState, PhysicsState};
    use rphys_scene::{BodyType, Color, ShapeKind, Vec2, WallConfig, WorldBounds};

    // ── helpers ───────────────────────────────────────────────────────────────

    fn default_ctx(width: u32, height: u32) -> RenderContext {
        RenderContext {
            width,
            height,
            camera_origin: Vec2::ZERO,
            scale: 50.0, // 50 px per meter
            background_color: Color::rgb(20, 20, 20),
        }
    }

    fn empty_state() -> PhysicsState {
        PhysicsState {
            bodies: vec![],
            time: 0.0,
            world_bounds: WorldBounds {
                width: 10.0,
                height: 10.0,
            },
            wall_config: WallConfig {
                visible: false,
                color: Color::WHITE,
                thickness: 0.1,
            },
        }
    }

    fn make_body(shape: ShapeKind, position: Vec2, color: Color, body_type: BodyType) -> BodyState {
        BodyState {
            id: BodyId(0),
            name: None,
            tags: vec![],
            position,
            rotation: 0.0,
            shape,
            color,
            is_alive: true,
            body_type,
        }
    }

    fn state_with(body: BodyState) -> PhysicsState {
        PhysicsState {
            bodies: vec![body],
            ..empty_state()
        }
    }

    // ── test: Frame::new and Frame::pixel ─────────────────────────────────────

    #[test]
    fn test_frame_new_all_zero() {
        let frame = Frame::new(4, 4);
        assert_eq!(frame.width, 4);
        assert_eq!(frame.height, 4);
        assert_eq!(frame.pixels.len(), 4 * 4 * 4);
        assert!(frame.pixels.iter().all(|&b| b == 0));
    }

    #[test]
    fn test_frame_pixel_reads_correctly() {
        let mut frame = Frame::new(2, 2);
        // Manually write a pixel at (1, 0): y=0, x=1, stride=2 → byte offset 4
        let idx = 4_usize;
        frame.pixels[idx] = 255;
        frame.pixels[idx + 1] = 128;
        frame.pixels[idx + 2] = 64;
        frame.pixels[idx + 3] = 200;
        assert_eq!(frame.pixel(1, 0), [255, 128, 64, 200]);
        // Other pixel untouched
        assert_eq!(frame.pixel(0, 0), [0, 0, 0, 0]);
    }

    #[test]
    #[should_panic]
    fn test_frame_pixel_out_of_bounds_panics() {
        let frame = Frame::new(4, 4);
        let _ = frame.pixel(4, 0); // x == width, should panic
    }

    // ── test: empty scene renders just background ─────────────────────────────

    #[test]
    fn test_empty_scene_background_color() {
        let renderer = TinySkiaRenderer;
        let ctx = default_ctx(100, 100);
        let bg = ctx.background_color;
        let frame = renderer.render(&empty_state(), &ctx);

        // Every pixel should be the background color (fully opaque).
        let center = frame.pixel(50, 50);
        assert_eq!(center[0], bg.r, "R channel");
        assert_eq!(center[1], bg.g, "G channel");
        assert_eq!(center[2], bg.b, "B channel");
        assert_eq!(center[3], bg.a, "A channel");
    }

    // ── test: single circle renders correct color at center ───────────────────

    #[test]
    fn test_circle_center_pixel_color() {
        let renderer = TinySkiaRenderer;
        // 200×200 frame, 50 px/m camera at origin.
        // Place a circle at world (2, 2) → pixel (100, 100) is the center
        // (frame height is 200, so py = 200 - 2*50 = 100 ✓).
        let ctx = RenderContext {
            width: 200,
            height: 200,
            camera_origin: Vec2::ZERO,
            scale: 50.0,
            background_color: Color::rgb(0, 0, 0),
        };

        let circle_color = Color::rgb(255, 0, 0); // red
        let body = make_body(
            ShapeKind::Circle { radius: 1.0 }, // 50 px radius
            Vec2::new(2.0, 2.0),
            circle_color,
            BodyType::Dynamic,
        );
        let state = state_with(body);
        let frame = renderer.render(&state, &ctx);

        // The center pixel should be red (or very close to it after AA).
        let [r, g, b, a] = frame.pixel(100, 100);
        assert_eq!(a, 255, "center pixel must be fully opaque");
        assert!(r > 200, "red channel should be high, got {r}");
        assert!(g < 20, "green channel should be low, got {g}");
        assert!(b < 20, "blue channel should be low, got {b}");
    }

    // ── test: dead bodies are skipped ─────────────────────────────────────────

    #[test]
    fn test_dead_body_not_rendered() {
        let renderer = TinySkiaRenderer;
        let ctx = RenderContext {
            width: 100,
            height: 100,
            camera_origin: Vec2::ZERO,
            scale: 10.0,
            background_color: Color::rgb(0, 0, 0),
        };
        let mut body = make_body(
            ShapeKind::Circle { radius: 1.0 },
            Vec2::new(5.0, 5.0), // pixel center (50, 50)
            Color::rgb(255, 0, 0),
            BodyType::Dynamic,
        );
        body.is_alive = false;

        let state = state_with(body);
        let frame = renderer.render(&state, &ctx);

        // Center should still be background (black).
        let [r, g, b, _] = frame.pixel(50, 50);
        assert!(r < 10, "dead body should not be rendered (r={r})");
        assert!(g < 10, "dead body should not be rendered (g={g})");
        assert!(b < 10, "dead body should not be rendered (b={b})");
    }

    // ── test: rectangle renders at expected pixel location ────────────────────

    #[test]
    fn test_rectangle_center_pixel() {
        let renderer = TinySkiaRenderer;
        // 200×200 frame, 20 px/m.
        // Place a 4×4m rectangle centered at world (5, 5).
        // Pixel center: px = 5*20 = 100, py = 200 - 5*20 = 100.
        let ctx = RenderContext {
            width: 200,
            height: 200,
            camera_origin: Vec2::ZERO,
            scale: 20.0,
            background_color: Color::rgb(0, 0, 0),
        };

        let rect_color = Color::rgb(0, 0, 255); // blue
        let body = make_body(
            ShapeKind::Rectangle {
                width: 4.0,
                height: 4.0,
            },
            Vec2::new(5.0, 5.0),
            rect_color,
            BodyType::Dynamic,
        );
        let state = state_with(body);
        let frame = renderer.render(&state, &ctx);

        let [r, g, b, a] = frame.pixel(100, 100);
        assert_eq!(a, 255, "center must be fully opaque");
        assert!(r < 20, "red should be low (got {r})");
        assert!(g < 20, "green should be low (got {g})");
        assert!(b > 200, "blue should be high (got {b})");
    }

    // ── test: polygon renders at expected pixel location ──────────────────────

    #[test]
    fn test_polygon_center_pixel() {
        let renderer = TinySkiaRenderer;
        // 200×200, 20 px/m.
        // Triangle centred at world (5, 5) → pixel (100, 100).
        let ctx = RenderContext {
            width: 200,
            height: 200,
            camera_origin: Vec2::ZERO,
            scale: 20.0,
            background_color: Color::rgb(0, 0, 0),
        };

        let tri_color = Color::rgb(0, 255, 0); // green
        let body = make_body(
            ShapeKind::Polygon {
                vertices: vec![
                    Vec2::new(0.0, 2.0),
                    Vec2::new(-2.0, -2.0),
                    Vec2::new(2.0, -2.0),
                ],
            },
            Vec2::new(5.0, 5.0),
            tri_color,
            BodyType::Dynamic,
        );
        let state = state_with(body);
        let frame = renderer.render(&state, &ctx);

        // The centroid of the triangle is roughly at (5, 5+0) world,
        // but let's just check well inside the triangle.
        // With centre at (5,5) and vertices at ±2m, the triangle spans
        // about 80px. Check a point near the bottom centre.
        let [r, g, b, a] = frame.pixel(100, 108); // slightly below centre (inside triangle)
        assert_eq!(a, 255, "polygon pixel must be opaque");
        assert!(r < 20, "r low (got {r})");
        assert!(g > 200, "g high (got {g})");
        assert!(b < 20, "b low (got {b})");
    }

    // ── test: static body rendered at reduced opacity ─────────────────────────

    #[test]
    fn test_static_body_reduced_opacity() {
        // Place an opaque white dynamic body and an opaque white static body
        // side by side and compare the rendered alpha at each center.
        // Actually tiny-skia blends premultiplied, so the easiest check is
        // to render static-only vs dynamic-only and verify the static version
        // is "less bright" (smaller premultiplied channel values).

        let renderer = TinySkiaRenderer;
        let ctx = RenderContext {
            width: 200,
            height: 100,
            camera_origin: Vec2::ZERO,
            scale: 20.0,
            background_color: Color::rgb(0, 0, 0),
        };

        let white = Color::rgba(255, 255, 255, 255);

        // Dynamic body at world (2.5, 2.5) → pixel (50, 50)
        let dynamic_body = make_body(
            ShapeKind::Circle { radius: 1.0 },
            Vec2::new(2.5, 2.5),
            white,
            BodyType::Dynamic,
        );
        // Static body at world (7.5, 2.5) → pixel (150, 50)
        let mut static_body = make_body(
            ShapeKind::Circle { radius: 1.0 },
            Vec2::new(7.5, 2.5),
            white,
            BodyType::Static,
        );
        static_body.id = BodyId(1);

        let state = PhysicsState {
            bodies: vec![dynamic_body, static_body],
            ..empty_state()
        };
        let frame = renderer.render(&state, &ctx);

        let [dr, _, _, _] = frame.pixel(50, 50);
        let [sr, _, _, _] = frame.pixel(150, 50);
        // Dynamic should be brighter (255) than static (≈204)
        assert_eq!(dr, 255, "dynamic body center should be full white");
        assert!(
            sr < dr,
            "static body should be dimmer: static={sr}, dynamic={dr}"
        );
    }

    // ── test: world_to_pixel coordinate transform ─────────────────────────────

    #[test]
    fn test_world_to_pixel_transform() {
        let ctx = RenderContext {
            width: 100,
            height: 100,
            camera_origin: Vec2::new(0.0, 0.0),
            scale: 10.0,
            background_color: Color::BLACK,
        };

        // World (0, 0) → pixel (0, 100) — bottom-left world = bottom-left screen
        let (px, py) = world_to_pixel(Vec2::new(0.0, 0.0), &ctx);
        assert!((px - 0.0).abs() < 0.001);
        assert!((py - 100.0).abs() < 0.001);

        // World (5, 5) → pixel (50, 50) — center
        let (px, py) = world_to_pixel(Vec2::new(5.0, 5.0), &ctx);
        assert!((px - 50.0).abs() < 0.001);
        assert!((py - 50.0).abs() < 0.001);

        // World (10, 10) → pixel (100, 0) — top-right
        let (px, py) = world_to_pixel(Vec2::new(10.0, 10.0), &ctx);
        assert!((px - 100.0).abs() < 0.001);
        assert!((py - 0.0).abs() < 0.001);
    }

    // ── test: camera_origin offset shifts rendering ───────────────────────────

    #[test]
    fn test_camera_origin_offset() {
        // When camera_origin = (5, 5), world (5, 5) → pixel (0, height)
        let ctx = RenderContext {
            width: 100,
            height: 100,
            camera_origin: Vec2::new(5.0, 5.0),
            scale: 10.0,
            background_color: Color::BLACK,
        };
        let (px, py) = world_to_pixel(Vec2::new(5.0, 5.0), &ctx);
        assert!((px - 0.0).abs() < 0.001);
        assert!((py - 100.0).abs() < 0.001);
    }
}
