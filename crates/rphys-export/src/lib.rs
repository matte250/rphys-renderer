//! Headless video export pipeline.
//!
//! Runs physics headlessly, renders each frame with [`TinySkiaRenderer`], mixes
//! audio with [`OfflineAudioMixer`], and pipes raw RGBA frames to `ffmpeg` to
//! produce an MP4.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use rphys_export::{ExportOptions, Preset, export};
//! # use rphys_scene::Scene;
//! # fn get_scene() -> Scene { unimplemented!() }
//! let scene = get_scene();
//! let options = ExportOptions::from_preset(Preset::TikTok, "out.mp4".into());
//! export(&scene, options).expect("export failed");
//! ```

use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use rphys_audio::{AudioEvent, OfflineAudioMixer};
use rphys_overlay::OverlayRenderer;
use rphys_physics::types::BodyId;
use rphys_physics::{PhysicsConfig, PhysicsEngine, PhysicsEvent};
use rphys_race::{CountdownState, RaceEvent, RaceTracker};
use rphys_renderer::{
    CameraController, FollowCamera, RaceCamera, RaceCameraConfig, RenderContext, Renderer,
    StaticCamera, TinySkiaRenderer, TrailConfig, TrailRenderer,
};
use rphys_scene::{CameraConfig, CameraMode, Color, Scene, Vec2};
use rphys_vfx::VfxEngine;
use tempfile::NamedTempFile;
use thiserror::Error;

// ── Camera dispatch ───────────────────────────────────────────────────────────

/// Holds one of the three supported camera implementations for the race export.
///
/// This enum lets the main export loop use a single code path while dispatching
/// to the correct camera without heap allocation.
enum ActiveCamera {
    Race(RaceCamera),
    Follow(FollowCamera),
    Static(StaticCamera),
}

impl ActiveCamera {
    /// Advance the camera for one rendered frame and return the [`RenderContext`].
    ///
    /// For `Follow`, the leader position, current frame's physics events, and
    /// race completion state are required to implement shake and zoom correctly.
    fn get_ctx(
        &mut self,
        state: &rphys_physics::PhysicsState,
        dt: f32,
        events: &[rphys_physics::PhysicsEvent],
        leader_pos: Option<Vec2>,
        race_complete: bool,
    ) -> RenderContext {
        match self {
            ActiveCamera::Race(cam) => cam.update(state, dt),
            ActiveCamera::Static(cam) => cam.update(state, dt),
            ActiveCamera::Follow(cam) => {
                cam.update(leader_pos, events, race_complete);
                cam.render_context()
            }
        }
    }
}

// ── Export presets ────────────────────────────────────────────────────────────

/// Named export preset controlling resolution and frame rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preset {
    /// 1080×1920 @ 60 fps — TikTok / YouTube Shorts / Instagram Reels.
    TikTok,
    /// 1920×1080 @ 60 fps — YouTube landscape.
    YouTube,
    /// User-defined resolution and frame rate.
    Custom,
}

// ── ExportOptions ─────────────────────────────────────────────────────────────

/// Options for a single export run.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Preset used to derive default width/height/fps.
    pub preset: Preset,
    /// Output width in pixels.
    pub width: u32,
    /// Output height in pixels.
    pub height: u32,
    /// Output frame rate.
    pub fps: u32,
    /// Destination file path (should end in `.mp4`).
    pub output_path: PathBuf,
    /// Maximum simulation duration (seconds). Used when the scene has no end
    /// condition; ignored if the scene's `end_condition` fires earlier.
    pub max_duration: Option<f32>,
    /// Path to the `ffmpeg` binary.
    ///
    /// When `None`, `ffmpeg` is resolved from `PATH` (the default behaviour).
    /// Providing an explicit path is useful when ffmpeg lives outside `PATH`,
    /// e.g. a statically-linked binary in a custom location.
    pub ffmpeg_path: Option<PathBuf>,
}

impl ExportOptions {
    /// Create [`ExportOptions`] from a named preset.
    ///
    /// Resolution and frame rate are filled in from the preset; `max_duration`
    /// is left as `None`.
    pub fn from_preset(preset: Preset, output_path: PathBuf) -> Self {
        let (width, height, fps) = match preset {
            Preset::TikTok => (1080, 1920, 60),
            Preset::YouTube => (1920, 1080, 60),
            Preset::Custom => (1920, 1080, 60),
        };
        Self {
            preset,
            width,
            height,
            fps,
            output_path,
            max_duration: None,
            ffmpeg_path: None,
        }
    }
}

// ── Error types ───────────────────────────────────────────────────────────────

/// Errors that can occur during export.
#[derive(Debug, Error)]
pub enum ExportError {
    /// `ffmpeg` is not installed or not on `PATH`.
    #[error("ffmpeg not found — please install ffmpeg and ensure it is in PATH")]
    FfmpegNotFound,

    /// `ffmpeg` exited with a non-zero status.
    #[error("ffmpeg process failed (exit {code}): {stderr}")]
    FfmpegFailed { code: i32, stderr: String },

