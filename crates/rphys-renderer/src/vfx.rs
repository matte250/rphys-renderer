//! Particle-based visual effects (VFX) for the rphys renderer.
//!
//! The main entry point is [`VfxSystem`], which owns a pool of [`Particle`]s,
//! a set of active [`BoostFlash`]es, and a lightweight deterministic LCG RNG.
//!
//! # Integration
//!
//! 1. Construct with [`VfxSystem::new`] from a cloned [`VfxConfig`].
//! 2. After each physics step, call the appropriate `emit_*` methods.
//! 3. Call [`VfxSystem::update`] with `dt` once per frame (via `push_frame`).
//! 4. Call [`VfxSystem::render_into`] with a `&mut Pixmap` to composite all
//!    live effects on top of the rendered physics frame.

use rphys_scene::{Color, VfxConfig, WinnerPopConfig};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::RenderContext;

// ── Deterministic LCG RNG ─────────────────────────────────────────────────────

/// Simple 64-bit multiplicative LCG pseudo-random number generator.
///
/// Uses Knuth LCG coefficients.  No external crate dependency; identical
/// seed produces identical output (deterministic replay).
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Return a pseudo-random `f32` in `[0, 1)`.
    fn next_f32(&mut self) -> f32 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let bits = (self.state >> 41) as u32;
        f32::from_bits(0x3F80_0000 | bits) - 1.0
    }

    /// Return a pseudo-random `f32` in `[lo, hi)`.
    fn next_range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f32() * (hi - lo)
    }
}

// ── Particle ──────────────────────────────────────────────────────────────────

/// A single active spark / burst / confetti particle.
#[derive(Debug, Clone)]
struct Particle {
    /// Current pixel-space position `[x, y]`.
    pos: [f32; 2],
    /// Velocity in pixel-space `[vx, vy]` (pixels per second).
    vel: [f32; 2],
    /// Remaining lifetime in seconds.  Particle is dropped when ≤ 0.
    lifetime: f32,
    /// Maximum lifetime — used to compute the fade alpha.
    max_lifetime: f32,
    /// Rendered dot radius in pixels.
    size_px: f32,
    /// Particle color.
    color: Color,
    /// Downward gravitational acceleration in px/s².  `0.0` = weightless.
    gravity_px: f32,
}

impl Particle {
    fn is_alive(&self) -> bool {
        self.lifetime > 0.0
    }

    fn alpha(&self) -> f32 {
        (self.lifetime / self.max_lifetime).clamp(0.0, 1.0)
    }
}

// ── BoostFlash ────────────────────────────────────────────────────────────────

/// A transient glow halo rendered around a boosted ball.
#[derive(Debug, Clone)]
struct BoostFlash {
    /// Pixel-space centre of the glow.
    center_px: [f32; 2],
    /// Total outer glow radius in pixels (ball radius + `radius_px` config).
    radius_px: f32,
    /// Remaining fade time in seconds.
    remaining: f32,
    /// Total flash duration — used to compute the fade alpha.
    duration: f32,
    /// Glow color.
    color: Color,
}

impl BoostFlash {
    fn alpha(&self) -> f32 {
        (self.remaining / self.duration).clamp(0.0, 1.0)
    }
}

// ── VfxSystem ─────────────────────────────────────────────────────────────────

/// Manages all live particles and boost-flash halos for a single scene.
///
/// Owned by the [`TrailRenderer`](crate::TrailRenderer) when the scene provides
/// a `vfx:` configuration block.  When the block is absent, `None` is stored
/// and all VFX code paths are bypassed (zero cost).
pub struct VfxSystem {
    cfg: VfxConfig,
    /// Live particle pool. Dead particles are compacted on every `update` call.
    particles: Vec<Particle>,
    /// Live boost-flash halos (one per boosted ball, keyed by proximity).
    flashes: Vec<BoostFlash>,
    /// Deterministic RNG seeded at construction time.
    rng: Rng,
}

impl VfxSystem {
    /// Create a new [`VfxSystem`] from a validated [`VfxConfig`].
    pub fn new(cfg: VfxConfig) -> Self {
        let cap = cfg.max_particles;
        Self {
            cfg,
            particles: Vec::with_capacity(cap),
            flashes: Vec::new(),
            rng: Rng::new(0xDEAD_BEEF_CAFE_1337),
        }
    }

