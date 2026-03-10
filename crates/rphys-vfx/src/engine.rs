//! [`VfxEngine`] — the central coordinator for all VFX effects.
//!
//! # Usage pattern (one rendered frame)
//!
//! ```rust,ignore
//! // 1. Snapshot current body positions (world coords) + pass camera scale.
//! vfx.begin_frame(&body_snapshot, finish_line_world, ctx.scale);
//!
//! // 2. Feed physics + race events (spawns particles / flashes).
//! vfx.feed_events(&physics_events, &race_events, &lookup);
//!
//! // 3. Advance particle lifetimes.
//! vfx.update(dt);
//!
//! // 4. Composite all VFX into the rendered frame (camera transform applied here).
//! vfx.render_into(&mut frame, &ctx);
//! ```

use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;

use rphys_physics::types::{BodyId, PhysicsEvent};
use rphys_race::{RaceEvent, WinnerInfo};
use rphys_renderer::{Frame, RenderContext};
use rphys_scene::{Color, Vec2, VfxConfig};

use crate::blend::{draw_dot, draw_glow};
use crate::particle::{ActiveFlash, KinematicParticle, ParticleKind};
use crate::rng::LcgRng;

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Internal parameter bundle describing a burst of spark particles.
struct SparkParams {
    count: usize,
    lifetime_secs: f32,
    size_px: f32,
    speed: f32,
    angle_min: f32,
    angle_max: f32,
    /// Optional palette; `None` = use the caller-supplied default color.
    colors: Option<Vec<Color>>,
}

/// A pending spark emission collected during event processing.
struct PendingSparks {
    pos: Vec2,
    color: Color,
    params: SparkParams,
}

/// A pending boost-flash upsert collected during event processing.
struct PendingFlash {
    body: BodyId,
    center_world: Vec2,
    body_radius_world: f32,
}

// ── VfxEngine ─────────────────────────────────────────────────────────────────

/// VFX engine: manages particles and boost flashes for a single race export.
///
/// All positions stored internally are **world-space** (meters, Y-up).
/// The camera transform is applied only at [`render_into`] time.
pub struct VfxEngine {
    // ── Configuration ─────────────────────────────────────────────────────
    config: VfxConfig,

    // ── Particle pool ──────────────────────────────────────────────────────
    /// Active kinematic particles (sparks, burst, confetti).
    particles: Vec<KinematicParticle>,

    // ── Boost-flash overlays ───────────────────────────────────────────────
    /// One entry per body currently producing a glow flash.
    flashes: HashMap<BodyId, ActiveFlash>,

    // ── Per-frame helpers ──────────────────────────────────────────────────
    /// Last known world-space position, color, and world-radius for each body.
    ///
    /// Updated at the start of every frame by [`begin_frame`].
    last_known_positions: HashMap<BodyId, (Vec2, Color, f32)>,

    /// Collision pairs already processed this frame (deduplication).
    ///
    /// The pair is stored as `(min_id, max_id)` so both orderings map to the
    /// same entry.
    frame_collision_dedup: HashSet<(u32, u32)>,

    // ── One-shot guard ─────────────────────────────────────────────────────
    /// `true` after the winner-pop burst has been emitted.
    winner_pop_fired: bool,

    // ── Finish-line world position ─────────────────────────────────────────
    /// World-space position of the finish line (updated each frame).
    finish_line_world: Option<Vec2>,

    /// Pixels-per-meter scale from the most recent frame.
    ///
    /// Used to convert config speed (px/s) → world units/s at emit time.
    current_scale: f32,

    // ── RNG ───────────────────────────────────────────────────────────────
    rng: LcgRng,
}

impl VfxEngine {
    /// Construct a new VFX engine from a validated [`VfxConfig`].
    pub fn new(config: VfxConfig) -> Self {
        let seed = (config.max_particles as u64)
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        Self {
            config,
            particles: Vec::new(),
            flashes: HashMap::new(),
            last_known_positions: HashMap::new(),
            frame_collision_dedup: HashSet::new(),
            winner_pop_fired: false,
            finish_line_world: None,
            current_scale: 1.0,
            rng: LcgRng::new(seed),
        }
    }

    // ── Per-frame API ─────────────────────────────────────────────────────────

