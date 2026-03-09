//! Particle and flash data types used by [`crate::engine::VfxEngine`].

use rphys_physics::types::BodyId;
use rphys_scene::{Color, Vec2};

// ── ParticleKind ──────────────────────────────────────────────────────────────

/// Visual appearance of a particle.
///
/// Currently only `Dot` is implemented; additional variants (e.g. `Sparkle`,
/// `Trail`) can be added in future iterations without breaking existing
/// callers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParticleKind {
    /// A filled circle of a given screen-space radius.
    Dot,
}

// ── KinematicParticle ─────────────────────────────────────────────────────────

/// A single particle that moves under constant velocity and fades out over
/// its lifetime.
///
/// Coordinates are **world-space** (meters, Y-up). The VFX engine converts
/// to pixel space at render time using the current [`rphys_renderer::RenderContext`].
#[derive(Debug, Clone)]
pub struct KinematicParticle {
    /// Current world-space position (meters, Y-up).
    pub pos: Vec2,
    /// World-space velocity (meters per second, Y-up).
    pub vel: Vec2,
    /// Remaining lifetime in seconds.  When `<= 0` the particle is dead.
    pub lifetime_rem: f32,
    /// Total lifetime the particle was created with (used to compute alpha).
    pub lifetime_total: f32,
    /// Dot radius in pixels.
    pub size_px: f32,
    /// Particle tint color (RGB only; alpha is derived from `alpha_factor()`).
    pub color: Color,
    /// Visual appearance.
    pub kind: ParticleKind,
}

impl KinematicParticle {
    /// Linear fade factor: `1.0` when freshly spawned, `0.0` when dead.
    ///
    /// Multiply this by the desired maximum opacity when drawing.
    #[inline]
    pub fn alpha_factor(&self) -> f32 {
        if self.lifetime_total <= 0.0 {
            return 0.0;
        }
        (self.lifetime_rem / self.lifetime_total).clamp(0.0, 1.0)
    }

    /// Advance the particle by `dt` seconds.  Call each physics step.
    #[inline]
    pub fn update(&mut self, dt: f32) {
        self.pos.x += self.vel.x * dt;
        self.pos.y += self.vel.y * dt;
        self.lifetime_rem -= dt;
    }

    /// `true` when the particle has expired and should be removed from the pool.
    #[inline]
    pub fn is_dead(&self) -> bool {
        self.lifetime_rem <= 0.0
    }
}

// ── ActiveFlash ───────────────────────────────────────────────────────────────

/// A boost-pad glow overlay anchored to a living body.
///
/// The flash fades linearly from full opacity to transparent over
/// `duration_secs` and is drawn as a translucent glow ring around the ball.
#[derive(Debug, Clone)]
pub struct ActiveFlash {
    /// The body this flash is attached to.
    pub body_id: BodyId,
    /// Current world-space centre of the body (meters, Y-up).
    ///
    /// Renamed from `center_px`; updated each frame from
    /// [`VfxEngine::begin_frame`].
    pub center_world: Vec2,
    /// Glow color (constant for the flash's lifetime).
    pub color: Color,
    /// Total glow radius = body screen-radius + `radius_ext_px`.
    pub radius_ext_px: f32,
    /// Remaining fade time in seconds.
    pub time_rem: f32,
    /// Total duration the flash was created with (for alpha calculation).
    pub duration: f32,
}

impl ActiveFlash {
    /// Linear fade factor: `1.0` when just activated, `0.0` when expired.
    #[inline]
    pub fn alpha_factor(&self) -> f32 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        (self.time_rem / self.duration).clamp(0.0, 1.0)
    }

    /// Advance the flash timer by `dt` seconds.
    #[inline]
    pub fn update(&mut self, dt: f32) {
        self.time_rem -= dt;
    }

    /// `true` when the flash has fully faded and should be removed.
    #[inline]
    pub fn is_expired(&self) -> bool {
        self.time_rem <= 0.0
    }
}
