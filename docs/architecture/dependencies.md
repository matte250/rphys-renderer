# Dependencies — rphys-renderer

Recommended crates for the MVP, with rationale.

> **⚠️ IMPORTANT: Always verify latest versions on crates.io before implementing.**
> The version numbers below are approximate — they reflect the time of writing, NOT necessarily the latest release. Before adding any dependency to Cargo.toml, run `cargo search <crate>` or check https://crates.io/<crate> for the actual latest stable version.

## Core

| Crate | Version | Used by | Purpose |
|---|---|---|---|
| `rapier2d` | 0.22+ | rphys-physics | 2D physics engine. Deterministic, performant, Rust-native. Has built-in collision detection, rigid body dynamics, and event reporting. |
| `serde` | 1.x | rphys-scene | Serialization framework. Required for YAML parsing. |
| `serde_yaml` | 0.9+ | rphys-scene | YAML deserialization backend for serde. |
| `tiny-skia` | 0.11+ | rphys-renderer | CPU software 2D renderer. No GPU dependency — works headless for export. Supports paths, fills, anti-aliasing. |
| `winit` | 0.30+ | rphys-preview | Cross-platform windowing. Handles event loop, keyboard input, window creation. |
| `pixels` | 0.14+ | rphys-preview | Framebuffer-to-window bridge. Takes raw RGBA pixels and presents them via wgpu surface. |
| `rodio` | 0.19+ | rphys-audio | Real-time audio playback. Simple API for mixing multiple sounds. |
| `symphonia` | 0.5+ | rphys-audio | Audio decoding (WAV, MP3, OGG). Used by rodio internally. |
| `hound` | 3.x | rphys-audio | WAV file writing for offline audio export. |
| `clap` | 4.x | rphys-cli | CLI argument parsing with derive macros. |
| `notify` | 6.x | rphys-preview | File system watcher for hot-reload. Cross-platform inotify/FSEvents/ReadDirectoryChanges. |

## Error Handling

| Crate | Used by | Purpose |
|---|---|---|
| `thiserror` | all library crates | Derive macro for structured error types. |
| `anyhow` | rphys-cli | Flexible error handling at the application boundary. |

## Export

| Crate | Used by | Purpose |
|---|---|---|
| `tempfile` | rphys-export | Temporary file creation for intermediate WAV during muxing. |

**ffmpeg** (external binary, not a Rust crate) is used for video encoding. Called via `std::process::Command`. Must be in PATH. This avoids pulling in heavy native encoding libraries.

## Why These Choices

**rapier2d over custom physics:** Rapier is the standard Rust physics engine. Deterministic mode, active maintenance, great docs. Writing our own collision detection would delay MVP by weeks.

**tiny-skia over wgpu for rendering:** CPU rendering avoids GPU driver issues in headless/export mode. For the clean/minimal MVP style (solid shapes, no shaders), CPU is fast enough. GPU rendering (wgpu) can be added later as a second `Renderer` impl for neon/glow effects.

**pixels for windowing:** Bridges the gap between our CPU-rendered frames and the window surface. Uses wgpu under the hood but we don't interact with it directly.

**ffmpeg over native encoding:** Video encoding libraries in Rust (e.g., `openh264`, `rav1e`) are either incomplete or add massive compile-time dependencies. Piping frames to ffmpeg is battle-tested and supports every codec/format combination.

## Workspace Layout

```toml
# Cargo.toml (workspace root)
[workspace]
members = [
    "crates/rphys-scene",
    "crates/rphys-physics",
    "crates/rphys-renderer",
    "crates/rphys-audio",
    "crates/rphys-preview",
    "crates/rphys-export",
    "crates/rphys-cli",
]

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
thiserror = "2"
rapier2d = { version = "0.22", features = ["enhanced-determinism"] }
tiny-skia = "0.11"
clap = { version = "4", features = ["derive"] }
anyhow = "1"
```
