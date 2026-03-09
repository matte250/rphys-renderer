//! `PhysicsEngine` — the core simulation driver.
//!
//! Wraps a rapier2d world with a stable `BodyId` handle layer, translates
//! scene objects into rigid bodies + colliders, and emits `PhysicsEvent`s.
//!
//! ## rapier2d 0.32 API notes (glamx-based)
//!
//! - `Vec2::new(x, y)` — use instead of the old `vector![x, y]` macro.
//! - `gravity` is passed **by value** (not by reference) to `pipeline.step`.
//! - `ContactManifold::contacts()` returns an iterator over contact points.
//! - `ContactData::impulse` is the normal impulse magnitude (`f32`).
//! - `SharedShape::convex_hull(&[Vec2])` — takes glamx `Vec2` slices.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use rapier2d::prelude::*;
use rphys_scene::{BodyType, BoostConfig, Color, Destructible, EndCondition, Scene, ShapeKind};

use crate::types::{
    BodyId, BodyInfo, BodyState, CollisionInfo, CompletionReason, PhysicsConfig, PhysicsError,
    PhysicsEvent, PhysicsState,
};

// ── Internal per-body metadata ────────────────────────────────────────────────

/// Full per-body data kept by the engine (superset of the public `BodyInfo`).
///
/// Note: `audio` lives in `body_info_map` (public `BodyInfo`) rather than here
/// to avoid duplication; `StoredBody` only holds engine-internal fields.
struct StoredBody {
    name: Option<String>,
    tags: Vec<String>,
    shape: ShapeKind,
    color: Color,
    destructible: Option<Destructible>,
    body_type: rphys_scene::BodyType,
    /// Speed-boost configuration, if this body is a boost pad.
    boost: Option<BoostConfig>,
}

// ── PhysicsEngine ─────────────────────────────────────────────────────────────

/// The physics simulation engine.
///
/// Build from a `Scene` via [`PhysicsEngine::new`], then drive by calling
/// [`step`](PhysicsEngine::step) or [`advance_to`](PhysicsEngine::advance_to).
pub struct PhysicsEngine {
    // ── rapier world ──────────────────────────────────────────────────────────
    gravity: Vec2,
    integration_params: IntegrationParameters,
    physics_pipeline: PhysicsPipeline,
    island_manager: IslandManager,
    broad_phase: DefaultBroadPhase,
    narrow_phase: NarrowPhase,
    rigid_body_set: RigidBodySet,
    collider_set: ColliderSet,
    impulse_joint_set: ImpulseJointSet,
    multibody_joint_set: MultibodyJointSet,
    ccd_solver: CCDSolver,

    // ── ID mapping ────────────────────────────────────────────────────────────
    /// rapier handle → stable `BodyId`.
    handle_to_id: HashMap<RigidBodyHandle, BodyId>,
    /// stable `BodyId` → rapier handle (entry removed when body is destroyed).
    id_to_handle: HashMap<BodyId, RigidBodyHandle>,
    /// collider handle → stable `BodyId` (for event lookup).
    collider_to_id: HashMap<ColliderHandle, BodyId>,
    /// stable `BodyId` → list of collider handles (for cleanup on destroy).
    id_to_colliders: HashMap<BodyId, Vec<ColliderHandle>>,
    /// collider handles belonging to world boundary walls.
    wall_colliders: HashSet<ColliderHandle>,

    // ── per-body data ─────────────────────────────────────────────────────────
    /// Internal metadata for all bodies ever created (survives destruction).
    body_data: HashMap<BodyId, StoredBody>,
    /// Public `BodyInfo` map — kept in sync with `body_data`.
    body_info_map: HashMap<BodyId, BodyInfo>,
    /// Running counter for assigning `BodyId`s.
    next_id: u32,

    // ── simulation state ──────────────────────────────────────────────────────
    elapsed: f32,
    complete: bool,
    config: PhysicsConfig,
    world_bounds: rphys_scene::WorldBounds,
    wall_config: rphys_scene::WallConfig,
    end_condition: Option<EndCondition>,

    // ── end-condition tracking ────────────────────────────────────────────────
    /// Body-body collision pairs that have occurred (normalised to ID order).
    collisions_occurred: HashSet<(BodyId, BodyId)>,
}

impl PhysicsEngine {
    /// Build the physics world from a parsed scene.
    pub fn new(scene: &Scene, config: PhysicsConfig) -> Result<Self, PhysicsError> {
        let mut engine = Self::empty(scene, config);
        engine.build_walls(scene)?;
        engine.build_bodies(scene)?;
        Ok(engine)
    }

    // ── World construction ────────────────────────────────────────────────────

