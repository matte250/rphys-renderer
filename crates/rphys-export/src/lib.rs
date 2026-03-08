//! Headless video export pipeline.
//!
//! Runs physics headlessly, renders each frame with [`TinySkiaRenderer`], and
//! pipes raw RGBA pixels to `ffmpeg` for encoding into an MP4 file.
//!
//! # Example
//!
//! ```rust,no_run
//! use rphys_export::{ExportOptions, Preset, NullProgress, export};
//! use rphys_scene::parse_scene;
//! use std::path::PathBuf;
//!
//! let scene = parse_scene(r##"
//! version: "1"
//! meta:
//!   name: "Demo"
//! environment:
//!   gravity: [0.0, -9.81]
//!   background_color: "#000000"
//!   world_bounds:
//!     width: 20.0
//!     height: 20.0
//!   walls:
//!     visible: true
//!     color: "#ffffff"
//!     thickness: 0.3
//! objects: []
//! "##).unwrap();
//!
//! let mut options = ExportOptions::from_preset(
//!     Preset::TikTok,
//!     PathBuf::from("output.mp4"),
//! );
//! options.max_duration = Some(5.0);
//!
//! // export(&scene, options, &mut NullProgress).unwrap();
//! ```

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use rphys_audio::{AudioEvent, OfflineAudioMixer};
use rphys_physics::{BodyId, PhysicsConfig, PhysicsEngine, PhysicsEvent};
use rphys_renderer::{RenderContext, Renderer, TinySkiaRenderer};
use rphys_scene::{Scene, Vec2};
use thiserror::Error;

// ── Preset ────────────────────────────────────────────────────────────────────

/// Video export preset specifying resolution and frame rate.
#[derive(Debug, Clone, PartialEq)]
pub enum Preset {
    /// 1080×1920, 60 fps — TikTok / YouTube Shorts / Instagram Reels.
    TikTok,
    /// 1920×1080, 60 fps — YouTube landscape.
    YouTube,
    /// User-defined resolution and frame rate.
    Custom,
}

impl Preset {
    /// Returns the `(width, height, fps)` dimensions for this preset.
    ///
    /// For [`Preset::Custom`] the returned values are defaults; callers should
    /// override them after calling [`ExportOptions::from_preset`].
    pub fn dimensions(&self) -> (u32, u32, u32) {
        match self {
            Preset::TikTok => (1080, 1920, 60),
            Preset::YouTube => (1920, 1080, 60),
            Preset::Custom => (1920, 1080, 60),
        }
    }
}

// ── ExportOptions ─────────────────────────────────────────────────────────────

/// Options that control the export pipeline.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// Preset for resolution and frame rate.
    pub preset: Preset,
    /// Output frame width in pixels.
    pub width: u32,
    /// Output frame height in pixels.
    pub height: u32,
    /// Output frame rate in frames per second.
    pub fps: u32,
    /// Path to write the final MP4 file.
    pub output_path: PathBuf,
    /// Cap the export duration in seconds.
    ///
    /// Required when the scene has no end condition. Takes precedence over
    /// [`rphys_scene::SceneMeta::duration_hint`].
    pub max_duration: Option<f32>,
    /// Whether to mix and include audio in the output.
    pub include_audio: bool,
}

impl ExportOptions {
    /// Create export options from a preset.
    ///
    /// Width, height, and fps are taken from the preset. Callers using
    /// [`Preset::Custom`] should override these fields after construction.
    pub fn from_preset(preset: Preset, output_path: PathBuf) -> Self {
        let (width, height, fps) = preset.dimensions();
        Self {
            preset,
            width,
            height,
            fps,
            output_path,
            max_duration: None,
            include_audio: true,
        }
    }
}

// ── ProgressSink ──────────────────────────────────────────────────────────────

/// Receiver for frame-by-frame progress updates during export.
pub trait ProgressSink: Send {
    /// Called after each frame is written to ffmpeg.
    ///
    /// `frame` is the 0-based frame index.  
    /// `total_estimate` is the estimated total frame count, or `None` if unknown.
    fn on_frame(&mut self, frame: u64, total_estimate: Option<u64>);

    /// Called once when encoding is complete.
    fn on_complete(&mut self, total_frames: u64, elapsed_secs: f64);
}

/// Progress sink that prints each frame update to stdout.
pub struct TerminalProgress;

