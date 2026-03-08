# Module Definitions — rphys-renderer

Each section below covers one crate in the workspace: its responsibility, public types and traits (Rust signatures), and its direct dependencies.

---

## `rphys-scene` — YAML Parser & Scene Model

### Responsibility
Parse and validate a YAML scene file into a strongly-typed `Scene` struct. Generate the JSON Schema. This is the only crate that touches the filesystem for scene loading.

### Public Types

```rust
// ── Domain primitives ────────────────────────────────────────────────────────

/// 2D vector used for positions, velocities, and gravity.
/// Thin newtype to avoid passing raw `[f32; 2]` across APIs.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// RGBA color, stored as 0–255 components.
/// Supports `"#RRGGBB"` and `"#RRGGBBAA"` hex strings in YAML.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

// ── Shape definitions ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeKind {
    Circle { radius: f32 },
    Rectangle { width: f32, height: f32 },
    Polygon { vertices: Vec<Vec2> },
}

// ── Material ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// Coefficient of restitution: 0.0 = no bounce, 1.0 = perfect bounce.
    pub restitution: f32,
    /// Coefficient of friction: 0.0 = frictionless, 1.0 = high friction.
    pub friction: f32,
    /// Density in kg/m². Determines mass from shape area.
    pub density: f32,
}

impl Default for Material {
    fn default() -> Self { /* restitution=0.5, friction=0.5, density=1.0 */ }
}

// ── Body type ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Default)]
pub enum BodyType {
    #[default]
    Dynamic,
    Static,
    Kinematic,
}

// ── Destructible config ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Destructible {
    /// Minimum impulse magnitude (N·s) to destroy this object.
    pub min_impact_force: f32,
}

// ── Audio mapping for a single object ────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ObjectAudio {
    /// Sound to play when this object bounces off something.
    pub bounce: Option<std::path::PathBuf>,
    /// Sound to play when this object is destroyed.
    pub destroy: Option<std::path::PathBuf>,
}

// ── Scene object ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct SceneObject {
    /// Optional human-readable identifier. Must be unique if provided.
    pub name: Option<String>,
    pub shape: ShapeKind,
    pub position: Vec2,
    pub velocity: Vec2,
    pub rotation: f32,             // radians, default 0.0
    pub angular_velocity: f32,     // rad/s, default 0.0
    pub body_type: BodyType,
    pub material: Material,
    pub color: Color,
    pub tags: Vec<String>,
    pub destructible: Option<Destructible>,
    pub audio: ObjectAudio,
}

// ── Environment ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct WorldBounds {
    pub width: f32,   // meters
    pub height: f32,  // meters
}

#[derive(Debug, Clone, PartialEq)]
pub struct WallConfig {
    pub visible: bool,
    pub color: Color,
    pub thickness: f32,  // meters
}

#[derive(Debug, Clone, PartialEq)]
pub struct Environment {
    pub gravity: Vec2,
    pub background_color: Color,
    pub world_bounds: WorldBounds,
    pub walls: WallConfig,
}

// ── End conditions ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum EndCondition {
    /// Simulation stops after this many seconds of physics time.
    TimeLimit { seconds: f32 },
    /// All objects with this tag have been destroyed.
    AllTaggedDestroyed { tag: String },
    /// The named object has left the world bounds.
    ObjectEscaped { name: String },
    /// Two named objects have collided with each other.
    ObjectsCollided { name_a: String, name_b: String },
    /// An object with this tag has collided with an object with another tag.
    TagsCollided { tag_a: String, tag_b: String },
    /// All sub-conditions must be true simultaneously.
    And { conditions: Vec<EndCondition> },
    /// Any sub-condition being true triggers completion.
    Or { conditions: Vec<EndCondition> },
}

// ── Global audio config ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SceneAudio {
    /// Fallback bounce sound when an object has no per-object bounce sound.
    pub default_bounce: Option<std::path::PathBuf>,
    /// Fallback destroy sound when an object has no per-object destroy sound.
    pub default_destroy: Option<std::path::PathBuf>,
    /// Master volume: 0.0 = silent, 1.0 = full.
    pub master_volume: f32,
}

// ── Metadata ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct SceneMeta {
    pub name: String,
    pub description: Option<String>,
    pub author: Option<String>,
    /// Hint for export duration when no end condition fires (seconds).
    pub duration_hint: Option<f32>,
}

// ── Top-level scene ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    /// Schema version string (e.g. "1"). Used for forward-compat checks.
    pub version: String,
    pub meta: SceneMeta,
    pub environment: Environment,
    pub objects: Vec<SceneObject>,
    /// Root end condition. None = run until time_limit or Ctrl+C.
    pub end_condition: Option<EndCondition>,
    pub audio: SceneAudio,
}
```