    // ── Emitters ──────────────────────────────────────────────────────────────

    /// Emit impact sparks at `pos_px` in the ball's `color`.
    ///
    /// Sparks radiate outward in random directions.  Call on every
    /// `PhysicsEvent::Collision` or `PhysicsEvent::WallBounce`.
    ///
    /// No-ops when `vfx.impact_sparks.enabled` is `false`.
    pub fn emit_impact_sparks(&mut self, pos_px: [f32; 2], color: Color) {
        let sc = &self.cfg.impact_sparks;
        if !sc.enabled {
            return;
        }
        let can_emit = self.cfg.max_particles.saturating_sub(self.live_count());
        let n = sc.count.min(can_emit);
        self.burst_radial(
            pos_px,
            color,
            n,
            sc.lifetime_secs,
            sc.size_px,
            sc.speed,
            0.0,
        );
    }

    /// Register (or refresh) a boost-flash halo for the ball at `center_px`.
    ///
    /// If the ball already has an active flash (within 1 px), it is refreshed
    /// rather than duplicated.
    ///
    /// `ball_radius_px` is the ball's rendered radius in pixels; the glow halo
    /// extends `vfx.boost_flash.radius_px` beyond that edge.
    ///
    /// No-ops when `vfx.boost_flash.enabled` is `false`.
    pub fn emit_boost_flash(&mut self, center_px: [f32; 2], ball_radius_px: f32) {
        let bc = &self.cfg.boost_flash;
        if !bc.enabled {
            return;
        }
        let radius_px = ball_radius_px + bc.radius_px;
        let duration = bc.duration_secs;
        let color = bc.color;

        // Replace an existing flash when the centres are within 1 px (same ball).
        for flash in &mut self.flashes {
            let dx = flash.center_px[0] - center_px[0];
            let dy = flash.center_px[1] - center_px[1];
            if dx * dx + dy * dy < 1.0 {
                flash.center_px = center_px;
                flash.radius_px = radius_px;
                flash.remaining = duration;
                flash.duration = duration;
                flash.color = color;
                return;
            }
        }

        self.flashes.push(BoostFlash {
            center_px,
            radius_px,
            remaining: duration,
            duration,
            color,
        });
    }

    /// Emit an elimination burst at `pos_px` in the eliminated ball's `color`.
    ///
    /// No-ops when `vfx.elimination_burst.enabled` is `false`.
    pub fn emit_elimination_burst(&mut self, pos_px: [f32; 2], color: Color) {
        let ec = &self.cfg.elimination_burst;
        if !ec.enabled {
            return;
        }
        let can_emit = self.cfg.max_particles.saturating_sub(self.live_count());
        let n = ec.count.min(can_emit);
        // Use gentle gravity so burst particles arc downward.
        self.burst_radial(
            pos_px,
            color,
            n,
            ec.lifetime_secs,
            ec.size_px,
            ec.speed,
            50.0,
        );
    }

    /// Emit a winner confetti pop centred on `pos_px`.
    ///
    /// `winner_color` is added to the palette alongside any colors in
    /// `vfx.winner_pop.colors` (or gold + white when the list is empty).
    ///
    /// No-ops when `vfx.winner_pop.enabled` is `false`.
    pub fn emit_winner_pop(&mut self, pos_px: [f32; 2], winner_color: Color) {
        let wc = self.cfg.winner_pop.clone();
        if !wc.enabled {
            return;
        }
        let can_emit = self.cfg.max_particles.saturating_sub(self.live_count());
        let n = wc.count.min(can_emit);
        self.burst_fan(pos_px, winner_color, n, &wc);
    }

    // ── Update ────────────────────────────────────────────────────────────────

    /// Advance all live particles and flash timers by `dt` seconds.
    ///
    /// Dead particles and expired flashes are compacted out of the pools.
    pub fn update(&mut self, dt: f32) {
        for p in &mut self.particles {
            if !p.is_alive() {
                continue;
            }
            p.vel[1] += p.gravity_px * dt;
            p.pos[0] += p.vel[0] * dt;
            p.pos[1] += p.vel[1] * dt;
            p.lifetime -= dt;
        }
        self.particles.retain(|p| p.is_alive());

        for f in &mut self.flashes {
            f.remaining -= dt;
        }
        self.flashes.retain(|f| f.remaining > 0.0);
    }

