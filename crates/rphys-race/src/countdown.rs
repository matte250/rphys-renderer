//! Pre-race countdown state machine.
//!
//! [`CountdownManager`] tracks a "3… 2… 1… GO!" countdown sequence. During the
//! countdown, physics is frozen and the export loop renders countdown text
//! instead of advancing the simulation.

// ── Public types ─────────────────────────────────────────────────────────────

/// State machine for the pre-race countdown.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CountdownState {
    /// Countdown has not started (disabled or not yet reached).
    Inactive,
    /// Countdown is running — numbers displayed (e.g. 3, 2, 1).
    Counting { elapsed_secs: f32 },
    /// "GO!" display phase (0.5 seconds after the last number).
    Go { elapsed_secs: f32 },
    /// Countdown completed; physics may now advance.
    Complete,
}

/// Events emitted by the countdown manager on state transitions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CountdownEvent {
    /// A countdown tick: N seconds remaining (e.g. 3, 2, 1).
    Tick { number: u32 },
    /// "GO!" display phase has started.
    Go,
    /// Countdown finished; race may begin.
    Complete,
}

// ── CountdownManager ─────────────────────────────────────────────────────────

/// Duration of the "GO!" display phase in seconds.
const GO_DURATION_SECS: f32 = 0.5;

/// Drives the pre-race countdown sequence.
///
/// Create with [`CountdownManager::new`] and call [`step`](Self::step) once per
/// frame. When `countdown_secs == 0` the manager starts in [`Complete`](CountdownState::Complete)
/// state and [`step`](Self::step) is a no-op.
#[derive(Debug, Clone)]
pub struct CountdownManager {
    /// Total countdown duration (seconds). 0 = disabled.
    total_secs: u32,
    /// Current state of the countdown.
    state: CountdownState,
    /// Number of the last tick that was emitted (counts down from `total_secs`).
    /// Used to avoid emitting duplicate ticks.
    last_emitted_tick: Option<u32>,
}

impl CountdownManager {
    /// Create a countdown manager with the given duration (seconds).
    ///
    /// If `countdown_secs == 0`, the countdown is disabled and [`step`](Self::step)
    /// will immediately return [`CountdownEvent::Complete`] on the first call.
    pub fn new(countdown_secs: u32) -> Self {
        if countdown_secs == 0 {
            return Self {
                total_secs: 0,
                state: CountdownState::Complete,
                last_emitted_tick: None,
            };
        }

        Self {
            total_secs: countdown_secs,
            state: CountdownState::Inactive,
            last_emitted_tick: None,
        }
    }

    /// Advance the countdown by `dt` seconds.
    ///
    /// Returns an event if a state transition occurred this step. The caller
    /// should check [`is_active`](Self::is_active) to decide whether physics
    /// should remain frozen.
    pub fn step(&mut self, dt: f32) -> Option<CountdownEvent> {
        match self.state {
            CountdownState::Inactive => {
                // Transition: Inactive → Counting (first step starts the countdown).
                self.state = CountdownState::Counting { elapsed_secs: 0.0 };
                let tick_number = self.total_secs;
                self.last_emitted_tick = Some(tick_number);
                Some(CountdownEvent::Tick {
                    number: tick_number,
                })
            }
            CountdownState::Counting { elapsed_secs } => {
                let new_elapsed = elapsed_secs + dt;

                if new_elapsed >= self.total_secs as f32 {
                    // Transition: Counting → Go
                    self.state = CountdownState::Go {
                        elapsed_secs: new_elapsed,
                    };
                    return Some(CountdownEvent::Go);
                }

                self.state = CountdownState::Counting {
                    elapsed_secs: new_elapsed,
                };

                // Check if we should emit a new tick.
                // Tick N is displayed when elapsed is in [total - N, total - N + 1).
                // We count down: total_secs, total_secs-1, ..., 1
                let current_tick = self.total_secs - new_elapsed.floor() as u32;
                let current_tick = current_tick.min(self.total_secs).max(1);

                if self.last_emitted_tick != Some(current_tick) {
                    self.last_emitted_tick = Some(current_tick);
                    return Some(CountdownEvent::Tick {
                        number: current_tick,
                    });
                }

                None
            }
            CountdownState::Go { elapsed_secs } => {
                let new_elapsed = elapsed_secs + dt;
                let go_start = self.total_secs as f32;

                if new_elapsed >= go_start + GO_DURATION_SECS {
                    // Transition: Go → Complete
                    self.state = CountdownState::Complete;
                    return Some(CountdownEvent::Complete);
                }

                self.state = CountdownState::Go {
                    elapsed_secs: new_elapsed,
                };
                None
            }
            CountdownState::Complete => None,
        }
    }