    /// Update the engine's snapshot of all live body positions.
    ///
    /// Must be called **once at the beginning of each rendered frame**, before
    /// [`feed_events`] and [`update`].
    ///
    /// # Parameters
    ///
    /// - `body_snapshot` — slice of `(BodyId, world_pos, color, world_radius_meters)`
    ///   for every **alive** body this frame.  Positions are in meters, Y-up.
    /// - `finish_line_world` — world-space position of the finish line (meters).
    /// - `scale` — pixels per meter from the current [`RenderContext`].
    ///   Stored as `current_scale` for speed conversion in emit calls.
    pub fn begin_frame(
        &mut self,
        body_snapshot: &[(BodyId, Vec2, Color, f32)],
        finish_line_world: Vec2,
        scale: f32,
    ) {
        self.current_scale = scale;

        // Refresh position cache.
        self.last_known_positions.clear();
        for &(id, pos, color, radius) in body_snapshot {
            self.last_known_positions.insert(id, (pos, color, radius));
        }

        // Update boost-flash centres to latest world positions.
        let updates: Vec<(BodyId, Vec2)> = self
            .flashes
            .keys()
            .filter_map(|id| {
                self.last_known_positions
                    .get(id)
                    .map(|&(pos, _, _)| (*id, pos))
            })
            .collect();
        for (id, pos) in updates {
            if let Some(flash) = self.flashes.get_mut(&id) {
                flash.center_world = pos;
            }
        }

        self.finish_line_world = Some(finish_line_world);
        self.frame_collision_dedup.clear();
    }

    /// Process physics and race events, spawning particles and flash overlays.
    ///
    /// Uses a two-phase approach (collect → execute) to satisfy the borrow
    /// checker: first we gather event data from `last_known_positions`, then
    /// we mutate `particles` and `flashes`.
    pub fn feed_events<F>(
        &mut self,
        physics_events: &[PhysicsEvent],
        race_events: &[RaceEvent],
        _lookup: &F,
    ) where
        F: Fn(BodyId) -> Option<(Vec2, Color, f32)>,
    {
        // ── Phase 1: collect pending work ─────────────────────────────────
        let mut pending_sparks: Vec<PendingSparks> = Vec::new();
        let mut pending_flashes: Vec<PendingFlash> = Vec::new();
        let mut winner_event: Option<WinnerInfo> = None;

        for event in physics_events {
            match event {
                PhysicsEvent::Collision(info) if self.config.impact_sparks.enabled => {
                    let a = info.body_a.0.min(info.body_b.0);
                    let b = info.body_a.0.max(info.body_b.0);
                    if !self.frame_collision_dedup.insert((a, b)) {
                        continue;
                    }
                    let pos_a = self
                        .last_known_positions
                        .get(&info.body_a)
                        .map(|&(p, _, _)| p);
                    let lookup_b = self.last_known_positions.get(&info.body_b).copied();

                    let (pos, color) = match (pos_a, lookup_b) {
                        (Some(pa), Some((pb, cb, _))) => {
                            let mid = Vec2::new((pa.x + pb.x) * 0.5, (pa.y + pb.y) * 0.5);
                            (mid, cb)
                        }
                        (Some(pa), None) => {
                            let color = self
                                .last_known_positions
                                .get(&info.body_a)
                                .map(|&(_, c, _)| c)
                                .unwrap_or(Color::WHITE);
                            (pa, color)
                        }
                        _ => continue,
                    };

                    let cfg = &self.config.impact_sparks;
                    pending_sparks.push(PendingSparks {
                        pos,
                        color,
                        params: SparkParams {
                            count: cfg.count,
                            lifetime_secs: cfg.lifetime_secs,
                            size_px: cfg.size_px,
                            speed: cfg.speed,
                            angle_min: 0.0,
                            angle_max: 2.0 * PI,
                            colors: None,
                        },
                    });
                }

                PhysicsEvent::BoostActivated { body } if self.config.boost_flash.enabled => {
                    if let Some(&(pos, _, radius)) = self.last_known_positions.get(body) {
                        pending_flashes.push(PendingFlash {
                            body: *body,
                            center_world: pos,
                            body_radius_world: radius,
                        });
                    }
                }

                PhysicsEvent::BumperActivated { contact_point, .. }
                    if self.config.impact_sparks.enabled =>
                {
                    // Use the contact point directly instead of computing midpoint
                    let color = Color::rgb(255, 200, 100); // Golden sparks for bumpers
                    let cfg = &self.config.impact_sparks;
                    pending_sparks.push(PendingSparks {
                        pos: *contact_point,
                        color,
                        params: SparkParams {
                            count: cfg.count,
                            lifetime_secs: cfg.lifetime_secs,
                            size_px: cfg.size_px,
                            speed: cfg.speed,
                            angle_min: 0.0,
                            angle_max: 2.0 * PI,
                            colors: None,
                        },
                    });
                }

                _ => {}
            }
        }

        for event in race_events {
            match event {
                RaceEvent::RacerEliminated { body_id, .. }
                    if self.config.elimination_burst.enabled =>
                {
                    if let Some(&(pos, color, _)) = self.last_known_positions.get(body_id) {
                        let cfg = &self.config.elimination_burst;
                        pending_sparks.push(PendingSparks {
                            pos,
                            color,
                            params: SparkParams {
                                count: cfg.count,
                                lifetime_secs: cfg.lifetime_secs,
                                size_px: cfg.size_px,
                                speed: cfg.speed,
                                angle_min: 0.0,
                                angle_max: 2.0 * PI,
                                colors: None,
                            },
                        });
                    }
                }

                RaceEvent::RaceComplete { winner }
                    if self.config.winner_pop.enabled && !self.winner_pop_fired =>
                {
                    winner_event = Some(winner.clone());
                }

                _ => {}
            }
        }

        // ── Phase 2: execute pending work ──────────────────────────────────
        for ps in pending_sparks {
            self.emit_sparks(ps.pos, ps.color, &ps.params);
        }
        for pf in pending_flashes {
            self.upsert_boost_flash(pf.body, pf.center_world, pf.body_radius_world);
        }
        if let Some(winner) = winner_event {
            self.winner_pop_fired = true;
            let pop_pos = self.finish_line_world.unwrap_or_else(|| {
                self.last_known_positions
                    .get(&winner.body_id)
                    .map(|&(p, _, _)| p)
                    .unwrap_or(Vec2::new(0.0, 0.0))
            });
            self.emit_winner_pop(pop_pos, &winner);
        }
    }