    /// Create an empty engine skeleton (all rapier structures default-initialised).
    fn empty(scene: &Scene, config: PhysicsConfig) -> Self {
        let gravity = Vec2::new(scene.environment.gravity.x, scene.environment.gravity.y);

        let integration_params = IntegrationParameters {
            dt: config.timestep,
            ..IntegrationParameters::default()
        };

        Self {
            gravity,
            integration_params,
            physics_pipeline: PhysicsPipeline::new(),
            island_manager: IslandManager::new(),
            broad_phase: DefaultBroadPhase::new(),
            narrow_phase: NarrowPhase::new(),
            rigid_body_set: RigidBodySet::new(),
            collider_set: ColliderSet::new(),
            impulse_joint_set: ImpulseJointSet::new(),
            multibody_joint_set: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),

            handle_to_id: HashMap::new(),
            id_to_handle: HashMap::new(),
            collider_to_id: HashMap::new(),
            id_to_colliders: HashMap::new(),
            wall_colliders: HashSet::new(),

            body_data: HashMap::new(),
            body_info_map: HashMap::new(),
            next_id: 0,

            elapsed: 0.0,
            complete: false,
            config,
            world_bounds: scene.environment.world_bounds.clone(),
            wall_config: scene.environment.walls.clone(),
            end_condition: scene.end_condition.clone(),

            collisions_occurred: HashSet::new(),
        }
    }

    /// Create the four static boundary wall colliders.
    fn build_walls(&mut self, scene: &Scene) -> Result<(), PhysicsError> {
        let wb = &scene.environment.world_bounds;
        let t = scene.environment.walls.thickness.max(0.1_f32);

        // (centre_x, centre_y, half_extent_x, half_extent_y)
        let specs: [(f32, f32, f32, f32); 4] = [
            // Bottom wall — top face at y = 0
            (wb.width / 2.0, -(t / 2.0), wb.width / 2.0 + t, t / 2.0),
            // Top wall — bottom face at y = height
            (
                wb.width / 2.0,
                wb.height + t / 2.0,
                wb.width / 2.0 + t,
                t / 2.0,
            ),
            // Left wall — right face at x = 0
            (-(t / 2.0), wb.height / 2.0, t / 2.0, wb.height / 2.0 + t),
            // Right wall — left face at x = width
            (
                wb.width + t / 2.0,
                wb.height / 2.0,
                t / 2.0,
                wb.height / 2.0 + t,
            ),
        ];

        for (cx, cy, hx, hy) in specs {
            let rb = RigidBodyBuilder::fixed()
                .translation(Vec2::new(cx, cy))
                .build();
            let rb_h = self.rigid_body_set.insert(rb);

            let coll = ColliderBuilder::cuboid(hx, hy)
                .active_events(ActiveEvents::COLLISION_EVENTS)
                .build();
            let coll_h = self
                .collider_set
                .insert_with_parent(coll, rb_h, &mut self.rigid_body_set);
            self.wall_colliders.insert(coll_h);
        }

        Ok(())
    }

    /// Create rapier bodies and colliders for every `SceneObject`.
    fn build_bodies(&mut self, scene: &Scene) -> Result<(), PhysicsError> {
        for obj in &scene.objects {
            self.insert_scene_object(obj)?;
        }
        Ok(())
    }

    /// Insert one scene object, recording all ID mappings.
    fn insert_scene_object(
        &mut self,
        obj: &rphys_scene::SceneObject,
    ) -> Result<BodyId, PhysicsError> {
        // ── rigid body ────────────────────────────────────────────────────────
        //
        // A Static body that specifies `angular_velocity` is promoted to
        // `KinematicVelocityBased` so that rapier actually integrates its
        // angular velocity each step.  A Fixed body ignores `angvel`; only
        // kinematic (velocity-based) bodies respect it.
        let effective_body_type =
            if obj.body_type == BodyType::Static && obj.angular_velocity.is_some() {
                BodyType::Kinematic
            } else {
                obj.body_type.clone()
            };

        let rb_builder = match effective_body_type {
            BodyType::Dynamic => RigidBodyBuilder::dynamic(),
            BodyType::Static => RigidBodyBuilder::fixed(),
            BodyType::Kinematic => RigidBodyBuilder::kinematic_velocity_based(),
        };

        let rb = rb_builder
            .translation(Vec2::new(obj.position.x, obj.position.y))
            .rotation(obj.rotation)
            .linvel(Vec2::new(obj.velocity.x, obj.velocity.y))
            .angvel(obj.angular_velocity.unwrap_or(0.0))
            .build();

        let rb_handle = self.rigid_body_set.insert(rb);

        // ── collider ──────────────────────────────────────────────────────────
        let shape = build_shape(&obj.shape).map_err(PhysicsError::BuildFailed)?;

        // Destructible objects: enable contact-force events so we detect when
        // the impulse threshold is exceeded.  The threshold stored in the scene
        // is in N·s; rapier's threshold is in N (force), so we divide by dt.
        let active_events = if obj.destructible.is_some() {
            ActiveEvents::COLLISION_EVENTS | ActiveEvents::CONTACT_FORCE_EVENTS
        } else {
            ActiveEvents::COLLISION_EVENTS
        };

        let force_threshold = obj
            .destructible
            .as_ref()
            .map(|d| d.min_impact_force / self.config.timestep)
            .unwrap_or(f32::MAX);

        let coll = ColliderBuilder::new(shape)
            .restitution(obj.material.restitution)
            .friction(obj.material.friction)
            .density(obj.material.density)
            .active_events(active_events)
            .contact_force_event_threshold(force_threshold)
            .build();

        let coll_handle =
            self.collider_set
                .insert_with_parent(coll, rb_handle, &mut self.rigid_body_set);

        // ── ID bookkeeping ────────────────────────────────────────────────────
        let id = BodyId(self.next_id);
        self.next_id += 1;

        self.handle_to_id.insert(rb_handle, id);
        self.id_to_handle.insert(id, rb_handle);
        self.collider_to_id.insert(coll_handle, id);
        self.id_to_colliders.insert(id, vec![coll_handle]);

        self.body_data.insert(
            id,
            StoredBody {
                name: obj.name.clone(),
                tags: obj.tags.clone(),
                shape: obj.shape.clone(),
                color: obj.color,
                destructible: obj.destructible.clone(),
                body_type: effective_body_type,
                boost: obj.boost.clone(),
            },
        );
        self.body_info_map.insert(
            id,
            BodyInfo {
                name: obj.name.clone(),
                tags: obj.tags.clone(),
                audio: obj.audio.clone(),
            },
        );

        Ok(id)
    }

    // ── Simulation ────────────────────────────────────────────────────────────

    /// Advance physics by exactly one fixed timestep.
    ///
    /// Returns all events that occurred during this step.
    /// A no-op (returns empty vec) if the simulation is already complete.
    pub fn step(&mut self) -> Result<Vec<PhysicsEvent>, PhysicsError> {
        if self.complete {
            return Ok(Vec::new());
        }
        self.step_internal()
    }

    /// Advance until `target_time` is reached, stepping as many times as needed.
    ///
    /// Useful for export mode (advance N steps per video frame).
    /// Respects [`PhysicsConfig::max_steps_per_call`] as a safety cap.
    pub fn advance_to(&mut self, target_time: f32) -> Result<Vec<PhysicsEvent>, PhysicsError> {
        let mut all_events = Vec::new();
        let mut steps = 0u32;
        while self.elapsed < target_time && !self.complete {
            if steps >= self.config.max_steps_per_call {
                break;
            }
            all_events.extend(self.step_internal()?);
            steps += 1;
        }
        Ok(all_events)
    }

    /// Execute one fixed-timestep of the simulation.
    fn step_internal(&mut self) -> Result<Vec<PhysicsEvent>, PhysicsError> {
        let mut output: Vec<PhysicsEvent> = Vec::new();

        // ── rapier step ───────────────────────────────────────────────────────
        let (coll_send, coll_recv) = mpsc::channel::<CollisionEvent>();
        let (force_send, force_recv) = mpsc::channel::<ContactForceEvent>();
        let event_handler = ChannelEventCollector::new(coll_send, force_send);

        // Note: gravity is passed by *value* in rapier2d 0.32.
        self.physics_pipeline.step(
            self.gravity,
            &self.integration_params,
            &mut self.island_manager,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.rigid_body_set,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            &mut self.ccd_solver,
            &(), // no query pipeline
            &event_handler,
        );

        // ── process collision events ──────────────────────────────────────────
        let mut bodies_to_destroy: HashSet<BodyId> = HashSet::new();

        while let Ok(event) = coll_recv.try_recv() {
            if let CollisionEvent::Started(h1, h2, _flags) = event {
                let is_wall1 = self.wall_colliders.contains(&h1);
                let is_wall2 = self.wall_colliders.contains(&h2);

                let impulse = self.sum_contact_impulse(h1, h2);

                if is_wall1 || is_wall2 {
                    let body_coll = if is_wall1 { h2 } else { h1 };
                    if let Some(&body_id) = self.collider_to_id.get(&body_coll) {
                        output.push(PhysicsEvent::WallBounce {
                            body: body_id,
                            impulse,
                        });
                    }
                } else {
                    let id1 = self.collider_to_id.get(&h1).copied();
                    let id2 = self.collider_to_id.get(&h2).copied();
                    if let (Some(id_a), Some(id_b)) = (id1, id2) {
                        self.collisions_occurred.insert(normalise_pair(id_a, id_b));
                        output.push(PhysicsEvent::Collision(CollisionInfo {
                            body_a: id_a,
                            body_b: id_b,
                            impulse,
                        }));
                    }
                }
            }
        }

        // ── process contact-force events (destructibles) ──────────────────────
        while let Ok(force_event) = force_recv.try_recv() {
            for &coll_handle in &[force_event.collider1, force_event.collider2] {
                if let Some(&body_id) = self.collider_to_id.get(&coll_handle) {
                    if let Some(stored) = self.body_data.get(&body_id) {
                        // The threshold was set to min_impact_force/dt, so
                        // receiving this event means the impulse was exceeded.
                        if stored.destructible.is_some() {
                            bodies_to_destroy.insert(body_id);
                        }
                    }
                }
            }
        }

        // ── destroy flagged bodies ────────────────────────────────────────────
        for body_id in &bodies_to_destroy {
            self.remove_body(*body_id);
            output.push(PhysicsEvent::Destroyed { body: *body_id });
        }

        // ── apply contact-based speed-boost impulses ──────────────────────────
        //
        // For every active contact pair, check whether one body carries a
        // `BoostConfig` and the other is a live dynamic body.  If so, apply
        // the configured impulse to the dynamic body.  This runs every step
        // the bodies remain in contact, giving continuous acceleration.
        output.extend(self.apply_boost_impulses());

        // ── advance time ──────────────────────────────────────────────────────
        self.elapsed += self.config.timestep;

        // ── evaluate end conditions ───────────────────────────────────────────
        if !self.complete {
            if let Some(reason) = self.evaluate_end_conditions() {
                self.complete = true;
                output.push(PhysicsEvent::SimulationComplete { reason });
            }
        }

        Ok(output)
    }

    /// Sum the solver-computed normal impulses for the contact pair (h1, h2).
    ///
    /// Uses `ContactManifold::contacts()` to iterate solver contact data.
    fn sum_contact_impulse(&self, h1: ColliderHandle, h2: ColliderHandle) -> f32 {
        self.narrow_phase
            .contact_pair(h1, h2)
            .map(|cp| {
                cp.manifolds
                    .iter()
                    .flat_map(|m| m.contacts())
                    .map(|c| c.data.impulse.abs())
                    .sum::<f32>()
            })
            .unwrap_or(0.0)
    }

    /// Scan all active contact pairs for boost configurations and apply impulses.
    ///
    /// For each contact where one body has a [`BoostConfig`] and the other is a
    /// live dynamic body, apply the configured impulse to the dynamic body and
    /// emit a [`PhysicsEvent::BoostActivated`] event.
    ///
    /// Two-phase design: collect `(target_id, impulse_vec)` pairs while
    /// borrowing the narrow phase immutably, then mutably update the rigid body
    /// set and emit events.
    fn apply_boost_impulses(&mut self) -> Vec<PhysicsEvent> {
        // Phase 1 — collect pending impulses (immutable borrow of narrow_phase /
        // body_data / id maps).
        let mut pending: Vec<(BodyId, Vec2)> = Vec::new();

        for contact_pair in self.narrow_phase.contact_pairs() {
            // Skip pairs with no solver-active contacts (separated but cached).
            if !contact_pair.has_any_active_contact() {
                continue;
            }

            let c1 = contact_pair.collider1;
            let c2 = contact_pair.collider2;

            // Both colliders must map to scene bodies (walls have no entry).
            let id_a = match self.collider_to_id.get(&c1).copied() {
                Some(id) => id,
                None => continue,
            };
            let id_b = match self.collider_to_id.get(&c2).copied() {
                Some(id) => id,
                None => continue,
            };

            // Check both orderings: (boost_body, dynamic_target).
            if let Some(pair) = self.resolve_boost(id_a, id_b) {
                pending.push(pair);
            } else if let Some(pair) = self.resolve_boost(id_b, id_a) {
                pending.push(pair);
            }
        }

        // Phase 2 — apply impulses and build events (mutable borrow of rigid_body_set).
        let mut events = Vec::with_capacity(pending.len());
        for (target_id, impulse_vec) in pending {
            if let Some(&rb_handle) = self.id_to_handle.get(&target_id) {
                if let Some(rb) = self.rigid_body_set.get_mut(rb_handle) {
                    rb.apply_impulse(impulse_vec, true);
                    events.push(PhysicsEvent::BoostActivated { body: target_id });
                }
            }
        }
        events
    }

    /// Check whether `boost_id` has a boost config applicable to `target_id`.
    ///
    /// Returns `Some((target_id, impulse_vector))` when:
    /// - `boost_id` exists and carries a [`BoostConfig`],
    /// - `target_id` exists, is still alive, and is [`BodyType::Dynamic`].
    ///
    /// Returns `None` otherwise.
    fn resolve_boost(&self, boost_id: BodyId, target_id: BodyId) -> Option<(BodyId, Vec2)> {
        let boost_cfg = self.body_data.get(&boost_id)?.boost.as_ref()?;
        let target_data = self.body_data.get(&target_id)?;

        if target_data.body_type != BodyType::Dynamic {
            return None;
        }
        // Body must not have been destroyed this step.
        if !self.id_to_handle.contains_key(&target_id) {
            return None;
        }

        let dir = &boost_cfg.direction;
        let impulse_vec = Vec2::new(dir.x * boost_cfg.impulse, dir.y * boost_cfg.impulse);
        Some((target_id, impulse_vec))
    }

    /// Remove a body from the simulation and clean up all index entries.
    ///
    /// `body_data` is intentionally preserved for snapshot/reporting history.
    fn remove_body(&mut self, body_id: BodyId) {
        let Some(&rb_handle) = self.id_to_handle.get(&body_id) else {
            return;
        };

        // Clean up collider ↔ body mappings before rapier removes them.
        if let Some(coll_handles) = self.id_to_colliders.remove(&body_id) {
            for ch in &coll_handles {
                self.collider_to_id.remove(ch);
            }
        }

        self.id_to_handle.remove(&body_id);
        self.handle_to_id.remove(&rb_handle);

        self.rigid_body_set.remove(
            rb_handle,
            &mut self.island_manager,
            &mut self.collider_set,
            &mut self.impulse_joint_set,
            &mut self.multibody_joint_set,
            true, // also remove attached colliders
        );
    }

    // ── End conditions ────────────────────────────────────────────────────────

    fn evaluate_end_conditions(&self) -> Option<CompletionReason> {
        let cond = self.end_condition.as_ref()?;
        self.eval_condition(cond)
    }

    fn eval_condition(&self, cond: &EndCondition) -> Option<CompletionReason> {
        match cond {
            EndCondition::TimeLimit { seconds } => {
                if self.elapsed >= *seconds {
                    Some(CompletionReason::TimeLimitReached)
                } else {
                    None
                }
            }

            EndCondition::AllTaggedDestroyed { tag } => {
                let any_exist = self
                    .body_data
                    .values()
                    .any(|d| d.tags.iter().any(|t| t == tag));
                let any_alive = self.body_data.iter().any(|(id, d)| {
                    d.tags.iter().any(|t| t == tag) && self.id_to_handle.contains_key(id)
                });
                if any_exist && !any_alive {
                    Some(CompletionReason::AllTaggedDestroyed { tag: tag.clone() })
                } else {
                    None
                }
            }

            EndCondition::ObjectEscaped { name } => {
                let escaped = self.body_data.iter().any(|(id, d)| {
                    d.name.as_deref() == Some(name.as_str())
                        && self.id_to_handle.contains_key(id)
                        && self.is_outside_bounds(*id)
                });
                if escaped {
                    Some(CompletionReason::ObjectEscaped { name: name.clone() })
                } else {
                    None
                }
            }

            EndCondition::ObjectsCollided { name_a, name_b } => {
                let id_a = self.find_body_by_name(name_a)?;
                let id_b = self.find_body_by_name(name_b)?;
                if self
                    .collisions_occurred
                    .contains(&normalise_pair(id_a, id_b))
                {
                    Some(CompletionReason::ObjectsCollided {
                        name_a: name_a.clone(),
                        name_b: name_b.clone(),
                    })
                } else {
                    None
                }
            }

            EndCondition::TagsCollided { tag_a, tag_b } => {
                let hit = self.collisions_occurred.iter().any(|(ia, ib)| {
                    let a = self.body_data.get(ia);
                    let b = self.body_data.get(ib);
                    if let (Some(da), Some(db)) = (a, b) {
                        let a_ta = da.tags.iter().any(|t| t == tag_a);
                        let b_tb = db.tags.iter().any(|t| t == tag_b);
                        let a_tb = da.tags.iter().any(|t| t == tag_b);
                        let b_ta = db.tags.iter().any(|t| t == tag_a);
                        (a_ta && b_tb) || (a_tb && b_ta)
                    } else {
                        false
                    }
                });
                if hit {
                    Some(CompletionReason::TagsCollided {
                        tag_a: tag_a.clone(),
                        tag_b: tag_b.clone(),
                    })
                } else {
                    None
                }
            }

            EndCondition::And { conditions } => {
                let mut last = None;
                for sub in conditions {
                    match self.eval_condition(sub) {
                        Some(r) => last = Some(r),
                        None => return None,
                    }
                }
                last
            }

            EndCondition::Or { conditions } => {
                for sub in conditions {
                    if let Some(r) = self.eval_condition(sub) {
                        return Some(r);
                    }
                }
                None
            }
            // FirstToReach is evaluated by rphys-race, not the physics engine.
            EndCondition::FirstToReach { .. } => None,
        }
    }

    fn is_outside_bounds(&self, body_id: BodyId) -> bool {
        let Some(&handle) = self.id_to_handle.get(&body_id) else {
            return false;
        };
        let Some(body) = self.rigid_body_set.get(handle) else {
            return false;
        };
        let t = body.translation();
        t.x < 0.0 || t.x > self.world_bounds.width || t.y < 0.0 || t.y > self.world_bounds.height
    }

    fn find_body_by_name(&self, name: &str) -> Option<BodyId> {
        self.body_data
            .iter()
            .find(|(_, d)| d.name.as_deref() == Some(name))
            .map(|(id, _)| *id)
    }

    // ── Public accessors ──────────────────────────────────────────────────────

    /// Snapshot the current world state for the renderer.
    ///
    /// Returns data for every body ever created; the renderer should skip
    /// entries where `is_alive` is `false`.
    pub fn state(&self) -> PhysicsState {
        let bodies = self
            .body_data
            .iter()
            .map(|(id, stored)| {
                let alive = self.id_to_handle.contains_key(id);

                let (position, rotation) = if alive {
                    self.id_to_handle
                        .get(id)
                        .and_then(|h| self.rigid_body_set.get(*h))
                        .map(|rb| {
                            let t = rb.translation();
                            (rphys_scene::Vec2::new(t.x, t.y), rb.rotation().angle())
                        })
                        .unwrap_or((rphys_scene::Vec2::ZERO, 0.0))
                } else {
                    (rphys_scene::Vec2::ZERO, 0.0)
                };

                BodyState {
                    id: *id,
                    name: stored.name.clone(),
                    tags: stored.tags.clone(),
                    position,
                    rotation,
                    shape: stored.shape.clone(),
                    color: stored.color,
                    is_alive: alive,
                    body_type: stored.body_type.clone(),
                }
            })
            .collect();

        PhysicsState {
            bodies,
            time: self.elapsed,
            world_bounds: self.world_bounds.clone(),
            wall_config: self.wall_config.clone(),
        }
    }

    /// Current physics time in seconds.
    pub fn time(&self) -> f32 {
        self.elapsed
    }

    /// `true` after a `SimulationComplete` event has been emitted.
    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Look up stable metadata for a body by its `BodyId`.
    ///
    /// Returns `None` if the ID was never assigned.  Data persists even after
    /// the body has been destroyed.
    pub fn body_info(&self, id: BodyId) -> Option<&BodyInfo> {
        self.body_info_map.get(&id)
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Convert a `ShapeKind` into a rapier `SharedShape`.
fn build_shape(shape: &ShapeKind) -> Result<SharedShape, String> {
    match shape {
        ShapeKind::Circle { radius } => Ok(SharedShape::ball(*radius)),
        ShapeKind::Rectangle { width, height } => {
            Ok(SharedShape::cuboid(width / 2.0, height / 2.0))
        }
        ShapeKind::Polygon { vertices } => {
            if vertices.len() < 3 {
                return Err(format!(
                    "Polygon must have at least 3 vertices, got {}",
                    vertices.len()
                ));
            }
            // rapier2d 0.32 (glamx) uses Vec2 for convex hull points.
            let pts: Vec<Vec2> = vertices.iter().map(|v| Vec2::new(v.x, v.y)).collect();
            SharedShape::convex_hull(&pts)
                .ok_or_else(|| "Polygon vertices are degenerate".to_string())
        }
    }
}

/// Return a normalised pair so `(a, b)` and `(b, a)` hash identically.
fn normalise_pair(a: BodyId, b: BodyId) -> (BodyId, BodyId) {
    if a.0 <= b.0 {
        (a, b)
    } else {
        (b, a)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_scene::{
        Color, Environment, Material, SceneAudio, SceneMeta, ShapeKind, Vec2 as SvVec2, WallConfig,
        WorldBounds,
    };

    // ── helpers ───────────────────────────────────────────────────────────────

    fn default_env() -> Environment {
        Environment {
            gravity: SvVec2::new(0.0, -9.81),
            background_color: Color::BLACK,
            world_bounds: WorldBounds {
                width: 20.0,
                height: 35.0,
            },
            walls: WallConfig {
                visible: true,
                color: Color::WHITE,
                thickness: 0.5,
            },
        }
    }

    fn default_meta() -> SceneMeta {
        SceneMeta {
            name: "test".to_string(),
            description: None,
            author: None,
            duration_hint: None,
        }
    }

    fn minimal_scene(objects: Vec<rphys_scene::SceneObject>) -> Scene {
        Scene {
            version: "1".to_string(),
            meta: default_meta(),
            environment: default_env(),
            objects,
            end_condition: None,
            audio: SceneAudio::default(),
            race: None,
        }
    }

    fn dynamic_ball(name: Option<&str>, pos: SvVec2, vel: SvVec2) -> rphys_scene::SceneObject {
        rphys_scene::SceneObject {
            name: name.map(str::to_string),
            shape: ShapeKind::Circle { radius: 0.5 },
            position: pos,
            velocity: vel,
            rotation: 0.0,
            angular_velocity: None,
            body_type: rphys_scene::BodyType::Dynamic,
            material: Material::default(),
            color: Color::rgb(255, 0, 0),
            tags: Vec::new(),
            destructible: None,
            boost: None,
            audio: rphys_scene::ObjectAudio::default(),
        }
    }

    fn export_config() -> PhysicsConfig {
        PhysicsConfig {
            timestep: 1.0 / 240.0,
            max_steps_per_call: u32::MAX,
        }
    }

    // ── test: build from minimal scene ────────────────────────────────────────

    #[test]
    fn test_build_from_scene() {
        let scene = minimal_scene(vec![dynamic_ball(
            Some("ball"),
            SvVec2::new(10.0, 5.0),
            SvVec2::ZERO,
        )]);
        assert!(PhysicsEngine::new(&scene, PhysicsConfig::default()).is_ok());
    }

    // ── test: gravity moves body ──────────────────────────────────────────────

    #[test]
    fn test_step_moves_body() {
        let initial_y = 20.0;
        let scene = minimal_scene(vec![dynamic_ball(
            Some("ball"),
            SvVec2::new(10.0, initial_y),
            SvVec2::ZERO,
        )]);
        let mut engine = PhysicsEngine::new(&scene, PhysicsConfig::default()).unwrap();

        for _ in 0..60 {
            engine.step().unwrap();
        }

        let state = engine.state();
        let ball = state
            .bodies
            .iter()
            .find(|b| b.name.as_deref() == Some("ball"))
            .unwrap();
        assert!(
            ball.position.y < initial_y,
            "ball should have fallen: was {initial_y}, now {}",
            ball.position.y
        );
        assert!(ball.is_alive);
    }

    // ── test: wall bounce event fires ─────────────────────────────────────────

    #[test]
    fn test_wall_bounce_event() {
        // Ball flying fast toward the right wall.
        let scene = minimal_scene(vec![dynamic_ball(
            Some("ball"),
            SvVec2::new(18.0, 17.5),
            SvVec2::new(50.0, 0.0),
        )]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();

        let events = engine.advance_to(2.0).unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PhysicsEvent::WallBounce { .. })),
            "expected a WallBounce event"
        );
    }

    // ── test: collision event fires ───────────────────────────────────────────

    #[test]
    fn test_collision_event_fires() {
        let scene = minimal_scene(vec![
            dynamic_ball(Some("a"), SvVec2::new(8.0, 17.5), SvVec2::new(10.0, 0.0)),
            dynamic_ball(Some("b"), SvVec2::new(12.0, 17.5), SvVec2::new(-10.0, 0.0)),
        ]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        let events = engine.advance_to(2.0).unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PhysicsEvent::Collision(_))),
            "expected at least one Collision event"
        );
    }

    // ── test: destructible body removed on impact ─────────────────────────────

    #[test]
    fn test_destructible_object_removed() {
        let mut ball = dynamic_ball(
            Some("frag"),
            SvVec2::new(10.0, 17.5),
            SvVec2::new(0.0, -200.0), // fast downward
        );
        ball.destructible = Some(rphys_scene::Destructible {
            min_impact_force: 0.001,
        });

        let scene = minimal_scene(vec![ball]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();

        let events = engine.advance_to(5.0).unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PhysicsEvent::Destroyed { .. })),
            "expected a Destroyed event"
        );

        let state = engine.state();
        let frag = state
            .bodies
            .iter()
            .find(|b| b.name.as_deref() == Some("frag"))
            .unwrap();
        assert!(
            !frag.is_alive,
            "destroyed body should not be alive in state"
        );
    }

    // ── test: TimeLimit end condition ─────────────────────────────────────────

    #[test]
    fn test_end_condition_time_limit() {
        let mut scene = minimal_scene(vec![]);
        scene.end_condition = Some(EndCondition::TimeLimit { seconds: 0.1 });

        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        let events = engine.advance_to(5.0).unwrap();

        assert!(
            events.iter().any(|e| matches!(
                e,
                PhysicsEvent::SimulationComplete {
                    reason: CompletionReason::TimeLimitReached
                }
            )),
            "expected SimulationComplete(TimeLimitReached)"
        );
        assert!(engine.is_complete());
    }

    // ── test: AllTaggedDestroyed end condition ────────────────────────────────

    #[test]
    fn test_end_condition_all_tagged_destroyed() {
        let mut ball = dynamic_ball(None, SvVec2::new(10.0, 17.5), SvVec2::new(0.0, -200.0));
        ball.tags = vec!["target".to_string()];
        ball.destructible = Some(rphys_scene::Destructible {
            min_impact_force: 0.001,
        });

        let mut scene = minimal_scene(vec![ball]);
        scene.end_condition = Some(EndCondition::AllTaggedDestroyed {
            tag: "target".to_string(),
        });

        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        let events = engine.advance_to(5.0).unwrap();

        assert!(
            events.iter().any(|e| matches!(
                e,
                PhysicsEvent::SimulationComplete {
                    reason: CompletionReason::AllTaggedDestroyed { .. }
                }
            )),
            "expected SimulationComplete(AllTaggedDestroyed)"
        );
    }

    // ── test: Or end condition ────────────────────────────────────────────────

    #[test]
    fn test_end_condition_or() {
        let mut scene = minimal_scene(vec![]);
        scene.end_condition = Some(EndCondition::Or {
            conditions: vec![
                EndCondition::TimeLimit { seconds: 0.05 },
                EndCondition::TimeLimit { seconds: 99.0 },
            ],
        });

        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        let events = engine.advance_to(1.0).unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PhysicsEvent::SimulationComplete { .. })),
            "Or condition should have triggered via first branch"
        );
    }

    // ── test: And end condition ───────────────────────────────────────────────

    #[test]
    fn test_end_condition_and() {
        let mut scene = minimal_scene(vec![]);
        // Both limits satisfied when elapsed >= 0.05.
        scene.end_condition = Some(EndCondition::And {
            conditions: vec![
                EndCondition::TimeLimit { seconds: 0.04 },
                EndCondition::TimeLimit { seconds: 0.05 },
            ],
        });

        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        let events = engine.advance_to(1.0).unwrap();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PhysicsEvent::SimulationComplete { .. })),
            "And condition should have triggered"
        );
    }

    // ── test: advance_to reaches target time ──────────────────────────────────

    #[test]
    fn test_advance_to_reaches_target_time() {
        let scene = minimal_scene(vec![]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        engine.advance_to(1.0).unwrap();
        let dt = 1.0 / 240.0;
        assert!(
            engine.time() >= 1.0 - dt,
            "expected time ~1.0, got {}",
            engine.time()
        );
    }

    // ── test: body_info lookup ────────────────────────────────────────────────

    #[test]
    fn test_body_info_lookup() {
        let scene = minimal_scene(vec![dynamic_ball(
            Some("myball"),
            SvVec2::new(10.0, 10.0),
            SvVec2::ZERO,
        )]);
        let engine = PhysicsEngine::new(&scene, PhysicsConfig::default()).unwrap();

        let state = engine.state();
        let body = state
            .bodies
            .iter()
            .find(|b| b.name.as_deref() == Some("myball"))
            .unwrap();
        let info = engine.body_info(body.id);
        assert!(info.is_some());
        assert_eq!(info.unwrap().name.as_deref(), Some("myball"));

        // Unknown ID → None.
        assert!(engine.body_info(BodyId(9999)).is_none());
    }

    // ── test: determinism ─────────────────────────────────────────────────────

    #[test]
    fn test_determinism() {
        let scene = minimal_scene(vec![
            dynamic_ball(Some("a"), SvVec2::new(5.0, 10.0), SvVec2::new(3.0, 0.0)),
            dynamic_ball(Some("b"), SvVec2::new(15.0, 10.0), SvVec2::new(-3.0, 0.0)),
        ]);

        let mut eng_a = PhysicsEngine::new(&scene, export_config()).unwrap();
        let mut eng_b = PhysicsEngine::new(&scene, export_config()).unwrap();

        for _ in 0..240 {
            eng_a.step().unwrap();
            eng_b.step().unwrap();
        }

        let sa = eng_a.state();
        let sb = eng_b.state();

        let mut bodies_a: Vec<_> = sa.bodies.iter().collect();
        let mut bodies_b: Vec<_> = sb.bodies.iter().collect();
        bodies_a.sort_by_key(|b| b.name.as_deref().unwrap_or(""));
        bodies_b.sort_by_key(|b| b.name.as_deref().unwrap_or(""));

        for (ba, bb) in bodies_a.iter().zip(bodies_b.iter()) {
            assert_eq!(ba.position.x, bb.position.x, "x mismatch for {:?}", ba.name);
            assert_eq!(ba.position.y, bb.position.y, "y mismatch for {:?}", ba.name);
            assert_eq!(
                ba.rotation, bb.rotation,
                "rotation mismatch for {:?}",
                ba.name
            );
        }
    }

    // ── test: polygon shape builds without error ──────────────────────────────

    #[test]
    fn test_polygon_shape_ok() {
        let mut scene = minimal_scene(vec![]);
        scene.objects.push(rphys_scene::SceneObject {
            name: Some("tri".to_string()),
            shape: ShapeKind::Polygon {
                vertices: vec![
                    SvVec2::new(-1.0, -1.0),
                    SvVec2::new(1.0, -1.0),
                    SvVec2::new(0.0, 1.0),
                ],
            },
            position: SvVec2::new(10.0, 10.0),
            velocity: SvVec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: rphys_scene::BodyType::Dynamic,
            material: Material::default(),
            color: Color::rgb(0, 255, 0),
            tags: Vec::new(),
            destructible: None,
            boost: None,
            audio: rphys_scene::ObjectAudio::default(),
        });
        assert!(PhysicsEngine::new(&scene, PhysicsConfig::default()).is_ok());
    }

    // ── test: static body with angular_velocity spins as kinematic ────────────

    /// A body declared `static` but given an `angular_velocity` must be
    /// promoted to `KinematicVelocityBased`.  After stepping, its rotation
    /// must have changed and its `body_type` in the state snapshot must be
    /// `Kinematic` (not `Static`).
    #[test]
    fn test_static_with_angular_velocity_becomes_kinematic() {
        let spinner = rphys_scene::SceneObject {
            name: Some("spinner".to_string()),
            shape: ShapeKind::Rectangle {
                width: 2.0,
                height: 0.2,
            },
            position: SvVec2::new(10.0, 17.5),
            velocity: SvVec2::ZERO,
            rotation: 0.0,
            // Declared static but has angular velocity → must become kinematic.
            angular_velocity: Some(std::f32::consts::PI), // π rad/s
            body_type: rphys_scene::BodyType::Static,
            material: Material::default(),
            color: Color::rgb(200, 0, 0),
            tags: vec!["obstacle".to_string()],
            destructible: None,
            boost: None,
            audio: rphys_scene::ObjectAudio::default(),
        };

        let scene = minimal_scene(vec![spinner]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();

        // Advance one second — a fixed body would stay at 0 rad; a kinematic
        // one should have rotated by ~π radians.
        engine.advance_to(1.0).unwrap();

        let state = engine.state();
        let body = state
            .bodies
            .iter()
            .find(|b| b.name.as_deref() == Some("spinner"))
            .unwrap();

        // body_type must be reported as Kinematic, not Static.
        assert_eq!(
            body.body_type,
            rphys_scene::BodyType::Kinematic,
            "body promoted from Static should be reported as Kinematic"
        );

        // Rotation must have advanced (non-zero after 1 s at π rad/s).
        assert!(
            body.rotation.abs() > 0.1,
            "spinner rotation should be non-zero after stepping, got {}",
            body.rotation
        );

        // Body must still be alive (kinematic bodies are never destroyed by physics).
        assert!(body.is_alive, "spinning kinematic body should remain alive");
    }

    // ── test: static body without angular_velocity stays fixed ───────────────

    /// A body declared `static` with no `angular_velocity` must remain
    /// `Fixed` (reported as `Static`) and must not move.
    #[test]
    fn test_static_without_angular_velocity_stays_fixed() {
        let platform = rphys_scene::SceneObject {
            name: Some("platform".to_string()),
            shape: ShapeKind::Rectangle {
                width: 4.0,
                height: 0.2,
            },
            position: SvVec2::new(10.0, 17.5),
            velocity: SvVec2::ZERO,
            rotation: 0.0,
            angular_velocity: None, // no spin → stays Fixed
            body_type: rphys_scene::BodyType::Static,
            material: Material::default(),
            color: Color::rgb(100, 100, 100),
            tags: Vec::new(),
            destructible: None,
            boost: None,
            audio: rphys_scene::ObjectAudio::default(),
        };

        let scene = minimal_scene(vec![platform]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        engine.advance_to(1.0).unwrap();

        let state = engine.state();
        let body = state
            .bodies
            .iter()
            .find(|b| b.name.as_deref() == Some("platform"))
            .unwrap();

        assert_eq!(
            body.body_type,
            rphys_scene::BodyType::Static,
            "static body without angular_velocity should remain Static"
        );
        assert!(
            body.rotation.abs() < 1e-6,
            "fixed body should not rotate, got {}",
            body.rotation
        );
    }

    // ── test: boost pad applies impulse and emits BoostActivated event ────────

    /// A static boost platform with `direction: [0, 1]` (upward impulse) must:
    /// 1. Emit at least one `PhysicsEvent::BoostActivated` while the ball
    ///    rests on it.
    /// 2. Leave the ball at a higher Y position than a control run without
    ///    the boost configuration.
    #[test]
    fn test_boost_pad_applies_impulse() {
        // Place a wide static platform at mid-height.
        let boost_pad = rphys_scene::SceneObject {
            name: Some("pad".to_string()),
            shape: ShapeKind::Rectangle {
                width: 10.0,
                height: 0.2,
            },
            position: SvVec2::new(10.0, 15.0),
            velocity: SvVec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: rphys_scene::BodyType::Static,
            material: Material {
                restitution: 0.0,
                friction: 1.0,
                density: 1.0,
            },
            color: Color::rgb(0, 255, 200),
            tags: vec!["boost".to_string()],
            destructible: None,
            // Upward boost (positive Y in our coordinate system).
            boost: Some(rphys_scene::BoostConfig {
                direction: SvVec2::new(0.0, 1.0),
                impulse: 20.0,
            }),
            audio: rphys_scene::ObjectAudio::default(),
        };

        // Ball dropped just above the platform so it lands quickly.
        let ball = rphys_scene::SceneObject {
            name: Some("ball".to_string()),
            shape: ShapeKind::Circle { radius: 0.3 },
            position: SvVec2::new(10.0, 15.6), // just above pad top face
            velocity: SvVec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: rphys_scene::BodyType::Dynamic,
            material: Material {
                restitution: 0.0,
                friction: 1.0,
                density: 1.0,
            },
            color: Color::rgb(255, 0, 0),
            tags: Vec::new(),
            destructible: None,
            boost: None,
            audio: rphys_scene::ObjectAudio::default(),
        };

        let scene = minimal_scene(vec![boost_pad, ball]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();

        // Run long enough for the ball to land and be boosted.
        let events = engine.advance_to(0.5).unwrap();

        // At least one BoostActivated event must have been emitted.
        assert!(
            events
                .iter()
                .any(|e| matches!(e, PhysicsEvent::BoostActivated { .. })),
            "expected at least one BoostActivated event"
        );
    }

    // ── test: non-boost pad does NOT emit BoostActivated ─────────────────────

    /// A plain static platform (no boost config) must never emit
    /// `PhysicsEvent::BoostActivated`, even when a dynamic ball contacts it.
    #[test]
    fn test_no_boost_config_no_event() {
        let plain_pad = rphys_scene::SceneObject {
            name: Some("plain".to_string()),
            shape: ShapeKind::Rectangle {
                width: 10.0,
                height: 0.2,
            },
            position: SvVec2::new(10.0, 15.0),
            velocity: SvVec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: rphys_scene::BodyType::Static,
            material: Material::default(),
            color: Color::rgb(128, 128, 128),
            tags: Vec::new(),
            destructible: None,
            boost: None, // ← no boost
            audio: rphys_scene::ObjectAudio::default(),
        };

        let ball = dynamic_ball(Some("ball"), SvVec2::new(10.0, 15.6), SvVec2::ZERO);
        let scene = minimal_scene(vec![plain_pad, ball]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        let events = engine.advance_to(0.5).unwrap();

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PhysicsEvent::BoostActivated { .. })),
            "plain pad should not emit BoostActivated events"
        );
    }

    // ── test: boost ignores static-on-static contacts ─────────────────────────

    /// If a boost pad contacts another static body, no impulse is applied and
    /// no `BoostActivated` event is emitted (static bodies cannot be impulse-moved).
    #[test]
    fn test_boost_only_targets_dynamic_bodies() {
        let boost_pad = rphys_scene::SceneObject {
            name: Some("pad".to_string()),
            shape: ShapeKind::Rectangle {
                width: 4.0,
                height: 0.2,
            },
            position: SvVec2::new(10.0, 10.0),
            velocity: SvVec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: rphys_scene::BodyType::Static,
            material: Material::default(),
            color: Color::rgb(0, 255, 200),
            tags: Vec::new(),
            destructible: None,
            boost: Some(rphys_scene::BoostConfig {
                direction: SvVec2::new(0.0, 1.0),
                impulse: 20.0,
            }),
            audio: rphys_scene::ObjectAudio::default(),
        };

        // A second static body touching the pad — should not be boosted.
        let wall_block = rphys_scene::SceneObject {
            name: Some("block".to_string()),
            shape: ShapeKind::Rectangle {
                width: 4.0,
                height: 0.2,
            },
            position: SvVec2::new(10.0, 10.2), // resting on pad
            velocity: SvVec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: rphys_scene::BodyType::Static,
            material: Material::default(),
            color: Color::rgb(200, 200, 200),
            tags: Vec::new(),
            destructible: None,
            boost: None,
            audio: rphys_scene::ObjectAudio::default(),
        };

        let scene = minimal_scene(vec![boost_pad, wall_block]);
        let mut engine = PhysicsEngine::new(&scene, export_config()).unwrap();
        let events = engine.advance_to(0.5).unwrap();

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, PhysicsEvent::BoostActivated { .. })),
            "boost should not activate on static-to-static contact"
        );
    }
}