    /// Current countdown state.
    pub fn state(&self) -> CountdownState {
        self.state
    }

    /// Whether the countdown is active (not yet complete).
    ///
    /// Returns `true` during `Inactive`, `Counting`, and `Go` phases.
    /// Returns `false` once `Complete`.
    pub fn is_active(&self) -> bool {
        !matches!(self.state, CountdownState::Complete)
    }

    /// Elapsed time into the countdown (seconds). Returns `0.0` if inactive or complete.
    pub fn elapsed_secs(&self) -> f32 {
        match self.state {
            CountdownState::Counting { elapsed_secs } | CountdownState::Go { elapsed_secs } => {
                elapsed_secs
            }
            _ => 0.0,
        }
    }

    /// The text that should be displayed for the current countdown state.
    ///
    /// Returns `None` when the countdown is `Inactive` or `Complete`.
    pub fn display_text(&self) -> Option<&'static str> {
        match self.state {
            CountdownState::Counting { elapsed_secs } => {
                let tick = self.total_secs - elapsed_secs.floor() as u32;
                let tick = tick.min(self.total_secs).max(1);
                Some(match tick {
                    1 => "1",
                    2 => "2",
                    3 => "3",
                    4 => "4",
                    5 => "5",
                    6 => "6",
                    7 => "7",
                    8 => "8",
                    9 => "9",
                    _ => "...",
                })
            }
            CountdownState::Go { .. } => Some("GO!"),
            _ => None,
        }
    }

    /// Reset to initial state (for testing only).
    #[cfg(test)]
    pub fn reset(&mut self) {
        if self.total_secs == 0 {
            self.state = CountdownState::Complete;
        } else {
            self.state = CountdownState::Inactive;
        }
        self.last_emitted_tick = None;
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_manager_new_disabled() {
        let mgr = CountdownManager::new(0);
        assert_eq!(mgr.state(), CountdownState::Complete);
        assert!(!mgr.is_active());
    }

    #[test]
    fn countdown_manager_new_enabled() {
        let mgr = CountdownManager::new(3);
        assert_eq!(mgr.state(), CountdownState::Inactive);
        assert!(mgr.is_active());
    }

    #[test]
    fn step_disabled_returns_none() {
        let mut mgr = CountdownManager::new(0);
        assert_eq!(mgr.step(1.0 / 60.0), None);
        assert!(!mgr.is_active());
    }

    #[test]
    fn step_generates_correct_tick_numbers() {
        let mut mgr = CountdownManager::new(3);

        // First step: should emit tick 3
        let event = mgr.step(0.0);
        assert_eq!(event, Some(CountdownEvent::Tick { number: 3 }));

        // Advance to t=1.0 — should emit tick 2
        let event = mgr.step(1.0);
        assert_eq!(event, Some(CountdownEvent::Tick { number: 2 }));

        // Advance to t=2.0 — should emit tick 1
        let event = mgr.step(1.0);
        assert_eq!(event, Some(CountdownEvent::Tick { number: 1 }));
    }

    #[test]
    fn go_display_after_counting() {
        let mut mgr = CountdownManager::new(3);
        mgr.step(0.0); // tick 3
        mgr.step(1.0); // tick 2
        mgr.step(1.0); // tick 1

        // Advance past counting phase
        let event = mgr.step(1.0);
        assert_eq!(event, Some(CountdownEvent::Go));
        assert!(matches!(mgr.state(), CountdownState::Go { .. }));
        assert!(mgr.is_active());
    }

    #[test]
    fn go_display_lasts_half_second() {
        let mut mgr = CountdownManager::new(3);
        mgr.step(0.0); // tick 3
        mgr.step(1.0); // tick 2
        mgr.step(1.0); // tick 1
        mgr.step(1.0); // GO!

        // Still in GO phase at 0.4s
        let event = mgr.step(0.4);
        assert_eq!(event, None);
        assert!(mgr.is_active());

        // Complete after 0.5s total
        let event = mgr.step(0.2);
        assert_eq!(event, Some(CountdownEvent::Complete));
        assert!(!mgr.is_active());
        assert_eq!(mgr.state(), CountdownState::Complete);
    }

    #[test]
    fn countdown_complete_after_total_duration() {
        let mut mgr = CountdownManager::new(3);
        let mut events = Vec::new();

        // Step through at 60fps
        let dt = 1.0 / 60.0;
        for _ in 0..300 {
            if let Some(event) = mgr.step(dt) {
                events.push(event);
            }
            if !mgr.is_active() {
                break;
            }
        }

        assert!(!mgr.is_active());

        // Should have: Tick(3), Tick(2), Tick(1), Go, Complete
        assert_eq!(events.len(), 5);
        assert_eq!(events[0], CountdownEvent::Tick { number: 3 });
        assert_eq!(events[1], CountdownEvent::Tick { number: 2 });
        assert_eq!(events[2], CountdownEvent::Tick { number: 1 });
        assert_eq!(events[3], CountdownEvent::Go);
        assert_eq!(events[4], CountdownEvent::Complete);
    }

    #[test]
    fn is_active_reflects_state() {
        let mut mgr = CountdownManager::new(1);
        assert!(mgr.is_active()); // Inactive

        mgr.step(0.0); // tick 1
        assert!(mgr.is_active()); // Counting

        mgr.step(1.0); // GO!
        assert!(mgr.is_active()); // Go

        mgr.step(0.6); // Complete
        assert!(!mgr.is_active()); // Complete
    }

    #[test]
    fn elapsed_secs_tracks_time() {
        let mut mgr = CountdownManager::new(3);
        assert_eq!(mgr.elapsed_secs(), 0.0);

        mgr.step(0.0); // starts counting
        mgr.step(1.5);
        let elapsed = mgr.elapsed_secs();
        assert!((elapsed - 1.5).abs() < 0.01);
    }

    #[test]
    fn display_text_returns_correct_values() {
        let mut mgr = CountdownManager::new(3);
        assert_eq!(mgr.display_text(), None); // Inactive

        mgr.step(0.0); // tick 3
        assert_eq!(mgr.display_text(), Some("3"));

        mgr.step(1.0); // tick 2
        assert_eq!(mgr.display_text(), Some("2"));

        mgr.step(1.0); // tick 1
        assert_eq!(mgr.display_text(), Some("1"));

        mgr.step(1.0); // GO!
        assert_eq!(mgr.display_text(), Some("GO!"));

        mgr.step(0.6); // Complete
        assert_eq!(mgr.display_text(), None);
    }

    #[test]
    fn reset_restores_initial_state() {
        let mut mgr = CountdownManager::new(3);
        mgr.step(0.0);
        mgr.step(1.0);
        mgr.reset();
        assert_eq!(mgr.state(), CountdownState::Inactive);
        assert!(mgr.is_active());
    }

    #[test]
    fn reset_disabled_stays_complete() {
        let mut mgr = CountdownManager::new(0);
        mgr.reset();
        assert_eq!(mgr.state(), CountdownState::Complete);
        assert!(!mgr.is_active());
    }

    #[test]
    fn single_second_countdown() {
        let mut mgr = CountdownManager::new(1);
        let mut events = Vec::new();

        let dt = 1.0 / 60.0;
        for _ in 0..200 {
            if let Some(event) = mgr.step(dt) {
                events.push(event);
            }
            if !mgr.is_active() {
                break;
            }
        }

        // Tick(1), Go, Complete
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], CountdownEvent::Tick { number: 1 });
        assert_eq!(events[1], CountdownEvent::Go);
        assert_eq!(events[2], CountdownEvent::Complete);
    }

    #[test]
    fn no_duplicate_ticks_within_same_second() {
        let mut mgr = CountdownManager::new(3);
        let dt = 1.0 / 60.0;

        let event = mgr.step(0.0); // first step: Tick(3)
        assert!(matches!(event, Some(CountdownEvent::Tick { number: 3 })));

        // Step many times within the first second — no more tick events.
        let mut extra_ticks = 0;
        for _ in 0..50 {
            if let Some(CountdownEvent::Tick { .. }) = mgr.step(dt) {
                extra_ticks += 1;
            }
        }
        assert_eq!(extra_ticks, 0, "should not emit duplicate ticks");
    }
}
