//! `rphys-scene` — YAML scene parser and model.
//!
//! # Overview
//!
//! This crate is the single entry point for loading `.yaml` scene files into
//! strongly-typed Rust structs.  It handles:
//!
//! - **Parsing**: YAML → [`Scene`] via [`parse_scene`] / [`parse_scene_file`]
//! - **Validation**: structural and value-level checks with clear error messages
//! - **Schema**: a static JSON Schema string via [`scene_json_schema`]
//!
//! # Example
//!
//! ```rust
//! use rphys_scene::parse_scene;
//!
//! // Note: r##"..."## is used so that hex color strings like "#000000"
//! // (which contain '"#') do not prematurely terminate the raw string.
//! let yaml = r##"
//! version: "1"
//! meta:
//!   name: "Test"
//! environment:
//!   gravity: [0.0, -9.81]
//!   background_color: "#000000"
//!   world_bounds:
//!     width: 20.0
//!     height: 35.56
//!   walls:
//!     visible: true
//!     color: "#ffffff"
//!     thickness: 0.3
//! objects: []
//! "##;
//!
//! let scene = parse_scene(yaml).unwrap();
//! assert_eq!(scene.meta.name, "Test");
//! ```

mod de;
mod parse;
mod schema;
mod types;
pub mod vfx;

pub use parse::{parse_scene, parse_scene_file};
pub use schema::scene_json_schema;
pub use types::{
    validate_vfx_config, BodyType, BoostConfig, BoostFlashConfig, CameraConfig, CameraMode,
    Checkpoint, Color, Destructible, EliminationBurstConfig, EndCondition, Environment,
    GravityWellConfig, ImpactSparksConfig, Material, ObjectAudio, RaceConfig, Scene, SceneAudio,
    SceneMeta, SceneObject, ShapeKind, Vec2, VfxConfig, VfxConfigError, WallConfig,
    WinnerPopConfig, WorldBounds,
};

// Re-export error types at crate root.
pub use parse::{ParseError, ValidationError};
