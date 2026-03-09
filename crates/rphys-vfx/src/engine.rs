//! [`VfxEngine`] — the central coordinator for all VFX effects.
//!
//! # Usage pattern (one rendered frame)
//!
//! ```rust,ignore
//! // 1. Snapshot current body positions.
//! vfx.begin_frame(&body_snapshot, finish_line_px);
//!
//! // 2. Feed physics + race events (spawns particles / flashes).
//! vfx.feed_events(&physics_events, &race_events, &lookup);
//!
//! // 3. Advance particle lifetimes.
//! vfx.update(dt);
//!
//! // 4. Composite all VFX into the rendered frame.
//! vfx.render_into(&mut frame);
//! ```

use std::collections::{HashMap, HashSet};
use std::f32::consts::PI;

use rphys_physics::types::{BodyId, PhysicsEvent};
use rphys_race::{RaceEvent, WinnerInfo};
use rphys_renderer::Frame;
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
    center_px: Vec2,
    body_radius_px: f32,
}

// ── VfxEngine ─────────────────────────────────────────────────────────────────

/// VFX engine: manages particles and boost flashes for a single race export.
///
/// All coordinates stored internally are **pixel-space** (top-left origin,
/// Y-down), computed once per frame when the render context is available.
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
    /// Last known pixel position + color + radius for each tracked body.
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

    // ── Finish-line pixel position ─────────────────────────────────────────
    /// Pixel-space position of the finish line (updated each frame).
    finish_line_px: Option<Vec2>,

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
            finish_line_px: None,
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
    /// - `body_snapshot` — slice of `(BodyId, pixel_pos, color, radius_px)`
    ///   for every **alive** body this frame.
    /// - `finish_line_px` — pixel-space position of the finish line.
    pub fn begin_frame(
        &mut self,
        body_snapshot: &[(BodyId, Vec2, Color, f32)],
        finish_line_px: Vec2,
    ) {
        // Refresh position cache.
        self.last_known_positions.clear();
        for &(id, pos, color, radius) in body_snapshot {
            self.last_known_positions.insert(id, (pos, color, radius));
        }

        // Update boost-flash centres.
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
                flash.center_px = pos;
            }
        }

        self.finish_line_px = Some(finish_line_px);
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
                            center_px: pos,
                            body_radius_px: radius,
                        });
                    }
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
            self.upsert_boost_flash(pf.body, pf.center_px, pf.body_radius_px);
        }
        if let Some(winner) = winner_event {
            self.winner_pop_fired = true;
            let pop_pos = self.finish_line_px.unwrap_or_else(|| {
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
    pub fn render_into(&self, frame: &mut Frame) {
        let w = frame.width;
        let h = frame.height;
        let pixels = &mut frame.pixels;

        // Boost flashes drawn first (behind particles).
        for flash in self.flashes.values() {
            let alpha = flash.alpha_factor() * 0.85;
            draw_glow(
                pixels,
                w,
                h,
                flash.center_px.x,
                flash.center_px.y,
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
            draw_dot(pixels, w, h, p.pos.x, p.pos.y, p.size_px, p.color, alpha);
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
            let speed = self.rng.next_range(params.speed * 0.5, params.speed * 1.5);

            let color = match &params.colors {
                Some(palette) if !palette.is_empty() => palette[i % palette.len()],
                _ => default_color,
            };

            self.particles.push(KinematicParticle {
                pos,
                vel: Vec2::new(angle.cos() * speed, angle.sin() * speed),
                lifetime_rem: params.lifetime_secs,
                lifetime_total: params.lifetime_secs,
                size_px: params.size_px,
                color,
                kind: ParticleKind::Dot,
            });
        }
    }

    fn upsert_boost_flash(&mut self, body: BodyId, center_px: Vec2, body_radius_px: f32) {
        let cfg = &self.config.boost_flash;
        let flash = ActiveFlash {
            body_id: body,
            center_px,
            color: cfg.color,
            radius_ext_px: body_radius_px + cfg.radius_px,
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
            let speed = self.rng.next_range(cfg.speed * 0.5, cfg.speed * 1.5);
            let color = palette[i % palette.len()];

            // In pixel-space, "up" = negative Y.
            self.particles.push(KinematicParticle {
                pos,
                vel: Vec2::new(angle.cos() * speed, -angle.sin() * speed),
                lifetime_rem: cfg.lifetime_secs,
                lifetime_total: cfg.lifetime_secs,
                size_px: cfg.size_px,
                color,
                kind: ParticleKind::Dot,
            });
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_physics::types::{BodyId, CollisionInfo, PhysicsEvent};
    use rphys_race::RaceEvent;
    use rphys_scene::{
        BoostFlashConfig, Color, EliminationBurstConfig, ImpactSparksConfig, Vec2, VfxConfig,
        WinnerPopConfig,
    };

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
        let snap = vec![body_snap(0, 100.0, 200.0), body_snap(1, 110.0, 200.0)];
        engine.begin_frame(&snap, Vec2::new(540.0, 1800.0));
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
        let snap = vec![body_snap(0, 50.0, 50.0), body_snap(1, 60.0, 50.0)];
        engine.begin_frame(&snap, Vec2::new(540.0, 900.0));
        engine.feed_events(&[make_collision(0, 1)], &[], &noop_lookup);
        assert!(engine.particle_count() > 0);
        engine.update(1.0); // 1.0 s >> 0.5 s lifetime
        assert_eq!(engine.particle_count(), 0, "particles should be reaped");
    }

    #[test]
    fn test_engine_collision_dedup_per_frame() {
        let mut engine = VfxEngine::new(enabled_impact_config());
        let snap = vec![body_snap(0, 50.0, 50.0), body_snap(1, 60.0, 50.0)];
        engine.begin_frame(&snap, Vec2::new(540.0, 900.0));
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
        let snap = vec![body_snap(0, 540.0, 900.0)];
        engine.begin_frame(&snap, Vec2::new(540.0, 900.0));

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
        engine.begin_frame(&snap, Vec2::new(540.0, 900.0));
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
        let snap = vec![body_snap(3, 100.0, 200.0)];
        engine.begin_frame(&snap, Vec2::new(0.0, 0.0));
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
        let snap = vec![body_snap(5, 50.0, 50.0)];
        engine.begin_frame(&snap, Vec2::new(0.0, 0.0));
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
        let snap = vec![body_snap(7, 200.0, 300.0)];
        engine.begin_frame(&snap, Vec2::new(0.0, 0.0));
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
        let snap = vec![body_snap(0, 50.0, 50.0), body_snap(1, 60.0, 50.0)];
        engine.begin_frame(&snap, Vec2::new(0.0, 0.0));
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
        let snap = vec![body_snap(0, 50.0, 50.0), body_snap(1, 60.0, 50.0)];
        engine.begin_frame(&snap, Vec2::new(540.0, 960.0));
        engine.feed_events(&[make_collision(0, 1)], &[], &noop_lookup);
        engine.update(0.01);

        let mut frame = Frame::new(64, 64);
        engine.render_into(&mut frame);
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