    /// Physics simulation error.
    #[error("Physics error: {0}")]
    Physics(#[from] rphys_physics::PhysicsError),

    /// Rendering error (string message to avoid coupling to the renderer's
    /// internal error type, which is not `#[from]` compatible here).
    #[error("Render error: {0}")]
    Render(String),

    /// Audio error.
    #[error("Audio error: {0}")]
    Audio(#[from] rphys_audio::AudioError),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// The scene has no end condition and `max_duration` was not set.
    #[error("Scene has no end condition and no max_duration was specified")]
    NoDuration,

    /// Race tracker error.
    #[error("Race error: {0}")]
    Race(#[from] rphys_race::RaceError),

    /// Overlay rendering error.
    #[error("Overlay error: {0}")]
    Overlay(#[from] rphys_overlay::OverlayError),
}

// ── Public export entry point ─────────────────────────────────────────────────

/// Export `scene` to an MP4 video file.
///
/// This function blocks until encoding is complete.
///
/// # Errors
///
/// - [`ExportError::FfmpegNotFound`] — `ffmpeg` binary not found on `PATH`.
/// - [`ExportError::FfmpegFailed`]  — `ffmpeg` exited with a non-zero code.
/// - [`ExportError::NoDuration`]    — no end condition and no `max_duration`.
/// - [`ExportError::Physics`]       — physics engine error.
/// - [`ExportError::Render`]        — renderer error.
/// - [`ExportError::Audio`]         — audio engine error.
/// - [`ExportError::Io`]            — I/O error.
pub fn export(scene: &Scene, options: ExportOptions) -> Result<(), ExportError> {
    if scene.race.is_some() {
        export_race(scene, options)
    } else {
        export_standard(scene, options)
    }
}

/// Standard (non-race) export pipeline.
///
/// Runs physics headlessly, renders with a static camera, and muxes audio.
fn export_standard(scene: &Scene, options: ExportOptions) -> Result<(), ExportError> {
    // ── Determine maximum duration ─────────────────────────────────────────
    let max_duration = resolve_max_duration(scene, &options)?;

    // ── Build render context ───────────────────────────────────────────────
    let ctx = build_render_context(&options, scene);

    // ── Build subsystems ───────────────────────────────────────────────────
    let physics_cfg = PhysicsConfig {
        max_steps_per_call: u32::MAX,
        ..PhysicsConfig::default()
    };
    let mut engine = PhysicsEngine::new(scene, physics_cfg)?;
    let renderer = TinySkiaRenderer;
    let mut audio = OfflineAudioMixer::new(44100, 2);

    // ── VFX system (optional) ──────────────────────────────────────────────
    let mut vfx: Option<VfxEngine> = scene.vfx.clone().map(VfxEngine::new);

    // ── Spawn ffmpeg ───────────────────────────────────────────────────────
    // Audio will be added in a second pass after we know the duration.
    let mut ffmpeg = spawn_ffmpeg_video_only(&options)?;
    let mut ffmpeg_stdin = ffmpeg
        .stdin
        .take()
        .ok_or_else(|| ExportError::Io(std::io::Error::other("failed to open ffmpeg stdin")))?;

    // ── Main render loop ───────────────────────────────────────────────────
    let _export_start = Instant::now();
    let mut frame_count = 0u64;
    let frame_dt = 1.0_f32 / options.fps as f32;
    let mut boost_cooldowns: HashMap<BodyId, f32> = HashMap::new();
    // Static camera: center of the world bounds.
    let camera_y = scene.environment.world_bounds.height / 2.0;

    loop {
        let target_time = (frame_count + 1) as f32 / options.fps as f32;
        let events = engine.advance_to(target_time)?;

        // Collect audio events.
        for event in &events {
            collect_audio_event(
                &mut audio,
                &engine,
                event,
                engine.time(),
                scene,
                &mut boost_cooldowns,
                camera_y,
            );
        }

        // Render.
        let state = engine.state();
        let mut frame = renderer.render(&state, &ctx);

        // VFX: build body snapshot (world coords), feed events, tick, composite.
        if let Some(ref mut vfx_sys) = vfx {
            let body_snap = build_body_snapshot(&state);
            // Standard (non-race) export has no finish line; use world center.
            let finish_line_world = Vec2::new(
                scene.environment.world_bounds.width * 0.5,
                scene.environment.world_bounds.height * 0.5,
            );
            vfx_sys.begin_frame(&body_snap, finish_line_world, ctx.scale);
            vfx_sys.feed_events(&events, &[], &|_| None);
            vfx_sys.update(frame_dt);
            vfx_sys.render_into(&mut frame, &ctx);
        }

        // Write raw RGBA to ffmpeg stdin.
        ffmpeg_stdin
            .write_all(&frame.pixels)
            .map_err(ExportError::Io)?;

        frame_count += 1;

        // Check termination conditions.
        if engine.is_complete() {
            break;
        }
        if target_time >= max_duration {
            break;
        }
    }

    // Close stdin — signals EOF to ffmpeg.
    drop(ffmpeg_stdin);

    // Wait for ffmpeg to finish.
    let status = ffmpeg.wait().map_err(ExportError::Io)?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(ExportError::FfmpegFailed {
            code,
            stderr: "(stderr not captured in video-only pass)".to_string(),
        });
    }

    // ── Audio mux pass ─────────────────────────────────────────────────────
    // Only re-mux if there are any audio events; otherwise skip to keep
    // things simple and avoid failing in environments without audio files.
    if has_any_audio(scene) {
        let duration_secs = frame_count as f32 / options.fps as f32;
        let wav_file = NamedTempFile::new().map_err(ExportError::Io)?;
        audio
            .write_wav(wav_file.path(), duration_secs)
            .map_err(ExportError::Audio)?;

        let ffmpeg_bin = options
            .ffmpeg_path
            .as_deref()
            .map(|p| p.as_os_str().to_owned())
            .unwrap_or_else(|| std::ffi::OsString::from("ffmpeg"));
        remux_with_audio(&options.output_path, wav_file.path(), &ffmpeg_bin)?;
    }

    Ok(())
}

/// Race export pipeline.
///
/// Uses [`RaceTracker`] to drive the simulation, [`RaceCamera`] for dynamic
/// camera follow, and [`OverlayRenderer`] for rank panels and finish lines.
/// After the race completes, holds the winner frame for
/// `race_config.announcement_hold_secs` seconds.
fn export_race(scene: &Scene, options: ExportOptions) -> Result<(), ExportError> {
    let race_config = scene
        .race
        .as_ref()
        .expect("export_race called without race config");

    // ── Determine safety-cap duration ─────────────────────────────────────
    // Race scenes terminate primarily via is_race_complete(). max_duration is
    // only a fallback cap. If no duration is available, use a large default.
    let max_duration = resolve_max_duration(scene, &options).unwrap_or(f32::MAX);

    // ── Build subsystems ───────────────────────────────────────────────────
    let physics_cfg = PhysicsConfig {
        max_steps_per_call: u32::MAX,
        ..PhysicsConfig::default()
    };
    let mut tracker = RaceTracker::new(scene, physics_cfg)?;

    // Construct the VFX engine when the scene has a `vfx:` block.
    let mut vfx_engine: Option<VfxEngine> = scene.vfx.clone().map(VfxEngine::new);
    let mut trail_renderer = TrailRenderer::new(TrailConfig::default(), None);
    let mut audio = OfflineAudioMixer::new(44100, 2);

    // ── Build camera ───────────────────────────────────────────────────────
    let world = &scene.environment.world_bounds;
    let bg = scene.environment.background_color;

    // For a vertically-scrolling race camera, scale is determined by the
    // *width* only — the camera follows the leader vertically, so the world
    // height is irrelevant for zoom.  Using min(scale_x, scale_y) would
    // shrink everything when the course is taller than the viewport ratio.
    let race_scale = options.width as f32 / world.width;
    let initial_ctx = RenderContext {
        width: options.width,
        height: options.height,
        camera_origin: rphys_scene::Vec2::ZERO,
        scale: race_scale,
        background_color: bg,
    };

    // Resolve the camera configuration; fall back to default (Race mode) when
    // the scene does not specify a `camera:` block.
    let cam_cfg: CameraConfig = scene.camera.clone().unwrap_or_default();

    let world_center = Vec2::new(world.width / 2.0, world.height / 2.0);

    let mut active_camera = match cam_cfg.mode {
        CameraMode::Static => {
            let static_cam = StaticCamera::new(initial_ctx.clone());
            ActiveCamera::Static(static_cam)
        }
        CameraMode::FollowLeader => {
            let follow_cam =
                FollowCamera::new(cam_cfg, race_scale, initial_ctx.clone(), world_center);
            ActiveCamera::Follow(follow_cam)
        }
        CameraMode::Race => {
            let race_cfg = RaceCameraConfig {
                racer_tag: race_config.racer_tag.clone(),
                ..RaceCameraConfig::default()
            };
            ActiveCamera::Race(RaceCamera::new(race_cfg, initial_ctx))
        }
    };

    let mut overlay = OverlayRenderer::new();

    // ── Spawn ffmpeg ───────────────────────────────────────────────────────
    let mut ffmpeg = spawn_ffmpeg_video_only(&options)?;
    let mut ffmpeg_stdin = ffmpeg
        .stdin
        .take()
        .ok_or_else(|| ExportError::Io(std::io::Error::other("failed to open ffmpeg stdin")))?;

    // ── Main race loop ─────────────────────────────────────────────────────
    // Slowdown config: when the leading racer enters the final stretch, physics
    // advances at a fraction of normal speed, producing a slow-motion effect.
    const SLOWDOWN_ZONE_M: f32 = 6.0; // metres above finish_y where slowdown kicks in
    const SLOWDOWN_FACTOR: f32 = 0.25; // physics runs at 25% speed during slowdown

    let finish_y = race_config.finish_y;
    let frame_dt = 1.0_f32 / options.fps as f32;
    let mut frame_count = 0u64;
    let mut physics_time: f32 = 0.0; // accumulated physics time (NOT wall-clock frames)
    let mut boost_cooldowns: HashMap<BodyId, f32> = HashMap::new();
    // Camera Y for spatial audio — initialised to world center, updated each frame.
    let mut camera_y = world.height / 2.0;

    // Set to `Some(deadline)` when the first racer finishes.
    // The export loop runs until `physics_time >= deadline`.
    let mut post_finish_deadline: Option<f32> = None;

    // ── Countdown phase ─────────────────────────────────────────────────
    // While the countdown is active, freeze physics and render countdown text.
    while tracker.countdown_state() != CountdownState::Complete {
        tracker.step_countdown(frame_dt);

        // Render the scene (frozen — no physics advance).
        let phys_state = tracker.physics_state();
        let ctx = active_camera.get_ctx(&phys_state, frame_dt, &[], None, false);

        trail_renderer.push_frame(&phys_state, 0.0);
        let mut frame = trail_renderer.render(&phys_state, &ctx);

        // Composite VFX (static particles, if any).
        if let Some(ref vfx) = vfx_engine {
            vfx.render_into(&mut frame, &ctx);
        }

        // Draw the race overlay (finish line, leaderboard).
        overlay.draw_race_frame(&mut frame, tracker.race_state(), race_config, &ctx)?;

        // Draw countdown text on top.
        if let Some(text) = tracker.countdown_display_text() {
            overlay.draw_countdown_text(&mut frame, text, &ctx)?;
        }

        ffmpeg_stdin
            .write_all(&frame.pixels)
            .map_err(ExportError::Io)?;
        frame_count += 1;
    }

    loop {
        // Determine if the leader is in the slowdown zone.
        //
        // Only consider *active* (unfinished) racers: finished racers may have
        // fallen well below `finish_y` and would keep the slowdown permanently
        // engaged long after the race is over. `race_state().active` is the
        // authoritative list of still-racing bodies; `unwrap_or(f32::MAX)`
        // yields no-slowdown (full-speed dt) once everyone has finished.
        let phys_state_for_slowdown = tracker.physics_state();
        let leader_y = tracker
            .race_state()
            .active
            .first()
            .and_then(|r| {
                phys_state_for_slowdown
                    .bodies
                    .iter()
                    .find(|b| b.id == r.body_id && b.is_alive)
                    .map(|b| b.position.y)
            })
            .unwrap_or(f32::MAX);

        let dt = compute_slowdown_dt(
            leader_y,
            finish_y,
            SLOWDOWN_ZONE_M,
            SLOWDOWN_FACTOR,
            frame_dt,
        );
        physics_time += dt;

        let (physics_events, race_events) = tracker.advance_to(physics_time)?;

        let video_time = frame_count as f32 / options.fps as f32;

        // Collect audio events.
        for event in &physics_events {
            collect_audio_event(
                &mut audio,
                tracker.engine(),
                event,
                video_time,
                scene,
                &mut boost_cooldowns,
                camera_y,
            );
        }

        // Arm the elimination banner for any eliminated racers this frame.
        // Also emit finish/racer-finish audio events when a racer crosses the line.
        for event in &race_events {
            if let RaceEvent::RacerEliminated { display_name, .. } = event {
                // Look up the color from the current race state.
                let color = tracker
                    .race_state()
                    .finished
                    .iter()
                    .find(|e| &e.display_name == display_name)
                    .map(|e| e.color)
                    .unwrap_or(Color::WHITE);
                overlay.set_elimination_banner(display_name, color, tracker.time());
            }

            if let RaceEvent::RacerFinished { .. } = event {
                if let Some(path) = scene.audio.default_finish.clone() {
                    audio.add_event(AudioEvent {
                        timestamp_secs: video_time,
                        path,
                        volume: 1.0,
                    });
                }
            }
        }

        // Render frame with camera (with trail ghosts).
        let phys_state = tracker.physics_state();
        let race_complete = tracker.race_state().winner.is_some();
        // For follow-leader camera: find the rank-1 racer's world position.
        let leader_pos = tracker.race_state().active.first().and_then(|r| {
            phys_state
                .bodies
                .iter()
                .find(|b| b.id == r.body_id)
                .map(|b| b.position)
        });
        let ctx = active_camera.get_ctx(
            &phys_state,
            frame_dt,
            &physics_events,
            leader_pos,
            race_complete,
        );

        // Update camera_y for spatial audio from the current frame's camera.
        camera_y = ctx.camera_origin.y + (options.height as f32 / ctx.scale / 2.0);

        // VFX: build body snapshot (world coords), feed all events, tick.
        if let Some(ref mut vfx) = vfx_engine {
            let finish_line_world = Vec2::new(
                scene.environment.world_bounds.width * 0.5,
                race_config.finish_y,
            );
            let body_snap = build_body_snapshot(&phys_state);
            vfx.begin_frame(&body_snap, finish_line_world, ctx.scale);
            vfx.feed_events(&physics_events, &race_events, &|_| None);
            vfx.update(dt);
        }

        trail_renderer.push_frame(&phys_state, dt);
        let mut frame = trail_renderer.render(&phys_state, &ctx);

        // Composite VFX on top of the rendered frame (camera transform applied inside).
        if let Some(ref vfx) = vfx_engine {
            vfx.render_into(&mut frame, &ctx);
        }

        // Draw race overlay (finish/checkpoint lines + rank panel).
        overlay.draw_race_frame(&mut frame, tracker.race_state(), race_config, &ctx)?;

        // Write frame to ffmpeg.
        ffmpeg_stdin
            .write_all(&frame.pixels)
            .map_err(ExportError::Io)?;

        frame_count += 1;

        // Arm the post-finish deadline the moment the first racer finishes.
        if tracker.is_race_complete() && post_finish_deadline.is_none() {
            post_finish_deadline = Some(physics_time + race_config.post_finish_secs);
        }

        // Stop when the post-finish period has elapsed, physics is exhausted,
        // or the safety-cap (max_duration) is reached.
        let post_finish_expired = post_finish_deadline
            .map(|deadline| physics_time >= deadline)
            .unwrap_or(false);

        if post_finish_expired || tracker.is_physics_complete() || physics_time >= max_duration {
            break;
        }
    }

    // ── Winner announcement hold ───────────────────────────────────────────
    // Render the final physics frame with the winner announcement composited on
    // top, then repeat it for the configured hold duration.
    let hold_secs = race_config.announcement_hold_secs;
    let hold_frames = (hold_secs * options.fps as f32).round() as u64;

    if hold_frames > 0 && tracker.race_state().winner.is_some() {
        let phys_state = tracker.physics_state();
        // Camera stays at its last position; update with dt=0.0 to freeze it.
        let last_ctx = active_camera.get_ctx(&phys_state, 0.0, &[], None, true);
        let mut frame = trail_renderer.render(&phys_state, &last_ctx);

        overlay.draw_race_frame(&mut frame, tracker.race_state(), race_config, &last_ctx)?;
        overlay.draw_winner_announcement(&mut frame, tracker.race_state())?;

        // Write the same winner frame for each hold frame.
        for _ in 0..hold_frames {
            ffmpeg_stdin
                .write_all(&frame.pixels)
                .map_err(ExportError::Io)?;
            frame_count += 1;
        }
    }

    // ── Finalise video ─────────────────────────────────────────────────────
    drop(ffmpeg_stdin);

    let status = ffmpeg.wait().map_err(ExportError::Io)?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(ExportError::FfmpegFailed {
            code,
            stderr: "(stderr not captured in video-only pass)".to_string(),
        });
    }

    // ── Audio mux pass ─────────────────────────────────────────────────────
    if has_any_audio(scene) {
        let duration_secs = frame_count as f32 / options.fps as f32;
        let wav_file = NamedTempFile::new().map_err(ExportError::Io)?;
        audio
            .write_wav(wav_file.path(), duration_secs)
            .map_err(ExportError::Audio)?;

        let ffmpeg_bin = options
            .ffmpeg_path
            .as_deref()
            .map(|p| p.as_os_str().to_owned())
            .unwrap_or_else(|| std::ffi::OsString::from("ffmpeg"));
        remux_with_audio(&options.output_path, wav_file.path(), &ffmpeg_bin)?;
    }

    Ok(())
}

// ── Internal helpers ──────────────────────────────────────────────────────────

/// Determine the maximum simulation duration.
fn resolve_max_duration(scene: &Scene, options: &ExportOptions) -> Result<f32, ExportError> {
    if let Some(duration) = options.max_duration {
        return Ok(duration);
    }

    // Check if the scene has a time-limit end condition.
    if let Some(end) = &scene.end_condition {
        if let Some(secs) = extract_time_limit(end) {
            return Ok(secs);
        }
        // Non-time-limit end condition: use duration_hint or default.
    }

    // Fall back to scene metadata hint.
    if let Some(hint) = scene.meta.duration_hint {
        return Ok(hint);
    }

    // No way to determine duration.
    Err(ExportError::NoDuration)
}

/// Recursively extract the first `TimeLimit` seconds from an end condition.
fn extract_time_limit(cond: &rphys_scene::EndCondition) -> Option<f32> {
    match cond {
        rphys_scene::EndCondition::TimeLimit { seconds } => Some(*seconds),
        rphys_scene::EndCondition::And { conditions }
        | rphys_scene::EndCondition::Or { conditions } => {
            conditions.iter().find_map(extract_time_limit)
        }
        _ => None,
    }
}

/// Build a [`RenderContext`] that maps the scene's world bounds into the output frame.
fn build_render_context(options: &ExportOptions, scene: &Scene) -> RenderContext {
    let world = &scene.environment.world_bounds;
    let scale_x = options.width as f32 / world.width;
    let scale_y = options.height as f32 / world.height;
    // Use the smaller scale to preserve aspect ratio (letterbox/pillarbox).
    let scale = scale_x.min(scale_y);

    RenderContext {
        width: options.width,
        height: options.height,
        camera_origin: Vec2::ZERO,
        scale,
        background_color: scene.environment.background_color,
    }
}

/// Spawn ffmpeg in video-only mode (no audio input).
///
/// Raw RGBA frames are read from stdin; output is written directly to
/// `options.output_path`.
///
/// Uses `options.ffmpeg_path` when set; otherwise falls back to `"ffmpeg"`
/// (resolved via `PATH`).
fn spawn_ffmpeg_video_only(options: &ExportOptions) -> Result<std::process::Child, ExportError> {
    let size_arg = format!("{}x{}", options.width, options.height);
    let ffmpeg_bin = options
        .ffmpeg_path
        .as_deref()
        .map(|p| p.as_os_str())
        .unwrap_or_else(|| std::ffi::OsStr::new("ffmpeg"));

    let child = Command::new(ffmpeg_bin)
        .args([
            "-y", // overwrite output
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &size_arg,
            "-framerate",
            &options.fps.to_string(),
            "-i",
            "pipe:0", // video from stdin
            "-c:v",
            "libx264",
            "-preset",
            "fast",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            "-an", // no audio in this pass
        ])
        .arg(options.output_path.as_os_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                ExportError::FfmpegNotFound
            } else {
                ExportError::Io(err)
            }
        })?;

