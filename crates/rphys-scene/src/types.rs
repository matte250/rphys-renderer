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

// ── Gravity well config ───────────────────────────────────────────────────────

/// Configuration for a gravity-well attractor or repulsor zone.
///
/// When a dynamic body enters the `radius` of an object carrying a
/// `GravityWellConfig`, the physics engine applies a continuous force that
/// pulls the body toward the well center (attractor) or pushes it away
/// (repulsor), scaling linearly with proximity.
#[derive(Debug, Clone, PartialEq)]
pub struct GravityWellConfig {
    /// Influence radius in meters.  Dynamic bodies inside this radius are
    /// affected by the well.  Must be > 0.
    pub radius: f32,
    /// Force magnitude in N applied per physics step (scales with proximity).
    /// Must be > 0.
    pub strength: f32,
    /// `false` = attractor (pulls toward center), `true` = repulsor (pushes
    /// away from center).
    pub repulsor: bool,
}

// ── Boost config ──────────────────────────────────────────────────────────────

/// Configuration for a speed-boost platform.
///
/// When a dynamic body contacts an object with a `BoostConfig`, the physics
/// engine applies an impulse in the specified direction to the dynamic body.
#[derive(Debug, Clone, PartialEq)]
pub struct BoostConfig {
    /// Unit vector (world space) indicating the impulse direction.
    ///
    /// For example, `[0.0, -1.0]` pushes downward.
    pub direction: Vec2,
    /// Magnitude of the impulse applied per contact frame (N·s).
    pub impulse: f32,
}

// ── Bumper config ─────────────────────────────────────────────────────────────

/// Configuration for a pinball-style bumper.
///
/// When a dynamic body contacts an object with a `BumperConfig`, the physics
/// engine applies an impulse to the dynamic body in the contact-normal direction
/// (away from the bumper's center), creating a reactive bounce effect.
#[derive(Debug, Clone, PartialEq)]
pub struct BumperConfig {
    /// Impulse magnitude (N·s) applied in the contact normal direction.
    /// Must be > 0.
    pub impulse: f32,
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
    ///
    /// `None` means no angular velocity was specified (body does not spin).
    /// `Some(0.0)` is an explicitly-set zero angular velocity.
    /// The physics layer uses `Some(_)` to decide whether to promote a static
    /// body to `KinematicVelocityBased` so that rapier drives its rotation.
    pub angular_velocity: Option<f32>,
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
    /// If `Some`, this object acts as a speed-boost platform.
    ///
    /// The physics engine will apply the configured impulse to any dynamic body
    /// that contacts this object.
    pub boost: Option<BoostConfig>,
    /// If `Some`, this object acts as a gravity-well attractor or repulsor zone.
    ///
    /// Dynamic bodies that enter the well's `radius` will be continuously
    /// attracted or repelled depending on `GravityWellConfig::repulsor`.
    pub gravity_well: Option<GravityWellConfig>,
    /// If `Some`, this object acts as a pinball bumper.
    ///
    /// The physics engine will apply the configured impulse to any dynamic body
    /// that contacts this object, in the contact-normal direction.
    pub bumper: Option<BumperConfig>,
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
    /// When `true`, the bottom boundary collider is omitted so dynamic bodies
    /// can fall below `y = 0` indefinitely.  Default: `false`.
    pub open_bottom: bool,
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
    /// When set, the last-place racer is eliminated every this many seconds.
    ///
    /// Must be > 0 when present. `None` disables elimination mode (default).
    pub elimination_interval_secs: Option<f32>,
    /// Seconds of **simulation time** to keep running after the first racer crosses
    /// the finish line, so that additional racers can also finish and be ranked.
    ///
    /// **Important:** Because the export loop applies a 4× slow-motion effect near
    /// the finish line (`SLOWDOWN_FACTOR = 0.25`), the resulting video will play
    /// for approximately `post_finish_secs × 4` wall-clock seconds. For example,
    /// `post_finish_secs: 8.0` produces roughly 32 video-seconds of post-finish footage.
    ///
    /// `0.0` (default) stops immediately when the winner is decided,
    /// preserving existing behaviour. Must be >= 0.
    pub post_finish_secs: f32,
    /// Countdown duration in seconds at the start of the race.
    ///
    /// Displays "3… 2… 1… GO!" (or shorter if < 3) and freezes physics during
    /// the countdown. Set to `0` to disable. Default: 3.
    pub countdown_seconds: u32,
}

