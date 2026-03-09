//! Fading ghost-trail renderer for marble races.
//!
//! [`TrailRenderer`] wraps [`TinySkiaRenderer`] and composites semi-transparent
//! "ghost" circles from previous frames behind the current physics snapshot,
//! producing a motion-trail effect for bodies tagged `"racer"`.
//!
//! # Usage
//!
//! ```rust,ignore
//! let mut trail = TrailRenderer::new(TrailConfig::default());
//!
//! // Each frame, call push_frame BEFORE render:
//! trail.push_frame(&phys_state);
//! let frame = trail.render(&phys_state, &ctx);
//! ```

use std::collections::{HashMap, VecDeque};

use rphys_physics::types::{BodyId, PhysicsState};
use rphys_scene::{Color, ShapeKind, Vec2};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::{Frame, RenderContext, Renderer, TinySkiaRenderer};

// ── TrailConfig ───────────────────────────────────────────────────────────────

/// Configuration for the [`TrailRenderer`].
#[derive(Debug, Clone)]
pub struct TrailConfig {
    /// Number of historical frames to retain in the ring buffer.
    ///
    /// Larger values produce longer trails at the cost of more memory.
    pub length: usize,
    /// Alpha of the most-recent ghost circle (0.0 – 1.0).
    ///
    /// Older ghosts fade linearly toward 0.
    pub max_alpha: f32,
    /// Ghost radius = body radius × this factor.
    ///
    /// Values < 1.0 draw smaller ghosts so they don't fully occlude the body.
    pub radius_factor: f32,
    /// Only bodies whose tag list intersects this set receive trails.
    ///
    /// An **empty** filter means *all* dynamic bodies are trailed.
    pub tags_filter: Vec<String>,
}

impl Default for TrailConfig {
    fn default() -> Self {
        Self {
            length: 12,
            max_alpha: 0.55,
            radius_factor: 0.75,
            tags_filter: vec!["racer".to_string()],
        }
    }
}

// ── TrailRenderer ─────────────────────────────────────────────────────────────

/// Renderer that overlays fading ghost circles behind each traced body.
///
/// Call [`push_frame`](TrailRenderer::push_frame) **before** each call to
/// [`render`](TrailRenderer::render) so that the snapshot ring-buffer is kept
/// one step behind the current physics state.
pub struct TrailRenderer {
    config: TrailConfig,
    /// Ring buffer of per-frame snapshots.
    ///
    /// Each entry maps `BodyId → (world_position, world_radius, color)`.
    /// The front is the **oldest** snapshot; the back is the **newest**.
    history: VecDeque<HashMap<BodyId, (Vec2, f32, Color)>>,
    /// Underlying full-fidelity renderer for the current frame.
    inner: TinySkiaRenderer,
}

impl TrailRenderer {
    /// Create a new [`TrailRenderer`] with the given configuration.
    pub fn new(config: TrailConfig) -> Self {
        Self {
            config,
            history: VecDeque::new(),
            inner: TinySkiaRenderer,
        }
    }

    /// Snapshot the current physics state into the history ring-buffer.
    ///
    /// Call this **once per frame, before** [`render`](TrailRenderer::render).
    /// Bodies that fail the tag filter are excluded from the snapshot.
    pub fn push_frame(&mut self, state: &PhysicsState) {
        let mut snapshot: HashMap<BodyId, (Vec2, f32, Color)> = HashMap::new();

        for body in &state.bodies {
            if !body.is_alive {
                continue;
            }

            // Apply tag filter (empty filter = accept all).
            if !self.config.tags_filter.is_empty() {
                let tag_match = body
                    .tags
                    .iter()
                    .any(|t| self.config.tags_filter.contains(t));
                if !tag_match {
                    continue;
                }
            }

            let radius = body_world_radius(body);
            snapshot.insert(body.id, (body.position, radius, body.color));
        }

        self.history.push_back(snapshot);

        // Evict oldest frame when the buffer exceeds `length`.
        while self.history.len() > self.config.length {
            self.history.pop_front();
        }
    }