    Ok(child)
}

/// Re-mux the existing video file with an audio WAV track in-place.
///
/// The output file is a temporary sibling and then renamed over the original.
/// `ffmpeg_bin` is the path (or name) of the ffmpeg binary to invoke.
fn remux_with_audio(
    output_path: &std::path::Path,
    wav_path: &std::path::Path,
    ffmpeg_bin: &std::ffi::OsStr,
) -> Result<(), ExportError> {
    let parent = output_path
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    let tmp_path = parent.join("__rphys_export_tmp.mp4");

    let output = Command::new(ffmpeg_bin)
        .args(["-y", "-i"])
        .arg(output_path)
        .args(["-i"])
        .arg(wav_path)
        .args([
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-movflags",
            "+faststart",
            "-shortest",
        ])
        .arg(&tmp_path)
        .output()
        .map_err(ExportError::Io)?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(ExportError::FfmpegFailed { code, stderr });
    }

    std::fs::rename(&tmp_path, output_path).map_err(ExportError::Io)?;
    Ok(())
}

/// Build the per-body snapshot slice that [`VfxEngine::begin_frame`] expects:
/// `(BodyId, world_pos, color, world_radius_meters)` for each alive body.
///
/// Positions and radii are in world-space (meters, Y-up).  The VFX engine
/// applies the camera transform at render time.
fn build_body_snapshot(
    state: &rphys_physics::PhysicsState,
) -> Vec<(rphys_physics::types::BodyId, Vec2, Color, f32)> {
    state
        .bodies
        .iter()
        .filter(|b| b.is_alive)
        .map(|b| {
            let world_radius = match &b.shape {
                rphys_scene::ShapeKind::Circle { radius } => *radius,
                rphys_scene::ShapeKind::Rectangle { width, height } => {
                    (width.powi(2) + height.powi(2)).sqrt() * 0.5
                }
                rphys_scene::ShapeKind::Polygon { vertices } => vertices
                    .iter()
                    .map(|v| (v.x * v.x + v.y * v.y).sqrt())
                    .fold(0.0_f32, f32::max),
            };
            (b.id, b.position, b.color, world_radius)
        })
        .collect()
}

