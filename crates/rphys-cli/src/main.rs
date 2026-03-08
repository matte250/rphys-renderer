//! `rphys` — 2D physics simulation renderer CLI.
//!
//! Parses CLI arguments and dispatches to the appropriate subsystem.
//!
//! # Subcommands
//!
//! - `export`  — render a scene file to an MP4 video
//! - `preview` — open a live preview window (stub; not yet implemented)

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

// ── Top-level CLI ─────────────────────────────────────────────────────────────

/// 2D physics simulation renderer.
#[derive(Debug, Parser)]
#[command(name = "rphys", about = "2D physics simulation renderer", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// Enable verbose logging.
    #[arg(long, global = true)]
    verbose: bool,
}

// ── Subcommands ───────────────────────────────────────────────────────────────

#[derive(Debug, Subcommand)]
enum Command {
    /// Export a scene to an MP4 video file.
    Export {
        /// Path to the `.yaml` scene file (positional).
        ///
        /// Either this or `--scene` must be provided.
        file: Option<PathBuf>,

        /// Path to the `.yaml` scene file (named alternative to the positional argument).
        ///
        /// Takes precedence over the positional `file` argument when both are given.
        #[arg(short = 's', long = "scene")]
        scene: Option<PathBuf>,

        /// Named export preset (controls resolution and frame rate).
        #[arg(short, long, value_enum, default_value = "tiktok")]
        preset: CliPreset,

        /// Destination output file (e.g. `out.mp4`).
        #[arg(short, long)]
        output: PathBuf,

        /// Override output width in pixels.
        #[arg(long)]
        width: Option<u32>,

        /// Override output height in pixels.
        #[arg(long)]
        height: Option<u32>,

        /// Override frame rate.
        #[arg(long)]
        fps: Option<u32>,

        /// Maximum simulation duration in seconds.
        /// Required when the scene has no end condition.
        #[arg(long)]
        duration: Option<f32>,

        /// Path to the `ffmpeg` binary.
        ///
        /// When not provided, `ffmpeg` is looked up on `PATH`.
        #[arg(long)]
        ffmpeg: Option<PathBuf>,
    },

    /// Open a live preview window for a scene file.
    ///
    /// NOTE: not yet implemented.
    Preview {
        /// Path to the `.yaml` scene file.
        file: PathBuf,
    },
}

// ── Preset value enum ─────────────────────────────────────────────────────────

/// Named export preset.
#[derive(Debug, Clone, ValueEnum)]
#[value(rename_all = "lower")]
enum CliPreset {
    /// 1080×1920 @ 60 fps (TikTok / YouTube Shorts / Instagram Reels).
    TikTok,
    /// 1920×1080 @ 60 fps (YouTube landscape).
    Youtube,
    /// Custom resolution — use `--width`, `--height`, and `--fps` to override.
    Custom,
}

impl From<CliPreset> for rphys_export::Preset {
    fn from(preset: CliPreset) -> Self {
        match preset {
            CliPreset::TikTok => rphys_export::Preset::TikTok,
            CliPreset::Youtube => rphys_export::Preset::YouTube,
            CliPreset::Custom => rphys_export::Preset::Custom,
        }
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Export {
            file,
            scene,
            preset,
            output,
            width,
            height,
            fps,
            duration,
            ffmpeg,
        } => {
            // `--scene` takes precedence over the positional `file` argument.
            let resolved_file = scene.or(file).ok_or_else(|| {
                anyhow::anyhow!(
                    "A scene file is required. Provide it as a positional argument or via --scene <PATH>."
                )
            })?;
            run_export(
                resolved_file,
                preset,
                output,
                width,
                height,
                fps,
                duration,
                ffmpeg,
            )
        }
        Command::Preview { file } => run_preview(file),
    }
}

// ── Export handler ────────────────────────────────────────────────────────────

/// Parse `scene_path` and export it to `output_path` using the given options.
#[allow(clippy::too_many_arguments)]
fn run_export(
    scene_path: PathBuf,
    preset: CliPreset,
    output_path: PathBuf,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<u32>,
    duration: Option<f32>,
    ffmpeg: Option<PathBuf>,
) -> Result<()> {
    let scene = rphys_scene::parse_scene_file(&scene_path)
        .with_context(|| format!("Failed to parse scene file '{}'", scene_path.display()))?;

    let export_preset = rphys_export::Preset::from(preset);
    let mut options = rphys_export::ExportOptions::from_preset(export_preset, output_path.clone());

    // Apply any CLI overrides on top of the preset defaults.
    if let Some(w) = width {
        options.width = w;
    }
    if let Some(h) = height {
        options.height = h;
    }
    if let Some(f) = fps {
        options.fps = f;
    }
    if let Some(d) = duration {
        options.max_duration = Some(d);
    }
    if let Some(ffmpeg_path) = ffmpeg {
        options.ffmpeg_path = Some(ffmpeg_path);
    }

    rphys_export::export(&scene, options)
        .with_context(|| format!("Export to '{}' failed", output_path.display()))?;

    println!("Export complete: {}", output_path.display());
    Ok(())
}

// ── Preview handler (stub) ────────────────────────────────────────────────────

