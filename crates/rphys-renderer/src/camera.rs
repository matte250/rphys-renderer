//! Camera system for `rphys-renderer`.
//!
//! Provides a [`CameraController`] trait and two implementations:
//! - [`StaticCamera`]: fixed camera that always returns the same [`RenderContext`].
//! - [`RaceCamera`]: smooth-following camera that tracks the leading racer.

use rphys_physics::{PhysicsEvent, PhysicsState};
use rphys_scene::{CameraConfig, Color, Vec2};

use crate::RenderContext;

// ── CameraController trait ────────────────────────────────────────────────────

/// Produces a [`RenderContext`] for each rendered frame.
///
/// Implementations maintain their own state (smoothed position, etc.) and
/// update it on each call. The trait requires [`Send`] so cameras can be moved
/// across thread boundaries in export pipelines.
pub trait CameraController: Send {
    /// Given the latest physics snapshot and elapsed frame time (in seconds),
    /// return the [`RenderContext`] to use for this frame's render call.
    fn update(&mut self, state: &PhysicsState, dt: f32) -> RenderContext;

    /// Reset camera to its initial position.
    ///
    /// Called on scene hot-reload or when the simulation is restarted.
    fn reset(&mut self);
}

// ── StaticCamera ──────────────────────────────────────────────────────────────

/// Fixed camera that always returns the same [`RenderContext`].
///
/// Wraps the existing fixed-camera behaviour used in non-race scenes.
pub struct StaticCamera {
    /// The fixed context returned on every call to [`update`](CameraController::update).
    ctx: RenderContext,
    /// The context to restore on [`reset`](CameraController::reset).
    initial_ctx: RenderContext,
}

impl StaticCamera {
    /// Construct a static camera from a pre-built [`RenderContext`].
    pub fn new(ctx: RenderContext) -> Self {
        Self {
            initial_ctx: ctx.clone(),
            ctx,
        }
    }

    /// Derive scale and origin so the world fits inside the output frame.
    ///
    /// Uses **uniform scaling** (the smaller of the horizontal and vertical
    /// scale factors) so no distortion occurs.  The world origin maps to the
    /// bottom-left corner of the frame (`camera_origin = Vec2::ZERO`).
    ///
    /// # Parameters
    ///
    /// - `width` / `height` — output frame dimensions in pixels.
    /// - `world_width` / `world_height` — world size in meters.
    /// - `background_color` — fill color for the frame background.
    pub fn from_world(
        width: u32,
        height: u32,
        world_width: f32,
        world_height: f32,
        background_color: Color,
    ) -> Self {
        let scale_x = width as f32 / world_width;
        let scale_y = height as f32 / world_height;
        let scale = scale_x.min(scale_y);

        let ctx = RenderContext {
            width,
            height,
            camera_origin: Vec2::ZERO,
            scale,
            background_color,
        };
        Self::new(ctx)
    }
}

impl CameraController for StaticCamera {
    /// Always returns the same context, regardless of physics state or time.
    fn update(&mut self, _state: &PhysicsState, _dt: f32) -> RenderContext {
        self.ctx.clone()
    }

    /// Restores the camera to the context provided at construction time.
    fn reset(&mut self) {
        self.ctx = self.initial_ctx.clone();
    }
}

// ── RaceCameraConfig ──────────────────────────────────────────────────────────

/// Tunable parameters for [`RaceCamera`].
#[derive(Debug, Clone)]
pub struct RaceCameraConfig {
    /// Tag used to identify racer bodies in [`PhysicsState::bodies`].
    ///
    /// Should match `RaceConfig::racer_tag`. Default: `"racer"`.
    pub racer_tag: String,

    /// Where on screen the leading racer appears, as a fraction of screen
    /// height **from the top**.  `0.0` = racer at the very top edge;
    /// `1.0` = racer at the very bottom edge.
    ///
    /// Default: `0.35` — leader sits 35 % down from the top of the frame,
    /// giving 65 % of lookahead below.
    pub leader_screen_fraction: f32,

    /// Per-frame exponential smoothing factor `[0.0, 1.0]`.
    ///
    /// Applied as `current += (target − current) × damping` once per frame.
    /// `0.0` means no movement; `1.0` means instant snap.
    ///
    /// Default: `0.15`.
    pub damping: f32,

    /// Clamp the camera so `camera_origin.y` never exceeds this value.
    ///
    /// Prevents the camera from scrolling above the top of the course.
    /// Default: [`f32::MAX`] (no upper clamp).
    pub max_origin_y: f32,

    /// Clamp the camera so `camera_origin.y` never falls below this value.
    ///
    /// Prevents revealing empty space below the world origin.
    /// Default: `0.0`.
    pub min_origin_y: f32,
}

impl Default for RaceCameraConfig {
    fn default() -> Self {
        Self {
            racer_tag: "racer".to_string(),
            leader_screen_fraction: 0.35,
            damping: 0.15,
            max_origin_y: f32::MAX,
            min_origin_y: 0.0,
        }
    }
}