    /// Advance all particles and flashes by `dt` seconds, removing expired ones.
    pub fn update(&mut self, dt: f32) {
        for p in &mut self.particles {
            p.update(dt);
        }
        self.particles.retain(|p| !p.is_dead());

        for flash in self.flashes.values_mut() {
            flash.update(dt);
        }
        self.flashes.retain(|_, f| !f.is_expired());
    }

    /// Composite all active VFX into `frame`.
    ///
    /// `ctx` provides the current camera transform used to project world-space
    /// particle and flash positions into pixel space before drawing.
    pub fn render_into(&self, frame: &mut Frame, ctx: &RenderContext) {
        let w = frame.width;
        let h = frame.height;
        let pixels = &mut frame.pixels;

        // Boost flashes drawn first (behind particles).
        for flash in self.flashes.values() {
            let (cx, cy) = world_to_pixel(flash.center_world, ctx);
            let alpha = flash.alpha_factor() * 0.85;
            draw_glow(
                pixels,
                w,
                h,
                cx,
                cy,
                flash.radius_ext_px,
                flash.color,
                alpha,
            );
        }

        // Kinematic particles drawn on top.
        for p in &self.particles {
            let alpha = p.alpha_factor() * 0.9;
            if alpha <= 0.001 {
                continue;
            }
            let (px, py) = world_to_pixel(p.pos, ctx);
            draw_dot(pixels, w, h, px, py, p.size_px, p.color, alpha);
        }
    }

    // ── Query accessors ────────────────────────────────────────────────────────

    /// Number of currently active particles.
    pub fn particle_count(&self) -> usize {
        self.particles.len()
    }

    /// Number of currently active boost flashes.
    pub fn flash_count(&self) -> usize {
        self.flashes.len()
    }

    // ── Internal helpers ───────────────────────────────────────────────────────

