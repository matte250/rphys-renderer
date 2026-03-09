//! Error types for `rphys-vfx`.

use rphys_scene::VfxConfigError;

/// Top-level error type for the VFX engine.
#[derive(Debug, thiserror::Error)]
pub enum VfxError {
    /// VFX configuration validation failed.
    #[error("VFX config error: {0}")]
    Config(#[from] VfxConfigError),
}