    // ── Render ────────────────────────────────────────────────────────────────

    /// Composite all live VFX (boost flash halos + particles) onto `pixmap`.
    ///
    /// Boost flash glows are drawn first (larger halos), then particles on top.
    /// Call this once per frame after the base physics frame has been written
    /// to `pixmap`.
    pub fn render_into(&self, pixmap: &mut Pixmap, _ctx: &RenderContext) {
        // 1. Boost flash glows (drawn below particles).
        for flash in &self.flashes {
            draw_glow(
                pixmap,
                flash.center_px,
                flash.radius_px,
                flash.color,
                flash.alpha(),
            );
        }

        // 2. Spark / burst / confetti particles.
        for p in &self.particles {
            if !p.is_alive() {
                continue;
            }
            draw_dot(pixmap, p.pos, p.size_px, p.color, p.alpha());
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Count of currently alive particles.
    fn live_count(&self) -> usize {
        self.particles.iter().filter(|p| p.is_alive()).count()
    }

    /// Emit `n` particles spreading in random radial directions from `pos_px`.
    ///
    /// `gravity_px` sets downward acceleration in px/s² (`0.0` = weightless).
    #[allow(clippy::too_many_arguments)]
    fn burst_radial(
        &mut self,
        pos_px: [f32; 2],
        color: Color,
        n: usize,
        lifetime: f32,
        size_px: f32,
        speed: f32,
        gravity_px: f32,
    ) {
        for _ in 0..n {
            let angle = self.rng.next_f32() * std::f32::consts::TAU;
            let v = self.rng.next_range(speed * 0.5, speed);
            self.particles.push(Particle {
                pos: pos_px,
                vel: [angle.cos() * v, angle.sin() * v],
                lifetime,
                max_lifetime: lifetime,
                size_px,
                color,
                gravity_px,
            });
        }
    }

    /// Emit `n` confetti particles in an upward fan pattern for the winner pop.
    fn burst_fan(&mut self, pos_px: [f32; 2], winner_color: Color, n: usize, wc: &WinnerPopConfig) {
        // Build the color palette.
        let palette: Vec<Color> = if wc.colors.is_empty() {
            vec![
                winner_color,
                Color::rgb(0xFF, 0xD7, 0x00), // gold
                Color::WHITE,
            ]
        } else {
            let mut p = wc.colors.clone();
            p.push(winner_color);
            p
        };

        // Fan centred upward; in pixel-space Y is down so "up" = negative Y velocity.
        let center_angle = -std::f32::consts::FRAC_PI_2;
        let half_spread = wc.spread_deg.to_radians() * 0.5;

        for i in 0..n {
            let angle = center_angle + self.rng.next_range(-half_spread, half_spread);
            let v = self.rng.next_range(wc.speed * 0.4, wc.speed);
            let color = palette[i % palette.len()];

            self.particles.push(Particle {
                pos: pos_px,
                vel: [angle.cos() * v, angle.sin() * v],
                lifetime: wc.lifetime_secs,
                max_lifetime: wc.lifetime_secs,
                size_px: wc.size_px,
                color,
                gravity_px: 120.0, // gentle downward drift
            });
        }
    }
}

// ── Low-level pixel drawing helpers ──────────────────────────────────────────

/// Convert an [`rphys_scene::Color`] + `alpha` to a `tiny_skia::Color`.
fn to_skia_color(c: Color, alpha: f32) -> tiny_skia::Color {
    let combined = (c.a as f32 / 255.0) * alpha;
    tiny_skia::Color::from_rgba(
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        combined,
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT)
}

/// Draw a filled, anti-aliased dot of `radius` at `pos` with the given color
/// and `alpha` multiplier.
fn draw_dot(pixmap: &mut Pixmap, pos: [f32; 2], radius: f32, color: Color, alpha: f32) {
    if radius <= 0.0 || alpha <= 0.0 {
        return;
    }
    let skia_color = to_skia_color(color, alpha);

    let Some(path) = ({
        let mut pb = PathBuilder::new();
        pb.push_circle(pos[0], pos[1], radius);
        pb.finish()
    }) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(skia_color);
    paint.anti_alias = true;

    pixmap.fill_path(
        &path,
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Draw a soft radial glow as layered concentric circles (outer transparent
/// → inner opaque at `alpha`).
fn draw_glow(pixmap: &mut Pixmap, center: [f32; 2], radius: f32, color: Color, alpha: f32) {
    if radius <= 0.0 || alpha <= 0.0 {
        return;
    }
    const LAYERS: usize = 6;
    for i in 0..LAYERS {
        let t = i as f32 / (LAYERS - 1) as f32;
        let layer_radius = radius * (1.0 - t * 0.5);
        let layer_alpha = alpha * t * 0.7;
        draw_dot(pixmap, center, layer_radius, color, layer_alpha);
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_scene::{
        BoostFlashConfig, EliminationBurstConfig, ImpactSparksConfig, Vec2, VfxConfig,
        WinnerPopConfig,
    };

    fn enabled_cfg() -> VfxConfig {
        VfxConfig {
            max_particles: 500,
            impact_sparks: ImpactSparksConfig {
                enabled: true,
                count: 12,
                lifetime_secs: 0.25,
                size_px: 2.0,
                speed: 200.0,
            },
            boost_flash: BoostFlashConfig {
                enabled: true,
                color: Color::WHITE,
                radius_px: 8.0,
                duration_secs: 0.3,
            },
            elimination_burst: EliminationBurstConfig {
                enabled: true,
                count: 30,
                lifetime_secs: 0.6,
                size_px: 3.0,
                speed: 300.0,
            },
            winner_pop: WinnerPopConfig {
                enabled: true,
                count: 60,
                lifetime_secs: 1.2,
                size_px: 4.0,
                speed: 350.0,
                spread_deg: 180.0,
                colors: vec![],
            },
        }
    }

    fn test_ctx() -> RenderContext {
        RenderContext {
            width: 400,
            height: 600,
            camera_origin: Vec2::ZERO,
            scale: 20.0,
            background_color: Color::rgb(10, 10, 20),
        }
    }

    // ── Particle pool ─────────────────────────────────────────────────────────

    #[test]
    fn test_emit_impact_sparks_adds_particles() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_impact_sparks([100.0, 200.0], Color::rgb(255, 0, 0));
        assert_eq!(sys.live_count(), 12);
    }

    #[test]
    fn test_emit_impact_sparks_noop_when_disabled() {
        let mut cfg = enabled_cfg();
        cfg.impact_sparks.enabled = false;
        let mut sys = VfxSystem::new(cfg);
        sys.emit_impact_sparks([0.0, 0.0], Color::WHITE);
        assert_eq!(sys.live_count(), 0);
    }

    #[test]
    fn test_emit_elimination_burst_adds_particles() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_elimination_burst([200.0, 300.0], Color::rgb(100, 200, 50));
        assert_eq!(sys.live_count(), 30);
    }

    #[test]
    fn test_emit_winner_pop_adds_particles() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_winner_pop([200.0, 400.0], Color::rgb(255, 215, 0));
        assert_eq!(sys.live_count(), 60);
    }

    #[test]
    fn test_max_particles_cap_respected() {
        let mut cfg = enabled_cfg();
        cfg.max_particles = 10;
        cfg.impact_sparks.count = 100;
        let mut sys = VfxSystem::new(cfg);
        sys.emit_impact_sparks([0.0, 0.0], Color::WHITE);
        assert!(
            sys.live_count() <= 10,
            "pool must not exceed max_particles; got {}",
            sys.live_count()
        );
    }

    // ── Update / lifetime ─────────────────────────────────────────────────────

    #[test]
    fn test_particles_expire_after_lifetime() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_impact_sparks([0.0, 0.0], Color::WHITE);
        assert!(sys.live_count() > 0);
        sys.update(1.0); // lifetime = 0.25 s
        assert_eq!(
            sys.live_count(),
            0,
            "all particles should be dead after 1 s"
        );
    }

    #[test]
    fn test_particles_move_each_frame() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_impact_sparks([100.0, 100.0], Color::WHITE);
        let initial = sys.particles[0].pos;
        sys.update(0.01);
        let after = sys.particles[0].pos;
        let moved = (after[0] - initial[0]).abs() > 1e-6 || (after[1] - initial[1]).abs() > 1e-6;
        assert!(moved, "particles must move after update");
    }

    // ── Boost flash ───────────────────────────────────────────────────────────

    #[test]
    fn test_emit_boost_flash_creates_flash() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_boost_flash([200.0, 300.0], 10.0);
        assert_eq!(sys.flashes.len(), 1);
        // Total radius = ball_radius (10) + config radius_px (8) = 18
        assert!((sys.flashes[0].radius_px - 18.0).abs() < 1e-5);
    }