// ── RaceCamera ────────────────────────────────────────────────────────────────

/// Dynamic camera that smooth-follows the leading racer.
///
/// The *leader* is the body tagged with [`RaceCameraConfig::racer_tag`] that
/// has the **lowest current Y position** (furthest toward a descending finish
/// line).  The camera exponentially smooths toward a target Y so the leader
/// appears at [`RaceCameraConfig::leader_screen_fraction`] from the top of the
/// frame.
///
/// If no racer bodies are present in the current state, the camera holds its
/// last position (graceful fallback).
pub struct RaceCamera {
    config: RaceCameraConfig,
    /// Current (smoothed) `camera_origin.y` in world space.
    current_origin_y: f32,
    render_width: u32,
    render_height: u32,
    scale: f32,
    background_color: Color,
    /// The `current_origin_y` at construction, used to restore on reset.
    initial_origin_y: f32,
}

impl RaceCamera {
    /// Create a race camera.
    ///
    /// `initial_ctx` supplies the render dimensions, scale, and background
    /// color from the scene.  The `camera_origin.y` is overridden each frame
    /// by the smooth-follow algorithm; `camera_origin.x` is always taken from
    /// `initial_ctx` (horizontal scrolling is not supported).
    pub fn new(config: RaceCameraConfig, initial_ctx: RenderContext) -> Self {
        let initial_origin_y = initial_ctx.camera_origin.y;
        Self {
            config,
            current_origin_y: initial_origin_y,
            render_width: initial_ctx.width,
            render_height: initial_ctx.height,
            scale: initial_ctx.scale,
            background_color: initial_ctx.background_color,
            initial_origin_y,
        }
    }
}

impl CameraController for RaceCamera {
    /// Locate the leading racer, compute target `camera_origin.y`, apply
    /// exponential smoothing and clamping, then return an updated
    /// [`RenderContext`].
    ///
    /// # Algorithm
    ///
    /// ```text
    /// viewport_height_meters = render_height / scale
    /// target_origin_y = leader.position_y - (1.0 - leader_screen_fraction) * viewport_height_meters
    /// target_origin_y = clamp(target_origin_y, min_origin_y, max_origin_y)
    /// current_origin_y += (target_origin_y - current_origin_y) * damping
    /// ```
    fn update(&mut self, state: &PhysicsState, _dt: f32) -> RenderContext {
        // 1. Find the body with the racer tag that has the lowest position_y.
        let leader_y = state
            .bodies
            .iter()
            .filter(|b| b.is_alive && b.tags.iter().any(|t| t == &self.config.racer_tag))
            .map(|b| b.position.y)
            .reduce(f32::min);

        if let Some(leader_y) = leader_y {
            // 2. Compute target origin so the leader appears at the configured
            //    fraction from the top.
            let viewport_height_meters = self.render_height as f32 / self.scale;
            let target_origin_y =
                leader_y - (1.0 - self.config.leader_screen_fraction) * viewport_height_meters;

            // 3. Clamp to configured bounds.
            let target_origin_y = target_origin_y
                .max(self.config.min_origin_y)
                .min(self.config.max_origin_y);

            // 4. Exponential smooth: move a fixed fraction toward the target
            //    each frame (simple per-frame damping, works for small dt).
            self.current_origin_y +=
                (target_origin_y - self.current_origin_y) * self.config.damping;
        }
        // If no racers found, hold the current position (graceful fallback).

        // 5. Build and return the updated context.
        RenderContext {
            width: self.render_width,
            height: self.render_height,
            camera_origin: Vec2::new(0.0, self.current_origin_y),
            scale: self.scale,
            background_color: self.background_color,
        }
    }

    /// Reset the camera to its initial position.
    fn reset(&mut self) {
        self.current_origin_y = self.initial_origin_y;
    }
}

// ── FollowCamera ──────────────────────────────────────────────────────────────

/// Dynamic camera that smoothly follows the race leader with camera shake and
/// finish-line zoom.
///
/// Intended for `mode: follow_leader` in the scene YAML.  In the export
/// pipeline, call [`FollowCamera::update`] each frame with the leader's world
/// position, the physics events for that frame, and whether the race is
/// complete.  Then read the resulting [`RenderContext`] via
/// [`FollowCamera::render_context`].
///
/// # Smooth-follow algorithm (per frame)
///
/// ```text
/// target = leader_pos - Vec2(0, look_ahead)   // look ahead below the leader
/// current_pos = lerp(current_pos, target, follow_lerp)
/// shake_offset = (random impulse on impact) then decay each frame
/// scale = base_scale * current_zoom
/// origin = current_pos - viewport_half_size + shake_offset
/// ```
pub struct FollowCamera {
    config: CameraConfig,
    /// Current smoothed camera target position in world space.
    current_pos: Vec2,
    /// Current shake offset (world space, decays each frame).
    shake_offset: Vec2,
    /// Current zoom multiplier (smoothly lerps toward `target_zoom`).
    current_zoom: f32,
    /// Target zoom (`config.zoom` normally; `config.zoom * finish_zoom_factor`
    /// after race completes with `finish_zoom = true`).
    target_zoom: f32,
    /// Base pixels-per-metre scale (`viewport_width / world_width`).
    base_scale: f32,
    render_width: u32,
    render_height: u32,
    background_color: Color,
    /// Fallback camera centre when no leader position is available.
    world_center: Vec2,
    /// Initial position stored for [`CameraController::reset`].
    initial_pos: Vec2,
    /// LCG PRNG state used to generate pseudo-random shake directions.
    rng_state: u64,
}