    /// Convert a config speed value (px/s) to world units/s using the cached scale.
    ///
    /// Config `speed` fields retain their legacy px/s semantic so that existing
    /// YAML scene files continue to work without changes.  This conversion
    /// happens once per emit call.
    #[inline]
    fn px_speed_to_world(&self, speed_px_per_s: f32) -> f32 {
        if self.current_scale > 0.0 {
            speed_px_per_s / self.current_scale
        } else {
            speed_px_per_s // fallback: no conversion if scale is zero
        }
    }

    fn emit_sparks(&mut self, pos: Vec2, default_color: Color, params: &SparkParams) {
        let available = self
            .config
            .max_particles
            .saturating_sub(self.particles.len());
        let count = params.count.min(available);

        for i in 0..count {
            let angle = if (params.angle_max - params.angle_min).abs() < 1e-5 {
                params.angle_min
            } else {
                self.rng.next_range(params.angle_min, params.angle_max)
            };
            let speed_px = self.rng.next_range(params.speed * 0.5, params.speed * 1.5);
            // Config speed is in px/s; convert to world units/s (m/s) for storage.
            let world_speed = self.px_speed_to_world(speed_px);

            let color = match &params.colors {
                Some(palette) if !palette.is_empty() => palette[i % palette.len()],
                _ => default_color,
            };

            self.particles.push(KinematicParticle {
                pos,
                // World-space Y-up: positive sin = upward — no negation needed.
                vel: Vec2::new(angle.cos() * world_speed, angle.sin() * world_speed),
                lifetime_rem: params.lifetime_secs,
                lifetime_total: params.lifetime_secs,
                size_px: params.size_px,
                color,
                kind: ParticleKind::Dot,
            });
        }
    }

    fn upsert_boost_flash(&mut self, body: BodyId, center_world: Vec2, body_radius_world: f32) {
        let cfg = &self.config.boost_flash;
        // Convert world-space radius to pixel-space for the glow halo size.
        let radius_ext_px = body_radius_world * self.current_scale + cfg.radius_px;
        let flash = ActiveFlash {
            body_id: body,
            center_world,
            color: cfg.color,
            radius_ext_px,
            time_rem: cfg.duration_secs,
            duration: cfg.duration_secs,
        };
        self.flashes.insert(body, flash);
    }

    fn emit_winner_pop(&mut self, pos: Vec2, winner: &WinnerInfo) {
        let cfg = self.config.winner_pop.clone();
        let spread_rad = cfg.spread_deg.to_radians();
        let angle_center = PI / 2.0;
        let angle_min = angle_center - spread_rad / 2.0;
        let angle_max = angle_center + spread_rad / 2.0;

        let palette: Vec<Color> = if cfg.colors.is_empty() {
            vec![
                Color::rgb(0xFF, 0xD7, 0x00), // gold
                Color::WHITE,
                winner.color,
            ]
        } else {
            cfg.colors.clone()
        };

        let available = self
            .config
            .max_particles
            .saturating_sub(self.particles.len());
        let count = cfg.count.min(available);

        for i in 0..count {
            let angle = self.rng.next_range(angle_min, angle_max);
            let speed_px = self.rng.next_range(cfg.speed * 0.5, cfg.speed * 1.5);
            // Config speed is in px/s; convert to world units/s (m/s).
            let world_speed = self.px_speed_to_world(speed_px);
            let color = palette[i % palette.len()];

            // World-space is Y-up: positive sin at angle_center=PI/2 → (0, +Y) = upward. ✓
            self.particles.push(KinematicParticle {
                pos,
                vel: Vec2::new(angle.cos() * world_speed, angle.sin() * world_speed),
                lifetime_rem: cfg.lifetime_secs,
                lifetime_total: cfg.lifetime_secs,
                size_px: cfg.size_px,
                color,
                kind: ParticleKind::Dot,
            });
        }
    }
}

// ── Private free functions ────────────────────────────────────────────────────

