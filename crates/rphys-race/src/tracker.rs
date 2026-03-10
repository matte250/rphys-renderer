//! [`RaceTracker`] — wraps `PhysicsEngine` with race-specific state tracking.

use std::collections::{HashMap, HashSet};

use rphys_physics::{BodyId, PhysicsConfig, PhysicsEngine, PhysicsEvent, PhysicsState};
use rphys_scene::{Color, RaceConfig, Scene};

use crate::countdown::{CountdownEvent, CountdownManager, CountdownState};
use crate::types::{FinishedEntry, RaceError, RaceEvent, RaceState, RacerStatus, WinnerInfo};

// ── Internal racer metadata ───────────────────────────────────────────────────

/// Stable metadata for one racer body, derived from the scene at construction.
struct RacerMeta {
    display_name: String,
    color: Color,
}

// ── RaceTracker ───────────────────────────────────────────────────────────────

/// Wraps [`PhysicsEngine`] to add race-specific simulation: rank tracking,
/// checkpoint detection, and finish-line events.
///
/// Callers interact with `RaceTracker` instead of `PhysicsEngine` directly
/// for race scenes.
///
/// # Example
///
/// ```rust,no_run
/// # use rphys_race::RaceTracker;
/// # use rphys_physics::PhysicsConfig;
/// # use rphys_scene::Scene;
/// # fn make_scene() -> Scene { todo!() }
/// let scene = make_scene();
/// let config = PhysicsConfig { max_steps_per_call: u32::MAX, ..Default::default() };
/// let mut tracker = RaceTracker::new(&scene, config).unwrap();
///
/// while !tracker.is_race_complete() && !tracker.is_physics_complete() {
///     let (_phys_events, _race_events) = tracker.step().unwrap();
/// }
/// ```
pub struct RaceTracker {
    /// The underlying physics engine.
    engine: PhysicsEngine,

    /// Race configuration from the scene (cloned for owned access).
    race_config: RaceConfig,

    /// Current race standings. Updated on every `step()` call.
    race_state: RaceState,

    /// Stable metadata (display name, color) for each racer body.
    racer_meta: HashMap<BodyId, RacerMeta>,

    /// Last checkpoint index crossed by each racer.
    ///
    /// Maintained separately from `race_state.active` to persist across
    /// the active/finished transition and avoid searching vecs each step.
    racer_last_checkpoint: HashMap<BodyId, Option<usize>>,

    /// Ranking from the previous step, used to detect rank changes.
    ///
    /// Maps `BodyId → rank` (1-based).
    previous_rankings: HashMap<BodyId, usize>,

    /// Body IDs that have already crossed the finish line.
    finished_ids: HashSet<BodyId>,

    /// `true` after a `SimulationComplete` event arrives from the engine.
    physics_complete: bool,

    /// Time at which the next elimination check should fire.
    ///
    /// Set to `f32::MAX` when elimination mode is disabled (no
    /// `elimination_interval_secs` in the race config).
    next_elimination_time: f32,

    /// Running count of eliminations that have occurred so far (1-based when
    /// reported in events).
    elimination_count: usize,

    /// Pre-race countdown state machine.
    countdown: CountdownManager,
}

impl RaceTracker {
    /// Build a race tracker from a scene that must contain a `race:` config.
    ///
    /// Returns [`RaceError::NoRaceConfig`] if `scene.race` is `None`.
    ///
    /// The provided `config` is used for the physics engine. Internally the
    /// tracker overrides `max_steps_per_call` to `u32::MAX` so that
    /// [`advance_to`](Self::advance_to) works correctly.
    pub fn new(scene: &Scene, config: PhysicsConfig) -> Result<Self, RaceError> {
        let race_config = scene.race.clone().ok_or(RaceError::NoRaceConfig)?;

        // Override max_steps_per_call so advance_to() is not artificially
        // capped. Callers can still use their own value for the timestep, etc.
        let engine_config = PhysicsConfig {
            max_steps_per_call: u32::MAX,
            ..config
        };

        let engine = PhysicsEngine::new(scene, engine_config)?;

        // ── Discover racer bodies from the initial physics state ───────────
        let initial_state = engine.state();

        let mut racer_meta: HashMap<BodyId, RacerMeta> = HashMap::new();
        let mut racer_last_checkpoint: HashMap<BodyId, Option<usize>> = HashMap::new();

        for body in &initial_state.bodies {
            if !body.tags.contains(&race_config.racer_tag) {
                continue;
            }
            let display_name = body
                .name
                .clone()
                .unwrap_or_else(|| format!("Racer {}", body.id.0));

            racer_meta.insert(
                body.id,
                RacerMeta {
                    display_name,
                    color: body.color,
                },
            );
            racer_last_checkpoint.insert(body.id, None);
        }

        // ── Build initial race state (no steps taken yet) ─────────────────
        let active = build_active_list(
            &initial_state,
            &racer_meta,
            &racer_last_checkpoint,
            &HashSet::new(),
        );

        let race_state = RaceState {
            active,
            finished: Vec::new(),
            winner: None,
            elapsed_secs: 0.0,
        };

        // Elimination mode: first check fires at t = interval (or never).
        let next_elimination_time = race_config.elimination_interval_secs.unwrap_or(f32::MAX);

        let countdown = CountdownManager::new(race_config.countdown_seconds);

        Ok(Self {
            engine,
            race_config,
            race_state,
            racer_meta,
            racer_last_checkpoint,
            previous_rankings: HashMap::new(),
            finished_ids: HashSet::new(),
            physics_complete: false,
            next_elimination_time,
            elimination_count: 0,
            countdown,
        })
    }