### Public Functions

```rust
/// Parse and validate a YAML string into a Scene.
/// Returns structured errors — never raw serde panics.
pub fn parse_scene(yaml: &str) -> Result<Scene, ParseError>;

/// Parse from a file path. Resolves relative asset paths against the
/// scene file's directory.
pub fn parse_scene_file(path: &std::path::Path) -> Result<Scene, ParseError>;

/// Return the JSON Schema string for the scene format.
/// Used by `rphys schema` command.
pub fn scene_json_schema() -> &'static str;
```

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("IO error reading scene file: {0}")]
    Io(#[from] std::io::Error),

    #[error("YAML syntax error at line {line}: {message}")]
    Syntax { line: usize, message: String },

    #[error("Validation failed:\n{}", format_errors(.0))]
    Validation(Vec<ValidationError>),

    #[error("Empty scene file — nothing to simulate")]
    EmptyScene,

    #[error("Unsupported schema version '{version}' (expected '1')")]
    UnsupportedVersion { version: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Object '{name}': missing required field '{field}'")]
    MissingField { name: String, field: &'static str },

    #[error("Object '{name}': unknown shape type '{shape}'")]
    UnknownShape { name: String, shape: String },

    #[error("Object '{name}': {message}")]
    InvalidValue { name: String, message: String },

    #[error("Duplicate object name '{name}'")]
    DuplicateName { name: String },

    #[error("Audio file not found: '{path}'")]
    AudioFileNotFound { path: std::path::PathBuf },

    #[error("End condition references unknown object '{name}'")]
    UnknownObjectReference { name: String },
}
```

### Dependencies
- `serde`, `serde_yaml` — deserialization
- `thiserror` — error types
- No other workspace crates

---

## `rphys-physics` — Simulation Engine

### Responsibility
Own the rapier2d world. Accept a `Scene`, run the fixed-timestep simulation, emit physics events, and evaluate end conditions. Knows nothing about rendering or audio — it only produces state and events.

### Public Types

```rust
// ── Stable object ID ─────────────────────────────────────────────────────────

/// Stable identifier for a simulated body. Opaque to callers.
/// Wraps rapier2d's RigidBodyHandle but doesn't expose it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BodyId(u32);

// ── Per-body snapshot ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BodyState {
    pub id: BodyId,
    /// Name from the original SceneObject, if any.
    pub name: Option<String>,
    pub tags: Vec<String>,
    pub position: rphys_scene::Vec2,
    pub rotation: f32,        // radians
    pub shape: rphys_scene::ShapeKind,
    pub color: rphys_scene::Color,
    pub is_alive: bool,
}

// ── Full world snapshot ──────────────────────────────────────────────────────

/// Immutable snapshot of the world at a point in physics time.
/// This is what the renderer reads each frame.
#[derive(Debug, Clone)]
pub struct PhysicsState {
    pub bodies: Vec<BodyState>,
    /// Physics time elapsed in seconds.
    pub time: f32,
    pub world_bounds: rphys_scene::WorldBounds,
    pub wall_config: rphys_scene::WallConfig,
}

// ── Physics events ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CollisionInfo {
    pub body_a: BodyId,
    pub body_b: BodyId,
    /// Impulse magnitude in N·s (useful for audio volume scaling).
    pub impulse: f32,
}

