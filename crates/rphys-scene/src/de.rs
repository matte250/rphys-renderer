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

#[derive(Debug, Deserialize)]
pub(crate) struct RawObjectAudio {
    pub bounce: Option<String>,
    pub destroy: Option<String>,
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
}

// ── Global audio ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub(crate) struct RawSceneAudio {
    pub default_bounce: Option<String>,
    pub default_destroy: Option<String>,
    pub master_volume: Option<f32>,
}
