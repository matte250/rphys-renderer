//! `rphys-vfx` — Particle-based visual effects for race scenes.
//!
//! This crate provides a CPU-side VFX engine that composites semi-transparent
//! particle effects directly into raw RGBA pixel buffers produced by
//! `rphys-renderer`.
//!
//! # Supported effects
//!
//! | Effect | Trigger | Description |
//! |---|---|---|
//! | Impact sparks | `PhysicsEvent::Collision` | Short-lived dot particles at the collision midpoint. |
//! | Boost flash | `PhysicsEvent::BoostActivated` | Glow halo around a ball that hit a boost pad. |
//! | Elimination burst | `RaceEvent::RacerEliminated` | Particle explosion at a racer's last position. |
//! | Winner pop | `RaceEvent::RaceComplete` | Confetti burst at the finish line. |
//!
//! # Quick start
//!
//! ```rust,ignore
//! use rphys_vfx::VfxEngine;
//! use rphys_scene::VfxConfig;
//!
//! let mut engine = VfxEngine::new(config);
//!
//! // Each rendered frame:
//! engine.begin_frame(&body_snapshot, finish_line_px);
//! engine.feed_events(&physics_events, &race_events, &lookup);
//! engine.update(dt);
//! engine.render_into(&mut frame);
//! ```

pub mod blend;
pub mod engine;
pub mod error;
pub mod particle;
pub mod rng;

// Flat re-exports for convenient use.
pub use engine::VfxEngine;
pub use error::VfxError;
// VfxConfig lives in rphys-scene to avoid a circular dependency.
pub use rphys_scene::VfxConfig;