#[derive(Debug, Clone)]
pub enum PhysicsEvent {
    /// Two bodies started contacting.
    Collision(CollisionInfo),
    /// A body hit a world boundary wall.
    WallBounce { body: BodyId, impulse: f32 },
    /// A destructible body was removed (impulse exceeded threshold).
    Destroyed { body: BodyId },
    /// An end condition was satisfied.
    SimulationComplete { reason: CompletionReason },
}

#[derive(Debug, Clone)]
pub enum CompletionReason {
    TimeLimitReached,
    AllTaggedDestroyed { tag: String },
    ObjectEscaped { name: String },
    ObjectsCollided { name_a: String, name_b: String },
    TagsCollided { tag_a: String, tag_b: String },
}

// ── Engine configuration ─────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PhysicsConfig {
    /// Timestep in seconds. Default: 1.0 / 240.0.
    pub timestep: f32,
    /// Maximum number of physics steps per call to `step()`. Guards against
    /// spiral-of-death in the preview accumulator.
    pub max_steps_per_call: u32,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self { timestep: 1.0 / 240.0, max_steps_per_call: 8 }
    }
}
```

### Public API

```rust
pub struct PhysicsEngine { /* private */ }

impl PhysicsEngine {
    /// Build the physics world from a parsed scene.
    pub fn new(scene: &rphys_scene::Scene, config: PhysicsConfig)
        -> Result<Self, PhysicsError>;

    /// Advance physics by exactly one fixed timestep.
    /// Returns all events that occurred during this step.
    pub fn step(&mut self) -> Result<Vec<PhysicsEvent>, PhysicsError>;

    /// Advance until `target_time` is reached, stepping as many times as
    /// needed. Useful for export mode (advance N steps per video frame).
    /// Returns all events from all steps taken.
    pub fn advance_to(
        &mut self,
        target_time: f32,
    ) -> Result<Vec<PhysicsEvent>, PhysicsError>;

    /// Snapshot the current world state for rendering.
    pub fn state(&self) -> PhysicsState;

    /// Current physics time in seconds.
    pub fn time(&self) -> f32;

    /// True after a SimulationComplete event has been emitted.
    pub fn is_complete(&self) -> bool;

    /// Look up a body's name and tags by its stable ID.
    pub fn body_info(&self, id: BodyId) -> Option<&BodyInfo>;
}

/// Metadata stored per-body (stable across steps).
#[derive(Debug)]
pub struct BodyInfo {
    pub name: Option<String>,
    pub tags: Vec<String>,
    pub audio: rphys_scene::ObjectAudio,
}
```

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum PhysicsError {
    #[error("Failed to build physics world: {0}")]
    BuildFailed(String),

    #[error("Physics step failed: {0}")]
    StepFailed(String),
}
```

### Internal Structure (not public, for implementers)

```
PhysicsEngine {
    rapier_world: RapierWorld     // rapier2d RigidBodySet, ColliderSet, etc.
    body_map: HashMap<RigidBodyHandle, BodyId>
    body_info: HashMap<BodyId, BodyInfo>
    end_conditions: Vec<EndConditionEvaluator>
    elapsed: f32
    complete: bool
    config: PhysicsConfig
}
```

### Dependencies
- `rapier2d` (with deterministic feature)
- `rphys-scene`
- `thiserror`

---

## `rphys-renderer` — Rendering Abstraction

### Responsibility
Define the `Renderer` trait and provide the `TinySkiaRenderer` implementation. Accept a `PhysicsState`, produce a `Frame` (raw RGBA pixels). No window management — that belongs to `rphys-preview`.

### Public Types

```rust
// ── Output frame ─────────────────────────────────────────────────────────────

/// A rendered frame: width × height × 4 bytes (RGBA, row-major).
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Raw RGBA pixels, length == width * height * 4.
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn new(width: u32, height: u32) -> Self;
    /// Access pixel at (x, y) as &[u8; 4].
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4];
}

// ── Render context ───────────────────────────────────────────────────────────

/// Configuration for how to map physics world → pixels.
#[derive(Debug, Clone)]
pub struct RenderContext {
    pub width: u32,
    pub height: u32,
    /// Pixels per meter. Derived from world_bounds + resolution.
    pub scale: f32,
    /// World origin in pixel space (bottom-left of world = pixel coord).
    pub origin: (f32, f32),
}

impl RenderContext {
    /// Derive scale and origin so the world bounds fill the output exactly.
    pub fn fit_to_world(
        width: u32,
        height: u32,
        world: &rphys_scene::WorldBounds,
    ) -> Self;

    /// Convert a physics Vec2 (meters, y-up) to pixel coordinates (y-down).
    pub fn world_to_pixel(&self, pos: rphys_scene::Vec2) -> (f32, f32);
}
```