/// Live preview stub — prints a message and exits cleanly.
fn run_preview(_scene_path: PathBuf) -> Result<()> {
    println!("preview not yet implemented");
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// Verify that the clap CLI definition itself is valid (catches mis-configured
    /// args at test time rather than at runtime).
    #[test]
    fn test_cli_definition_is_valid() {
        Cli::command().debug_assert();
    }

    /// `export` subcommand with all required args parses without error.
    #[test]
    fn test_export_args_parse() {
        let cli = Cli::try_parse_from([
            "rphys",
            "export",
            "scene.yaml",
            "--output",
            "out.mp4",
            "--preset",
            "tiktok",
        ])
        .expect("should parse");

        match cli.command {
            Command::Export {
                file,
                output,
                preset,
                ..
            } => {
                assert_eq!(file, Some(PathBuf::from("scene.yaml")));
                assert_eq!(output, PathBuf::from("out.mp4"));
                assert!(matches!(preset, CliPreset::TikTok));
            }
            _ => panic!("expected Export command"),
        }
    }

    /// `--scene` named argument is accepted as an alternative to the positional file.
    #[test]
    fn test_export_scene_flag_parses() {
        let cli = Cli::try_parse_from([
            "rphys",
            "export",
            "--scene",
            "race.yaml",
            "--output",
            "out.mp4",
            "--preset",
            "tiktok",
        ])
        .expect("should parse with --scene flag");

        match cli.command {
            Command::Export { file, scene, .. } => {
                assert_eq!(scene, Some(PathBuf::from("race.yaml")));
                assert!(
                    file.is_none(),
                    "--scene should not populate positional file"
                );
            }
            _ => panic!("expected Export command"),
        }
    }

    /// `--ffmpeg` flag is accepted and parsed correctly.
    #[test]
    fn test_export_ffmpeg_flag_parses() {
        let cli = Cli::try_parse_from([
            "rphys",
            "export",
            "scene.yaml",
            "--output",
            "out.mp4",
            "--ffmpeg",
            "/usr/local/bin/ffmpeg",
        ])
        .expect("should parse with --ffmpeg flag");

        match cli.command {
            Command::Export { ffmpeg, .. } => {
                assert_eq!(ffmpeg, Some(PathBuf::from("/usr/local/bin/ffmpeg")));
            }
            _ => panic!("expected Export command"),
        }
    }

    /// `preview` subcommand parses the file path correctly.
    #[test]
    fn test_preview_args_parse() {
        let cli = Cli::try_parse_from(["rphys", "preview", "scene.yaml"]).expect("should parse");

        match cli.command {
            Command::Preview { file } => {
                assert_eq!(file, PathBuf::from("scene.yaml"));
            }
            _ => panic!("expected Preview command"),
        }
    }

    /// Optional overrides (width, height, fps, duration) are accepted.
    #[test]
    fn test_export_optional_overrides_parse() {
        let cli = Cli::try_parse_from([
            "rphys",
            "export",
            "scene.yaml",
            "--output",
            "out.mp4",
            "--preset",
            "custom",
            "--width",
            "1280",
            "--height",
            "720",
            "--fps",
            "30",
            "--duration",
            "15",
        ])
        .expect("should parse");

        match cli.command {
            Command::Export {
                width,
                height,
                fps,
                duration,
                preset,
                ffmpeg,
                ..
            } => {
                assert_eq!(width, Some(1280));
                assert_eq!(height, Some(720));
                assert_eq!(fps, Some(30));
                assert!((duration.unwrap() - 15.0).abs() < 1e-5);
                assert!(matches!(preset, CliPreset::Custom));
                assert!(ffmpeg.is_none());
            }
            _ => panic!("expected Export command"),
        }
    }

    /// `--preset youtube` maps to the YouTube variant.
    #[test]
    fn test_preset_youtube_maps_correctly() {
        let cli = Cli::try_parse_from([
            "rphys",
            "export",
            "scene.yaml",
            "--output",
            "out.mp4",
            "--preset",
            "youtube",
        ])
        .expect("should parse");

        match cli.command {
            Command::Export { preset, .. } => {
                assert!(matches!(preset, CliPreset::Youtube));
                let export_preset = rphys_export::Preset::from(preset);
                assert!(matches!(export_preset, rphys_export::Preset::YouTube));
            }
            _ => panic!("expected Export command"),
        }
    }

    /// Smoke test: `--scene` flag routes to the correct scene file.
    #[test]
    fn test_scene_flag_is_used_when_no_positional_file() {
        let cli = Cli::try_parse_from([
            "rphys",
            "export",
            "--scene",
            "/path/to/race.yaml",
            "--output",
            "out.mp4",
        ])
        .expect("should parse");

        match cli.command {
            Command::Export { file, scene, .. } => {
                // Positional file must be None; --scene must be Some.
                assert!(file.is_none());
                assert_eq!(scene, Some(PathBuf::from("/path/to/race.yaml")));
            }
            _ => panic!("expected Export command"),
        }
    }

    /// The global `--verbose` flag is accepted on any subcommand.
    #[test]
    fn test_verbose_global_flag() {
        let cli = Cli::try_parse_from(["rphys", "--verbose", "preview", "scene.yaml"])
            .expect("should parse");
        assert!(cli.verbose);
    }
}
