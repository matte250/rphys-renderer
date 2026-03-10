//! Public types for the physics simulation engine.

use rphys_scene::{BodyType, Color, ObjectAudio, ShapeKind, Vec2, WallConfig, WorldBounds};

// ── Stable object ID ──────────────────────────────────────────────────────────

/// Stable identifier for a simulated body. Opaque to callers.
///
/// Wraps an internal counter ID — does not directly expose rapier handles.
/// Remains valid even after other bodies have been removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyId(pub u32);

// ── Per-body snapshot ─────────────────────────────────────────────────────────

/// Snapshot of one body's state at a point in physics time.
#[derive(Debug, Clone)]
pub struct BodyState {
    /// Stable identifier for this body.
    pub id: BodyId,
    /// Name from the original `SceneObject`, if any.
    pub name: Option<String>,
    /// Tags copied from the original `SceneObject`.
    pub tags: Vec<String>,
    /// Current world-space position in meters.
    pub position: Vec2,
    /// Current rotation in radians (counter-clockwise positive).
    pub rotation: f32,
    /// Shape geometry (unchanged from scene definition).
    pub shape: ShapeKind,
    /// Fill color for rendering.
    pub color: Color,
    /// `true` if the body is still in the simulation.
    pub is_alive: bool,
    /// Rigid body simulation mode (dynamic / static / kinematic).
    ///
    /// Used by the renderer to determine opacity: static bodies are drawn
    /// at 80% opacity, dynamic and kinematic bodies at full opacity.
    pub body_type: BodyType,
}

// ── Full world snapshot ───────────────────────────────────────────────────────

/// Immutable snapshot of the physics world at a point in simulation time.
///
/// This is what the renderer reads each frame.
#[derive(Debug, Clone)]
pub struct PhysicsState {
    /// All bodies (alive and destroyed) at this point in time.
    pub bodies: Vec<BodyState>,
    /// Physics time elapsed in seconds.
    pub time: f32,
    /// World boundary dimensions.
    pub world_bounds: WorldBounds,
    /// Wall render configuration.
    pub wall_config: WallConfig,
}

// ── Physics events ────────────────────────────────────────────────────────────

/// Detailed information about a body-body collision.
#[derive(Debug, Clone)]
pub struct CollisionInfo {
    /// First body in the collision pair.
    pub body_a: BodyId,
    /// Second body in the collision pair.
    pub body_b: BodyId,
    /// Estimated contact impulse magnitude in N·s.
    /// Useful for scaling audio volume.
    pub impulse: f32,
}

/// Events that may occur during a physics step.
#[derive(Debug, Clone)]
pub enum PhysicsEvent {
    /// Two scene bodies started contacting.
    Collision(CollisionInfo),
    /// A body made contact with a world boundary wall.
    WallBounce {
        /// The body that bounced.
        body: BodyId,
        /// Estimated contact impulse magnitude in N·s.
        impulse: f32,
    },
    /// A destructible body was removed because a collision impulse exceeded its threshold.
    Destroyed {
        /// The body that was destroyed.
        body: BodyId,
    },
    /// A dynamic body contacted a boost pad and received a speed impulse.
    ///
    /// Emitted once per step for each active boost contact. Useful for
    /// triggering audio or visual feedback.
    BoostActivated {
        /// The dynamic body that received the boost impulse.
        body: BodyId,
    },
    /// A dynamic body contacted a bumper and received an outward impulse.
    ///
    /// The impulse direction is determined by the contact normal (from
    /// bumper center to dynamic body center). Emitted once per step for
    /// each active bumper contact.
    BumperActivated {
        /// The dynamic body that received the bumper impulse.
        body: BodyId,
        /// The contact point where the bumper activated.
        contact_point: Vec2,
        /// Magnitude of the impulse applied.
        impulse_magnitude: f32,
    },
    /// A dynamic body was within the influence radius of a gravity well and
    /// received an attractive or repulsive force impulse this step.
    ///
    /// Emitted once per affected body per step. Useful for triggering
    /// audio or visual feedback on bodies caught in a gravity well.
    GravityWellPull {
        /// The dynamic body that was pulled or pushed.
        body: BodyId,
        /// The body that carries the gravity-well configuration.
        well_body: BodyId,
    },
    /// An end condition was satisfied and the simulation is stopping.
    SimulationComplete {
        /// The reason the simulation ended.
        reason: CompletionReason,
    },
}

/// Reason a simulation reached its end condition.
#[derive(Debug, Clone, PartialEq)]
pub enum CompletionReason {
    /// The time limit was reached.
    TimeLimitReached,
    /// All objects with the given tag were destroyed.
    AllTaggedDestroyed {
        /// The tag that was watched.
        tag: String,
    },
    /// The named object escaped outside the world bounds.
    ObjectEscaped {
        /// Name of the body that escaped.
        name: String,
    },
    /// Two named objects collided.
    ObjectsCollided {
        /// Name of the first body.
        name_a: String,
        /// Name of the second body.
        name_b: String,
    },
    /// An object with `tag_a` collided with an object with `tag_b`.
    TagsCollided {
        /// First tag.
        tag_a: String,
        /// Second tag.
        tag_b: String,
    },
    /// The first body to reach a tagged zone won the race.
    ///
    /// Detected by `rphys-race` (Sprint 2 Wave 2); the physics engine
    /// itself never emits this variant.
    FirstToReach {
        /// The tag of the finish zone that was reached.
        tag: String,
        /// The body that arrived first.
        winner_body: BodyId,
        /// Optional display name of the winning body.
        winner_name: Option<String>,
    },
}

// ── Engine configuration ──────────────────────────────────────────────────────

/// Configuration for the physics engine.
#[derive(Debug, Clone)]
pub struct PhysicsConfig {
    /// Fixed timestep in seconds. Default: `1.0 / 240.0` (240 Hz).
    pub timestep: f32,
    /// Maximum steps executed in a single `advance_to()` call.
    ///
    /// Guards the preview accumulator loop against spiral-of-death if the
    /// host machine falls behind real time. Export mode should set this to
    /// `u32::MAX` or a sufficiently large value.
    pub max_steps_per_call: u32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            timestep: 1.0 / 240.0,
            max_steps_per_call: 8,
        }
    }
}

// ── Per-body public metadata ──────────────────────────────────────────────────

/// Stable metadata for a body, valid for its entire lifetime.
#[derive(Debug)]
pub struct BodyInfo {
    /// Name from the original `SceneObject`, if any.
    pub name: Option<String>,
    /// Tags from the original `SceneObject`.
    pub tags: Vec<String>,
    /// Per-object audio configuration.
    pub audio: ObjectAudio,
}

// ── Error types ───────────────────────────────────────────────────────────────

/// Errors produced by the physics engine.
#[derive(Debug, thiserror::Error)]
pub enum PhysicsError {
    /// World construction failed.
    #[error("Failed to build physics world: {0}")]
    BuildFailed(String),

    /// A physics step failed.
    #[error("Physics step failed: {0}")]
    StepFailed(String),
}