    /// Render faded ghost trails followed by the current physics frame.
    ///
    /// # Algorithm
    ///
    /// 1. Fill a blank pixmap with the background color.
    /// 2. For each historical snapshot, oldest-first, draw semi-transparent
    ///    ghost circles.  Alpha increases linearly from near-zero (oldest) to
    ///    `max_alpha` (most recent history frame).
    /// 3. Render the current physics state with the inner [`TinySkiaRenderer`].
    /// 4. Composite the current frame **over** the trail layer using standard
    ///    premultiplied-alpha blending.
    pub fn render(&self, state: &PhysicsState, ctx: &RenderContext) -> Frame {
        // 1. Render the current physics state (opaque background + bodies).
        let current_frame = self.inner.render(state, ctx);

        // Guard: tiny-skia can't create zero-dimension pixmaps.
        let Some(mut pixmap) = Pixmap::new(ctx.width, ctx.height) else {
            return current_frame;
        };

        // 2. Copy current frame into pixmap.
        //    Both use premultiplied RGBA with stride = width × 4 (no padding).
        {
            let dst = pixmap.data_mut();
            let src = &current_frame.pixels;
            let copy_len = dst.len().min(src.len());
            dst[..copy_len].copy_from_slice(&src[..copy_len]);
        }

        // 3. Draw ghost circles ON TOP (oldest → newest, increasing alpha).
        //    Ghosts are rendered over the full frame so they're always visible.
        //    The most-recent ghost overlaps the ball but is semi-transparent,
        //    giving a natural "comet tail" look.
        let history_len = self.history.len();
        if history_len > 0 {
            for (i, snapshot) in self.history.iter().enumerate() {
                let alpha = self.config.max_alpha * (i + 1) as f32 / history_len as f32;
                for &(pos, world_radius, color) in snapshot.values() {
                    let (cx, cy) = world_to_pixel(pos, ctx);
                    let ghost_radius_px = world_radius * self.config.radius_factor * ctx.scale;
                    draw_ghost_circle(&mut pixmap, cx, cy, ghost_radius_px, color, alpha);
                }
            }
        }

        Frame {
            width: ctx.width,
            height: ctx.height,
            pixels: pixmap.data().to_vec(),
        }
    }
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Approximate world-space "radius" of a body for use as a ghost circle size.
///
/// For circles this is exact; for rectangles and polygons it returns a
/// bounding-disk approximation.
fn body_world_radius(body: &rphys_physics::types::BodyState) -> f32 {
    match &body.shape {
        ShapeKind::Circle { radius } => *radius,
        ShapeKind::Rectangle { width, height } => {
            // Half-diagonal of the rectangle.
            (width * width + height * height).sqrt() * 0.5
        }
        ShapeKind::Polygon { vertices } => {
            // Distance from origin to the farthest vertex.
            vertices
                .iter()
                .map(|v| (v.x * v.x + v.y * v.y).sqrt())
                .fold(0.0_f32, f32::max)
        }
    }
}

/// Draw a filled, anti-aliased ghost circle on `pixmap` with the given alpha.
fn draw_ghost_circle(pixmap: &mut Pixmap, cx: f32, cy: f32, radius: f32, color: Color, alpha: f32) {
    if radius <= 0.0 || alpha <= 0.0 {
        return;
    }

    let ghost_color = tiny_skia::Color::from_rgba(
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        alpha,
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT);

    let Some(path) = ({
        let mut pb = PathBuilder::new();
        pb.push_circle(cx, cy, radius);
        pb.finish()
    }) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(ghost_color);
    paint.anti_alias = true;

    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Convert a world-space position to pixel-space `(x, y)` using the render context.
///
/// Applies camera offset, scale, and Y-axis flip (physics is Y-up; screen is Y-down).
fn world_to_pixel(world: Vec2, ctx: &RenderContext) -> (f32, f32) {
    let px = (world.x - ctx.camera_origin.x) * ctx.scale;
    let py = ctx.height as f32 - (world.y - ctx.camera_origin.y) * ctx.scale;
    (px, py)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_physics::types::{BodyId, BodyState, PhysicsState};
    use rphys_scene::{BodyType, Color, ShapeKind, Vec2, WallConfig, WorldBounds};

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn default_ctx() -> RenderContext {
        RenderContext {
            width: 200,
            height: 200,
            camera_origin: Vec2::ZERO,
            scale: 20.0,
            background_color: Color::rgb(10, 10, 10),
        }
    }

    fn make_body_at(id: u32, pos: Vec2, tags: Vec<String>) -> BodyState {
        BodyState {
            id: BodyId(id),
            name: None,
            tags,
            position: pos,
            rotation: 0.0,
            shape: ShapeKind::Circle { radius: 0.5 },
            color: Color::rgb(255, 100, 0),
            is_alive: true,
            body_type: BodyType::Dynamic,
        }
    }

    fn state_with_bodies(bodies: Vec<BodyState>) -> PhysicsState {
        PhysicsState {
            bodies,
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

    // ── Test 1: TrailRenderer produces a non-empty frame ─────────────────────

    /// Creates a TrailRenderer, pushes one frame, renders, and asserts the
    /// resulting Frame pixel buffer is non-empty and has expected dimensions.
    #[test]
    fn test_trail_renderer_produces_frame() {
        let config = TrailConfig {
            length: 5,
            max_alpha: 0.3,
            radius_factor: 0.75,
            tags_filter: vec![],
        };
        let mut trail = TrailRenderer::new(config);

        let body = make_body_at(0, Vec2::new(5.0, 5.0), vec![]);
        let state = state_with_bodies(vec![body]);

        trail.push_frame(&state);
        let frame = trail.render(&state, &default_ctx());

        assert_eq!(frame.width, 200, "frame width must match ctx");
        assert_eq!(frame.height, 200, "frame height must match ctx");
        assert_eq!(
            frame.pixels.len(),
            (200 * 200 * 4) as usize,
            "pixel buffer must be width×height×4 bytes"
        );
        // At least some pixels must be non-zero (background + body).
        assert!(
            frame.pixels.iter().any(|&b| b != 0),
            "frame should not be all-zero"
        );
    }

    // ── Test 2: History is bounded by `length` ────────────────────────────────

    /// Pushes more frames than `length` and verifies the deque never exceeds it.
    #[test]
    fn test_trail_history_bounded() {
        let max_len = 4_usize;
        let config = TrailConfig {
            length: max_len,
            max_alpha: 0.3,
            radius_factor: 0.75,
            tags_filter: vec![],
        };
        let mut trail = TrailRenderer::new(config);

        let state = state_with_bodies(vec![make_body_at(0, Vec2::new(5.0, 5.0), vec![])]);

        // Push 3× more frames than the configured length.
        for _ in 0..(max_len * 3) {
            trail.push_frame(&state);
        }

        assert!(
            trail.history.len() <= max_len,
            "history length {} must not exceed configured max {}",
            trail.history.len(),
            max_len,
        );
    }

    // ── Test 3: Ghost appears at past position ────────────────────────────────

    /// Pushes a frame with a ball at (5, 5), then renders with the ball moved
    /// to (5, 8).  The pixel area around (5, 5) must contain color from the ghost.
    ///
    /// With scale=20, world (5,5) → pixel (100, 100).
    /// The ghost circle has world radius = 0.5 * 0.75 = 0.375 m → 7.5 px.
    #[test]
    fn test_trail_renders_ghost_at_past_position() {
        let config = TrailConfig {
            length: 5,
            max_alpha: 0.5,     // reasonably visible
            radius_factor: 1.0, // full radius for easier detection
            tags_filter: vec![],
        };
        let mut trail = TrailRenderer::new(config);

        let ctx = RenderContext {
            width: 400,
            height: 400,
            camera_origin: Vec2::ZERO,
            scale: 20.0,
            background_color: Color::rgb(0, 0, 0),
        };

        // Push first state: ball at world (5, 5) → pixel (100, 300).
        // pixel_y = 400 - (5.0 - 0.0) * 20.0 = 400 - 100 = 300
        let body_at_origin = make_body_at(0, Vec2::new(5.0, 5.0), vec![]);
        let state_a = state_with_bodies(vec![body_at_origin]);
        trail.push_frame(&state_a);

        // Current state: ball moved to world (5, 18) → pixel (100, 40).
        // pixel_y = 400 - (18.0 - 0.0) * 20.0 = 400 - 360 = 40
        let body_moved = make_body_at(0, Vec2::new(5.0, 18.0), vec![]);
        let state_b = state_with_bodies(vec![body_moved]);

        let frame = trail.render(&state_b, &ctx);

        // The pixel at the ghost's center (100, 300) should be colored (ghost
        // circle drawn there from the pushed snapshot).
        let [r, g, b, a] = frame.pixel(100, 300);
        assert!(
            r > 10 || g > 10 || b > 10 || a > 10,
            "ghost at (5,5) → pixel (100,300) should be colored, got r={r} g={g} b={b} a={a}",
        );

        // The ghost center area should NOT overlap with the current ball center.
        // Verify the current ball IS rendered at its new position (100, 40).
        let [r2, g2, b2, _] = frame.pixel(100, 40);
        assert!(
            r2 > 50 || g2 > 50 || b2 > 50,
            "current ball at pixel (100,40) should be visible, got r={r2} g={g2} b={b2}",
        );
    }

    // ── Bonus test: tag filter excludes non-matching bodies ───────────────────

    /// Bodies without the required tag should not appear in history.
    #[test]
    fn test_tag_filter_excludes_unmatched_bodies() {
        let config = TrailConfig {
            length: 10,
            max_alpha: 0.5,
            radius_factor: 0.75,
            tags_filter: vec!["racer".to_string()],
        };
        let mut trail = TrailRenderer::new(config);

        // Body without the "racer" tag.
        let untagged = make_body_at(0, Vec2::new(5.0, 5.0), vec!["obstacle".to_string()]);
        let state = state_with_bodies(vec![untagged]);

        trail.push_frame(&state);

        // History should have one snapshot entry, but it should be empty
        // (body didn't match the tag filter).
        assert_eq!(trail.history.len(), 1, "one snapshot should be pushed");
        assert!(
            trail.history[0].is_empty(),
            "snapshot should be empty when no body matches the tag filter"
        );
    }

    // ── Bonus test: empty tag filter accepts all bodies ────────────────────────

    #[test]
    fn test_empty_tag_filter_accepts_all() {
        let config = TrailConfig {
            length: 10,
            max_alpha: 0.3,
            radius_factor: 0.75,
            tags_filter: vec![], // no filter
        };
        let mut trail = TrailRenderer::new(config);

        let body = make_body_at(0, Vec2::new(3.0, 3.0), vec![]);
        let state = state_with_bodies(vec![body]);

        trail.push_frame(&state);

        assert_eq!(
            trail.history[0].len(),
            1,
            "body should be included when tags_filter is empty"
        );
    }
}
