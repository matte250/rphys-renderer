//! Public domain types for the rphys scene model.
//!
//! All types are plain Rust structs/enums — no serde derives here.
//! Serialization is handled through intermediate `de::Raw*` types.

use std::path::PathBuf;

// ── Domain primitives ─────────────────────────────────────────────────────────

/// 2D vector used for positions, velocities, and gravity.
///
/// Coordinates follow standard math convention: `x` increases right,
/// `y` increases upward.  The renderer flips `y` for screen space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec2 {
    /// Horizontal component (meters or m/s depending on context).
    pub x: f32,
    /// Vertical component (meters or m/s depending on context).
    pub y: f32,
}

impl Vec2 {
    /// Construct a new `Vec2`.
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// The zero vector `(0, 0)`.
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
}

impl From<[f32; 2]> for Vec2 {
    fn from(arr: [f32; 2]) -> Self {
        Self::new(arr[0], arr[1])
    }
}

/// RGBA color stored as 0–255 components.
///
/// In YAML, colors are written as `"#RRGGBB"` or `"#RRGGBBAA"` hex strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    /// Red channel (0–255).
    pub r: u8,
    /// Green channel (0–255).
    pub g: u8,
    /// Blue channel (0–255).
    pub b: u8,
    /// Alpha channel (0–255).  255 = fully opaque.
    pub a: u8,
}

impl Color {
    /// Construct a fully-opaque RGB color.
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }

    /// Construct an RGBA color.
    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Opaque white.
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };

    /// Opaque black.
    pub const BLACK: Color = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
}

// ── Shape definitions ─────────────────────────────────────────────────────────

/// Geometric shape of a scene object.
///
/// Shape-specific parameters determine the collider geometry.
/// All dimensions are in meters.
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeKind {
    /// Circular collider.
    Circle {
        /// Radius in meters.
        radius: f32,
    },
    /// Axis-aligned rectangular collider.
    Rectangle {
        /// Width in meters.
        width: f32,
        /// Height in meters.
        height: f32,
    },
    /// Convex polygon collider.
    ///
    /// Vertices are offsets from the object's center position, in meters.
    /// Should be specified in counter-clockwise order.
    Polygon {
        /// Vertex offsets from the object center (meters).
        vertices: Vec<Vec2>,
    },
}

// ── Material ──────────────────────────────────────────────────────────────────

/// Physical material properties.
///
/// These values are passed directly to the rapier2d collider.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// Coefficient of restitution: 0.0 = no bounce, 1.0 = perfect elastic bounce.
    pub restitution: f32,
    /// Coefficient of friction: 0.0 = frictionless (ice), 1.0 = high friction (rubber).
    pub friction: f32,
    /// Density in kg/m².  Mass is derived from shape area × density.
    pub density: f32,
}

impl Default for Material {
    fn default() -> Self {
        Self {
            restitution: 0.5,
            friction: 0.5,
            density: 1.0,
        }
    }
}

// ── Body type ─────────────────────────────────────────────────────────────────

/// Rigid body simulation mode.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum BodyType {
    /// Affected by gravity and collisions.  Default.
    #[default]
    Dynamic,
    /// Never moves; infinite mass.  Walls, floors, fixed platforms.
    Static,
    /// Position is set programmatically (future feature — use `Static` for now).
    Kinematic,
}

// ── Destructible config ───────────────────────────────────────────────────────

/// Configuration for a destructible object.
///
/// When a collision impulse exceeds `min_impact_force`, the body is removed
/// from the simulation and a `Destroyed` physics event is emitted.
#[derive(Debug, Clone, PartialEq)]
pub struct Destructible {
    /// Minimum impulse magnitude (N·s) required to destroy this object.
    pub min_impact_force: f32,
}

// ── Audio mapping for a single object ────────────────────────────────────────

/// Per-object audio overrides.
///
/// Paths are resolved relative to the scene file's directory.
/// A `None` value means "use the global default from [`SceneAudio`]".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjectAudio {
    /// Sound to play when this object bounces off something.
    pub bounce: Option<PathBuf>,
    /// Sound to play when this object is destroyed.
    pub destroy: Option<PathBuf>,
}