impl Default for RaceConfig {
    fn default() -> Self {
        Self {
            finish_y: 0.0,
            racer_tag: "racer".to_string(),
            announcement_hold_secs: 2.0,
            checkpoints: Vec::new(),
            elimination_interval_secs: None,
            post_finish_secs: 0.0,
            countdown_seconds: 0,
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

// ── Camera types ──────────────────────────────────────────────────────────────

/// Camera mode used during export and preview.
///
/// Selects which [`CameraController`](rphys_renderer::CameraController)
/// implementation is constructed for the export pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CameraMode {
    /// Fixed camera that shows the entire world with no scrolling.
    Static,
    /// Smooth-following camera that tracks the leading racer (existing behaviour).
    #[default]
    Race,
    /// Dynamic follow-leader camera with shake and finish zoom.
    FollowLeader,
}

/// Optional camera configuration block.
///
/// Add a `camera:` section to the scene YAML to customise camera behaviour.
/// All fields are optional; missing fields use the documented defaults.
/// If the `camera:` key is absent entirely, a `CameraConfig::default()` is used.
///
/// # Example
///
/// ```yaml
/// camera:
///   mode: follow_leader
///   follow_lerp: 0.06
///   look_ahead: 3.0
///   shake_on_impact: true
///   shake_intensity: 0.25
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct CameraConfig {
    /// Camera mode. Default: [`CameraMode::Race`] (existing smooth-follow behaviour).
    pub mode: CameraMode,

    /// Position smoothing factor applied per frame.
    ///
    /// `0.0` = instant snap, `1.0` = camera never moves.
    /// Applied as `new_pos = lerp(cur, target, follow_lerp)`.
    /// Default: `0.08`.
    pub follow_lerp: f32,

    /// Meters ahead of the leader in the travel direction to offset the camera.
    ///
    /// The leader is falling (downward race), so this adds lookahead below.
    /// Default: `2.0`.
    pub look_ahead: f32,

    /// Enable camera shake on collision events. Default: `true`.
    pub shake_on_impact: bool,

    /// Maximum shake displacement in meters. Default: `0.3`.
    pub shake_intensity: f32,

    /// Per-frame shake decay multiplier (`< 1.0` decays, `1.0` = no decay).
    /// Default: `0.85`.
    pub shake_decay: f32,

    /// Extra zoom multiplier on top of base scale (`1.0` = no change). Default: `1.0`.
    pub zoom: f32,

    /// Zoom in when the race winner is decided. Default: `true`.
    pub finish_zoom: bool,

    /// Zoom multiplier applied on race complete. Default: `1.5`.
    pub finish_zoom_factor: f32,

    /// Per-frame smoothing factor for the finish zoom transition. Default: `0.04`.
    pub finish_zoom_lerp: f32,

    /// Lock the horizontal camera position to the world centre.
    ///
    /// When `true` (default), the camera only tracks the leader's Y position
    /// and stays centred horizontally — both side walls remain visible at all
    /// times.  Set to `false` to allow full 2-D panning.
    /// Default: `true`.
    pub lock_horizontal: bool,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            mode: CameraMode::Race,
            follow_lerp: 0.08,
            look_ahead: 2.0,
            shake_on_impact: true,
            shake_intensity: 0.3,
            shake_decay: 0.85,
            zoom: 1.0,
            finish_zoom: true,
            finish_zoom_factor: 1.5,
            finish_zoom_lerp: 0.04,
            lock_horizontal: true,
        }
    }
}

// ── VFX config types ──────────────────────────────────────────────────────────

/// Validation error for a [`VfxConfig`] field.
#[derive(Debug, thiserror::Error)]
pub enum VfxConfigError {
    /// A field has a value outside its valid range.
    #[error("vfx.{field}: value {value} is out of range ({reason})")]
    OutOfRange {
        /// Dotted path to the invalid field (e.g. `"impact_sparks.count"`).
        field: &'static str,
        /// The value that failed validation.
        value: f32,
        /// Human-readable reason the value is invalid.
        reason: &'static str,
    },
    /// A hex color string is malformed.
    #[error("vfx.{field}: invalid hex color '{value}' — expected #RRGGBB or #RRGGBBAA")]
    InvalidColor {
        /// Dotted field path.
        field: &'static str,
        /// The offending string value.
        value: String,
    },
}