    #[test]
    fn test_emit_boost_flash_replaces_for_same_position() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_boost_flash([200.0, 300.0], 10.0);
        sys.emit_boost_flash([200.0, 300.0], 12.0); // same position → refresh
        assert_eq!(sys.flashes.len(), 1, "second emit should refresh, not add");
    }

    #[test]
    fn test_boost_flash_expires_after_duration() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_boost_flash([100.0, 100.0], 5.0);
        sys.update(1.0); // duration = 0.3 s
        assert!(sys.flashes.is_empty(), "flash must expire after duration");
    }

    #[test]
    fn test_boost_flash_noop_when_disabled() {
        let mut cfg = enabled_cfg();
        cfg.boost_flash.enabled = false;
        let mut sys = VfxSystem::new(cfg);
        sys.emit_boost_flash([0.0, 0.0], 5.0);
        assert!(sys.flashes.is_empty());
    }

    // ── Render ────────────────────────────────────────────────────────────────

    #[test]
    fn test_render_into_does_not_panic() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_impact_sparks([200.0, 300.0], Color::rgb(255, 0, 0));
        sys.emit_boost_flash([200.0, 300.0], 10.0);
        sys.emit_elimination_burst([150.0, 200.0], Color::rgb(0, 255, 0));
        sys.emit_winner_pop([200.0, 400.0], Color::rgb(255, 215, 0));

        let ctx = test_ctx();
        let mut pixmap = Pixmap::new(ctx.width, ctx.height).expect("pixmap");
        sys.render_into(&mut pixmap, &ctx); // must not panic
    }

    #[test]
    fn test_render_into_writes_pixels() {
        let mut sys = VfxSystem::new(enabled_cfg());
        // Emit sparks and pin them all to the center.
        sys.emit_impact_sparks([200.0, 300.0], Color::rgb(255, 255, 255));
        for p in &mut sys.particles {
            p.pos = [200.0, 300.0];
            p.vel = [0.0, 0.0];
        }

        let ctx = test_ctx();
        let mut pixmap = Pixmap::new(ctx.width, ctx.height).expect("pixmap");
        sys.render_into(&mut pixmap, &ctx);

        let any_nonzero = pixmap.data().iter().any(|&b| b != 0);
        assert!(
            any_nonzero,
            "render_into must write at least some non-zero pixels"
        );
    }

    // ── LCG RNG ───────────────────────────────────────────────────────────────

    #[test]
    fn test_rng_output_in_range() {
        let mut rng = Rng::new(42);
        for _ in 0..1_000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "LCG output {v} out of [0, 1)");
        }
    }

    #[test]
    fn test_rng_deterministic() {
        let mut a = Rng::new(99_999);
        let mut b = Rng::new(99_999);
        for _ in 0..200 {
            assert_eq!(a.next_f32(), b.next_f32(), "RNG must be deterministic");
        }
    }

    // ── Particle alpha ────────────────────────────────────────────────────────

    #[test]
    fn test_particle_alpha_starts_at_one() {
        let p = Particle {
            pos: [0.0, 0.0],
            vel: [0.0, 0.0],
            lifetime: 0.5,
            max_lifetime: 0.5,
            size_px: 2.0,
            color: Color::WHITE,
            gravity_px: 0.0,
        };
        assert!((p.alpha() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_particle_alpha_decreases_over_time() {
        let mut sys = VfxSystem::new(enabled_cfg());
        sys.emit_impact_sparks([0.0, 0.0], Color::WHITE);

        let before: Vec<f32> = sys.particles.iter().map(|p| p.alpha()).collect();
        sys.update(0.1);
        let after: Vec<f32> = sys.particles.iter().map(|p| p.alpha()).collect();

        for (b, a) in before.iter().zip(after.iter()) {
            assert!(a <= b, "alpha must not increase over time");
        }
    }
}