### The Renderer Trait

```rust
/// A rendering backend that can draw a single frame.
///
/// Implementors must be `Send` so they can be used across threads
/// (e.g., moved into an export thread).
pub trait Renderer: Send {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Draw a complete physics state into a new Frame.
    fn render(
        &mut self,
        state: &rphys_scene::PhysicsState,
        ctx: &RenderContext,
    ) -> Result<Frame, Self::Error>;
}
```

### Built-in Implementation

```rust
/// CPU software renderer using tiny-skia.
/// Supports: solid-color circles, rectangles, polygons.
/// MVP visual style: flat colors, no effects.
pub struct TinySkiaRenderer {
    // private pixmap pool for reuse
}

impl TinySkiaRenderer {
    pub fn new() -> Self;
}

impl Renderer for TinySkiaRenderer {
    type Error = RenderError;
    fn render(
        &mut self,
        state: &rphys_scene::PhysicsState,
        ctx: &RenderContext,
    ) -> Result<Frame, RenderError>;
}
```

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("Failed to allocate pixmap ({width}×{height})")]
    PixmapAlloc { width: u32, height: u32 },

    #[error("Draw error: {0}")]
    DrawFailed(String),
}
```

### Draw Order (within `render()`)

1. Fill background color
2. Draw world boundary walls (if `wall_config.visible`)
3. For each alive `BodyState` in `state.bodies` (in order — painter's model):
   - Match `shape` → draw filled shape at `position + rotation`
4. Draw destroyed bodies: skip (already removed from state)

### Dependencies
- `tiny-skia`
- `rphys-scene` (for `PhysicsState`, `Vec2`, `Color`, `ShapeKind`)
- `thiserror`

---

## `rphys-audio` — Event-Driven Audio

### Responsibility
Respond to `PhysicsEvent`s by playing or recording sounds. Two implementations: `RodioAudioEngine` (real-time, for preview) and `OfflineAudioMixer` (buffer-based, for export). The trait keeps calling code identical between modes.

### Public Types

```rust
// ── Audio buffer for export ──────────────────────────────────────────────────

/// PCM audio buffer for mixing into an export.
/// Format: 44100 Hz, stereo (interleaved), f32 samples.
#[derive(Debug, Default)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl AudioBuffer {
    /// Duration in seconds.
    pub fn duration(&self) -> f32;
    /// Write to a WAV file at the given path.
    pub fn write_wav(&self, path: &std::path::Path) -> Result<(), AudioError>;
}

// ── Sound library ────────────────────────────────────────────────────────────

/// Pre-loaded, decompressed sounds keyed by file path.
/// Shared between multiple audio engines to avoid redundant disk reads.
pub struct SoundLibrary {
    /* private */
}

impl SoundLibrary {
    pub fn new() -> Self;
    /// Load a sound file into the library. Returns error if file not found.
    pub fn load(&mut self, path: &std::path::Path) -> Result<SoundId, AudioError>;
    /// Load all sounds referenced by a scene (pre-warms on startup).
    pub fn preload_scene(
        &mut self,
        scene: &rphys_scene::Scene,
    ) -> Result<(), AudioError>;
}

/// Opaque handle to a loaded sound in the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SoundId(u32);
```

### The AudioEngine Trait

```rust
pub trait AudioEngine: Send {
    type Error: std::error::Error + Send + Sync + 'static;