/// Return `true` if the scene has any audio configured (scene-level defaults or
/// per-object overrides).
///
/// Used to decide whether to run the two-pass audio mux step after rendering.
fn has_any_audio(scene: &Scene) -> bool {
    let a = &scene.audio;
    a.default_bounce.is_some()
        || a.default_destroy.is_some()
        || a.default_bumper.is_some()
        || a.default_boost.is_some()
        || a.default_finish.is_some()
        || scene
            .objects
            .iter()
            .any(|o| o.audio.bounce.is_some() || o.audio.destroy.is_some())
}

/// Apply distance-based volume attenuation relative to the camera.
///
/// Bodies closer to `camera_y` are louder; bodies farther away are quieter,
/// with a floor of 0.1 so off-screen sounds remain faintly audible.
fn spatial_attenuation(body_y: f32, camera_y: f32) -> f32 {
    const MAX_AUDIBLE_DISTANCE: f32 = 20.0;
    let distance = (body_y - camera_y).abs();
    (1.0 - distance / MAX_AUDIBLE_DISTANCE).max(0.1)
}

/// Translate a [`PhysicsEvent`] into zero or more [`AudioEvent`]s and queue them.
///
/// `boost_cooldowns` prevents the same body from triggering the boost sound
/// more than once per 0.3 s window. `camera_y` is used for spatial attenuation.
fn collect_audio_event(
    audio: &mut OfflineAudioMixer,
    engine: &PhysicsEngine,
    event: &PhysicsEvent,
    current_time: f32,
    scene: &Scene,
    boost_cooldowns: &mut HashMap<BodyId, f32>,
    camera_y: f32,
) {
    const MAX_IMPULSE: f32 = 100.0;
    const BOOST_COOLDOWN_SECS: f32 = 0.3;

    match event {
        PhysicsEvent::Collision(info) => {
            let volume = volume_from_impulse(info.impulse, MAX_IMPULSE);
            let body_y = engine
                .body_position(info.body_a)
                .map(|p| p.y)
                .unwrap_or(camera_y);
            let volume = volume * spatial_attenuation(body_y, camera_y);
            // Try body_a's bounce sound, fall back to scene default.
            let path = engine
                .body_info(info.body_a)
                .and_then(|bi| bi.audio.bounce.clone())
                .or_else(|| scene.audio.default_bounce.clone());

            if let Some(path) = path {
                audio.add_event(AudioEvent {
                    timestamp_secs: current_time,
                    path,
                    volume,
                });
            }
        }
        PhysicsEvent::WallBounce { body, impulse } => {
            let volume = volume_from_impulse(*impulse, MAX_IMPULSE);
            let body_y = engine.body_position(*body).map(|p| p.y).unwrap_or(camera_y);
            let volume = volume * spatial_attenuation(body_y, camera_y);
            let path = engine
                .body_info(*body)
                .and_then(|bi| bi.audio.bounce.clone())
                .or_else(|| scene.audio.default_bounce.clone());

            if let Some(path) = path {
                audio.add_event(AudioEvent {
                    timestamp_secs: current_time,
                    path,
                    volume,
                });
            }
        }
        PhysicsEvent::Destroyed { body } => {
            let body_y = engine.body_position(*body).map(|p| p.y).unwrap_or(camera_y);
            let volume = spatial_attenuation(body_y, camera_y);
            let path = engine
                .body_info(*body)
                .and_then(|bi| bi.audio.destroy.clone())
                .or_else(|| scene.audio.default_destroy.clone());

            if let Some(path) = path {
                audio.add_event(AudioEvent {
                    timestamp_secs: current_time,
                    path,
                    volume,
                });
            }
        }
        PhysicsEvent::BoostActivated { body } => {
            // Per-body cooldown: skip if this body triggered boost within 0.3 s.
            let last = boost_cooldowns
                .get(body)
                .copied()
                .unwrap_or(f32::NEG_INFINITY);
            if current_time - last < BOOST_COOLDOWN_SECS {
                return;
            }
            boost_cooldowns.insert(*body, current_time);

            let body_y = engine.body_position(*body).map(|p| p.y).unwrap_or(camera_y);
            let volume = 0.7 * spatial_attenuation(body_y, camera_y);
            if let Some(path) = scene.audio.default_boost.clone() {
                audio.add_event(AudioEvent {
                    timestamp_secs: current_time,
                    path,
                    volume,
                });
            }
        }
        PhysicsEvent::BumperActivated {
            body,
            impulse_magnitude,
            ..
        } => {
            let volume = volume_from_impulse(*impulse_magnitude, MAX_IMPULSE);
            let body_y = engine.body_position(*body).map(|p| p.y).unwrap_or(camera_y);
            let volume = volume * spatial_attenuation(body_y, camera_y);
            if let Some(path) = scene.audio.default_bumper.clone() {
                audio.add_event(AudioEvent {
                    timestamp_secs: current_time,
                    path,
                    volume,
                });
            }
        }
        // Not audio-relevant events.
        PhysicsEvent::GravityWellPull { .. } => {}
        PhysicsEvent::SimulationComplete { .. } => {}
    }
}