/// Impact-sparks effect configuration.
///
/// Emits a burst of short-lived particle dots at the collision point when
/// a ball hits a wall or another ball.
#[derive(Debug, Clone, PartialEq)]
pub struct ImpactSparksConfig {
    /// Whether the effect is active.  Default: `false`.
    pub enabled: bool,
    /// Number of spark particles emitted per impact.  Default: `12`.
    pub count: usize,
    /// Seconds each spark lives before being removed.  Default: `0.25`.
    pub lifetime_secs: f32,
    /// Dot radius of each spark in pixels.  Default: `2.0`.
    pub size_px: f32,
    /// Outward launch speed in pixels per second.  Default: `200.0`.
    pub speed: f32,
}

impl Default for ImpactSparksConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            count: 12,
            lifetime_secs: 0.25,
            size_px: 2.0,
            speed: 200.0,
        }
    }
}

/// Boost-flash effect configuration.
///
/// Renders a glowing halo around a ball when it contacts a boost pad.
#[derive(Debug, Clone, PartialEq)]
pub struct BoostFlashConfig {
    /// Whether the effect is active.  Default: `false`.
    pub enabled: bool,
    /// Color of the flash glow.  Default: opaque white (`#FFFFFF`).
    pub color: Color,
    /// Glow radius extension beyond the ball's edge, in pixels.  Default: `8.0`.
    pub radius_px: f32,
    /// Duration of the flash fade-out in seconds.  Default: `0.3`.
    pub duration_secs: f32,
}

impl Default for BoostFlashConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            color: Color::WHITE,
            radius_px: 8.0,
            duration_secs: 0.3,
        }
    }
}

/// Elimination-burst effect configuration.
///
/// Explodes a burst of particles at a ball's last position when it is
/// eliminated from the race.
#[derive(Debug, Clone, PartialEq)]
pub struct EliminationBurstConfig {
    /// Whether the effect is active.  Default: `false`.
    pub enabled: bool,
    /// Number of burst particles.  Default: `30`.
    pub count: usize,
    /// Seconds each particle lives.  Default: `0.6`.
    pub lifetime_secs: f32,
    /// Dot radius of each particle in pixels.  Default: `3.0`.
    pub size_px: f32,
    /// Outward launch speed in pixels per second.  Default: `300.0`.
    pub speed: f32,
}

impl Default for EliminationBurstConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            count: 30,
            lifetime_secs: 0.6,
            size_px: 3.0,
            speed: 300.0,
        }
    }
}

/// Winner-pop confetti effect configuration.
///
/// Fires a wide confetti burst at the finish line when the first racer wins.
#[derive(Debug, Clone, PartialEq)]
pub struct WinnerPopConfig {
    /// Whether the effect is active.  Default: `false`.
    pub enabled: bool,
    /// Number of confetti particles.  Default: `60`.
    pub count: usize,
    /// Seconds each particle lives.  Default: `1.2`.
    pub lifetime_secs: f32,
    /// Dot radius of each particle in pixels.  Default: `4.0`.
    pub size_px: f32,
    /// Launch speed in pixels per second.  Default: `350.0`.
    pub speed: f32,
    /// Fan spread angle in degrees (centred upward).  Default: `180.0`.
    pub spread_deg: f32,
    /// Explicit palette of colors for confetti particles.
    ///
    /// When empty (the default), the renderer uses gold + white + the
    /// winner ball's color.
    pub colors: Vec<Color>,
}

impl Default for WinnerPopConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            count: 60,
            lifetime_secs: 1.2,
            size_px: 4.0,
            speed: 350.0,
            spread_deg: 180.0,
            colors: Vec::new(),
        }
    }
}

