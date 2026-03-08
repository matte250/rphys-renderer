//! Error types for `rphys-overlay`.

/// Errors that can occur while drawing race overlays.
#[derive(Debug, thiserror::Error)]
pub enum OverlayError {
    /// The embedded font failed to initialise.
    ///
    /// This should never fire in practice since the font is baked into the
    /// binary at compile time, but is included for defensive correctness.
    #[error("Font initialization failed: {0}")]
    FontInit(String),

    /// A text rasterization step failed.
    #[error("Text rasterization failed: {0}")]
    Rasterize(String),
}
