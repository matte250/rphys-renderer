//! Public types for the `rphys-race` crate.
//!
//! These types represent the race-specific state and events produced by
//! [`RaceTracker`](crate::RaceTracker) during a race simulation.

use rphys_physics::{BodyId, PhysicsError};
use rphys_scene::Color;

// ── Race state snapshot ───────────────────────────────────────────────────────

/// Snapshot of the race standings at a point in time.
///
/// Cloneable so the overlay renderer can hold the most recent copy.
#[derive(Debug, Clone)]
pub struct RaceState {
    /// All active (not yet finished) racers, sorted by current rank.
    ///
    /// Index 0 is the leader (rank 1). Rank is determined by Y position:
    /// lower Y = further along the course = better rank.
    pub active: Vec<RacerStatus>,

    /// Racers who have crossed the finish line, in the order they finished.
    pub finished: Vec<FinishedEntry>,

    /// Set to `Some` once the first racer crosses the finish line.
    pub winner: Option<WinnerInfo>,

    /// Elapsed simulation time (seconds) when this snapshot was taken.
    pub elapsed_secs: f32,
}

/// Live status of one active (still-racing) racer.
#[derive(Debug, Clone, PartialEq)]
pub struct RacerStatus {
    /// Stable body identifier.
    pub body_id: BodyId,

    /// The racer's display name (from `SceneObject::name`).
    ///
    /// Falls back to `"Racer {id}"` if the scene object has no name.
    pub display_name: String,

    /// The racer's color from the scene object.
    pub color: Color,

    /// Current rank among active racers (1-based).
    ///
    /// Rank 1 is the leader. Determined solely by Y position.
    pub rank: usize,

    /// Current world Y position.
    pub position_y: f32,

    /// Index of the last checkpoint this racer crossed (0-based into
    /// `RaceConfig::checkpoints`).
    ///
    /// `None` if the racer has not yet crossed any checkpoint.
    pub last_checkpoint: Option<usize>,
}

/// A racer who has crossed the finish line.
#[derive(Debug, Clone, PartialEq)]
pub struct FinishedEntry {
    /// Stable body identifier.
    pub body_id: BodyId,
    /// The racer's display name.
    pub display_name: String,
    /// The racer's color.
    pub color: Color,
    /// Final rank in the race (1 = winner, 2 = runner-up, …).
    pub finish_rank: usize,
    /// Simulation time (seconds) when they crossed the finish line.
    pub finish_time_secs: f32,
}

/// Winner information, set once the first racer finishes.
///
/// Used by the overlay renderer for the winner announcement panel.
#[derive(Debug, Clone, PartialEq)]
pub struct WinnerInfo {
    /// Stable body identifier of the winner.
    pub body_id: BodyId,
    /// The winner's display name.
    pub display_name: String,
    /// The winner's color.
    pub color: Color,
    /// Simulation time (seconds) when the winner crossed the finish line.
    pub finish_time_secs: f32,
}

// ── Race events ───────────────────────────────────────────────────────────────

/// Events emitted by [`RaceTracker`](crate::RaceTracker) in addition to the
/// standard [`PhysicsEvent`](rphys_physics::PhysicsEvent)s.
#[derive(Debug, Clone)]
pub enum RaceEvent {
    /// The leaderboard order changed — at least one rank swap occurred this step.
    RankChanged {
        /// New rankings after the change: `(body_id, rank)` pairs.
        new_rankings: Vec<(BodyId, usize)>,
    },

    /// A racer crossed a checkpoint for the first time.
    CheckpointCrossed {
        /// The racer who crossed the checkpoint.
        body_id: BodyId,
        /// The racer's display name.
        display_name: String,
        /// Zero-based index into `RaceConfig::checkpoints`.
        checkpoint_index: usize,
        /// World Y coordinate of the crossed checkpoint.
        checkpoint_y: f32,
        /// The racer's rank at the moment of crossing.
        rank_at_crossing: usize,
    },

    /// A racer's position reached or passed the finish line.
    RacerFinished {
        /// The racer who finished.
        body_id: BodyId,
        /// The racer's display name.
        display_name: String,
        /// Finish position (1 = winner).
        finish_rank: usize,
        /// Simulation time when they finished (seconds).
        finish_time_secs: f32,
    },

    /// The race is complete — the first racer has crossed the finish line.
    RaceComplete {
        /// Information about the winner.
        winner: WinnerInfo,
    },
}

// ── Error type ────────────────────────────────────────────────────────────────

/// Errors produced by [`RaceTracker`](crate::RaceTracker).
#[derive(Debug, thiserror::Error)]
pub enum RaceError {
    /// The scene has no `race:` configuration section.
    #[error("Scene has no race configuration (missing 'race:' section)")]
    NoRaceConfig,

    /// A physics engine error occurred.
    #[error("Physics error: {0}")]
    Physics(#[from] PhysicsError),
}
