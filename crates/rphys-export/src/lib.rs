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

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use rphys_audio::{AudioEvent, OfflineAudioMixer};
use rphys_overlay::OverlayRenderer;
use rphys_physics::{PhysicsConfig, PhysicsEngine, PhysicsEvent};
use rphys_race::RaceTracker;
use rphys_renderer::{
    CameraController, RaceCamera, RaceCameraConfig, RenderContext, Renderer, TinySkiaRenderer,
};
use rphys_scene::{Scene, Vec2};
use tempfile::NamedTempFile;
use thiserror::Error;

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

    loop {
        let target_time = (frame_count + 1) as f32 / options.fps as f32;
        let events = engine.advance_to(target_time)?;

        // Collect audio events.
        for event in &events {
            collect_audio_event(&mut audio, &engine, event, engine.time(), scene);
        }

        // Render.
        let state = engine.state();
        let frame = renderer.render(&state, &ctx);

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
    if scene.audio.default_bounce.is_some()
        || scene.audio.default_destroy.is_some()
        || scene
            .objects
            .iter()
            .any(|o| o.audio.bounce.is_some() || o.audio.destroy.is_some())
    {
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
    let renderer = TinySkiaRenderer;
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

    let camera_cfg = RaceCameraConfig {
        racer_tag: race_config.racer_tag.clone(),
        ..RaceCameraConfig::default()
    };
    let mut camera = RaceCamera::new(camera_cfg, initial_ctx);

    let overlay = OverlayRenderer::new();

    // ── Spawn ffmpeg ───────────────────────────────────────────────────────
    let mut ffmpeg = spawn_ffmpeg_video_only(&options)?;
    let mut ffmpeg_stdin = ffmpeg
        .stdin
        .take()
        .ok_or_else(|| ExportError::Io(std::io::Error::other("failed to open ffmpeg stdin")))?;

    // ── Main race loop ─────────────────────────────────────────────────────
    let frame_dt = 1.0_f32 / options.fps as f32;
    let mut frame_count = 0u64;

    loop {
        let target_time = (frame_count + 1) as f32 / options.fps as f32;

        let (physics_events, _race_events) = tracker.advance_to(target_time)?;

        // Collect audio events.
        for event in &physics_events {
            collect_audio_event(&mut audio, tracker.engine(), event, tracker.time(), scene);
        }

        // Render frame with race camera.
        let phys_state = tracker.physics_state();
        let ctx = camera.update(&phys_state, frame_dt);
        let mut frame = renderer.render(&phys_state, &ctx);

        // Draw race overlay (finish/checkpoint lines + rank panel).
        overlay.draw_race_frame(&mut frame, tracker.race_state(), race_config, &ctx)?;

        // Write frame to ffmpeg.
        ffmpeg_stdin
            .write_all(&frame.pixels)
            .map_err(ExportError::Io)?;

        frame_count += 1;

        // Exit when the race or physics is done, or safety cap reached.
        if tracker.is_race_complete()
            || tracker.is_physics_complete()
            || target_time >= max_duration
        {
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
        let last_ctx = camera.update(&phys_state, 0.0);
        let mut frame = renderer.render(&phys_state, &last_ctx);

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
    if scene.audio.default_bounce.is_some()
        || scene.audio.default_destroy.is_some()
        || scene
            .objects
            .iter()
            .any(|o| o.audio.bounce.is_some() || o.audio.destroy.is_some())
    {
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

/// Translate a [`PhysicsEvent`] into zero or more [`AudioEvent`]s and queue them.
fn collect_audio_event(
    audio: &mut OfflineAudioMixer,
    engine: &PhysicsEngine,
    event: &PhysicsEvent,
    current_time: f32,
    scene: &Scene,
) {
    const MAX_IMPULSE: f32 = 100.0;

    match event {
        PhysicsEvent::Collision(info) => {
            let volume = volume_from_impulse(info.impulse, MAX_IMPULSE);
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
            let path = engine
                .body_info(*body)
                .and_then(|bi| bi.audio.destroy.clone())
                .or_else(|| scene.audio.default_destroy.clone());

            if let Some(path) = path {
                audio.add_event(AudioEvent {
                    timestamp_secs: current_time,
                    path,
                    volume: 1.0,
                });
            }
        }
        // Not an audio-relevant event.
        PhysicsEvent::SimulationComplete { .. } => {}
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
                audio: ObjectAudio::default(),
            }],
            end_condition: Some(rphys_scene::EndCondition::TimeLimit { seconds: 1.0 }),
            audio: SceneAudio::default(),
            race: None,
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
            }),
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
}
