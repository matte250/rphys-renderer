//! VFX configuration — public re-exports and helpers.
//!
//! The canonical type definitions live in [`crate::types`].  This module
//! re-exports them so that downstream crates can import everything from
//! `rphys_scene::vfx::*` (or directly via `rphys_scene::VfxConfig`).
//!
//! # YAML usage
//!
//! Add a `vfx:` block at the scene root to enable visual effects:
//!
//! ```yaml
//! vfx:
//!   max_particles: 500
//!   impact_sparks:
//!     enabled: true
//!     count: 12
//!   boost_flash:
//!     enabled: true
//!     color: "#FFD700"
//!   winner_pop:
//!     enabled: true
//!     count: 60
//! ```
//!
//! When the `vfx:` key is absent, [`Scene::vfx`](crate::Scene::vfx) is `None`
//! and no VFX code paths run (zero performance overhead on legacy scenes).

pub use crate::types::validate_vfx_config;
pub use crate::types::{
    BoostFlashConfig, EliminationBurstConfig, ImpactSparksConfig, VfxConfig, VfxConfigError,
    WinnerPopConfig,
};

/// Parse a `"#RRGGBB"` or `"#RRGGBBAA"` hex color string into `(r, g, b, a)`.
///
/// Returns [`VfxConfigError::InvalidColor`] when the string is malformed.
///
/// # Errors
///
/// - Missing `#` prefix
/// - Wrong length (must be 6 or 8 hex digits after `#`)
/// - Non-hex characters
pub fn parse_hex_color_vfx(
    s: &str,
    field: &'static str,
) -> Result<(u8, u8, u8, u8), VfxConfigError> {
    let bad = || VfxConfigError::InvalidColor {
        field,
        value: s.to_string(),
    };
    let hex = s.strip_prefix('#').ok_or_else(bad)?;
    match hex.len() {
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| bad())?;
            let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| bad())?;
            let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| bad())?;
            Ok((r, g, b, 255))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).map_err(|_| bad())?;
            let g = u8::from_str_radix(&hex[2..4], 16).map_err(|_| bad())?;
            let b = u8::from_str_radix(&hex[4..6], 16).map_err(|_| bad())?;
            let a = u8::from_str_radix(&hex[6..8], 16).map_err(|_| bad())?;
            Ok((r, g, b, a))
        }
        _ => Err(bad()),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vfx_config_default_all_disabled() {
        let cfg = VfxConfig::default();
        assert!(!cfg.impact_sparks.enabled);
        assert!(!cfg.boost_flash.enabled);
        assert!(!cfg.elimination_burst.enabled);
        assert!(!cfg.winner_pop.enabled);
        assert_eq!(cfg.max_particles, 500);
    }

    #[test]
    fn test_validate_ok_with_defaults() {
        assert!(validate_vfx_config(&VfxConfig::default()).is_ok());
    }

    #[test]
    fn test_validate_rejects_zero_max_particles() {
        let mut cfg = VfxConfig::default();
        cfg.max_particles = 0;
        assert!(validate_vfx_config(&cfg).is_err());
    }

    #[test]
    fn test_parse_hex_color_rgb() {
        let (r, g, b, a) = parse_hex_color_vfx("#FF8800", "test").unwrap();
        assert_eq!((r, g, b, a), (0xFF, 0x88, 0x00, 0xFF));
    }

    #[test]
    fn test_parse_hex_color_rgba() {
        let (_r, _g, _b, a) = parse_hex_color_vfx("#FF880080", "test").unwrap();
        assert_eq!(a, 0x80);
    }

    #[test]
    fn test_parse_hex_color_no_hash_fails() {
        assert!(parse_hex_color_vfx("FF8800", "test").is_err());
    }

    #[test]
    fn test_validate_enabled_impact_sparks_zero_lifetime_rejected() {
        let mut cfg = VfxConfig::default();
        cfg.impact_sparks.enabled = true;
        cfg.impact_sparks.lifetime_secs = 0.0;
        let err = validate_vfx_config(&cfg).unwrap_err();
        assert!(err.to_string().contains("lifetime_secs"));
    }

    #[test]
    fn test_validate_enabled_winner_pop_bad_spread_rejected() {
        let mut cfg = VfxConfig::default();
        cfg.winner_pop.enabled = true;
        cfg.winner_pop.spread_deg = 0.0;
        assert!(validate_vfx_config(&cfg).is_err());
    }

    #[test]
    fn test_validate_boost_flash_zero_radius_rejected() {
        let mut cfg = VfxConfig::default();
        cfg.boost_flash.enabled = true;
        cfg.boost_flash.radius_px = 0.0;
        assert!(validate_vfx_config(&cfg).is_err());
    }

    #[test]
    fn test_validate_elimination_burst_zero_count_rejected() {
        let mut cfg = VfxConfig::default();
        cfg.elimination_burst.enabled = true;
        cfg.elimination_burst.count = 0;
        assert!(validate_vfx_config(&cfg).is_err());
    }

    #[test]
    fn test_validate_winner_pop_boundary_spread_ok() {
        let mut cfg = VfxConfig::default();
        cfg.winner_pop.enabled = true;
        // spread_deg = 360.0 is the max valid value
        cfg.winner_pop.spread_deg = 360.0;
        assert!(validate_vfx_config(&cfg).is_ok());
    }
}