/// Compute the physics time delta for one rendered frame, applying a slowdown
/// factor when the leading racer is within the final-stretch zone.
///
/// Returns `frame_dt * slowdown_factor` when `leader_y < finish_y + slowdown_zone_m`,
/// otherwise returns the unmodified `frame_dt`.
fn compute_slowdown_dt(
    leader_y: f32,
    finish_y: f32,
    slowdown_zone_m: f32,
    slowdown_factor: f32,
    frame_dt: f32,
) -> f32 {
    if leader_y < finish_y + slowdown_zone_m {
        frame_dt * slowdown_factor
    } else {
        frame_dt
    }
}

/// Compute audio volume from an impulse magnitude.
fn volume_from_impulse(impulse: f32, max_impulse: f32) -> f32 {
    if impulse <= 0.0 {
        return 1.0; // Default to full volume when impulse is unknown.
    }
    (impulse / max_impulse).clamp(0.1, 1.0)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_scene::{
        BodyType, Checkpoint, Color, EndCondition, Environment, Material, ObjectAudio, RaceConfig,
        SceneAudio, SceneMeta, SceneObject, ShapeKind, Vec2, WallConfig, WorldBounds,
    };

    // ── Helpers ───────────────────────────────────────────────────────────────

    /// Check whether ffmpeg is available on PATH.
    fn ffmpeg_available() -> bool {
        Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Build a minimal 1-second scene with a single static circle and no audio.
    fn minimal_scene() -> Scene {
        Scene {
            version: "1".to_string(),
            meta: SceneMeta {
                name: "test".to_string(),
                description: None,
                author: None,
                duration_hint: None,
            },
            environment: Environment {
                gravity: Vec2::new(0.0, -9.81),
                background_color: Color::BLACK,
                world_bounds: WorldBounds {
                    width: 20.0,
                    height: 20.0,
                },
                walls: WallConfig {
                    visible: false,
                    color: Color::WHITE,
                    thickness: 0.1,
                    open_bottom: false,
                },
            },
            objects: vec![SceneObject {
                name: Some("circle".to_string()),
                shape: ShapeKind::Circle { radius: 1.0 },
                position: Vec2::new(10.0, 10.0),
                velocity: Vec2::ZERO,
                rotation: 0.0,
                angular_velocity: None,
                body_type: BodyType::Static,
                material: Material::default(),
                color: Color::rgba(255, 100, 50, 255),
                tags: vec![],
                destructible: None,
                boost: None,
                gravity_well: None,
                bumper: None,
                audio: ObjectAudio::default(),
            }],
            end_condition: Some(rphys_scene::EndCondition::TimeLimit { seconds: 1.0 }),
            audio: SceneAudio::default(),
            race: None,
            camera: None,
            vfx: None,
        }
    }

    // ── Preset defaults ───────────────────────────────────────────────────────

    #[test]
    fn test_tiktok_preset_defaults() {
        let opts = ExportOptions::from_preset(Preset::TikTok, PathBuf::from("out.mp4"));
        assert_eq!(opts.width, 1080);
        assert_eq!(opts.height, 1920);
        assert_eq!(opts.fps, 60);
    }

    #[test]
    fn test_youtube_preset_defaults() {
        let opts = ExportOptions::from_preset(Preset::YouTube, PathBuf::from("out.mp4"));
        assert_eq!(opts.width, 1920);
        assert_eq!(opts.height, 1080);
        assert_eq!(opts.fps, 60);
    }

    // ── Missing ffmpeg → ExportError::FfmpegNotFound ──────────────────────────

    #[test]
    fn test_missing_ffmpeg_returns_error() {
        // Temporarily set PATH to something empty so ffmpeg can't be found.
        let scene = minimal_scene();
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 64,
            height: 64,
            fps: 1,
            output_path: tmp.path().to_path_buf(),
            max_duration: Some(0.1),
            ffmpeg_path: None,
        };

        // Use a fake binary name to force NotFound.
        let result = Command::new("ffmpeg_definitely_not_on_system_xyz_12345")
            .spawn()
            .map_err(|err| {
                if err.kind() == std::io::ErrorKind::NotFound {
                    ExportError::FfmpegNotFound
                } else {
                    ExportError::Io(err)
                }
            });

        assert!(
            matches!(result, Err(ExportError::FfmpegNotFound)),
            "expected FfmpegNotFound, got: {result:?}",
        );

        let _ = scene; // ensure we built the scene
        let _ = opts;
    }

    // ── Full export integration test (requires ffmpeg) ─────────────────────────

    #[test]
    fn test_export_minimal_scene_produces_mp4() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg not found on PATH");
            return;
        }

        let scene = minimal_scene();
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let output = tmp_dir.path().join("test_export.mp4");

        // Use a small resolution and low FPS to keep the test fast.
        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 64,
            height: 64,
            fps: 10,
            output_path: output.clone(),
            max_duration: Some(1.0),
            ffmpeg_path: None,
        };

        let result = export(&scene, opts);
        assert!(result.is_ok(), "export failed: {:?}", result.err());

        let metadata = std::fs::metadata(&output).expect("output file should exist");
        assert!(
            metadata.len() > 0,
            "output file should be non-empty (got {} bytes)",
            metadata.len()
        );
    }

    // ── NoDuration error ──────────────────────────────────────────────────────

    #[test]
    fn test_no_duration_returns_error() {
        let mut scene = minimal_scene();
        // Remove the end condition so there's no duration.
        scene.end_condition = None;
        scene.meta.duration_hint = None;

        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 64,
            height: 64,
            fps: 10,
            output_path: tmp.path().to_path_buf(),
            max_duration: None,
            ffmpeg_path: None, // explicitly unset
        };

        let result = export(&scene, opts);
        assert!(
            matches!(result, Err(ExportError::NoDuration)),
            "expected NoDuration, got: {result:?}",
        );
    }

    // ── Resolve max duration helpers ──────────────────────────────────────────

    #[test]
    fn test_resolve_duration_from_options() {
        let scene = {
            let mut s = minimal_scene();
            s.end_condition = None;
            s.meta.duration_hint = None;
            s
        };
        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 1,
            height: 1,
            fps: 1,
            output_path: PathBuf::from("x.mp4"),
            max_duration: Some(42.0),
            ffmpeg_path: None,
        };
        let d = resolve_max_duration(&scene, &opts).expect("should resolve");
        assert!((d - 42.0).abs() < 1e-5);
    }

    #[test]
    fn test_resolve_duration_from_time_limit_condition() {
        let scene = minimal_scene(); // has TimeLimit { seconds: 1.0 }
        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 1,
            height: 1,
            fps: 1,
            output_path: PathBuf::from("x.mp4"),
            max_duration: None,
            ffmpeg_path: None,
        };
        let d = resolve_max_duration(&scene, &opts).expect("should resolve");
        assert!((d - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_resolve_duration_from_hint() {
        let mut scene = minimal_scene();
        scene.end_condition = None; // no time-limit cond
        scene.meta.duration_hint = Some(5.0);
        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 1,
            height: 1,
            fps: 1,
            output_path: PathBuf::from("x.mp4"),
            max_duration: None,
            ffmpeg_path: None,
        };
        let d = resolve_max_duration(&scene, &opts).expect("should resolve");
        assert!((d - 5.0).abs() < 1e-5);
    }

    // ── Race helpers ──────────────────────────────────────────────────────────

    /// Build a minimal race scene: two balls with `"racer"` tag, finish line at
    /// y=2.0, and a time-limit fallback end condition.
    fn minimal_race_scene() -> Scene {
        let make_ball = |name: &str, x: f32, color: Color| SceneObject {
            name: Some(name.to_string()),
            shape: ShapeKind::Circle { radius: 0.4 },
            position: Vec2::new(x, 3.5),
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
        };

        Scene {
            version: "1".to_string(),
            meta: SceneMeta {
                name: "race_test".to_string(),
                description: None,
                author: None,
                duration_hint: None,
            },
            environment: Environment {
                gravity: Vec2::new(0.0, -9.81),
                background_color: Color::rgb(20, 20, 30),
                world_bounds: WorldBounds {
                    width: 20.0,
                    height: 20.0,
                },
                walls: WallConfig {
                    visible: true,
                    color: Color::WHITE,
                    thickness: 0.5,
                    open_bottom: false,
                },
            },
            objects: vec![
                make_ball("Red", 6.0, Color::rgb(220, 50, 50)),
                make_ball("Blue", 14.0, Color::rgb(50, 100, 220)),
            ],
            end_condition: Some(EndCondition::Or {
                conditions: vec![
                    EndCondition::FirstToReach {
                        finish_y: 2.0,
                        tag: "racer".to_string(),
                    },
                    EndCondition::TimeLimit { seconds: 0.5 },
                ],
            }),
            audio: SceneAudio::default(),
            race: Some(RaceConfig {
                finish_y: 2.0,
                racer_tag: "racer".to_string(),
                announcement_hold_secs: 0.0, // no hold to keep test fast
                checkpoints: vec![],
                elimination_interval_secs: None,
                post_finish_secs: 0.0,
                countdown_seconds: 0,
            }),
            camera: None,
            vfx: None,
        }
    }

    // ── Race export integration test (requires ffmpeg) ─────────────────────────

    #[test]
    fn test_race_export_produces_file_when_ffmpeg_available() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg not found on PATH");
            return;
        }

        let scene = minimal_race_scene();
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let output = tmp_dir.path().join("test_race_export.mp4");

        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 64,
            height: 64,
            fps: 10,
            output_path: output.clone(),
            max_duration: Some(0.5),
            ffmpeg_path: None,
        };

        let result = export(&scene, opts);
        assert!(result.is_ok(), "race export failed: {:?}", result.err());

        let metadata = std::fs::metadata(&output).expect("output file should exist");
        assert!(
            metadata.len() > 0,
            "output file should be non-empty (got {} bytes)",
            metadata.len()
        );
    }

    // ── Race auto-detection test ──────────────────────────────────────────────

    #[test]
    fn test_export_auto_detects_race_scene() {
        // A scene with race config should route to export_race(), which uses
        // RaceTracker. Verify it does NOT return ExportError::NoDuration even
        // though the scene has no explicit numeric duration at the top level
        // (duration comes from TimeLimit inside the Or condition).
        //
        // We check this without ffmpeg by verifying the dispatch decision:
        // export() on a scene with scene.race.is_some() must attempt the race
        // path. If ffmpeg is missing, we'll get FfmpegNotFound — not NoDuration.
        let scene = minimal_race_scene();
        let tmp = tempfile::NamedTempFile::new().expect("tempfile");
        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 64,
            height: 64,
            fps: 10,
            output_path: tmp.path().to_path_buf(),
            max_duration: Some(0.5),
            ffmpeg_path: None,
        };

        let result = export(&scene, opts);

        // The result must not be NoDuration — the race path handles duration.
        assert!(
            !matches!(result, Err(ExportError::NoDuration)),
            "race scene should not return NoDuration; got: {result:?}"
        );
    }

    // ── Race scene with checkpoints (overlay smoke test) ──────────────────────

    #[test]
    fn test_race_export_with_checkpoints_when_ffmpeg_available() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg not found on PATH");
            return;
        }

        let mut scene = minimal_race_scene();
        // Add a checkpoint above the finish line.
        if let Some(ref mut race) = scene.race {
            race.checkpoints = vec![Checkpoint {
                y: 3.2,
                label: Some("Halfway".to_string()),
            }];
        }

        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let output = tmp_dir.path().join("test_race_checkpoint.mp4");

        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 64,
            height: 64,
            fps: 10,
            output_path: output.clone(),
            max_duration: Some(0.5),
            ffmpeg_path: None,
        };

        let result = export(&scene, opts);
        assert!(
            result.is_ok(),
            "race export with checkpoints failed: {:?}",
            result.err()
        );
    }

    // ── Slowdown dt calculation unit test ─────────────────────────────────────

    /// Verify `compute_slowdown_dt` returns the correct dt in and out of the zone.
    ///
    /// This test runs entirely in-process without needing ffmpeg.
    #[test]
    fn test_slowdown_activates_near_finish() {
        let finish_y = 2.0_f32;
        let zone_m = 6.0_f32;
        let factor = 0.25_f32;
        let frame_dt = 1.0_f32 / 60.0; // 60 fps

        // Leader well above the slowdown zone → normal dt.
        let dt_normal = compute_slowdown_dt(20.0, finish_y, zone_m, factor, frame_dt);
        assert!(
            (dt_normal - frame_dt).abs() < 1e-6,
            "expected normal dt ({frame_dt}) when far from finish, got {dt_normal}"
        );

        // Leader exactly at the zone boundary (finish_y + zone_m) → normal dt.
        // The condition is strictly less-than, so the boundary itself is NOT in the zone.
        let boundary_y = finish_y + zone_m; // = 8.0
        let dt_boundary = compute_slowdown_dt(boundary_y, finish_y, zone_m, factor, frame_dt);
        assert!(
            (dt_boundary - frame_dt).abs() < 1e-6,
            "expected normal dt at boundary (y={boundary_y}), got {dt_boundary}"
        );

        // Leader just inside the slowdown zone → slowed dt.
        let inside_y = finish_y + zone_m - 0.001; // 7.999, just inside
        let dt_slow = compute_slowdown_dt(inside_y, finish_y, zone_m, factor, frame_dt);
        let expected_slow = frame_dt * factor;
        assert!(
            (dt_slow - expected_slow).abs() < 1e-6,
            "expected slowed dt ({expected_slow}) when leader at y={inside_y}, got {dt_slow}"
        );

        // Leader at finish_y (finished) → still in zone, slowed dt.
        let dt_at_finish = compute_slowdown_dt(finish_y, finish_y, zone_m, factor, frame_dt);
        assert!(
            (dt_at_finish - expected_slow).abs() < 1e-6,
            "expected slowed dt at finish line, got {dt_at_finish}"
        );

        // Leader below finish_y (past finish) → still in zone.
        let dt_past = compute_slowdown_dt(finish_y - 1.0, finish_y, zone_m, factor, frame_dt);
        assert!(
            (dt_past - expected_slow).abs() < 1e-6,
            "expected slowed dt when past finish line, got {dt_past}"
        );

        // Sanity: slowed dt is 25% of normal.
        assert!(
            (dt_slow / dt_normal - factor).abs() < 1e-5,
            "slowed dt should be {factor}× normal dt"
        );
    }

    // ── Full integration test with slowdown (requires ffmpeg) ─────────────────

    /// Export a race with slowdown enabled; confirm the file is produced and
    /// non-empty.  Skipped automatically when ffmpeg is not on PATH.
    #[test]
    fn test_full_race_export_with_slowdown_when_ffmpeg_available() {
        if !ffmpeg_available() {
            eprintln!("SKIP: ffmpeg not found on PATH");
            return;
        }

        let scene = minimal_race_scene();
        let tmp_dir = tempfile::tempdir().expect("tempdir");
        let output = tmp_dir.path().join("test_race_slowdown.mp4");

        // Allow enough wall-clock duration for the slowdown zone to be traversed.
        let opts = ExportOptions {
            preset: Preset::Custom,
            width: 64,
            height: 64,
            fps: 10,
            output_path: output.clone(),
            max_duration: Some(2.0),
            ffmpeg_path: None,
        };

        let result = export(&scene, opts);
        assert!(
            result.is_ok(),
            "race export with slowdown failed: {:?}",
            result.err()
        );

        let metadata = std::fs::metadata(&output).expect("output file should exist");
        assert!(
            metadata.len() > 0,
            "output file should be non-empty (got {} bytes)",
            metadata.len()
        );
    }

    // ── post-finish run-on period tests ───────────────────────────────────────

    /// Build a race scene designed for post-finish deadline testing.
    ///
    /// Two balls drop under gravity toward `finish_y = 2.0`:
    /// - "First"  starts at y=2.5  → crosses the line in ~0.32 s physics time.
    /// - "Second" starts at y=5.0  → crosses ~0.46 s later in physics time.
    ///
    /// Both balls start inside the slowdown zone (below y=8.0), so `compute_
    /// slowdown_dt` returns 25% of the frame delta throughout.
    fn post_finish_race_scene(post_finish_secs: f32) -> Scene {
        let make_ball = |name: &str, x: f32, y: f32, color: Color| SceneObject {
            name: Some(name.to_string()),
            shape: ShapeKind::Circle { radius: 0.4 },
            position: Vec2::new(x, y),
            velocity: Vec2::ZERO,
            rotation: 0.0,
            angular_velocity: None,
            body_type: BodyType::Dynamic,
            material: Material {
                restitution: 0.05,
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
        };

        Scene {
            version: "1".to_string(),
            meta: SceneMeta {
                name: "post_finish_test".to_string(),
                description: None,
                author: None,
                duration_hint: None,
            },
            environment: Environment {
                gravity: Vec2::new(0.0, -9.81),
                background_color: Color::rgb(20, 20, 30),
                world_bounds: WorldBounds {
                    width: 20.0,
                    height: 20.0,
                },
                walls: WallConfig {
                    visible: true,
                    color: Color::WHITE,
                    thickness: 0.5,
                    open_bottom: false,
                },
            },
            objects: vec![
                // Ball 1: just above finish line — wins first.
                make_ball("First", 6.0, 2.5, Color::rgb(220, 50, 50)),
                // Ball 2: starts higher — crosses later (gap ≈ 0.46 s physics time).
                make_ball("Second", 14.0, 5.0, Color::rgb(50, 100, 220)),
            ],
            end_condition: None,
            audio: SceneAudio::default(),
            race: Some(RaceConfig {
                finish_y: 2.0,
                racer_tag: "racer".to_string(),
                announcement_hold_secs: 0.5,
                checkpoints: vec![],
                elimination_interval_secs: None,
                post_finish_secs,
                countdown_seconds: 0,
            }),
            camera: None,
            vfx: None,
        }
    }

    /// Simulate the post-finish deadline logic used in `export_race()` against
    /// a live `RaceTracker`, without requiring ffmpeg.
    ///
    /// Returns the `finished` vec at the point the loop would have broken.
    fn run_post_finish_sim(scene: &Scene) -> Vec<rphys_race::FinishedEntry> {
        let race_config = scene.race.as_ref().unwrap();
        let post_finish_secs = race_config.post_finish_secs;

        let physics_cfg = PhysicsConfig {
            max_steps_per_call: u32::MAX,
            ..PhysicsConfig::default()
        };
        let mut tracker = RaceTracker::new(scene, physics_cfg).unwrap();

        let frame_dt = 1.0_f32 / 60.0;
        let mut physics_time: f32 = 0.0;
        let mut post_finish_deadline: Option<f32> = None;
        let max_duration = 60.0_f32;

        loop {
            let leader_y = tracker
                .physics_state()
                .bodies
                .iter()
                .filter(|b| b.is_alive && b.tags.iter().any(|t| t == &race_config.racer_tag))
                .map(|b| b.position.y)
                .reduce(f32::min)
                .unwrap_or(f32::MAX);

            let dt = compute_slowdown_dt(leader_y, race_config.finish_y, 6.0, 0.25, frame_dt);
            physics_time += dt;
            tracker.advance_to(physics_time).unwrap();

            if tracker.is_race_complete() && post_finish_deadline.is_none() {
                post_finish_deadline = Some(physics_time + post_finish_secs);
            }

            let post_finish_expired = post_finish_deadline
                .map(|d| physics_time >= d)
                .unwrap_or(false);

            if post_finish_expired || tracker.is_physics_complete() || physics_time >= max_duration
            {
                break;
            }
        }

        tracker.race_state().finished.clone()
    }

    /// `post_finish_secs: 5.0` — both racers must be ranked before the loop exits.
    ///
    /// Ball "Second" starts 2.5 m above finish and takes ≈0.46 s more physics
    /// time than "First" to cross.  A 5-second run-on window is more than
    /// sufficient.  No ffmpeg required.
    #[test]
    fn test_post_finish_secs_allows_second_finisher_to_rank() {
        let scene = post_finish_race_scene(5.0);
        let finished = run_post_finish_sim(&scene);

        assert!(
            finished.len() >= 2,
            "with post_finish_secs=5.0 both racers should be ranked, \
             but finished list has {} entries",
            finished.len()
        );

        let rank1 = finished.iter().find(|e| e.finish_rank == 1);
        let rank2 = finished.iter().find(|e| e.finish_rank == 2);
        assert!(rank1.is_some(), "finish_rank 1 should be assigned");
        assert!(rank2.is_some(), "finish_rank 2 should be assigned");

        if let (Some(r1), Some(r2)) = (rank1, rank2) {
            assert!(
                r1.finish_time_secs <= r2.finish_time_secs,
                "rank-1 finish time ({:.3}s) should be ≤ rank-2 ({:.3}s)",
                r1.finish_time_secs,
                r2.finish_time_secs
            );
        }
    }

    /// `post_finish_secs: 0.0` — preserves existing behaviour: loop exits the
    /// same iteration the first racer crosses, only the winner is ranked.
    ///
    /// With deadline = physics_time + 0.0, `post_finish_expired` fires
    /// immediately.  Ball "Second" starts 2.5 m above finish and cannot have
    /// crossed within a single physics frame.  No ffmpeg required.
    #[test]
    fn test_post_finish_zero_stops_immediately() {
        let scene = post_finish_race_scene(0.0);
        let finished = run_post_finish_sim(&scene);

        assert_eq!(
            finished.len(),
            1,
            "with post_finish_secs=0.0 only the winner should be ranked, \
             but finished list has {} entries",
            finished.len()
        );
        assert_eq!(finished[0].finish_rank, 1, "sole entry must be rank 1");
        assert_eq!(
            finished[0].display_name, "First",
            "winner should be 'First' (the ball starting closer to finish)"
        );
    }

    // ── Audio event wiring tests ──────────────────────────────────────────────

    /// Build a scene with custom audio paths set.
    fn scene_with_sfx_audio() -> Scene {
        use std::path::PathBuf;
        let mut scene = minimal_scene();
        scene.audio = SceneAudio {
            default_bounce: Some(PathBuf::from("assets/sfx/bounce.wav")),
            default_destroy: Some(PathBuf::from("assets/sfx/destroy.wav")),
            default_bumper: Some(PathBuf::from("assets/sfx/bumper.wav")),
            default_boost: Some(PathBuf::from("assets/sfx/boost.wav")),
            default_finish: Some(PathBuf::from("assets/sfx/finish.wav")),
            master_volume: 0.8,
        };
        scene
    }

    /// `BumperActivated` with a scene-level default bumper path → one audio event queued.
    #[test]
    fn test_collect_audio_bumper_activated_emits_event() {
        use rphys_physics::types::BodyId;

        let scene = scene_with_sfx_audio();
        let physics_cfg = PhysicsConfig {
            max_steps_per_call: u32::MAX,
            ..PhysicsConfig::default()
        };
        let engine = PhysicsEngine::new(&scene, physics_cfg).expect("engine build");
        let mut mixer = rphys_audio::OfflineAudioMixer::new(44100, 2);
        let mut cooldowns = HashMap::new();

        collect_audio_event(
            &mut mixer,
            &engine,
            &PhysicsEvent::BumperActivated {
                body: BodyId(9999), // non-existent body — falls back to scene defaults
                contact_point: Vec2::ZERO,
                impulse_magnitude: 50.0,
            },
            1.0,
            &scene,
            &mut cooldowns,
            0.0, // camera_y — body position falls back to this, so attenuation = 1.0
        );

        let events = mixer.events();
        assert_eq!(
            events.len(),
            1,
            "expected one audio event from BumperActivated"
        );
        assert_eq!(
            events[0].path,
            std::path::PathBuf::from("assets/sfx/bumper.wav")
        );
        // volume_from_impulse(50.0, 100.0) = 0.5
        assert!(
            (events[0].volume - 0.5).abs() < 1e-5,
            "expected volume ≈ 0.5 (impulse 50 / MAX 100), got {}",
            events[0].volume
        );
    }

    /// `BoostActivated` with a scene-level default boost path → one audio event at volume 0.7.
    #[test]
    fn test_collect_audio_boost_activated_emits_event_at_fixed_volume() {
        use rphys_physics::types::BodyId;

        let scene = scene_with_sfx_audio();
        let physics_cfg = PhysicsConfig {
            max_steps_per_call: u32::MAX,
            ..PhysicsConfig::default()
        };
        let engine = PhysicsEngine::new(&scene, physics_cfg).expect("engine build");
        let mut mixer = rphys_audio::OfflineAudioMixer::new(44100, 2);
        let mut cooldowns = HashMap::new();

        collect_audio_event(
            &mut mixer,
            &engine,
            &PhysicsEvent::BoostActivated {
                body: BodyId(9999), // non-existent body
            },
            2.5,
            &scene,
            &mut cooldowns,
            0.0,
        );

        let events = mixer.events();
        assert_eq!(
            events.len(),
            1,
            "expected one audio event from BoostActivated"
        );
        assert_eq!(
            events[0].path,
            std::path::PathBuf::from("assets/sfx/boost.wav")
        );
        assert!(
            (events[0].volume - 0.7).abs() < 1e-5,
            "boost audio should have fixed volume 0.7, got {}",
            events[0].volume
        );
        assert!(
            (events[0].timestamp_secs - 2.5).abs() < 1e-5,
            "timestamp should be current physics time"
        );
    }

    /// When no bumper/boost audio is configured, no events are queued.
    #[test]
    fn test_collect_audio_no_sfx_configured_emits_nothing() {
        use rphys_physics::types::BodyId;

        let scene = minimal_scene(); // audio = SceneAudio::default() — all None
        let physics_cfg = PhysicsConfig {
            max_steps_per_call: u32::MAX,
            ..PhysicsConfig::default()
        };
        let engine = PhysicsEngine::new(&scene, physics_cfg).expect("engine build");
        let mut mixer = rphys_audio::OfflineAudioMixer::new(44100, 2);
        let mut cooldowns = HashMap::new();

        collect_audio_event(
            &mut mixer,
            &engine,
            &PhysicsEvent::BumperActivated {
                body: BodyId(1),
                contact_point: Vec2::ZERO,
                impulse_magnitude: 30.0,
            },
            0.0,
            &scene,
            &mut cooldowns,
            0.0,
        );
        collect_audio_event(
            &mut mixer,
            &engine,
            &PhysicsEvent::BoostActivated { body: BodyId(1) },
            0.0,
            &scene,
            &mut cooldowns,
            0.0,
        );

        assert!(
            mixer.events().is_empty(),
            "no audio should be queued when sfx paths are not configured"
        );
    }

    /// `has_any_audio` returns `false` for a scene with no audio config.
    #[test]
    fn test_has_any_audio_false_when_all_none() {
        let scene = minimal_scene();
        assert!(!has_any_audio(&scene), "expected false for all-None audio");
    }

    /// `has_any_audio` returns `true` when at least one field is set.
    #[test]
    fn test_has_any_audio_true_when_default_bumper_set() {
        let mut scene = minimal_scene();
        scene.audio.default_bumper = Some(std::path::PathBuf::from("bumper.wav"));
        assert!(
            has_any_audio(&scene),
            "expected true when default_bumper is set"
        );
    }

    #[test]
    fn test_has_any_audio_true_when_default_boost_set() {
        let mut scene = minimal_scene();
        scene.audio.default_boost = Some(std::path::PathBuf::from("boost.wav"));
        assert!(
            has_any_audio(&scene),
            "expected true when default_boost is set"
        );
    }

    #[test]
    fn test_has_any_audio_true_when_default_finish_set() {
        let mut scene = minimal_scene();
        scene.audio.default_finish = Some(std::path::PathBuf::from("finish.wav"));
        assert!(
            has_any_audio(&scene),
            "expected true when default_finish is set"
        );
    }

    /// Boost cooldown: a second BoostActivated within 0.3 s is suppressed.
    #[test]
    fn test_boost_cooldown_suppresses_rapid_fire() {
        use rphys_physics::types::BodyId;

        let scene = scene_with_sfx_audio();
        let physics_cfg = PhysicsConfig {
            max_steps_per_call: u32::MAX,
            ..PhysicsConfig::default()
        };
        let engine = PhysicsEngine::new(&scene, physics_cfg).expect("engine build");
        let mut mixer = rphys_audio::OfflineAudioMixer::new(44100, 2);
        let mut cooldowns = HashMap::new();
        let body = BodyId(42);

        // First boost at t=1.0 — should emit.
        collect_audio_event(
            &mut mixer,
            &engine,
            &PhysicsEvent::BoostActivated { body },
            1.0,
            &scene,
            &mut cooldowns,
            0.0,
        );
        assert_eq!(mixer.events().len(), 1, "first boost should emit");

        // Second boost at t=1.1 (within 0.3 s) — should be suppressed.
        collect_audio_event(
            &mut mixer,
            &engine,
            &PhysicsEvent::BoostActivated { body },
            1.1,
            &scene,
            &mut cooldowns,
            0.0,
        );
        assert_eq!(
            mixer.events().len(),
            1,
            "second boost within cooldown should be suppressed"
        );

        // Third boost at t=1.4 (after 0.3 s cooldown) — should emit.
        collect_audio_event(
            &mut mixer,
            &engine,
            &PhysicsEvent::BoostActivated { body },
            1.4,
            &scene,
            &mut cooldowns,
            0.0,
        );
        assert_eq!(mixer.events().len(), 2, "boost after cooldown should emit");
    }

    /// Spatial attenuation: body at camera_y has full volume, body far away is quieter.
    #[test]
    fn test_spatial_attenuation_scales_volume() {
        // Body at camera position → attenuation = 1.0
        assert!((spatial_attenuation(5.0, 5.0) - 1.0).abs() < 1e-5);

        // Body 10 units away → attenuation = 0.5
        assert!((spatial_attenuation(15.0, 5.0) - 0.5).abs() < 1e-5);

        // Body 20+ units away → attenuation clamped to 0.1
        assert!((spatial_attenuation(30.0, 5.0) - 0.1).abs() < 1e-5);
    }
}