    /// Handle one physics event. Called once per event per physics step.
    /// `physics_time` = current simulation time in seconds (for scheduling).
    fn handle_event(
        &mut self,
        event: &rphys_physics::PhysicsEvent,
        body_info: &rphys_physics::BodyInfo,
        scene_audio: &rphys_scene::SceneAudio,
        library: &SoundLibrary,
        physics_time: f32,
    ) -> Result<(), Self::Error>;

    /// Flush audio accumulated since the last flush. Only meaningful for
    /// `OfflineAudioMixer`; `RodioAudioEngine` returns an empty buffer.
    fn flush(&mut self) -> AudioBuffer;
}
```

### Implementations

```rust
/// Real-time audio playback via rodio. Used in preview mode.
pub struct RodioAudioEngine {
    /* private: rodio OutputStream + Sink pool */
}

impl RodioAudioEngine {
    pub fn new() -> Result<Self, AudioError>;
}

impl AudioEngine for RodioAudioEngine { /* ... */ }

/// Offline audio mixer. Records sounds at precise timestamps for export.
/// No real-time playback.
pub struct OfflineAudioMixer {
    /* private: Vec<(timestamp_samples, sound_data)> */
}

impl OfflineAudioMixer {
    pub fn new(sample_rate: u32) -> Self;
}

impl AudioEngine for OfflineAudioMixer { /* ... */ }
```

### Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("Audio device unavailable: {0}")]
    DeviceUnavailable(String),

    #[error("Sound file not found: '{path}'")]
    FileNotFound { path: std::path::PathBuf },

    #[error("Failed to decode audio file '{path}': {reason}")]
    DecodeFailed { path: std::path::PathBuf, reason: String },

    #[error("Failed to write WAV: {0}")]
    WavWriteFailed(String),
}
```

### Volume Scaling (Nice-to-Have for MVP)
If `PhysicsEvent::Collision.impulse > 0`, volume = `clamp(impulse / MAX_IMPULSE, 0.1, 1.0)`. If `impulse == 0`, play at full volume (conservative default).

### Dependencies
- `rodio` + `symphonia` (audio decoding)
- `hound` (WAV writing for export)
- `rphys-scene`, `rphys-physics`
- `thiserror`

---

## `rphys-preview` — Live Preview Window

### Responsibility
Own the winit event loop, manage the render surface, run the physics + render tick, watch the YAML file for changes and hot-reload the scene.

### Public API

```rust
#[derive(Debug, Clone)]
pub struct PreviewOptions {
    /// Path to the .yaml scene file.
    pub scene_path: std::path::PathBuf,
    /// Initial window size in physical pixels. Default: 540×960 (half 1080p vertical).
    pub window_size: (u32, u32),
    /// If true, loop the simulation when the end condition is reached.
    pub loop_on_complete: bool,
}

impl Default for PreviewOptions {
    fn default() -> Self { /* 540×960, no loop */ }
}

/// Run the preview. Blocks until the window is closed or ESC is pressed.
/// This function owns the main thread (required by winit on most platforms).
pub fn run_preview(options: PreviewOptions) -> Result<(), PreviewError>;

#[derive(Debug, thiserror::Error)]
pub enum PreviewError {
    #[error("Failed to create window: {0}")]
    WindowCreation(String),

    #[error("Scene error: {0}")]
    Scene(#[from] rphys_scene::ParseError),

    #[error("Physics error: {0}")]
    Physics(#[from] rphys_physics::PhysicsError),

    #[error("Render error: {0}")]
    Render(String),

    #[error("File watcher error: {0}")]
    Watcher(String),
}
```

### Internal Flow

```
run_preview()
    │
    ├── parse scene
    ├── build PhysicsEngine
    ├── create RodioAudioEngine
    ├── create TinySkiaRenderer
    ├── create winit EventLoop + Window
    ├── create pixels::Pixels surface
    ├── start notify Watcher on scene_path
    │
    └── event_loop.run():
            RedrawRequested:
                accumulate dt → step physics N times
                collect events → audio engine
                check complete → pause or restart
                render state → Frame
                copy Frame to pixels surface → present
            FileChanged:
                re-parse scene
                if ok  → rebuild PhysicsEngine, restart
                if err → log error, keep running
            KeyboardInput(Escape) | CloseRequested:
                exit
```