impl ProgressSink for TerminalProgress {
    fn on_frame(&mut self, frame: u64, total_estimate: Option<u64>) {
        if let Some(total) = total_estimate {
            let pct = if total > 0 { frame * 100 / total } else { 0 };
            println!("Frame {frame} / {total} ({pct}%)");
        } else {
            println!("Frame {frame}");
        }
    }

    fn on_complete(&mut self, total_frames: u64, elapsed_secs: f64) {
        println!("Export complete: {total_frames} frames in {elapsed_secs:.1}s");
    }
}

/// Silent progress sink — does nothing.  Useful in tests and headless pipelines.
pub struct NullProgress;

impl ProgressSink for NullProgress {
    fn on_frame(&mut self, _frame: u64, _total_estimate: Option<u64>) {}
    fn on_complete(&mut self, _total_frames: u64, _elapsed_secs: f64) {}
}

// ── ExportError ───────────────────────────────────────────────────────────────

/// Errors that can occur during the export pipeline.
#[derive(Debug, Error)]
pub enum ExportError {
    /// ffmpeg is not installed or not found in `PATH`.
    #[error("ffmpeg not found — please install ffmpeg and ensure it is in PATH")]
    FfmpegNotFound,

    /// ffmpeg exited with a non-zero status code.
    #[error("ffmpeg process failed (exit {code}): {stderr}")]
    FfmpegFailed { code: i32, stderr: String },