/// Top-level VFX configuration, parsed from the optional `vfx:` YAML block.
///
/// When the block is absent the renderer receives `None` and skips all VFX
/// code paths (zero performance cost on legacy scenes).
#[derive(Debug, Clone, PartialEq)]
pub struct VfxConfig {
    /// Global particle cap across all live effects.
    ///
    /// New bursts that would exceed this cap are silently truncated.
    /// Default: `500`.
    pub max_particles: usize,
    /// Impact sparks emitted on ball/wall collisions.
    pub impact_sparks: ImpactSparksConfig,
    /// Boost-pad glow flash on boosted balls.
    pub boost_flash: BoostFlashConfig,
    /// Explosion burst when a racer is eliminated.
    pub elimination_burst: EliminationBurstConfig,
    /// Confetti burst when the race winner crosses the finish line.
    pub winner_pop: WinnerPopConfig,
}

impl Default for VfxConfig {
    fn default() -> Self {
        Self {
            max_particles: 500,
            impact_sparks: ImpactSparksConfig::default(),
            boost_flash: BoostFlashConfig::default(),
            elimination_burst: EliminationBurstConfig::default(),
            winner_pop: WinnerPopConfig::default(),
        }
    }
}

/// Validate all fields of a [`VfxConfig`] and return the first error found,
/// or `Ok(())` if everything is in range.
///
/// # Errors
///
/// Returns [`VfxConfigError::OutOfRange`] when any numeric field is <= 0
/// (particle counts, lifetimes, speeds, sizes) or outside its allowed
/// range (`spread_deg` must be in `(0, 360]`).
pub fn validate_vfx_config(cfg: &VfxConfig) -> Result<(), VfxConfigError> {
    macro_rules! require_pos {
        ($field:expr, $val:expr) => {
            if $val <= 0.0 {
                return Err(VfxConfigError::OutOfRange {
                    field: $field,
                    value: $val,
                    reason: "must be > 0",
                });
            }
        };
    }
    macro_rules! require_usize_pos {
        ($field:expr, $val:expr) => {
            if $val == 0 {
                return Err(VfxConfigError::OutOfRange {
                    field: $field,
                    value: $val as f32,
                    reason: "must be >= 1",
                });
            }
        };
    }

    // max_particles
    require_usize_pos!("max_particles", cfg.max_particles);

    // impact_sparks
    require_usize_pos!("impact_sparks.count", cfg.impact_sparks.count);
    require_pos!(
        "impact_sparks.lifetime_secs",
        cfg.impact_sparks.lifetime_secs
    );
    require_pos!("impact_sparks.size_px", cfg.impact_sparks.size_px);
    require_pos!("impact_sparks.speed", cfg.impact_sparks.speed);

    // boost_flash
    require_pos!("boost_flash.radius_px", cfg.boost_flash.radius_px);
    require_pos!("boost_flash.duration_secs", cfg.boost_flash.duration_secs);

    // elimination_burst
    require_usize_pos!("elimination_burst.count", cfg.elimination_burst.count);
    require_pos!(
        "elimination_burst.lifetime_secs",
        cfg.elimination_burst.lifetime_secs
    );
    require_pos!("elimination_burst.size_px", cfg.elimination_burst.size_px);
    require_pos!("elimination_burst.speed", cfg.elimination_burst.speed);

    // winner_pop
    require_usize_pos!("winner_pop.count", cfg.winner_pop.count);
    require_pos!("winner_pop.lifetime_secs", cfg.winner_pop.lifetime_secs);
    require_pos!("winner_pop.size_px", cfg.winner_pop.size_px);
    require_pos!("winner_pop.speed", cfg.winner_pop.speed);
    if !(0.0..=360.0).contains(&cfg.winner_pop.spread_deg) || cfg.winner_pop.spread_deg <= 0.0 {
        return Err(VfxConfigError::OutOfRange {
            field: "winner_pop.spread_deg",
            value: cfg.winner_pop.spread_deg,
            reason: "must be in range (0, 360]",
        });
    }

    Ok(())
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
    /// Optional camera configuration.
    ///
    /// When `None`, the export pipeline uses [`CameraConfig::default()`].
    pub camera: Option<CameraConfig>,
    /// Optional particle-based visual effects configuration.
    ///
    /// When `None`, no VFX code paths run (zero overhead on legacy scenes).
    pub vfx: Option<VfxConfig>,
}