// ── Scene object ──────────────────────────────────────────────────────────────

/// A single simulated body in the scene.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneObject {
    /// Optional human-readable identifier.  Must be unique if provided.
    pub name: Option<String>,
    /// Collider geometry.
    pub shape: ShapeKind,
    /// Initial position in meters from the world origin (bottom-left = `(0, 0)`).
    pub position: Vec2,
    /// Initial velocity in m/s.
    pub velocity: Vec2,
    /// Initial rotation in **radians** (counter-clockwise positive).
    pub rotation: f32,
    /// Initial angular velocity in **rad/s**.
    pub angular_velocity: f32,
    /// Simulation mode (dynamic / static / kinematic).
    pub body_type: BodyType,
    /// Physical material properties.
    pub material: Material,
    /// Fill color for rendering.
    pub color: Color,
    /// Arbitrary labels used for grouping and end conditions.
    pub tags: Vec<String>,
    /// If `Some`, the object can be destroyed by high-impulse collisions.
    pub destructible: Option<Destructible>,
    /// Per-object sound overrides.
    pub audio: ObjectAudio,
}

// ── Environment ───────────────────────────────────────────────────────────────

/// World boundary rectangle.
///
/// The world origin `(0, 0)` is at the **bottom-left** corner.
/// `Y` increases upward (standard math convention).
#[derive(Debug, Clone, PartialEq)]
pub struct WorldBounds {
    /// World width in meters.
    pub width: f32,
    /// World height in meters.
    pub height: f32,
}

/// Configuration for the world boundary walls.
///
/// Walls are always static colliders regardless of other settings.
#[derive(Debug, Clone, PartialEq)]
pub struct WallConfig {
    /// Whether the walls are drawn on screen.
    pub visible: bool,
    /// Wall fill color (relevant only when `visible = true`).
    pub color: Color,
    /// Wall thickness in meters.
    pub thickness: f32,
}

/// Global world environment settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Environment {
    /// Gravity vector in m/s².  Earth standard: `[0, -9.81]`.
    pub gravity: Vec2,
    /// Background fill color for rendered frames.
    pub background_color: Color,
    /// Axis-aligned world boundary.
    pub world_bounds: WorldBounds,
    /// Boundary wall configuration.
    pub walls: WallConfig,
}

// ── Race types ────────────────────────────────────────────────────────────────

/// A visual checkpoint line at a world Y coordinate.
///
/// Checkpoints are optional milestone markers displayed as horizontal lines
/// in the race overlay. They do not affect ranking.
#[derive(Debug, Clone, PartialEq)]
pub struct Checkpoint {
    /// World Y coordinate of this checkpoint.
    pub y: f32,
    /// Optional label rendered alongside the checkpoint line in the overlay.
    pub label: Option<String>,
}

/// Configuration for a race scene.
///
/// Present only when the `race:` key exists in the YAML. Its presence signals
/// that race mode should be used for both export and preview.
#[derive(Debug, Clone, PartialEq)]
pub struct RaceConfig {
    /// World Y coordinate of the finish line.
    ///
    /// The race ends when any racer's Y ≤ this value. Must be >= 0.
    pub finish_y: f32,
    /// Tag that identifies racer bodies. Default: `"racer"`.
    pub racer_tag: String,
    /// How long (in seconds) to hold the winner frame at the end of export.
    ///
    /// Must be > 0. Default: 2.0.
    pub announcement_hold_secs: f32,
    /// Optional milestone Y-coordinates shown as horizontal lines with labels.
    ///
    /// Each checkpoint `y` must be greater than `finish_y`.
    pub checkpoints: Vec<Checkpoint>,
}

impl Default for RaceConfig {
    fn default() -> Self {
        Self {
            finish_y: 0.0,
            racer_tag: "racer".to_string(),
            announcement_hold_secs: 2.0,
            checkpoints: Vec::new(),
        }
    }
}

// ── End conditions ────────────────────────────────────────────────────────────