    // ── Simulation ────────────────────────────────────────────────────────────

    /// Advance physics by exactly one fixed timestep.
    ///
    /// Returns both the standard physics events and any race events that
    /// occurred during this step.
    ///
    /// A no-op (returns empty vecs) if the simulation is already complete.
    pub fn step(&mut self) -> Result<(Vec<PhysicsEvent>, Vec<RaceEvent>), RaceError> {
        if self.physics_complete {
            return Ok((Vec::new(), Vec::new()));
        }

        let physics_events = self.engine.step()?;

        // Check if physics completed this step.
        for event in &physics_events {
            if matches!(event, PhysicsEvent::SimulationComplete { .. }) {
                self.physics_complete = true;
            }
        }

        let race_events = self.update_race_state()?;
        Ok((physics_events, race_events))
    }

    /// Advance simulation until `target_time` is reached, accumulating all events.
    ///
    /// Useful for export mode where multiple physics steps are needed per video frame.
    pub fn advance_to(
        &mut self,
        target_time: f32,
    ) -> Result<(Vec<PhysicsEvent>, Vec<RaceEvent>), RaceError> {
        let mut all_physics = Vec::new();
        let mut all_race = Vec::new();

        while self.engine.time() < target_time && !self.physics_complete {
            let (phys, race) = self.step()?;
            all_physics.extend(phys);
            all_race.extend(race);
        }

        Ok((all_physics, all_race))
    }

    // ── State accessors ───────────────────────────────────────────────────────

    /// Snapshot the current physics world for the renderer.
    pub fn physics_state(&self) -> PhysicsState {
        self.engine.state()
    }

    /// Current race standings.
    pub fn race_state(&self) -> &RaceState {
        &self.race_state
    }

    /// Borrow the inner [`PhysicsEngine`] (for `body_info` lookups, etc.).
    pub fn engine(&self) -> &PhysicsEngine {
        &self.engine
    }

    /// Current physics time in seconds.
    pub fn time(&self) -> f32 {
        self.engine.time()
    }

    /// `true` after a `SimulationComplete` event has been emitted by the engine.
    pub fn is_physics_complete(&self) -> bool {
        self.physics_complete
    }

    /// `true` after the first racer has crossed the finish line.
    pub fn is_race_complete(&self) -> bool {
        self.race_state.winner.is_some()
    }

    // ── Countdown ──────────────────────────────────────────────────────────

    /// Step the countdown and return any event.
    ///
    /// This is called by the export loop *instead of* `advance_to()` during
    /// the countdown phase. Physics is frozen during this period.
    pub fn step_countdown(&mut self, dt: f32) -> Option<CountdownEvent> {
        self.countdown.step(dt)
    }

    /// Current countdown state.
    pub fn countdown_state(&self) -> CountdownState {
        self.countdown.state()
    }

