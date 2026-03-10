//! Intermediate (raw) types used for YAML deserialization.
//!
//! These structs map 1-to-1 with the YAML schema and derive `serde::Deserialize`.
//! After deserialization, they are validated and converted into the public
//! domain types in [`crate::types`].
//!
//! Design notes:
//! - Optional fields use `Option<T>` so we can distinguish "not provided" from
//!   "provided as null".
//! - Shape fields (`radius`, `size`, `vertices`) are flat on `RawObject`
//!   because the YAML format places them at the object level, not nested.
//! - End conditions use `#[serde(tag = "type")]` for internally-tagged enums.

use serde::Deserialize;

// ── Top-level scene ───────────────────────────────────────────────────────────

/// Raw top-level scene — mirrors the YAML top-level structure.
#[derive(Debug, Deserialize)]
pub(crate) struct RawScene {
    pub version: String,
    pub meta: RawMeta,
    pub environment: RawEnvironment,
    pub objects: Vec<RawObject>,
    pub end_condition: Option<RawEndCondition>,
    pub audio: Option<RawSceneAudio>,
    pub race: Option<RawRaceConfig>,
    pub camera: Option<RawCameraConfig>,
    pub vfx: Option<RawVfxConfig>,
}

// ── Meta ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct RawMeta {
    pub name: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub duration_hint: Option<f32>,
}