/// Condition that terminates the simulation when satisfied.
///
/// Simple conditions evaluate a single predicate; composite conditions
/// (`And`, `Or`) combine multiple sub-conditions.
#[derive(Debug, Clone, PartialEq)]
pub enum EndCondition {
    /// Simulation stops after this many seconds of physics time.
    TimeLimit {
        /// Duration in seconds.
        seconds: f32,
    },
    /// All objects tagged with `tag` have been destroyed.
    AllTaggedDestroyed {
        /// Tag that all destroyed objects must share.
        tag: String,
    },
    /// The named object has left the world bounds.
    ObjectEscaped {
        /// Name of the object to watch.
        name: String,
    },
    /// Two named objects have collided with each other (first contact).
    ObjectsCollided {
        /// Name of the first object.
        name_a: String,
        /// Name of the second object.
        name_b: String,
    },
    /// Any object with `tag_a` has collided with any object with `tag_b`.
    TagsCollided {
        /// First tag.
        tag_a: String,
        /// Second tag.
        tag_b: String,
    },
    /// All sub-conditions must be simultaneously true.
    And {
        /// Sub-conditions, all of which must hold.
        conditions: Vec<EndCondition>,
    },
    /// Any sub-condition being true triggers completion.
    Or {
        /// Sub-conditions, any one of which is sufficient.
        conditions: Vec<EndCondition>,
    },
    /// Race condition: fires when the first body tagged with `tag` crosses
    /// below `finish_y`.
    ///
    /// The `tag` field should match `RaceConfig::racer_tag` when used together
    /// with a `race:` section.
    FirstToReach {
        /// World Y coordinate of the finish line. Must be >= 0.
        finish_y: f32,
        /// Tag identifying racer bodies. Default when omitted in YAML: `"racer"`.
        tag: String,
    },
}

// ── Global audio config ───────────────────────────────────────────────────────

/// Global audio defaults for the scene.
///
/// Per-object [`ObjectAudio`] overrides these.
/// Paths are resolved relative to the scene file's directory.
#[derive(Debug, Clone, PartialEq)]
pub struct SceneAudio {
    /// Fallback bounce sound when an object has no per-object bounce sound.
    pub default_bounce: Option<PathBuf>,
    /// Fallback destroy sound when an object has no per-object destroy sound.
    pub default_destroy: Option<PathBuf>,
    /// Master volume multiplier: 0.0 = silent, 1.0 = full.
    pub master_volume: f32,
}

impl Default for SceneAudio {
    /// Returns a `SceneAudio` with no sounds and `master_volume = 1.0` (full volume).
    fn default() -> Self {
        Self {
            default_bounce: None,
            default_destroy: None,
            master_volume: 1.0,
        }
    }
}

// ── Metadata ──────────────────────────────────────────────────────────────────

/// Scene metadata (name, description, author, etc.).
#[derive(Debug, Clone, PartialEq)]
pub struct SceneMeta {
    /// Human-readable scene name.
    pub name: String,
    /// Optional description of what the scene demonstrates.
    pub description: Option<String>,
    /// Optional author attribution.
    pub author: Option<String>,
    /// Hint for export duration in seconds.
    ///
    /// Used when no end condition fires and `--duration` was not given.
    pub duration_hint: Option<f32>,
}

// ── Top-level scene ───────────────────────────────────────────────────────────

/// A complete, validated scene definition.
///
/// Obtain a `Scene` by calling [`parse_scene`](crate::parse_scene) or
/// [`parse_scene_file`](crate::parse_scene_file).
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Schema version string.  Currently always `"1"`.
    pub version: String,
    /// Scene metadata.
    pub meta: SceneMeta,
    /// World environment settings.
    pub environment: Environment,
    /// List of simulated bodies.
    pub objects: Vec<SceneObject>,
    /// Optional termination condition.  `None` = run until stopped.
    pub end_condition: Option<EndCondition>,
    /// Global audio configuration.
    pub audio: SceneAudio,
    /// Present when the scene is a race. `None` for non-race scenes.
    ///
    /// When `Some`, race mode is used for both export and preview.
    pub race: Option<RaceConfig>,
}