    /// The text to display for the current countdown state, if any.
    pub fn countdown_display_text(&self) -> Option<&'static str> {
        self.countdown.display_text()
    }

    // ── Private: race state update ────────────────────────────────────────────

    /// Update race state from the engine's current snapshot.
    ///
    /// Called once per `step()`. Returns all race events that occurred.
    fn update_race_state(&mut self) -> Result<Vec<RaceEvent>, RaceError> {
        let mut race_events: Vec<RaceEvent> = Vec::new();
        let current_time = self.engine.time();
        let phys_state = self.engine.state();

        // ── 1. Collect live racer positions ──────────────────────────────
        // (alive bodies with the racer tag that have not yet finished)
        let mut racer_positions: Vec<(BodyId, f32)> = phys_state
            .bodies
            .iter()
            .filter(|body| {
                body.is_alive
                    && body.tags.contains(&self.race_config.racer_tag)
                    && !self.finished_ids.contains(&body.id)
            })
            .map(|body| (body.id, body.position.y))
            .collect();

        // ── 2. Sort by Y ascending (lowest Y = furthest along = rank 1) ──
        racer_positions.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        // ── 3. Assign ranks and detect changes ───────────────────────────
        let new_rankings: HashMap<BodyId, usize> = racer_positions
            .iter()
            .enumerate()
            .map(|(idx, (body_id, _))| (*body_id, idx + 1))
            .collect();

        let rankings_changed = new_rankings != self.previous_rankings;
        if rankings_changed && !new_rankings.is_empty() {
            let mut ranking_vec: Vec<(BodyId, usize)> =
                new_rankings.iter().map(|(id, rank)| (*id, *rank)).collect();
            ranking_vec.sort_by_key(|(_, rank)| *rank);
            race_events.push(RaceEvent::RankChanged {
                new_rankings: ranking_vec,
            });
        }
        self.previous_rankings = new_rankings.clone();

        // ── 4. Check checkpoints and finish line ─────────────────────────
        // We iterate racers in rank order (lowest Y first = rank 1 first)
        // to get consistent rank_at_crossing values.
        for (body_id, position_y) in &racer_positions {
            let body_id = *body_id;
            let position_y = *position_y;
            let rank = new_rankings[&body_id];

            // -- Checkpoint detection --
            self.detect_checkpoint_crossings(body_id, position_y, rank, &mut race_events);

            // -- Finish line detection --
            if position_y <= self.race_config.finish_y {
                // Move this racer to finished.
                let finish_rank = self.finished_ids.len() + 1;
                self.finished_ids.insert(body_id);

                let meta = &self.racer_meta[&body_id];
                let entry = FinishedEntry {
                    body_id,
                    display_name: meta.display_name.clone(),
                    color: meta.color,
                    finish_rank,
                    finish_time_secs: current_time,
                };

                race_events.push(RaceEvent::RacerFinished {
                    body_id,
                    display_name: meta.display_name.clone(),
                    finish_rank,
                    finish_time_secs: current_time,
                });

                // First finisher → winner.
                if self.race_state.winner.is_none() {
                    let winner = WinnerInfo {
                        body_id,
                        display_name: meta.display_name.clone(),
                        color: meta.color,
                        finish_time_secs: current_time,
                    };
                    self.race_state.winner = Some(winner.clone());
                    race_events.push(RaceEvent::RaceComplete {
                        winner: winner.clone(),
                    });
                }

                self.race_state.finished.push(entry);
            }
        }

        // ── 5. Elimination round check ───────────────────────────────────
        // Collect active body IDs (bodies that haven't finished yet).
        let active_ids: Vec<BodyId> = racer_positions
            .iter()
            .filter(|(id, _)| !self.finished_ids.contains(id))
            .map(|(id, _)| *id)
            .collect();

        if current_time >= self.next_elimination_time && active_ids.len() > 1 {
            // Find last-place body: highest rank among active racers.
            let last_place_id = active_ids
                .iter()
                .max_by_key(|id| new_rankings.get(id).copied().unwrap_or(0))
                .copied();

            if let Some(body_id) = last_place_id {
                let rank_at_elimination = new_rankings.get(&body_id).copied().unwrap_or(0);

                // Remove from physics engine.
                self.engine.remove_body(body_id);

                // Track as finished so it leaves the active list.
                self.finished_ids.insert(body_id);

                // Add to finished list (they place last among remaining).
                let finish_rank = self.finished_ids.len();
                let meta = &self.racer_meta[&body_id];
                let eliminated_name = meta.display_name.clone();
                let eliminated_color = meta.color;
                self.race_state.finished.push(FinishedEntry {
                    body_id,
                    display_name: eliminated_name.clone(),
                    color: eliminated_color,
                    finish_rank,
                    finish_time_secs: current_time,
                });

                self.elimination_count += 1;
                let elimination_number = self.elimination_count;

                race_events.push(RaceEvent::RacerEliminated {
                    body_id,
                    display_name: eliminated_name,
                    rank_at_elimination,
                    elimination_number,
                });

                // Advance timer for the next round.
                if let Some(interval) = self.race_config.elimination_interval_secs {
                    self.next_elimination_time += interval;
                }
            }
        }

        // ── 6. Rebuild active list from current state ────────────────────
        self.race_state.active = build_active_list(
            &phys_state,
            &self.racer_meta,
            &self.racer_last_checkpoint,
            &self.finished_ids,
        );

        // Apply the new rankings to the active list.
        for racer in &mut self.race_state.active {
            if let Some(&rank) = new_rankings.get(&racer.body_id) {
                racer.rank = rank;
            }
        }

        // Sort active list by rank.
        self.race_state.active.sort_by_key(|r| r.rank);

        self.race_state.elapsed_secs = current_time;

        Ok(race_events)
    }

    /// Check if a racer has crossed any new checkpoints and emit events.
    fn detect_checkpoint_crossings(
        &mut self,
        body_id: BodyId,
        position_y: f32,
        rank: usize,
        race_events: &mut Vec<RaceEvent>,
    ) {
        let meta = &self.racer_meta[&body_id];
        let last_cp = self.racer_last_checkpoint[&body_id];

        for (checkpoint_index, checkpoint) in self.race_config.checkpoints.iter().enumerate() {
            // Has the racer reached this checkpoint's Y?
            if position_y > checkpoint.y {
                continue;
            }

            // Has the racer already been credited for this checkpoint?
            let already_crossed = match last_cp {
                None => false,
                Some(last) => checkpoint_index <= last,
            };

            if already_crossed {
                continue;
            }

            // New checkpoint crossing!
            race_events.push(RaceEvent::CheckpointCrossed {
                body_id,
                display_name: meta.display_name.clone(),
                checkpoint_index,
                checkpoint_y: checkpoint.y,
                rank_at_crossing: rank,
            });

            // Update last_checkpoint to the highest index crossed so far.
            self.racer_last_checkpoint
                .insert(body_id, Some(checkpoint_index));
        }
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Build the `active` list from the current physics state.
///
/// Bodies in `finished_ids` are excluded. Positions and colors are read from
/// `phys_state`; display names and colors come from `racer_meta`. Ranks are
/// set to 0 as a placeholder — the caller applies correct ranks after sorting.
fn build_active_list(
    phys_state: &PhysicsState,
    racer_meta: &HashMap<BodyId, RacerMeta>,
    racer_last_checkpoint: &HashMap<BodyId, Option<usize>>,
    finished_ids: &HashSet<BodyId>,
) -> Vec<RacerStatus> {
    phys_state
        .bodies
        .iter()
        .filter(|body| {
            body.is_alive && racer_meta.contains_key(&body.id) && !finished_ids.contains(&body.id)
        })
        .map(|body| {
            let meta = &racer_meta[&body.id];
            let last_checkpoint = racer_last_checkpoint.get(&body.id).copied().flatten();
            RacerStatus {
                body_id: body.id,
                display_name: meta.display_name.clone(),
                color: meta.color,
                rank: 0, // filled in by caller
                position_y: body.position.y,
                last_checkpoint,
            }
        })
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_scene::{
        BodyType, Checkpoint, Color, EndCondition, Environment, Material, ObjectAudio, RaceConfig,
        SceneAudio, SceneMeta, SceneObject, ShapeKind, Vec2, WallConfig, WorldBounds,
    };

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn default_env() -> Environment {
        Environment {
            gravity: Vec2::new(0.0, -9.81),
            background_color: Color::BLACK,
            world_bounds: WorldBounds {
                width: 20.0,
                height: 40.0,
            },
            walls: WallConfig {
                visible: true,
                color: Color::WHITE,
                thickness: 0.5,
                open_bottom: false,
            },
        }
    }

    fn default_meta() -> SceneMeta {
        SceneMeta {
            name: "race_test".to_string(),
            description: None,
            author: None,
            duration_hint: None,
        }
    }

    fn race_ball(name: &str, x: f32, y: f32, color: Color) -> SceneObject {
        SceneObject {
            name: Some(name.to_string()),
            shape: ShapeKind::Circle { radius: 0.4 },
            position: Vec2::new(x, y),
            velocity: Vec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: BodyType::Dynamic,
            material: Material {
                restitution: 0.1,
                friction: 0.5,
                density: 1.0,
            },
            color,
            tags: vec!["racer".to_string()],
            destructible: None,
            boost: None,
            gravity_well: None,
            bumper: None,
            audio: ObjectAudio::default(),
        }
    }

    fn non_racer_ball(name: &str) -> SceneObject {
        SceneObject {
            name: Some(name.to_string()),
            shape: ShapeKind::Circle { radius: 0.4 },
            position: Vec2::new(10.0, 20.0),
            velocity: Vec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: BodyType::Dynamic,
            material: Material::default(),
            color: Color::WHITE,
            tags: vec!["spectator".to_string()],
            destructible: None,
            boost: None,
            gravity_well: None,
            bumper: None,
            audio: ObjectAudio::default(),
        }
    }

    fn make_race_scene(
        objects: Vec<SceneObject>,
        race_config: RaceConfig,
        end_condition: Option<EndCondition>,
    ) -> Scene {
        Scene {
            version: "1".to_string(),
            meta: default_meta(),
            environment: default_env(),
            objects,
            end_condition,
            audio: SceneAudio::default(),
            race: Some(race_config),
            camera: None,
            vfx: None,
        }
    }

    /// Build a race scene with **zero gravity** so balls stay put for timing
    /// tests (e.g. elimination interval checks that need balls alive for 3–5 s).
    fn make_race_scene_zero_gravity(
        objects: Vec<SceneObject>,
        race_config: RaceConfig,
        end_condition: Option<EndCondition>,
    ) -> Scene {
        Scene {
            version: "1".to_string(),
            meta: default_meta(),
            environment: Environment {
                gravity: Vec2::new(0.0, 0.0),
                ..default_env()
            },
            objects,
            end_condition,
            audio: SceneAudio::default(),
            race: Some(race_config),
            camera: None,
            vfx: None,
        }
    }

    fn make_non_race_scene() -> Scene {
        Scene {
            version: "1".to_string(),
            meta: default_meta(),
            environment: default_env(),
            objects: vec![non_racer_ball("spectator")],
            end_condition: None,
            audio: SceneAudio::default(),
            race: None,
            camera: None,
            vfx: None,
        }
    }

    fn export_config() -> PhysicsConfig {
        PhysicsConfig {
            timestep: 1.0 / 240.0,
            max_steps_per_call: u32::MAX,
        }
    }

    fn simple_race_config() -> RaceConfig {
        RaceConfig {
            finish_y: 2.0,
            racer_tag: "racer".to_string(),
            announcement_hold_secs: 2.0,
            checkpoints: Vec::new(),
            elimination_interval_secs: None,
            post_finish_secs: 0.0,
            countdown_seconds: 0,
        }
    }

    // ── test: NoRaceConfig for non-race scene ─────────────────────────────────

    #[test]
    fn test_no_race_config_returns_error() {
        let scene = make_non_race_scene();
        let result = RaceTracker::new(&scene, export_config());
        assert!(
            matches!(result, Err(RaceError::NoRaceConfig)),
            "expected NoRaceConfig error"
        );
    }

    // ── test: racers identified by tag ────────────────────────────────────────

    #[test]
    fn test_racers_identified_by_tag() {
        let red = race_ball("Red", 5.0, 30.0, Color::rgb(255, 0, 0));
        let blue = race_ball("Blue", 10.0, 30.0, Color::rgb(0, 0, 255));
        let spectator = non_racer_ball("Spec");

        let scene = make_race_scene(vec![red, blue, spectator], simple_race_config(), None);

        let tracker = RaceTracker::new(&scene, export_config()).unwrap();
        let active = &tracker.race_state().active;

        // Only the two racers should be active, not the spectator.
        assert_eq!(
            active.len(),
            2,
            "expected 2 active racers, got {}",
            active.len()
        );

        let names: Vec<&str> = active.iter().map(|r| r.display_name.as_str()).collect();
        assert!(names.contains(&"Red"), "Red should be in active racers");
        assert!(names.contains(&"Blue"), "Blue should be in active racers");
        assert!(!names.contains(&"Spec"), "Spec should not be a racer");
    }

    // ── test: display_name falls back to "Racer {id}" when unnamed ───────────

    #[test]
    fn test_unnamed_racer_gets_fallback_name() {
        let mut unnamed = race_ball("X", 5.0, 30.0, Color::rgb(255, 0, 0));
        unnamed.name = None; // Remove the name.

        let scene = make_race_scene(vec![unnamed], simple_race_config(), None);

        let tracker = RaceTracker::new(&scene, export_config()).unwrap();
        let active = &tracker.race_state().active;

        assert_eq!(active.len(), 1);
        assert!(
            active[0].display_name.starts_with("Racer "),
            "unnamed racer should get fallback name, got: {}",
            active[0].display_name
        );
    }

    // ── test: ranks update as balls fall ─────────────────────────────────────

    #[test]
    fn test_ranks_update_as_balls_fall() {
        // Place Red higher up (higher Y) than Blue.
        // Both fall under gravity. Red starts further from finish so Blue
        // should initially lead (lower Y = lower rank number = better rank).
        let red = race_ball("Red", 5.0, 35.0, Color::rgb(255, 0, 0));
        let blue = race_ball("Blue", 15.0, 25.0, Color::rgb(0, 0, 255));

        let scene = make_race_scene(
            vec![red, blue],
            simple_race_config(),
            Some(EndCondition::TimeLimit { seconds: 0.5 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        // Run for a short time.
        tracker.advance_to(0.1).unwrap();

        let active = tracker.race_state().active.clone();
        assert!(!active.is_empty(), "should have active racers");

        // Verify ranks are 1-based and consistent with Y positions.
        let mut sorted_by_rank: Vec<_> = active.iter().collect();
        sorted_by_rank.sort_by_key(|r| r.rank);

        // Rank 1 should have the lowest Y.
        if sorted_by_rank.len() >= 2 {
            assert!(
                sorted_by_rank[0].position_y <= sorted_by_rank[1].position_y,
                "rank 1 should have lower or equal Y than rank 2: rank1.y={}, rank2.y={}",
                sorted_by_rank[0].position_y,
                sorted_by_rank[1].position_y
            );
        }

        // Initially Blue (starts at y=25.0) should be rank 1, Red (y=35.0) rank 2.
        // After a short time, Blue should still be rank 1 since it starts lower.
        let blue_status = active.iter().find(|r| r.display_name == "Blue").unwrap();
        assert_eq!(
            blue_status.rank, 1,
            "Blue (lower start Y) should be rank 1 initially"
        );
    }

    // ── test: RaceComplete fires when first racer crosses finish ──────────────

    #[test]
    fn test_race_complete_fires_on_first_finisher() {
        // Ball just above the finish line — should cross quickly.
        let fast = race_ball("Fast", 10.0, 3.0, Color::rgb(0, 255, 0));
        // Ball far from finish.
        let slow = race_ball("Slow", 5.0, 35.0, Color::rgb(255, 0, 0));

        let scene = make_race_scene(
            vec![fast, slow],
            RaceConfig {
                finish_y: 2.0,
                racer_tag: "racer".to_string(),
                announcement_hold_secs: 2.0,
                checkpoints: Vec::new(),
                elimination_interval_secs: None,
                post_finish_secs: 0.0,
                countdown_seconds: 0,
            },
            Some(EndCondition::TimeLimit { seconds: 10.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        let mut saw_race_complete = false;
        let mut race_events_all: Vec<RaceEvent> = Vec::new();

        // Run until physics completes or race is done.
        let mut iters = 0;
        loop {
            let (_, race_events) = tracker.step().unwrap();
            for event in &race_events {
                if matches!(event, RaceEvent::RaceComplete { .. }) {
                    saw_race_complete = true;
                }
            }
            race_events_all.extend(race_events);
            iters += 1;
            if tracker.is_race_complete() || tracker.is_physics_complete() || iters > 10_000 {
                break;
            }
        }

        assert!(
            saw_race_complete,
            "expected RaceComplete event, not seen after {iters} steps"
        );
        assert!(
            tracker.is_race_complete(),
            "tracker should report race complete"
        );
    }

    // ── test: WinnerInfo contains correct name and color ──────────────────────

    #[test]
    fn test_winner_info_correct_name_and_color() {
        let winning_color = Color::rgb(0, 200, 100);
        let winner_ball = race_ball("Champion", 10.0, 3.0, winning_color);
        let loser_ball = race_ball("Loser", 5.0, 38.0, Color::rgb(255, 0, 0));

        let scene = make_race_scene(
            vec![winner_ball, loser_ball],
            RaceConfig {
                finish_y: 2.0,
                racer_tag: "racer".to_string(),
                announcement_hold_secs: 2.0,
                checkpoints: Vec::new(),
                elimination_interval_secs: None,
                post_finish_secs: 0.0,
                countdown_seconds: 0,
            },
            Some(EndCondition::TimeLimit { seconds: 10.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        let mut iters = 0;
        while !tracker.is_race_complete() && !tracker.is_physics_complete() && iters < 10_000 {
            tracker.step().unwrap();
            iters += 1;
        }

        assert!(tracker.is_race_complete(), "race should have completed");

        let winner = tracker.race_state().winner.as_ref().unwrap();
        assert_eq!(
            winner.display_name, "Champion",
            "winner should be Champion, got: {}",
            winner.display_name
        );
        assert_eq!(
            winner.color, winning_color,
            "winner color should match scene object color"
        );
        assert!(
            winner.finish_time_secs > 0.0,
            "finish time should be positive"
        );
    }

    // ── test: checkpoint events fire in order ─────────────────────────────────

    #[test]
    fn test_checkpoint_events_fire() {
        // Ball starts high up, checkpoint at y=15.0, finish at y=2.0
        let ball = race_ball("Runner", 10.0, 30.0, Color::rgb(100, 100, 255));

        let scene = make_race_scene(
            vec![ball],
            RaceConfig {
                finish_y: 2.0,
                racer_tag: "racer".to_string(),
                announcement_hold_secs: 2.0,
                checkpoints: vec![Checkpoint {
                    y: 20.0,
                    label: Some("Halfway".to_string()),
                }],
                elimination_interval_secs: None,
                post_finish_secs: 0.0,
                countdown_seconds: 0,
            },
            Some(EndCondition::TimeLimit { seconds: 15.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();
        let mut checkpoint_events: Vec<RaceEvent> = Vec::new();

        let mut iters = 0;
        while !tracker.is_race_complete() && !tracker.is_physics_complete() && iters < 50_000 {
            let (_, race_events) = tracker.step().unwrap();
            for event in race_events {
                if matches!(event, RaceEvent::CheckpointCrossed { .. }) {
                    checkpoint_events.push(event);
                }
            }
            iters += 1;
        }

        // The runner should have crossed the checkpoint at y=20.0.
        assert!(
            !checkpoint_events.is_empty(),
            "expected at least one CheckpointCrossed event"
        );

        if let RaceEvent::CheckpointCrossed {
            checkpoint_index,
            checkpoint_y,
            display_name,
            ..
        } = &checkpoint_events[0]
        {
            assert_eq!(*checkpoint_index, 0);
            assert_eq!(*checkpoint_y, 20.0);
            assert_eq!(display_name, "Runner");
        } else {
            panic!("first checkpoint event has wrong type");
        }

        // Checkpoint should only fire once.
        assert_eq!(
            checkpoint_events.len(),
            1,
            "checkpoint should fire exactly once, fired {}",
            checkpoint_events.len()
        );
    }

    // ── test: advance_to aggregates events ────────────────────────────────────

    #[test]
    fn test_advance_to_aggregates_events() {
        let ball = race_ball("Roller", 10.0, 3.0, Color::rgb(200, 100, 0));

        let scene = make_race_scene(
            vec![ball],
            simple_race_config(),
            Some(EndCondition::TimeLimit { seconds: 5.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        // advance_to should work without panicking and return events.
        let result = tracker.advance_to(1.0);
        assert!(result.is_ok(), "advance_to should succeed");

        let (phys_events, _race_events) = result.unwrap();
        // At minimum some physics steps should have been taken.
        assert!(
            !phys_events.is_empty() || tracker.time() > 0.0,
            "some steps should have been executed"
        );
    }

    // ── test: finished racers move out of active list ─────────────────────────

    #[test]
    fn test_finished_racer_removed_from_active() {
        let ball = race_ball("Sprinter", 10.0, 3.0, Color::rgb(0, 255, 200));

        let scene = make_race_scene(
            vec![ball],
            simple_race_config(),
            Some(EndCondition::TimeLimit { seconds: 10.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        let mut iters = 0;
        while !tracker.is_race_complete() && !tracker.is_physics_complete() && iters < 10_000 {
            tracker.step().unwrap();
            iters += 1;
        }

        if tracker.is_race_complete() {
            assert_eq!(
                tracker.race_state().active.len(),
                0,
                "finished racer should not be in active list"
            );
            assert_eq!(
                tracker.race_state().finished.len(),
                1,
                "finished racer should be in finished list"
            );
        }
    }

    // ── Elimination test helper ───────────────────────────────────────────────
    //
    // Uses finish_y = -100.0 so balls bounce off the floor wall and never
    // cross the finish line.  Elimination drives the race instead.
    fn elimination_race_config(interval_secs: f32) -> RaceConfig {
        RaceConfig {
            finish_y: -100.0,
            racer_tag: "racer".to_string(),
            announcement_hold_secs: 2.0,
            checkpoints: Vec::new(),
            elimination_interval_secs: Some(interval_secs),
            post_finish_secs: 0.0,
            countdown_seconds: 0,
        }
    }

    // ── test: elimination — last-place removed after interval ─────────────────

    #[test]
    fn test_elimination_last_place_removed_after_interval() {
        // Three racers; elimination every 3 seconds.
        // Place them at different heights so ranks are deterministic.
        // Zero gravity keeps balls alive past the 3s elimination window.
        let red = race_ball("Red", 5.0, 35.0, Color::rgb(255, 0, 0));
        let blue = race_ball("Blue", 10.0, 30.0, Color::rgb(0, 0, 255));
        let green = race_ball("Green", 15.0, 25.0, Color::rgb(0, 200, 0));

        let scene = make_race_scene_zero_gravity(
            vec![red, blue, green],
            RaceConfig {
                finish_y: 2.0,
                racer_tag: "racer".to_string(),
                announcement_hold_secs: 2.0,
                checkpoints: Vec::new(),
                elimination_interval_secs: Some(3.0),
                post_finish_secs: 0.0,
                countdown_seconds: 0,
            },
            Some(EndCondition::TimeLimit { seconds: 20.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        let mut eliminated_events: Vec<RaceEvent> = Vec::new();

        // Run past the first 3-second elimination window.
        let mut iters = 0;
        while tracker.time() < 3.5 && !tracker.is_physics_complete() && iters < 100_000 {
            let (_, race_events) = tracker.step().unwrap();
            for event in race_events {
                if matches!(event, RaceEvent::RacerEliminated { .. }) {
                    eliminated_events.push(event);
                }
            }
            iters += 1;
        }

        assert!(
            !eliminated_events.is_empty(),
            "expected at least one RacerEliminated event after 3 s, got none after {iters} steps"
        );

        // After elimination, active count should be 2.
        let active_count = tracker.race_state().active.len();
        let finished_count = tracker.race_state().finished.len();
        assert!(
            active_count <= 2,
            "expected ≤2 active racers after elimination, got {active_count}"
        );
        assert!(
            finished_count >= 1,
            "expected ≥1 in finished list after elimination, got {finished_count}"
        );
    }

    // ── test: eliminated racer moves from active to finished ─────────────────

    #[test]
    fn test_eliminated_racer_moves_to_finished() {
        // Zero gravity keeps balls alive past the 3s elimination window.
        let red = race_ball("Red", 5.0, 35.0, Color::rgb(255, 0, 0));
        let blue = race_ball("Blue", 10.0, 25.0, Color::rgb(0, 0, 255));

        let scene = make_race_scene_zero_gravity(
            vec![red, blue],
            RaceConfig {
                finish_y: 2.0,
                racer_tag: "racer".to_string(),
                announcement_hold_secs: 2.0,
                checkpoints: Vec::new(),
                elimination_interval_secs: Some(3.0),
                post_finish_secs: 0.0,
                countdown_seconds: 0,
            },
            Some(EndCondition::TimeLimit { seconds: 20.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        // Both start as active.
        assert_eq!(
            tracker.race_state().active.len(),
            2,
            "should start with 2 active racers"
        );
        assert_eq!(
            tracker.race_state().finished.len(),
            0,
            "should start with 0 finished"
        );

        // Advance past first elimination.
        let mut eliminated = false;
        let mut iters = 0;
        while tracker.time() < 4.0 && !tracker.is_physics_complete() && iters < 200_000 {
            let (_, race_events) = tracker.step().unwrap();
            for event in &race_events {
                if matches!(event, RaceEvent::RacerEliminated { .. }) {
                    eliminated = true;
                }
            }
            iters += 1;
        }

        assert!(eliminated, "expected an elimination to occur");

        // After elimination: 1 active, 1 finished (as long as no one crossed the line).
        let state = tracker.race_state();
        assert!(
            state.finished.len() >= 1,
            "eliminated racer should be in finished list"
        );
        // Total tracked should still be 2.
        assert_eq!(
            state.active.len() + state.finished.len(),
            2,
            "total racers (active + finished) should still equal 2"
        );
    }

    // ── test: no elimination when only 1 racer remains ───────────────────────

    #[test]
    fn test_no_elimination_with_single_racer() {
        // Single racer — elimination should never fire (guard: active.len() > 1).
        let solo = race_ball("Solo", 10.0, 25.0, Color::rgb(200, 100, 0));

        let scene = make_race_scene(
            vec![solo],
            elimination_race_config(1.0),
            Some(EndCondition::TimeLimit { seconds: 5.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        let mut eliminated_events: Vec<RaceEvent> = Vec::new();
        let mut iters = 0;
        while tracker.time() < 5.0 && !tracker.is_physics_complete() && iters < 500_000 {
            let (_, race_events) = tracker.step().unwrap();
            for event in race_events {
                if matches!(event, RaceEvent::RacerEliminated { .. }) {
                    eliminated_events.push(event);
                }
            }
            iters += 1;
        }

        assert!(
            eliminated_events.is_empty(),
            "should never eliminate when only 1 racer remains, but got {} events",
            eliminated_events.len()
        );
    }

    // ── test: next_elimination_time advances after each round ────────────────

    #[test]
    fn test_next_elimination_time_advances() {
        // Use 4 racers and a 2s interval so two eliminations can occur.
        // finish_y = -100 so no one crosses the finish line.
        let red = race_ball("Red", 3.0, 38.0, Color::rgb(255, 0, 0));
        let blue = race_ball("Blue", 7.0, 30.0, Color::rgb(0, 0, 255));
        let green = race_ball("Green", 11.0, 20.0, Color::rgb(0, 200, 0));
        let yellow = race_ball("Yellow", 15.0, 10.0, Color::rgb(255, 220, 0));

        let scene = make_race_scene(
            vec![red, blue, green, yellow],
            elimination_race_config(2.0),
            Some(EndCondition::TimeLimit { seconds: 30.0 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();

        let mut elimination_count = 0usize;
        let mut iters = 0;

        // Run past two elimination windows.
        while tracker.time() < 5.0 && !tracker.is_physics_complete() && iters < 500_000 {
            let (_, race_events) = tracker.step().unwrap();
            for event in &race_events {
                if let RaceEvent::RacerEliminated {
                    elimination_number, ..
                } = event
                {
                    elimination_count += 1;
                    assert_eq!(
                        *elimination_number, elimination_count,
                        "elimination_number should be 1-based and match count"
                    );
                }
            }
            iters += 1;
        }

        // At least 2 eliminations (at ~2s and ~4s).
        assert!(
            elimination_count >= 2,
            "expected ≥2 eliminations with 2s interval over 5s, got {elimination_count}"
        );
    }

    // ── test: is_physics_complete reflects engine state ───────────────────────

    #[test]
    fn test_is_physics_complete() {
        let scene = make_race_scene(
            vec![race_ball("A", 10.0, 20.0, Color::rgb(100, 100, 100))],
            simple_race_config(),
            Some(EndCondition::TimeLimit { seconds: 0.1 }),
        );

        let mut tracker = RaceTracker::new(&scene, export_config()).unwrap();
        assert!(
            !tracker.is_physics_complete(),
            "should not be complete at start"
        );

        tracker.advance_to(1.0).unwrap();
        assert!(
            tracker.is_physics_complete(),
            "should be complete after time limit"
        );
    }
}