/// Convert a world-space position to pixel-space using the camera context.
///
/// Equivalent to the private helper in `rphys-renderer`.  Duplicated here to
/// avoid adding a public export from that crate for a three-line utility.
///
/// Formula:
/// - `px = (world.x − camera_origin.x) × scale`
/// - `py = height − (world.y − camera_origin.y) × scale`  (Y-axis flip: world
///   is Y-up, pixels are Y-down from top-left)
#[inline]
fn world_to_pixel(world: Vec2, ctx: &RenderContext) -> (f32, f32) {
    let px = (world.x - ctx.camera_origin.x) * ctx.scale;
    let py = ctx.height as f32 - (world.y - ctx.camera_origin.y) * ctx.scale;
    (px, py)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_physics::types::{BodyId, CollisionInfo, PhysicsEvent};
    use rphys_race::RaceEvent;
    use rphys_renderer::RenderContext;
    use rphys_scene::{
        BoostFlashConfig, Color, EliminationBurstConfig, ImpactSparksConfig, Vec2, VfxConfig,
        WinnerPopConfig,
    };

    /// A test [`RenderContext`] with scale=50 (50 px/m), 1080×1920 frame.
    fn test_ctx() -> RenderContext {
        RenderContext {
            width: 1080,
            height: 1920,
            camera_origin: Vec2::ZERO,
            scale: 50.0,
            background_color: Color::BLACK,
        }
    }

    fn enabled_impact_config() -> VfxConfig {
        VfxConfig {
            max_particles: 500,
            impact_sparks: ImpactSparksConfig {
                enabled: true,
                count: 8,
                lifetime_secs: 0.5,
                size_px: 2.0,
                speed: 100.0,
            },
            ..VfxConfig::default()
        }
    }

    fn enabled_winner_pop_config() -> VfxConfig {
        VfxConfig {
            max_particles: 500,
            winner_pop: WinnerPopConfig {
                enabled: true,
                count: 20,
                lifetime_secs: 1.0,
                size_px: 3.0,
                speed: 200.0,
                spread_deg: 180.0,
                colors: vec![],
            },
            ..VfxConfig::default()
        }
    }

    fn make_collision(a: u32, b: u32) -> PhysicsEvent {
        PhysicsEvent::Collision(CollisionInfo {
            body_a: BodyId(a),
            body_b: BodyId(b),
            impulse: 1.0,
        })
    }

    fn body_snap(id: u32, x: f32, y: f32) -> (BodyId, Vec2, Color, f32) {
        (BodyId(id), Vec2::new(x, y), Color::rgb(255, 0, 0), 5.0)
    }

    fn noop_lookup(_id: BodyId) -> Option<(Vec2, Color, f32)> {
        None
    }

    #[test]
    fn test_engine_emits_sparks_on_collision() {
        let mut engine = VfxEngine::new(enabled_impact_config());
        let snap = vec![body_snap(0, 2.0, 4.0), body_snap(1, 2.2, 4.0)];
        engine.begin_frame(&snap, Vec2::new(10.8, 36.0), 50.0);
        let events = vec![make_collision(0, 1)];
        engine.feed_events(&events, &[], &noop_lookup);
        assert_eq!(
            engine.particle_count(),
            8,
            "should emit config.count particles"
        );
    }

    #[test]
    fn test_engine_sparks_cleared_after_lifetime() {
        let mut engine = VfxEngine::new(enabled_impact_config());
        let snap = vec![body_snap(0, 1.0, 1.0), body_snap(1, 1.2, 1.0)];
        engine.begin_frame(&snap, Vec2::new(10.8, 18.0), 50.0);
        engine.feed_events(&[make_collision(0, 1)], &[], &noop_lookup);
        assert!(engine.particle_count() > 0);
        engine.update(1.0); // 1.0 s >> 0.5 s lifetime
        assert_eq!(engine.particle_count(), 0, "particles should be reaped");
    }

    #[test]
    fn test_engine_collision_dedup_per_frame() {
        let mut engine = VfxEngine::new(enabled_impact_config());
        let snap = vec![body_snap(0, 1.0, 1.0), body_snap(1, 1.2, 1.0)];
        engine.begin_frame(&snap, Vec2::new(10.8, 18.0), 50.0);
        let events = vec![make_collision(0, 1), make_collision(0, 1)];
        engine.feed_events(&events, &[], &noop_lookup);
        assert_eq!(
            engine.particle_count(),
            8,
            "duplicate pair should be deduped"
        );
    }

    #[test]
    fn test_winner_pop_fires_once_only() {
        use rphys_race::WinnerInfo;
        let mut engine = VfxEngine::new(enabled_winner_pop_config());
        let snap = vec![body_snap(0, 5.0, 10.0)];
        engine.begin_frame(&snap, Vec2::new(10.8, 18.0), 50.0);

        let winner = WinnerInfo {
            body_id: BodyId(0),
            display_name: "Red".to_string(),
            color: Color::rgb(255, 0, 0),
            finish_time_secs: 1.0,
        };
        engine.feed_events(
            &[],
            &[RaceEvent::RaceComplete {
                winner: winner.clone(),
            }],
            &noop_lookup,
        );
        let count_after_first = engine.particle_count();

        // Second emission attempt must be ignored.
        engine.begin_frame(&snap, Vec2::new(10.8, 18.0), 50.0);
        engine.feed_events(&[], &[RaceEvent::RaceComplete { winner }], &noop_lookup);
        assert_eq!(
            engine.particle_count(),
            count_after_first,
            "winner pop must fire at most once"
        );
    }

    #[test]
    fn test_boost_flash_created_on_boost_event() {
        let cfg = VfxConfig {
            boost_flash: BoostFlashConfig {
                enabled: true,
                color: Color::rgb(255, 255, 255),
                radius_px: 8.0,
                duration_secs: 0.3,
            },
            ..VfxConfig::default()
        };
        let mut engine = VfxEngine::new(cfg);
        let snap = vec![body_snap(3, 2.0, 4.0)];
        engine.begin_frame(&snap, Vec2::new(10.8, 0.0), 50.0);
        let events = vec![PhysicsEvent::BoostActivated { body: BodyId(3) }];
        engine.feed_events(&events, &[], &noop_lookup);
        assert_eq!(engine.flash_count(), 1, "boost flash should be created");
    }

    #[test]
    fn test_boost_flash_expires_after_duration() {
        let cfg = VfxConfig {
            boost_flash: BoostFlashConfig {
                enabled: true,
                color: Color::WHITE,
                radius_px: 8.0,
                duration_secs: 0.2,
            },
            ..VfxConfig::default()
        };
        let mut engine = VfxEngine::new(cfg);
        let snap = vec![body_snap(5, 1.0, 1.0)];
        engine.begin_frame(&snap, Vec2::new(0.0, 0.0), 50.0);
        engine.feed_events(
            &[PhysicsEvent::BoostActivated { body: BodyId(5) }],
            &[],
            &noop_lookup,
        );
        assert_eq!(engine.flash_count(), 1);
        engine.update(0.5);
        assert_eq!(engine.flash_count(), 0, "flash should expire");
    }

    #[test]
    fn test_elimination_burst_emits_particles() {
        let cfg = VfxConfig {
            elimination_burst: EliminationBurstConfig {
                enabled: true,
                count: 15,
                lifetime_secs: 0.6,
                size_px: 3.0,
                speed: 200.0,
            },
            ..VfxConfig::default()
        };
        let mut engine = VfxEngine::new(cfg);
        let snap = vec![body_snap(7, 4.0, 6.0)];
        engine.begin_frame(&snap, Vec2::new(0.0, 0.0), 50.0);
        engine.feed_events(
            &[],
            &[RaceEvent::RacerEliminated {
                body_id: BodyId(7),
                display_name: "Green".to_string(),
                rank_at_elimination: 5,
                elimination_number: 1,
            }],
            &noop_lookup,
        );
        assert_eq!(engine.particle_count(), 15);
    }

    #[test]
    fn test_max_particles_cap() {
        let cfg = VfxConfig {
            max_particles: 5,
            impact_sparks: ImpactSparksConfig {
                enabled: true,
                count: 100,
                lifetime_secs: 0.5,
                size_px: 2.0,
                speed: 100.0,
            },
            ..VfxConfig::default()
        };
        let mut engine = VfxEngine::new(cfg);
        let snap = vec![body_snap(0, 1.0, 1.0), body_snap(1, 1.2, 1.0)];
        engine.begin_frame(&snap, Vec2::new(0.0, 0.0), 50.0);
        engine.feed_events(&[make_collision(0, 1)], &[], &noop_lookup);
        assert!(
            engine.particle_count() <= 5,
            "particle count {} must not exceed max_particles 5",
            engine.particle_count()
        );
    }

    #[test]
    fn test_render_into_does_not_panic() {
        let cfg = VfxConfig {
            impact_sparks: ImpactSparksConfig {
                enabled: true,
                count: 5,
                lifetime_secs: 0.5,
                size_px: 2.0,
                speed: 100.0,
            },
            ..VfxConfig::default()
        };
        let mut engine = VfxEngine::new(cfg);
        let snap = vec![body_snap(0, 1.0, 1.0), body_snap(1, 1.2, 1.0)];
        engine.begin_frame(&snap, Vec2::new(10.8, 19.2), 50.0);
        engine.feed_events(&[make_collision(0, 1)], &[], &noop_lookup);
        engine.update(0.01);

        let mut frame = Frame::new(64, 64);
        engine.render_into(&mut frame, &test_ctx());
    }

    /// Verify that winner pop particles are anchored in world space:
    /// rendering with two cameras that differ only in `camera_origin` should
    /// produce pixel positions that shift by exactly
    /// `Δcamera_origin * scale` pixels.
    #[test]
    fn test_winner_pop_stays_world_anchored() {
        use rphys_race::WinnerInfo;

        let mut engine = VfxEngine::new(enabled_winner_pop_config());

        // Emit at world position (5.0, 10.0) with scale=50.
        let world_finish = Vec2::new(5.0, 10.0);
        engine.begin_frame(&[], world_finish, 50.0);

        let winner = WinnerInfo {
            body_id: BodyId(0),
            display_name: "Test".to_string(),
            color: Color::rgb(255, 0, 0),
            finish_time_secs: 1.0,
        };
        engine.feed_events(&[], &[RaceEvent::RaceComplete { winner }], &noop_lookup);
        assert!(engine.particle_count() > 0, "particles should be emitted");

        // Camera A: origin=(0,0), scale=50, 1080×1920.
        let ctx_a = RenderContext {
            width: 1080,
            height: 1920,
            camera_origin: Vec2::ZERO,
            scale: 50.0,
            background_color: Color::BLACK,
        };
        // Camera B: origin=(0,5), scale=50 — camera panned down 5 m in world.
        // A particle at world.y=10 should render 5*50=250 px lower in frame_b.
        let ctx_b = RenderContext {
            width: 1080,
            height: 1920,
            camera_origin: Vec2::new(0.0, 5.0),
            scale: 50.0,
            background_color: Color::BLACK,
        };

        // Directly compute pixel positions for the first particle under each camera.
        let first = &engine.particles[0];
        let (_, py_a) = world_to_pixel(first.pos, &ctx_a);
        let (_, py_b) = world_to_pixel(first.pos, &ctx_b);

        // Camera B has camera_origin.y=5 (camera panned down 5 m in world).
        // Formula: py = height - (world.y - camera_origin.y) * scale
        //   py_a = 1920 - (world.y - 0) * 50
        //   py_b = 1920 - (world.y - 5) * 50 = py_a + 250
        // So py_b > py_a by 250: the particle appears 250 px lower in camera B,
        // which is correct — the camera moved down, so world objects scroll down.
        let diff = py_b - py_a;
        assert!(
            (diff - 250.0).abs() < 1e-3,
            "pixel Y should shift by 5m×50px/m=250px when camera pans 5m down; got diff={diff:.3}"
        );
    }

    #[test]
    fn test_particle_alpha_factor() {
        use crate::particle::KinematicParticle;
        use crate::particle::ParticleKind;
        let p = KinematicParticle {
            pos: Vec2::new(0.0, 0.0),
            vel: Vec2::new(0.0, 0.0),
            lifetime_rem: 0.25,
            lifetime_total: 0.5,
            size_px: 2.0,
            color: Color::WHITE,
            kind: ParticleKind::Dot,
        };
        let alpha = p.alpha_factor();
        assert!(
            (alpha - 0.5).abs() < 1e-5,
            "half-life should give alpha 0.5, got {alpha}"
        );
    }
}