### Hot-reload Contract
- Only the `PhysicsEngine` (and scene) are rebuilt on reload.
- The window, renderer, and audio engine persist.
- If re-parse fails, a warning is printed to stderr. The running simulation continues unaffected.

### Dependencies
- `winit`, `pixels` (window + surface)
- `notify` (file watching)
- `rphys-scene`, `rphys-physics`, `rphys-renderer`, `rphys-audio`
- `thiserror`

---

## `rphys-export` — Video Encoder

### Responsibility
Run physics headlessly, render each frame, pipe raw pixels to ffmpeg, collect offline audio, and mux everything into a final MP4.

### Public Types

```rust
#[derive(Debug, Clone)]
pub enum Preset {
    /// 1080×1920, 60fps — TikTok / YouTube Shorts / Instagram Reels.
    TikTok,
    /// 1920×1080, 60fps — YouTube landscape.
    YouTube,
    Custom,
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub preset: Preset,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub output_path: std::path::PathBuf,
    /// Override maximum duration (seconds). Used when scene has no end condition.
    pub max_duration: Option<f32>,
    /// Whether to include audio in the output.
    pub include_audio: bool,
}

impl ExportOptions {
    pub fn from_preset(preset: Preset, output_path: std::path::PathBuf) -> Self;
}

/// Sink for progress updates. Implement for custom progress UI.
pub trait ProgressSink: Send {
    /// Called after each frame is encoded.
    fn on_frame(&mut self, frame: u64, total_estimate: Option<u64>);
    fn on_complete(&mut self, total_frames: u64, elapsed_secs: f64);
}

/// Terminal progress sink: prints "Frame N / M (P%) — ETA Xs".
pub struct TerminalProgress;
impl ProgressSink for TerminalProgress { /* ... */ }

/// Silent sink (for tests).
pub struct NullProgress;
impl ProgressSink for NullProgress { /* ... */ }
```

### Public API

```rust
/// Export a scene to MP4. Blocks until encoding is complete.
pub fn export(
    scene: &rphys_scene::Scene,
    options: ExportOptions,
    progress: &mut dyn ProgressSink,
) -> Result<(), ExportError>;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("ffmpeg not found — please install ffmpeg and ensure it is in PATH")]
    FfmpegNotFound,

    #[error("ffmpeg process failed (exit {code}): {stderr}")]
    FfmpegFailed { code: i32, stderr: String },

    #[error("Physics error: {0}")]
    Physics(#[from] rphys_physics::PhysicsError),

    #[error("Render error: {0}")]
    Render(String),

    #[error("Audio error: {0}")]
    Audio(#[from] rphys_audio::AudioError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Scene has no end condition and no --duration was specified")]
    NoDuration,
}
```

### Internal Export Pipeline

```rust
// Pseudocode for export() internals — not public API

fn export(scene, options, progress) -> Result<()> {
    let physics_steps_per_frame = PHYSICS_HZ / options.fps;
    let ctx = RenderContext::fit_to_world(options.width, options.height, &scene.environment.world_bounds);

    let mut engine  = PhysicsEngine::new(scene, PhysicsConfig::default())?;
    let mut renderer = TinySkiaRenderer::new();
    let mut audio    = OfflineAudioMixer::new(44100);
    let library      = preload_sounds(scene)?;

    let mut ffmpeg = spawn_ffmpeg(&options)?;   // piped child process
    let mut frame_n = 0u64;

    loop {
        let target_t = (frame_n + 1) as f32 / options.fps as f32;
        let events = engine.advance_to(target_t)?;

        for event in &events {
            let info = engine.body_info_from_event(event);
            audio.handle_event(event, info, &scene.audio, &library, engine.time())?;
        }

        let state = engine.state();
        let frame = renderer.render(&state, &ctx)?;
        ffmpeg.stdin.write_all(&frame.pixels)?;

        progress.on_frame(frame_n, estimate_total(&engine, &options));
        frame_n += 1;

        if engine.is_complete() || exceeds_max_duration(&engine, &options) {
            break;
        }
    }

    drop(ffmpeg.stdin); // signal EOF to ffmpeg

    if options.include_audio {
        let audio_buf = audio.flush();
        let wav_path  = write_temp_wav(&audio_buf)?;
        remux_audio(&options.output_path, &wav_path)?;
    }

    ffmpeg.wait()?;
    progress.on_complete(frame_n, ...);
    Ok(())
}
```