impl FollowCamera {
    /// Construct a `FollowCamera`.
    ///
    /// # Parameters
    ///
    /// - `config` — camera configuration from the scene YAML.
    /// - `base_scale` — pixels per metre; typically `viewport_width / world_width`.
    /// - `initial_ctx` — supplies render dimensions and background color.
    /// - `world_center` — fallback world-space point when no leader is present.
    pub fn new(
        config: CameraConfig,
        base_scale: f32,
        initial_ctx: RenderContext,
        world_center: Vec2,
    ) -> Self {
        let initial_zoom = config.zoom;
        Self {
            current_pos: world_center,
            shake_offset: Vec2::ZERO,
            current_zoom: initial_zoom,
            target_zoom: initial_zoom,
            base_scale,
            render_width: initial_ctx.width,
            render_height: initial_ctx.height,
            background_color: initial_ctx.background_color,
            world_center,
            initial_pos: world_center,
            rng_state: 0x5851_F42D_4C95_7F2D,
            config,
        }
    }

    /// Advance camera state for one rendered frame.
    ///
    /// - `leader_pos` — current world-space position of the rank-1 racer, or
    ///   `None` if no racers are alive (camera holds its current position).
    /// - `events` — physics events emitted during this frame's simulation
    ///   steps.  Any [`PhysicsEvent::Collision`] or
    ///   [`PhysicsEvent::WallBounce`] event triggers a shake impulse (when
    ///   `shake_on_impact` is `true`).
    /// - `race_complete` — set to `true` once the race winner has been decided.
    ///   When `finish_zoom` is also `true`, the zoom begins transitioning
    ///   toward `finish_zoom_factor`.
    pub fn update(
        &mut self,
        leader_pos: Option<Vec2>,
        events: &[PhysicsEvent],
        race_complete: bool,
    ) {
        self.advance(leader_pos, events, race_complete);
    }

