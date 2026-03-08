//! `rphys-race` — race state tracking for rphys-renderer.
//!
//! This crate wraps [`rphys_physics::PhysicsEngine`] with race-specific logic:
//! - Per-racer rank tracking (by world Y position)
//! - Checkpoint crossing detection
//! - Finish line detection and winner announcement
//! - [`RaceEvent`] emission alongside standard [`PhysicsEvent`]s
//!
//! # Usage
//!
//! ```rust,no_run
//! use rphys_race::{RaceTracker, RaceEvent};
//! use rphys_physics::PhysicsConfig;
//! # use rphys_scene::{Scene, Color, Environment, WorldBounds, WallConfig,
//! #     SceneMeta, SceneAudio, Vec2, RaceConfig};
//! # fn make_race_scene() -> Scene { todo!() }
//!
//! let scene = make_race_scene();
//! let config = PhysicsConfig {
//!     max_steps_per_call: u32::MAX,
//!     ..Default::default()
//! };
//!
//! let mut tracker = RaceTracker::new(&scene, config).unwrap();
//!
//! while !tracker.is_race_complete() && !tracker.is_physics_complete() {
//!     let (phys_events, race_events) = tracker.step().unwrap();
//!     for event in &race_events {
//!         match event {
//!             RaceEvent::RankChanged { new_rankings } => {
//!                 // Update leaderboard display.
//!                 let _ = new_rankings;
//!             }
//!             RaceEvent::RaceComplete { winner } => {
//!                 println!("Winner: {}", winner.display_name);
//!             }
//!             _ => {}
//!         }
//!     }
//! }
//! ```

pub mod tracker;
pub mod types;

pub use tracker::RaceTracker;
pub use types::{FinishedEntry, RaceError, RaceEvent, RaceState, RacerStatus, WinnerInfo};