// ── Environment ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct RawEnvironment {
    /// `[x, y]` gravity in m/s².
    pub gravity: [f32; 2],
    pub background_color: String,
    pub world_bounds: RawWorldBounds,
    pub walls: RawWallConfig,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawWorldBounds {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawWallConfig {
    pub visible: Option<bool>,
    pub color: Option<String>,
    pub thickness: Option<f32>,
    /// When `true`, the bottom boundary collider is omitted. Omitted in YAML means `false`.
    pub open_bottom: Option<bool>,
}

// ── Object ────────────────────────────────────────────────────────────────────

/// Raw scene object with all possible fields as `Option<T>`.
///
/// Shape-specific fields (`radius`, `size`, `vertices`) are placed here at the
/// top level because the YAML format inlines them alongside `shape: <type>`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawObject {
    pub name: Option<String>,

    // Shape discriminator — "circle", "rectangle", or "polygon".
    pub shape: Option<String>,

    // Circle-specific
    pub radius: Option<f32>,

    // Rectangle-specific (YAML: `size: [w, h]`)
    pub size: Option<[f32; 2]>,

    // Polygon-specific (YAML: `vertices: [[x,y], ...]`)
    pub vertices: Option<Vec<[f32; 2]>>,

    // Common positional/kinematic fields
    pub position: Option<[f32; 2]>,
    pub velocity: Option<[f32; 2]>,
    /// Rotation in **degrees** (YAML convention).  Converted to radians on load.
    pub rotation: Option<f32>,
    /// Angular velocity in **degrees/s** (YAML convention).  Converted to rad/s.
    pub angular_velocity: Option<f32>,

    // Physics / rendering
    pub body_type: Option<String>,
    pub material: Option<RawMaterial>,
    pub color: Option<String>,
    pub tags: Option<Vec<String>>,
    pub destructible: Option<RawDestructible>,
    pub boost: Option<RawBoostConfig>,
    pub gravity_well: Option<RawGravityWellConfig>,
    pub bumper: Option<RawBumperConfig>,
    pub audio: Option<RawObjectAudio>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawMaterial {
    pub restitution: Option<f32>,
    pub friction: Option<f32>,
    pub density: Option<f32>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawDestructible {
    pub min_impact_force: f32,
}

/// Raw boost configuration — mirrors the `boost:` YAML block on a scene object.
///
/// `direction` is a two-element `[x, y]` array; `impulse` is the N·s magnitude.
#[derive(Debug, Deserialize)]
pub(crate) struct RawBoostConfig {
    /// Direction vector `[x, y]` (world space, should be a unit vector).
    pub direction: [f32; 2],
    /// Impulse magnitude in N·s applied per contact frame.
    pub impulse: f32,
}

/// Raw gravity-well configuration — mirrors the `gravity_well:` YAML block.
#[derive(Debug, Deserialize)]
pub(crate) struct RawGravityWellConfig {
    /// Influence radius in meters.
    pub radius: f32,
    /// Force magnitude in N per physics step (scales with proximity).
    pub strength: f32,
    /// `false` = attractor, `true` = repulsor.
    #[serde(default)]
    pub repulsor: bool,
}

/// Raw bumper configuration — mirrors the `bumper:` YAML block.
#[derive(Debug, Deserialize)]
pub(crate) struct RawBumperConfig {
    /// Impulse magnitude in N·s applied in the contact normal direction.
    pub impulse: f32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawObjectAudio {
    pub bounce: Option<String>,
    pub destroy: Option<String>,
}

// ── Race types ────────────────────────────────────────────────────────────────

/// Raw checkpoint entry — a milestone Y-coordinate in a race scene.
#[derive(Debug, Deserialize)]
pub(crate) struct RawCheckpoint {
    pub y: f32,
    pub label: Option<String>,
}

/// Raw race configuration — mirrors the `race:` YAML block.
#[derive(Debug, Deserialize)]
pub(crate) struct RawRaceConfig {
    pub finish_y: f32,
    pub racer_tag: Option<String>,
    pub announcement_hold_secs: Option<f32>,
    pub elimination_interval_secs: Option<f32>,
    pub post_finish_secs: Option<f32>,
    #[serde(default)]
    pub checkpoints: Vec<RawCheckpoint>,
}

// ── End conditions ────────────────────────────────────────────────────────────

/// End condition — internally tagged by the `type` field.
///
/// Serde maps `type: time_limit` to `RawEndCondition::TimeLimit { .. }`.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum RawEndCondition {
    TimeLimit { seconds: f32 },
    AllTaggedDestroyed { tag: String },
    ObjectEscaped { name: String },
    ObjectsCollided { name_a: String, name_b: String },
    TagsCollided { tag_a: String, tag_b: String },
    And { conditions: Vec<RawEndCondition> },
    Or { conditions: Vec<RawEndCondition> },
    FirstToReach { finish_y: f32, tag: Option<String> },
}

// ── Global audio ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct RawSceneAudio {
    pub default_bounce: Option<String>,
    pub default_destroy: Option<String>,
    pub master_volume: Option<f32>,
}

// ── Camera config ─────────────────────────────────────────────────────────────

/// Raw camera configuration — mirrors the optional `camera:` YAML block.
#[derive(Debug, Deserialize)]
pub(crate) struct RawCameraConfig {
    /// `"static"`, `"race"`, or `"follow_leader"`. Default: `"race"`.
    pub mode: Option<String>,
    pub follow_lerp: Option<f32>,
    pub look_ahead: Option<f32>,
    pub shake_on_impact: Option<bool>,
    pub shake_intensity: Option<f32>,
    pub shake_decay: Option<f32>,
    pub zoom: Option<f32>,
    pub finish_zoom: Option<bool>,
    pub finish_zoom_factor: Option<f32>,
    pub finish_zoom_lerp: Option<f32>,
    pub lock_horizontal: Option<bool>,
}

// ── VFX config ────────────────────────────────────────────────────────────────

/// Raw impact-sparks sub-config.
#[derive(Debug, Deserialize)]
pub(crate) struct RawImpactSparksConfig {
    pub enabled: Option<bool>,
    pub count: Option<usize>,
    pub lifetime_secs: Option<f32>,
    pub size_px: Option<f32>,
    pub speed: Option<f32>,
}

/// Raw boost-flash sub-config.
#[derive(Debug, Deserialize)]
pub(crate) struct RawBoostFlashConfig {
    pub enabled: Option<bool>,
    pub color: Option<String>,
    pub radius_px: Option<f32>,
    pub duration_secs: Option<f32>,
}

/// Raw elimination-burst sub-config.
#[derive(Debug, Deserialize)]
pub(crate) struct RawEliminationBurstConfig {
    pub enabled: Option<bool>,
    pub count: Option<usize>,
    pub lifetime_secs: Option<f32>,
    pub size_px: Option<f32>,
    pub speed: Option<f32>,
}

/// Raw winner-pop sub-config.
#[derive(Debug, Deserialize)]
pub(crate) struct RawWinnerPopConfig {
    pub enabled: Option<bool>,
    pub count: Option<usize>,
    pub lifetime_secs: Option<f32>,
    pub size_px: Option<f32>,
    pub speed: Option<f32>,
    pub spread_deg: Option<f32>,
    pub colors: Option<Vec<String>>,
}

/// Raw top-level VFX config — mirrors the optional `vfx:` YAML block.
#[derive(Debug, Deserialize)]
pub(crate) struct RawVfxConfig {
    pub max_particles: Option<usize>,
    pub impact_sparks: Option<RawImpactSparksConfig>,
    pub boost_flash: Option<RawBoostFlashConfig>,
    pub elimination_burst: Option<RawEliminationBurstConfig>,
    pub winner_pop: Option<RawWinnerPopConfig>,
}
