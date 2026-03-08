//! `rphys-race` — Race state tracking for multi-ball race scenes.
//!
//! This crate provides [`RaceTracker`], [`RaceState`], and related types used
//! to drive race-mode simulations. The [`RaceTracker`] wraps a
//! [`PhysicsEngine`](rphys_physics::PhysicsEngine) and enriches each step with
//! rank tracking, checkpoint detection, and finish-line events.

mod types;

pub use types::{
    FinishedEntry, RaceError, RaceEvent, RaceState, RacerStatus, WinnerInfo,
};