    /// Physics simulation error.
    #[error("Physics error: {0}")]
    Physics(#[from] rphys_physics::PhysicsError),

    /// Rendering error.
    #[error("Render error: {0}")]
    Render(String),

    /// Audio engine error.
    #[error("Audio error: {0}")]
    Audio(#[from] rphys_audio::AudioError),

    /// I/O error (writing frames, temp files, etc.).
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Scene has no end condition and no maximum duration was specified.
    #[error("Scene has no end condition and no max_duration was specified")]
    NoDuration,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Export a scene to MP4. Blocks until encoding is complete.
///
/// Runs physics headlessly, renders each frame, and pipes raw RGBA data to
/// `ffmpeg`.  Audio events are collected during the run and mixed offline.
///
/// # Errors
///
/// Returns [`ExportError::FfmpegNotFound`] if `ffmpeg` is not available in
/// `PATH`, [`ExportError::NoDuration`] if the scene has no end condition and
/// [`ExportOptions::max_duration`] is `None`.
pub fn export(
    scene: &Scene,
    options: ExportOptions,
    progress: &mut dyn ProgressSink,
) -> Result<(), ExportError> {
    export_with_ffmpeg(scene, options, progress, "ffmpeg")
}

// ── Internal pipeline ─────────────────────────────────────────────────────────

/// Core export implementation parameterised over the ffmpeg binary path.
///
/// Exposed as `pub(crate)` so unit tests can inject a non-existent path to
/// trigger [`ExportError::FfmpegNotFound`] without modifying `PATH`.
pub(crate) fn export_with_ffmpeg(
    scene: &Scene,
    options: ExportOptions,
    progress: &mut dyn ProgressSink,
    ffmpeg_binary: &str,
) -> Result<(), ExportError> {
    // ── duration check ────────────────────────────────────────────────────────
    let max_duration = options.max_duration.or(scene.meta.duration_hint);
    if scene.end_condition.is_none() && max_duration.is_none() {
        return Err(ExportError::NoDuration);
    }

    // ── ffmpeg availability ───────────────────────────────────────────────────
    if !ffmpeg_available(ffmpeg_binary) {
        return Err(ExportError::FfmpegNotFound);
    }

    // ── render context ────────────────────────────────────────────────────────
    let ctx = make_render_context(&options, scene);

    // ── physics engine ────────────────────────────────────────────────────────
    // Use a large step cap: export is not real-time so spiral-of-death is not
    // a concern; we just need to advance the required number of steps per frame.
    let physics_config = PhysicsConfig {
        timestep: 1.0 / 240.0,
        max_steps_per_call: u32::MAX,
    };
    let mut engine = PhysicsEngine::new(scene, physics_config)?;

    // ── renderer + audio mixer ────────────────────────────────────────────────
    let renderer = TinySkiaRenderer;
    let mut audio_mixer = OfflineAudioMixer::new(44100, 2);

    // ── spawn ffmpeg (video-only pass) ────────────────────────────────────────
    let ffmpeg_args = build_ffmpeg_video_args(&options);
    let mut child = Command::new(ffmpeg_binary)
        .args(&ffmpeg_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ExportError::FfmpegNotFound)?;

    let mut stdin = child.stdin.take().expect("ffmpeg stdin is piped");

    // ── frame loop ────────────────────────────────────────────────────────────
    let fps = options.fps as f32;
    let total_estimate = max_duration.map(|d| (d * fps).ceil() as u64);
    let start = std::time::Instant::now();
    let mut frame_n = 0u64;

    loop {
        // Stop if we have reached the requested maximum duration.
        if let Some(max_dur) = max_duration {
            if engine.time() >= max_dur {
                break;
            }
        }

        let target_t = (frame_n + 1) as f32 / fps;
        let events = engine.advance_to(target_t)?;

        // Collect audio events.
        let physics_time = engine.time();
        for event in &events {
            collect_audio_event(event, &engine, physics_time, scene, &mut audio_mixer);
        }

        // Render and pipe frame.
        let state = engine.state();
        let frame = renderer.render(&state, &ctx);
        stdin.write_all(&frame.pixels).map_err(ExportError::Io)?;

        progress.on_frame(frame_n, total_estimate);
        frame_n += 1;

        if engine.is_complete() {
            break;
        }
    }

    // Signal EOF to ffmpeg.
    drop(stdin);

    // Wait for ffmpeg to finish the video pass.
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(ExportError::FfmpegFailed { code, stderr });
    }

    // ── optional audio mux pass ───────────────────────────────────────────────
    if options.include_audio {
        let duration_secs = engine.time();
        mux_audio(ffmpeg_binary, &options, &mut audio_mixer, duration_secs)?;
    }

    let elapsed = start.elapsed().as_secs_f64();
    progress.on_complete(frame_n, elapsed);
    Ok(())
}

// ── ffmpeg helpers ────────────────────────────────────────────────────────────

/// Return `true` if `ffmpeg_binary` resolves to an executable.
fn ffmpeg_available(ffmpeg_binary: &str) -> bool {
    Command::new(ffmpeg_binary)
        .arg("-version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Build the ffmpeg arguments for the video-only encoding pass.
fn build_ffmpeg_video_args(options: &ExportOptions) -> Vec<String> {
    vec![
        "-y".to_string(),
        "-f".to_string(),
        "rawvideo".to_string(),
        "-pixel_format".to_string(),
        "rgba".to_string(),
        "-video_size".to_string(),
        format!("{}x{}", options.width, options.height),
        "-framerate".to_string(),
        options.fps.to_string(),
        "-i".to_string(),
        "pipe:0".to_string(),
        "-c:v".to_string(),
        "libx264".to_string(),
        "-preset".to_string(),
        "fast".to_string(),
        "-crf".to_string(),
        "18".to_string(),
        "-pix_fmt".to_string(),
        "yuv420p".to_string(),
        "-movflags".to_string(),
        "+faststart".to_string(),
        options.output_path.to_string_lossy().to_string(),
    ]
}

/// Mix collected audio and re-mux it into the already-encoded video file.
///
/// Writes a temporary WAV file, runs a second ffmpeg pass, then replaces the
/// output file with the muxed result.
fn mux_audio(
    ffmpeg_binary: &str,
    options: &ExportOptions,
    mixer: &mut OfflineAudioMixer,
    duration_secs: f32,
) -> Result<(), ExportError> {
    // Write the mixed audio to a named temp file.
    let wav_file = tempfile::NamedTempFile::new()?;
    mixer.write_wav(wav_file.path(), duration_secs)?;

    // Write the muxed output to a second temp file, then rename to output_path.
    let mux_file = tempfile::NamedTempFile::new()?;
    let mux_path = mux_file.path().to_path_buf();

    let status = Command::new(ffmpeg_binary)
        .args([
            "-y",
            "-i",
            &options.output_path.to_string_lossy(),
            "-i",
            &wav_file.path().to_string_lossy(),
            "-c:v",
            "copy",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
            &mux_path.to_string_lossy(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status()?;

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(ExportError::FfmpegFailed {
            code,
            stderr: "audio mux pass failed".to_string(),
        });
    }

    // Replace the video-only file with the muxed one.
    std::fs::copy(&mux_path, &options.output_path)?;
    Ok(())
}

// ── Render context ────────────────────────────────────────────────────────────

/// Build a [`RenderContext`] that maps the scene's world bounds into the output
/// resolution.
fn make_render_context(options: &ExportOptions, scene: &Scene) -> RenderContext {
    let bounds = &scene.environment.world_bounds;
    let scale_x = options.width as f32 / bounds.width;
    let scale_y = options.height as f32 / bounds.height;
    let scale = scale_x.min(scale_y);

    RenderContext {
        width: options.width,
        height: options.height,
        camera_origin: Vec2::ZERO,
        scale,
        background_color: scene.environment.background_color,
    }
}

// ── Audio collection ──────────────────────────────────────────────────────────

/// Inspect a physics event and, if it has an associated sound, queue an
/// [`AudioEvent`] in the mixer.
fn collect_audio_event(
    event: &PhysicsEvent,
    engine: &PhysicsEngine,
    physics_time: f32,
    scene: &Scene,
    mixer: &mut OfflineAudioMixer,
) {
    match event {
        PhysicsEvent::Collision(info) => {
            if let Some(path) = bounce_path(info.body_a, engine, scene) {
                mixer.add_event(AudioEvent {
                    timestamp_secs: physics_time,
                    path,
                    volume: impulse_to_volume(info.impulse),
                });
            }
        }
        PhysicsEvent::WallBounce { body, impulse } => {
            if let Some(path) = bounce_path(*body, engine, scene) {
                mixer.add_event(AudioEvent {
                    timestamp_secs: physics_time,
                    path,
                    volume: impulse_to_volume(*impulse),
                });
            }
        }
        PhysicsEvent::Destroyed { body } => {
            if let Some(path) = destroy_path(*body, engine, scene) {
                mixer.add_event(AudioEvent {
                    timestamp_secs: physics_time,
                    path,
                    volume: 1.0,
                });
            }
        }
        PhysicsEvent::SimulationComplete { .. } => {}
    }
}

/// Look up the bounce sound path for a body, falling back to the scene default.
fn bounce_path(id: BodyId, engine: &PhysicsEngine, scene: &Scene) -> Option<PathBuf> {
    engine
        .body_info(id)
        .and_then(|info| info.audio.bounce.clone())
        .or_else(|| scene.audio.default_bounce.clone())
}

/// Look up the destroy sound path for a body, falling back to the scene default.
fn destroy_path(id: BodyId, engine: &PhysicsEngine, scene: &Scene) -> Option<PathBuf> {
    engine
        .body_info(id)
        .and_then(|info| info.audio.destroy.clone())
        .or_else(|| scene.audio.default_destroy.clone())
}

/// Map an impulse magnitude to a linear volume scalar.
///
/// Uses a logarithmic-feel clamped mapping:  
/// `volume = clamp(impulse / 100.0, 0.1, 1.0)`.  
/// If `impulse == 0.0` (no contact-force data), plays at full volume.
fn impulse_to_volume(impulse: f32) -> f32 {
    if impulse <= 0.0 {
        1.0
    } else {
        (impulse / 100.0).clamp(0.1, 1.0)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_scene::{Color, Environment, Scene, SceneAudio, SceneMeta, WallConfig, WorldBounds};

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Build a minimal scene with no objects and a 1-second time-limit end
    /// condition so that no `max_duration` override is needed.
    fn minimal_scene_with_time_limit(seconds: f32) -> Scene {
        use rphys_scene::EndCondition;
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
                    thickness: 0.3,
                },
            },
            objects: vec![],
            end_condition: Some(EndCondition::TimeLimit { seconds }),
            audio: SceneAudio::default(),
        }
    }

    /// Build a minimal scene with no end condition but a `max_duration` passed
    /// through `ExportOptions`.
    fn minimal_scene_no_end() -> Scene {
        Scene {
            version: "1".to_string(),
            meta: SceneMeta {
                name: "test-no-end".to_string(),
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
                    thickness: 0.3,
                },
            },
            objects: vec![],
            end_condition: None,
            audio: SceneAudio::default(),
        }
    }

    // ── Preset dimension tests ────────────────────────────────────────────────

    #[test]
    fn test_preset_tiktok_dimensions() {
        let (w, h, fps) = Preset::TikTok.dimensions();
        assert_eq!(w, 1080, "TikTok width");
        assert_eq!(h, 1920, "TikTok height");
        assert_eq!(fps, 60, "TikTok fps");
    }

    #[test]
    fn test_preset_youtube_dimensions() {
        let (w, h, fps) = Preset::YouTube.dimensions();
        assert_eq!(w, 1920, "YouTube width");
        assert_eq!(h, 1080, "YouTube height");
        assert_eq!(fps, 60, "YouTube fps");
    }

    #[test]
    fn test_preset_custom_has_sensible_defaults() {
        let (w, h, fps) = Preset::Custom.dimensions();
        assert!(w > 0, "Custom width > 0");
        assert!(h > 0, "Custom height > 0");
        assert!(fps > 0, "Custom fps > 0");
    }

    #[test]
    fn test_from_preset_tiktok() {
        let opts = ExportOptions::from_preset(Preset::TikTok, PathBuf::from("out.mp4"));
        assert_eq!(opts.width, 1080);
        assert_eq!(opts.height, 1920);
        assert_eq!(opts.fps, 60);
        assert!(opts.max_duration.is_none());
        assert!(opts.include_audio);
    }

    // ── NoDuration error ──────────────────────────────────────────────────────

    #[test]
    fn test_no_duration_returns_error() {
        let scene = minimal_scene_no_end();
        let options = ExportOptions {
            preset: Preset::Custom,
            width: 320,
            height: 240,
            fps: 10,
            output_path: PathBuf::from("/tmp/should_not_exist.mp4"),
            max_duration: None, // no cap
            include_audio: false,
        };
        let result = export_with_ffmpeg(&scene, options, &mut NullProgress, "ffmpeg");
        assert!(
            matches!(result, Err(ExportError::NoDuration)),
            "expected NoDuration, got: {result:?}",
        );
    }

    // ── FfmpegNotFound error ──────────────────────────────────────────────────

    #[test]
    fn test_ffmpeg_not_found_returns_error() {
        let scene = minimal_scene_with_time_limit(1.0);
        let options = ExportOptions {
            preset: Preset::Custom,
            width: 320,
            height: 240,
            fps: 10,
            output_path: PathBuf::from("/tmp/should_not_exist.mp4"),
            max_duration: None,
            include_audio: false,
        };
        // Use a path that is guaranteed not to exist.
        let result = export_with_ffmpeg(
            &scene,
            options,
            &mut NullProgress,
            "/nonexistent/ffmpeg_does_not_exist",
        );
        assert!(
            matches!(result, Err(ExportError::FfmpegNotFound)),
            "expected FfmpegNotFound, got: {result:?}",
        );
    }

    // ── Full export integration test ──────────────────────────────────────────

    /// Skip if ffmpeg is not installed — do NOT fail.
    fn ffmpeg_is_present() -> bool {
        ffmpeg_available("ffmpeg")
    }

    #[test]
    fn test_export_produces_file_when_ffmpeg_available() {
        if !ffmpeg_is_present() {
            eprintln!("SKIP test_export_produces_file_when_ffmpeg_available — ffmpeg not found");
            return;
        }

        let scene = minimal_scene_with_time_limit(1.0);
        let out_file = tempfile::NamedTempFile::new().expect("tempfile");

        let options = ExportOptions {
            preset: Preset::Custom,
            // Use a small resolution to keep the test fast.
            width: 160,
            height: 120,
            fps: 10,
            output_path: out_file.path().to_path_buf(),
            max_duration: None,   // scene has a 1s time-limit end condition
            include_audio: false, // no audio files in minimal scene
        };

        let result = export_with_ffmpeg(&scene, options, &mut NullProgress, "ffmpeg");
        assert!(result.is_ok(), "export failed: {result:?}");

        let metadata = std::fs::metadata(out_file.path()).expect("stat output file");
        assert!(metadata.len() > 0, "output file is empty");
    }

    #[test]
    fn test_export_with_max_duration_produces_file() {
        if !ffmpeg_is_present() {
            eprintln!("SKIP test_export_with_max_duration_produces_file — ffmpeg not found");
            return;
        }

        let scene = minimal_scene_no_end();
        let out_file = tempfile::NamedTempFile::new().expect("tempfile");

        let options = ExportOptions {
            preset: Preset::Custom,
            width: 160,
            height: 120,
            fps: 10,
            output_path: out_file.path().to_path_buf(),
            max_duration: Some(0.5),
            include_audio: false,
        };

        let result = export_with_ffmpeg(&scene, options, &mut NullProgress, "ffmpeg");
        assert!(result.is_ok(), "export failed: {result:?}");

        let metadata = std::fs::metadata(out_file.path()).expect("stat output file");
        assert!(metadata.len() > 0, "output file is empty");
    }
}