### ffmpeg Command Template

```
ffmpeg
  -y                              # overwrite output
  -f rawvideo
  -pixel_format rgba
  -video_size {W}x{H}
  -framerate {FPS}
  -i pipe:0                       # read frames from stdin
  [-i {audio.wav}]                # optional audio track
  -c:v libx264
  -preset fast
  -crf 18                         # near-lossless quality
  [-c:a aac -b:a 192k]           # audio codec
  -pix_fmt yuv420p                # universal playback compat
  -movflags +faststart            # web streaming optimisation
  {output.mp4}
```

### Dependencies
- `rphys-scene`, `rphys-physics`, `rphys-renderer`, `rphys-audio`
- `std::process` (spawn ffmpeg)
- `tempfile` (temp WAV path)
- `thiserror`

---

## `rphys-cli` — Binary Entry Point

### Responsibility
Parse CLI arguments, wire up the appropriate subsystems, surface errors as user-friendly messages.

### Subcommands

```rust
// Defined with clap derive macros

#[derive(clap::Parser)]
#[command(name = "rphys", about = "2D physics simulation renderer")]
struct Cli {
    #[command(subcommand)]
    command: Command,

    #[arg(long, global = true)]
    verbose: bool,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Open a live preview window for a scene file.
    Preview {
        file: std::path::PathBuf,

        #[arg(long, default_value = "540x960")]
        size: String,

        #[arg(long)]
        loop_on_complete: bool,
    },

    /// Export a scene to an MP4 video file.
    Render {
        file: std::path::PathBuf,

        #[arg(short, long)]
        output: std::path::PathBuf,

        #[arg(long, value_enum, default_value = "tiktok")]
        preset: CliPreset,

        #[arg(long)]
        width: Option<u32>,

        #[arg(long)]
        height: Option<u32>,

        #[arg(long)]
        fps: Option<u32>,

        #[arg(long)]
        duration: Option<String>,   // e.g. "10s", "1m30s"

        #[arg(long, default_value = "true")]
        audio: bool,
    },

    /// Validate a scene file without running simulation.
    Validate {
        file: std::path::PathBuf,
    },

    /// Print the JSON Schema for the scene format.
    Schema,

    /// Create an example scene.yaml in the current directory.
    Init {
        #[arg(default_value = "scene.yaml")]
        output: std::path::PathBuf,
    },
}

#[derive(clap::ValueEnum, Clone)]
enum CliPreset { TikTok, Youtube, Custom }
```

### Error Presentation

```rust
fn main() -> anyhow::Result<()> {
    // All errors become anyhow::Error with `.context()` chains,
    // printed by anyhow's default formatter.
    // e.g.:  "Failed to parse scene.yaml\nCaused by: YAML syntax error at line 12: ..."
}
```

### Dependencies
- `clap` (with derive feature)
- `anyhow`
- All workspace crates: `rphys-scene`, `rphys-physics`, `rphys-renderer`, `rphys-audio`, `rphys-preview`, `rphys-export`

---

## Testability Summary

| Crate | Test strategy |
|---|---|
| `rphys-scene` | Unit: parse valid + invalid YAML strings in `#[cfg(test)]` |
| `rphys-physics` | Unit: build world from minimal scene, step N times, assert state |
| `rphys-renderer` | Unit: render a single static body, assert pixel at known coordinate |
| `rphys-audio` | Unit: `OfflineAudioMixer` with mock events, assert buffer contains samples |
| `rphys-export` | Integration: export a short scene to a temp file, check MP4 exists + size > 0 |
| `rphys-preview` | Integration: run preview with `--headless` flag for CI (if implemented) |

All library crates can be tested without a GPU, display server, or audio device.