    /// Build a [`RenderContext`] from the current (post-[`update`](Self::update))
    /// camera state.
    pub fn render_context(&self) -> RenderContext {
        let scale = self.base_scale * self.current_zoom;
        let viewport_w = self.render_width as f32 / scale;
        let viewport_h = self.render_height as f32 / scale;

        // Camera origin = position of world-space point at the top-left of the
        // viewport.  Adding shake on top makes the whole view jitter.
        let origin_x = self.current_pos.x - viewport_w / 2.0 + self.shake_offset.x;
        let origin_y = self.current_pos.y - viewport_h / 2.0 + self.shake_offset.y;

        RenderContext {
            width: self.render_width,
            height: self.render_height,
            camera_origin: Vec2::new(origin_x, origin_y),
            scale,
            background_color: self.background_color,
        }
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    /// Core per-frame update called by both the inherent `update` method and
    /// the [`CameraController`] trait implementation.
    fn advance(&mut self, leader_pos: Option<Vec2>, events: &[PhysicsEvent], race_complete: bool) {
        // 1. Compute target position — three cases:
        //    a) Leader present → smooth-follow toward the leader.
        //    b) No leader + race complete → freeze (hold last position exactly).
        //    c) No leader + race not complete → lerp to world center (startup fallback).
        match leader_pos {
            Some(pos) => {
                let target = Vec2::new(pos.x, pos.y - self.config.look_ahead);
                let lerp = self.config.follow_lerp;
                if !self.config.lock_horizontal {
                    self.current_pos.x += (target.x - self.current_pos.x) * lerp;
                }
                // When lock_horizontal is true, current_pos.x stays at world_center.x
                // (set at construction) so both side walls are always on screen.
                self.current_pos.y += (target.y - self.current_pos.y) * lerp;
            }
            None if race_complete => {
                // Race is over and all racers have left the active list —
                // freeze the camera exactly at its last position; do not
                // update current_pos at all.
            }
            None => {
                // Pre-race startup fallback: lerp toward world center so the
                // camera doesn't stay at an arbitrary initial position before
                // any racer bodies become active.
                let lerp = self.config.follow_lerp;
                let target = self.world_center;
                if !self.config.lock_horizontal {
                    self.current_pos.x += (target.x - self.current_pos.x) * lerp;
                }
                self.current_pos.y += (target.y - self.current_pos.y) * lerp;
            }
        }

        // 3. Camera shake.
        if self.config.shake_on_impact {
            let has_impact = events.iter().any(|e| {
                matches!(
                    e,
                    PhysicsEvent::Collision(_) | PhysicsEvent::WallBounce { .. }
                )
            });
            if has_impact {
                // Add a random-direction impulse to the shake offset.
                let angle = self.lcg_f32() * std::f32::consts::TAU;
                let magnitude = self.lcg_f32() * self.config.shake_intensity;
                self.shake_offset.x += magnitude * angle.cos();
                self.shake_offset.y += magnitude * angle.sin();
            }
        }
        // Decay shake every frame regardless of impacts.
        self.shake_offset.x *= self.config.shake_decay;
        self.shake_offset.y *= self.config.shake_decay;

        // 4. Finish zoom.
        if race_complete && self.config.finish_zoom {
            self.target_zoom = self.config.zoom * self.config.finish_zoom_factor;
        }
        self.current_zoom += (self.target_zoom - self.current_zoom) * self.config.finish_zoom_lerp;
    }

    /// Pseudo-random `f32` in `[0, 1)` from a simple LCG.
    ///
    /// Used exclusively for shake direction/magnitude so there is no
    /// dependency on the `rand` crate.  The sequence is deterministic given
    /// the same `rng_state` seed.
    fn lcg_f32(&mut self) -> f32 {
        // Knuth multiplicative LCG coefficients (64-bit).
        self.rng_state = self
            .rng_state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        // Extract the upper 23 bits as the mantissa of a [1, 2) float, then
        // subtract 1 to get [0, 1).
        let bits = (self.rng_state >> 41) as u32;
        f32::from_bits(0x3F80_0000 | bits) - 1.0
    }
}

impl CameraController for FollowCamera {
    /// Update using only the physics state snapshot (no explicit events).
    ///
    /// Extracts the leader position from `state.bodies` and calls the internal
    /// advance with an empty event slice.  Shake is disabled in this path
    /// because collision events are not available.  Use the inherent
    /// [`FollowCamera::update`] method for full functionality.
    fn update(&mut self, state: &PhysicsState, _dt: f32) -> RenderContext {
        let leader_pos = state
            .bodies
            .iter()
            .filter(|b| b.is_alive)
            .min_by(|a, b| {
                a.position
                    .y
                    .partial_cmp(&b.position.y)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|b| b.position);

        self.advance(leader_pos, &[], false);
        self.render_context()
    }

    /// Reset the camera to its initial state.
    fn reset(&mut self) {
        self.current_pos = self.initial_pos;
        self.shake_offset = Vec2::ZERO;
        self.current_zoom = self.config.zoom;
        self.target_zoom = self.config.zoom;
        self.rng_state = 0x5851_F42D_4C95_7F2D;
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_physics::types::{BodyId, BodyState, PhysicsState};
    use rphys_scene::{BodyType, ShapeKind, WallConfig, WorldBounds};

    // ── helpers ───────────────────────────────────────────────────────────────

    fn default_bg() -> Color {
        Color::rgb(20, 20, 20)
    }

    fn base_ctx() -> RenderContext {
        RenderContext {
            width: 720,
            height: 1280,
            camera_origin: Vec2::ZERO,
            scale: 50.0,
            background_color: default_bg(),
        }
    }

    fn empty_state() -> PhysicsState {
        PhysicsState {
            bodies: vec![],
            time: 0.0,
            world_bounds: WorldBounds {
                width: 14.4,
                height: 25.6,
            },
            wall_config: WallConfig {
                visible: false,
                color: Color::WHITE,
                thickness: 0.1,
                open_bottom: false,
            },
        }
    }

    fn make_racer(id: u32, position_y: f32) -> BodyState {
        BodyState {
            id: BodyId(id),
            name: Some(format!("Racer{id}")),
            tags: vec!["racer".to_string()],
            position: Vec2::new(0.0, position_y),
            rotation: 0.0,
            shape: ShapeKind::Circle { radius: 0.5 },
            color: Color::rgb(255, 0, 0),
            is_alive: true,
            body_type: BodyType::Dynamic,
        }
    }

    fn state_with_racers(positions_y: &[f32]) -> PhysicsState {
        let bodies = positions_y
            .iter()
            .enumerate()
            .map(|(i, &y)| make_racer(i as u32, y))
            .collect();
        PhysicsState {
            bodies,
            ..empty_state()
        }
    }

    // ── StaticCamera::update always returns the same context ──────────────────

    #[test]
    fn test_static_camera_always_returns_same_ctx() {
        let ctx = base_ctx();
        let mut cam = StaticCamera::new(ctx.clone());
        let mut state = empty_state();

        let result1 = cam.update(&state, 0.016);
        state.time = 1.0;
        let result2 = cam.update(&state, 0.016);

        assert_eq!(result1.camera_origin.x, ctx.camera_origin.x);
        assert_eq!(result1.camera_origin.y, ctx.camera_origin.y);
        assert_eq!(result1.scale, ctx.scale);
        assert_eq!(result2.camera_origin.x, ctx.camera_origin.x);
        assert_eq!(result2.camera_origin.y, ctx.camera_origin.y);
        assert_eq!(result2.scale, ctx.scale);
    }

    #[test]
    fn test_static_camera_ignores_dt() {
        let ctx = base_ctx();
        let mut cam = StaticCamera::new(ctx.clone());
        let state = empty_state();

        let r1 = cam.update(&state, 0.0);
        let r2 = cam.update(&state, 999.0);

        assert_eq!(r1.camera_origin.y, r2.camera_origin.y);
    }

    // ── StaticCamera::from_world scale and origin ─────────────────────────────

    #[test]
    fn test_from_world_uniform_scale_width_limited() {
        // 100 px wide, 200 px tall; world is 10 m × 10 m
        // scale_x = 100/10 = 10, scale_y = 200/10 = 20 → min = 10
        let cam = StaticCamera::from_world(100, 200, 10.0, 10.0, default_bg());
        assert!(
            (cam.ctx.scale - 10.0).abs() < 1e-5,
            "expected scale 10.0, got {}",
            cam.ctx.scale
        );
        assert_eq!(cam.ctx.camera_origin.x, 0.0);
        assert_eq!(cam.ctx.camera_origin.y, 0.0);
    }

    #[test]
    fn test_from_world_uniform_scale_height_limited() {
        // 200 px wide, 100 px tall; world is 10 m × 10 m
        // scale_x = 200/10 = 20, scale_y = 100/10 = 10 → min = 10
        let cam = StaticCamera::from_world(200, 100, 10.0, 10.0, default_bg());
        assert!(
            (cam.ctx.scale - 10.0).abs() < 1e-5,
            "expected scale 10.0, got {}",
            cam.ctx.scale
        );
    }

    #[test]
    fn test_from_world_exact_fit() {
        // 720×1280 frame, 14.4 m × 25.6 m world → both scales = 50
        let cam = StaticCamera::from_world(720, 1280, 14.4, 25.6, default_bg());
        assert!(
            (cam.ctx.scale - 50.0).abs() < 1e-3,
            "expected scale ~50.0, got {}",
            cam.ctx.scale
        );
        assert_eq!(cam.ctx.width, 720);
        assert_eq!(cam.ctx.height, 1280);
    }

    // ── RaceCamera follows leader downward ────────────────────────────────────

    #[test]
    fn test_race_camera_follows_leader_downward() {
        let config = RaceCameraConfig::default();
        let mut cam = RaceCamera::new(config, base_ctx());

        // Leader at y = 20.0 (high up — near course start)
        let state_high = state_with_racers(&[20.0]);
        // Run many frames to let the camera converge
        let mut ctx = base_ctx();
        for _ in 0..200 {
            ctx = cam.update(&state_high, 0.016);
        }
        let origin_high = ctx.camera_origin.y;

        // Reset and put leader much lower (y = 5.0 — closer to finish)
        cam.reset();
        let state_low = state_with_racers(&[5.0]);
        for _ in 0..200 {
            ctx = cam.update(&state_low, 0.016);
        }
        let origin_low = ctx.camera_origin.y;

        assert!(
            origin_low < origin_high,
            "camera origin should be lower when leader is lower: low={origin_low}, high={origin_high}"
        );
    }

    #[test]
    fn test_race_camera_picks_lowest_y_as_leader() {
        // Disable lower clamping so we can verify the exact target formula.
        let config = RaceCameraConfig {
            min_origin_y: f32::NEG_INFINITY,
            ..Default::default()
        };
        let mut cam = RaceCamera::new(config, base_ctx());

        // Three racers: y = 30, 15, 5. Leader is at y = 5.
        let state = state_with_racers(&[30.0, 15.0, 5.0]);
        for _ in 0..300 {
            cam.update(&state, 0.016);
        }
        // Camera should converge to the origin that places y=5 at leader_screen_fraction from top.
        let viewport_h = 1280.0_f32 / 50.0_f32;
        let config_ref = RaceCameraConfig::default();
        let expected_origin = 5.0 - (1.0 - config_ref.leader_screen_fraction) * viewport_h;
        let actual_origin = cam.current_origin_y;

        assert!(
            (actual_origin - expected_origin).abs() < 1.0,
            "camera should track leader at y=5; expected origin≈{expected_origin:.2}, got {actual_origin:.2}"
        );
    }

    // ── RaceCamera: no racers — holds position ────────────────────────────────

    #[test]
    fn test_race_camera_no_racers_holds_position() {
        let config = RaceCameraConfig::default();
        let initial_ctx = base_ctx(); // camera_origin.y = 0.0
        let mut cam = RaceCamera::new(config, initial_ctx.clone());

        let state = empty_state(); // no bodies at all
        let ctx1 = cam.update(&state, 0.016);
        let ctx2 = cam.update(&state, 0.016);
        let ctx3 = cam.update(&state, 0.016);

        // Should all return the initial origin (no change without racers)
        assert_eq!(ctx1.camera_origin.y, initial_ctx.camera_origin.y);
        assert_eq!(ctx2.camera_origin.y, initial_ctx.camera_origin.y);
        assert_eq!(ctx3.camera_origin.y, initial_ctx.camera_origin.y);
    }

    #[test]
    fn test_race_camera_no_tagged_racers_holds_position() {
        let config = RaceCameraConfig {
            racer_tag: "racer".to_string(),
            ..Default::default()
        };
        let initial_ctx = base_ctx();
        let mut cam = RaceCamera::new(config, initial_ctx.clone());

        // Bodies exist but none are tagged "racer"
        let mut state = empty_state();
        state.bodies.push(BodyState {
            id: BodyId(0),
            name: None,
            tags: vec!["obstacle".to_string()],
            position: Vec2::new(0.0, 5.0),
            rotation: 0.0,
            shape: ShapeKind::Circle { radius: 0.5 },
            color: Color::rgb(128, 128, 128),
            is_alive: true,
            body_type: BodyType::Static,
        });

        let ctx = cam.update(&state, 0.016);
        assert_eq!(
            ctx.camera_origin.y, initial_ctx.camera_origin.y,
            "camera should not move when no racer-tagged bodies present"
        );
    }

    // ── RaceCamera: dead racers are ignored ───────────────────────────────────

    #[test]
    fn test_race_camera_ignores_dead_racers() {
        let config = RaceCameraConfig::default();
        let initial_ctx = base_ctx();
        let mut cam = RaceCamera::new(config, initial_ctx.clone());

        // Dead racer at y = 5.0 (very low)
        let mut racer = make_racer(0, 5.0);
        racer.is_alive = false;
        let state = PhysicsState {
            bodies: vec![racer],
            ..empty_state()
        };

        let ctx = cam.update(&state, 0.016);
        // Camera should not have moved toward y=5 because racer is dead
        assert_eq!(ctx.camera_origin.y, initial_ctx.camera_origin.y);
    }

    // ── RaceCamera: clamping ──────────────────────────────────────────────────

    #[test]
    fn test_race_camera_clamps_min_origin_y() {
        let config = RaceCameraConfig {
            min_origin_y: 10.0,
            damping: 1.0, // instant snap for test
            ..Default::default()
        };
        let initial_ctx = RenderContext {
            camera_origin: Vec2::new(0.0, 10.0),
            ..base_ctx()
        };
        let mut cam = RaceCamera::new(config, initial_ctx);

        // Leader at y = 0.5 — target_origin_y will be deeply negative
        let state = state_with_racers(&[0.5]);
        let ctx = cam.update(&state, 0.016);

        assert!(
            ctx.camera_origin.y >= 10.0,
            "camera_origin.y should be clamped to min_origin_y=10.0, got {}",
            ctx.camera_origin.y
        );
    }

    #[test]
    fn test_race_camera_clamps_max_origin_y() {
        let config = RaceCameraConfig {
            max_origin_y: 5.0,
            damping: 1.0, // instant snap
            ..Default::default()
        };
        let mut cam = RaceCamera::new(config, base_ctx());

        // Leader at y = 1000 — target would be very high
        let state = state_with_racers(&[1000.0]);
        let ctx = cam.update(&state, 0.016);

        assert!(
            ctx.camera_origin.y <= 5.0,
            "camera_origin.y should be clamped to max_origin_y=5.0, got {}",
            ctx.camera_origin.y
        );
    }

    // ── RaceCamera: reset ─────────────────────────────────────────────────────

    #[test]
    fn test_race_camera_reset_restores_initial_position() {
        let config = RaceCameraConfig::default();
        let initial_ctx = base_ctx(); // origin.y = 0.0
        let mut cam = RaceCamera::new(config, initial_ctx);

        // Racer at y = 50.0 → target_origin_y is well above min_origin_y=0,
        // so the camera moves upward from its initial position.
        let state = state_with_racers(&[50.0]);
        for _ in 0..100 {
            cam.update(&state, 0.016);
        }
        assert!(
            cam.current_origin_y > 1.0,
            "camera should have moved up toward the racer (y=50); got {}",
            cam.current_origin_y
        );

        cam.reset();
        assert!(
            (cam.current_origin_y - 0.0).abs() < 1e-5,
            "after reset, current_origin_y should be 0.0, got {}",
            cam.current_origin_y
        );
    }

    // ── FollowCamera helpers ──────────────────────────────────────────────────

    fn default_follow_config() -> CameraConfig {
        CameraConfig::default()
    }

    fn follow_cam_at(world_center: Vec2) -> FollowCamera {
        use rphys_scene::CameraMode;
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4; // viewport_width / world_width
        FollowCamera::new(config, base_scale, ctx, world_center)
    }

    fn collision_event() -> PhysicsEvent {
        use rphys_physics::types::{BodyId, CollisionInfo};
        PhysicsEvent::Collision(CollisionInfo {
            body_a: BodyId(0),
            body_b: BodyId(1),
            impulse: 10.0,
        })
    }

    // ── FollowCamera: instant snap with follow_lerp=1.0 ──────────────────────

    #[test]
    fn test_follow_camera_lerp_one_snaps_instantly() {
        use rphys_scene::CameraMode;
        let world_center = Vec2::new(7.2, 12.8);
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            follow_lerp: 1.0, // instant snap
            look_ahead: 0.0,  // no look-ahead offset so we can predict exactly
            shake_on_impact: false,
            finish_zoom: false,
            lock_horizontal: false, // allow x to move so we can verify snap on both axes
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4;
        let mut cam = FollowCamera::new(config, base_scale, ctx, world_center);

        let leader = Vec2::new(5.0, 20.0);
        cam.update(Some(leader), &[], false);

        // With lerp=1.0 the position should snap exactly to the target.
        assert!(
            (cam.current_pos.x - leader.x).abs() < 1e-5,
            "x should snap instantly: got {}, expected {}",
            cam.current_pos.x,
            leader.x
        );
        assert!(
            (cam.current_pos.y - leader.y).abs() < 1e-5,
            "y should snap instantly: got {}, expected {}",
            cam.current_pos.y,
            leader.y
        );
    }

    // ── FollowCamera: no movement with follow_lerp=0.0 ───────────────────────

    #[test]
    fn test_follow_camera_lerp_zero_never_moves() {
        use rphys_scene::CameraMode;
        let world_center = Vec2::new(7.2, 12.8);
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            follow_lerp: 0.0, // camera never moves
            shake_on_impact: false,
            finish_zoom: false,
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4;
        let mut cam = FollowCamera::new(config, base_scale, ctx, world_center);
        let initial_pos = cam.current_pos;

        // Update toward a very different leader position
        for _ in 0..50 {
            cam.update(Some(Vec2::new(100.0, 100.0)), &[], false);
        }

        assert!(
            (cam.current_pos.x - initial_pos.x).abs() < 1e-5,
            "camera x should not have moved with lerp=0: {} vs {}",
            cam.current_pos.x,
            initial_pos.x
        );
        assert!(
            (cam.current_pos.y - initial_pos.y).abs() < 1e-5,
            "camera y should not have moved with lerp=0: {} vs {}",
            cam.current_pos.y,
            initial_pos.y
        );
    }

    // ── FollowCamera: shake on Collision when shake_on_impact=true ────────────

    #[test]
    fn test_follow_camera_shake_on_impact_true() {
        use rphys_scene::CameraMode;
        let world_center = Vec2::new(7.2, 12.8);
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            shake_on_impact: true,
            shake_intensity: 0.5,
            shake_decay: 1.0, // no decay — keeps shake offset for inspection
            follow_lerp: 0.0, // don't move so we can inspect shake alone
            finish_zoom: false,
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4;
        let mut cam = FollowCamera::new(config, base_scale, ctx, world_center);

        let events = [collision_event()];
        cam.update(None, &events, false);

        let shake_mag = (cam.shake_offset.x * cam.shake_offset.x
            + cam.shake_offset.y * cam.shake_offset.y)
            .sqrt();
        assert!(
            shake_mag > 0.0,
            "shake_offset should be non-zero after a Collision event; got {:?}",
            cam.shake_offset
        );
    }

    // ── FollowCamera: no shake when shake_on_impact=false ─────────────────────

    #[test]
    fn test_follow_camera_shake_disabled() {
        use rphys_scene::CameraMode;
        let world_center = Vec2::new(7.2, 12.8);
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            shake_on_impact: false,
            follow_lerp: 0.0,
            finish_zoom: false,
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4;
        let mut cam = FollowCamera::new(config, base_scale, ctx, world_center);

        let events = [collision_event()];
        cam.update(None, &events, false);

        assert!(
            (cam.shake_offset.x).abs() < 1e-9 && (cam.shake_offset.y).abs() < 1e-9,
            "shake_offset should remain zero when shake_on_impact=false; got {:?}",
            cam.shake_offset
        );
    }

    // ── FollowCamera: zoom lerps toward finish_zoom_factor on race complete ───

    #[test]
    fn test_follow_camera_finish_zoom_lerps() {
        use rphys_scene::CameraMode;
        let world_center = Vec2::new(7.2, 12.8);
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            zoom: 1.0,
            finish_zoom: true,
            finish_zoom_factor: 2.0,
            finish_zoom_lerp: 0.5, // large lerp so we converge quickly in tests
            follow_lerp: 0.0,
            shake_on_impact: false,
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4;
        let mut cam = FollowCamera::new(config, base_scale, ctx, world_center);

        // Before race complete — zoom should stay at initial value.
        for _ in 0..10 {
            cam.update(None, &[], false);
        }
        assert!(
            (cam.current_zoom - 1.0).abs() < 1e-5,
            "zoom should not change before race complete; got {}",
            cam.current_zoom
        );

        // After race complete — zoom should start approaching finish_zoom_factor.
        for _ in 0..20 {
            cam.update(None, &[], true);
        }
        assert!(
            cam.current_zoom > 1.0,
            "zoom should have increased toward finish_zoom_factor; got {}",
            cam.current_zoom
        );
        assert!(
            cam.current_zoom <= 2.0,
            "zoom should not exceed finish_zoom_factor=2.0; got {}",
            cam.current_zoom
        );
        // After many frames it should be close to the target.
        for _ in 0..200 {
            cam.update(None, &[], true);
        }
        assert!(
            (cam.current_zoom - 2.0).abs() < 0.05,
            "zoom should converge near finish_zoom_factor=2.0; got {}",
            cam.current_zoom
        );
    }

    // ── FollowCamera: zoom stays at 1.0 when finish_zoom=false ───────────────

    #[test]
    fn test_follow_camera_finish_zoom_disabled() {
        use rphys_scene::CameraMode;
        let world_center = Vec2::new(7.2, 12.8);
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            zoom: 1.0,
            finish_zoom: false,
            finish_zoom_factor: 2.0,
            follow_lerp: 0.0,
            shake_on_impact: false,
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4;
        let mut cam = FollowCamera::new(config, base_scale, ctx, world_center);

        for _ in 0..50 {
            cam.update(None, &[], true); // race_complete=true but finish_zoom=false
        }

        assert!(
            (cam.current_zoom - 1.0).abs() < 1e-5,
            "zoom should stay at 1.0 when finish_zoom=false; got {}",
            cam.current_zoom
        );
    }

    // ── FollowCamera: freezes when race complete and no leader ────────────────

    /// When `leader_pos` is `None` and `race_complete` is `true`, the camera
    /// must **not** move — it freezes exactly at its last known position.
    ///
    /// This prevents the camera from drifting to `world_center` once all
    /// racers have crossed the finish line and left the active list.
    #[test]
    fn test_follow_camera_freezes_when_race_complete_no_leader() {
        use rphys_scene::CameraMode;
        let world_center = Vec2::new(7.2, 12.8);
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            follow_lerp: 0.5, // non-zero so we can observe movement
            shake_on_impact: false,
            finish_zoom: false,
            lock_horizontal: false,
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4;
        let mut cam = FollowCamera::new(config, base_scale, ctx, world_center);

        // Drive the camera to a known position by following a leader.
        let leader = Vec2::new(3.0, 8.0);
        for _ in 0..100 {
            cam.update(Some(leader), &[], false);
        }
        let pos_after_follow = cam.current_pos;

        // Now simulate: race complete, no leader (all racers finished).
        for _ in 0..100 {
            cam.update(None, &[], true);
        }

        // Camera must not have moved at all.
        assert!(
            (cam.current_pos.x - pos_after_follow.x).abs() < 1e-5,
            "camera x should be frozen at {:.4}, got {:.4}",
            pos_after_follow.x,
            cam.current_pos.x
        );
        assert!(
            (cam.current_pos.y - pos_after_follow.y).abs() < 1e-5,
            "camera y should be frozen at {:.4}, got {:.4}",
            pos_after_follow.y,
            cam.current_pos.y
        );
    }

    // ── FollowCamera: falls back to world_center before race starts ───────────

    /// When `leader_pos` is `None` and `race_complete` is `false` (pre-race
    /// startup), the camera must lerp toward `world_center` (existing behaviour
    /// unchanged).
    #[test]
    fn test_follow_camera_startup_fallback_to_world_center() {
        use rphys_scene::CameraMode;
        let world_center = Vec2::new(7.2, 12.8);
        let config = CameraConfig {
            mode: CameraMode::FollowLeader,
            follow_lerp: 0.5,
            shake_on_impact: false,
            finish_zoom: false,
            lock_horizontal: false,
            ..CameraConfig::default()
        };
        let ctx = base_ctx();
        let base_scale = ctx.width as f32 / 14.4;
        // Start camera far from world_center.
        let mut cam = FollowCamera::new(config, base_scale, ctx, world_center);
        cam.current_pos = Vec2::new(0.0, 0.0);

        // With lerp=0.5 and no leader, camera should move toward world_center.
        for _ in 0..100 {
            cam.update(None, &[], false); // race NOT complete
        }

        // After many frames the camera should have converged near world_center.
        assert!(
            (cam.current_pos.x - world_center.x).abs() < 0.1,
            "x should converge to world_center.x ({:.2}), got {:.2}",
            world_center.x,
            cam.current_pos.x
        );
        assert!(
            (cam.current_pos.y - world_center.y).abs() < 0.1,
            "y should converge to world_center.y ({:.2}), got {:.2}",
            world_center.y,
            cam.current_pos.y
        );
    }
}
