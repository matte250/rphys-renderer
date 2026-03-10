//! `rphys-physics` — physics simulation engine for rphys-renderer.
//!
//! This crate wraps [`rapier2d`] to provide a fixed-timestep 2D physics
//! simulation with deterministic output.
//!
//! # Usage
//!
//! ```rust,no_run
//! use rphys_physics::{PhysicsEngine, PhysicsConfig};
//! # use rphys_scene::{Scene, Color, Environment, WorldBounds, WallConfig, SceneMeta, SceneAudio, Vec2};
//! # fn make_scene() -> Scene {
//! #     Scene {
//! #         version: "1".to_string(),
//! #         meta: SceneMeta { name: "t".to_string(), description: None, author: None, duration_hint: None },
//! #         environment: Environment {
//! #             gravity: Vec2::new(0.0, -9.81),
//! #             background_color: Color::BLACK,
//! #             world_bounds: WorldBounds { width: 20.0, height: 35.0 },
//! #             walls: WallConfig { visible: true, color: Color::WHITE, thickness: 0.5, open_bottom: false },
//! #         },
//! #         objects: vec![],
//! #         end_condition: None,
//! #         audio: SceneAudio::default(),
//! #         race: None,
//! #         camera: None,
//! #         vfx: None,
//! #     }
//! # }
//!
//! let scene = make_scene();
//! let mut engine = PhysicsEngine::new(&scene, PhysicsConfig::default()).unwrap();
//!
//! loop {
//!     let events = engine.step().unwrap();
//!     let state = engine.state();
//!     // hand `state` to the renderer …
//!     if engine.is_complete() { break; }
//! }
//! ```

mod engine;
pub mod types;

pub use engine::PhysicsEngine;
pub use types::{
    BodyId, BodyInfo, BodyState, CollisionInfo, CompletionReason, PhysicsConfig, PhysicsError,
    PhysicsEvent, PhysicsState,
};
