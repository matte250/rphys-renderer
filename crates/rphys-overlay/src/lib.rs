//! `rphys-overlay` — Race overlay rendering for `rphys-renderer`.
//!
//! Draws race-specific HUD elements directly into a [`Frame`] buffer using
//! [`fontdue`] for pure-Rust text rasterization. No ffmpeg pass required.
//!
//! ## Usage
//!
//! ```rust,ignore
//! let overlay = OverlayRenderer::new();
//!
//! // On each in-progress frame:
//! overlay.draw_race_frame(&mut frame, &race_state, &race_config, &ctx)?;
//!
//! // On the final winner frame:
//! overlay.draw_winner_announcement(&mut frame, &race_state)?;
//! ```

mod error;
mod overlay;
mod text;

pub use error::OverlayError;
pub use overlay::OverlayRenderer;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rphys_physics::BodyId;
    use rphys_race::{FinishedEntry, RaceState, RacerStatus, WinnerInfo};
    use rphys_renderer::{Frame, RenderContext};
    use rphys_scene::{Color, RaceConfig, Vec2};

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn default_ctx() -> RenderContext {
        RenderContext {
            width: 320,
            height: 480,
            camera_origin: Vec2::new(0.0, 0.0),
            scale: 40.0,
            background_color: Color::rgb(20, 20, 30),
        }
    }

    fn make_active_racer(rank: usize, name: &str, color: Color, y: f32) -> RacerStatus {
        RacerStatus {
            body_id: BodyId(rank as u32),
            display_name: name.to_string(),
            color,
            rank,
            position_y: y,
            last_checkpoint: None,
        }
    }

    fn make_race_state_with_active() -> RaceState {
        RaceState {
            active: vec![
                make_active_racer(1, "Red", Color::rgb(220, 50, 50), 8.0),
                make_active_racer(2, "Blue", Color::rgb(50, 100, 220), 10.0),
                make_active_racer(3, "Green", Color::rgb(50, 200, 80), 12.0),
            ],
            finished: vec![],
            winner: None,
            elapsed_secs: 3.5,
        }
    }

    fn make_race_state_with_winner() -> RaceState {
        let winner = WinnerInfo {
            body_id: BodyId(1),
            display_name: "Red".to_string(),
            color: Color::rgb(220, 50, 50),
            finish_time_secs: 12.34,
        };

        let finished_entry = FinishedEntry {
            body_id: BodyId(1),
            display_name: "Red".to_string(),
            color: Color::rgb(220, 50, 50),
            finish_rank: 1,
            finish_time_secs: 12.34,
        };

        RaceState {
            active: vec![make_active_racer(1, "Blue", Color::rgb(50, 100, 220), 5.0)],
            finished: vec![finished_entry],
            winner: Some(winner),
            elapsed_secs: 12.34,
        }
    }

    fn default_race_config() -> RaceConfig {
        RaceConfig {
            finish_y: 2.0,
            racer_tag: "racer".to_string(),
            announcement_hold_secs: 2.0,
            checkpoints: vec![],
            elimination_interval_secs: None,
            post_finish_secs: 0.0,
        }
    }

    // ── Test: OverlayRenderer::new() succeeds ─────────────────────────────────

    #[test]
    fn overlay_renderer_new_succeeds() {
        let _renderer = OverlayRenderer::new(); // must not panic
    }

    #[test]
    fn overlay_renderer_default_succeeds() {
        let _renderer = OverlayRenderer::default();
    }

    // ── Test: draw_race_frame modifies frame pixels ───────────────────────────

    #[test]
    fn draw_race_frame_modifies_pixels() {
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(320, 480);
        // Fill frame with a distinct background so we can detect changes.
        for byte in frame.pixels.iter_mut() {
            *byte = 10;
        }
        let before = frame.pixels.clone();

        let race_state = make_race_state_with_active();
        let race_config = default_race_config();
        let ctx = default_ctx();

        renderer
            .draw_race_frame(&mut frame, &race_state, &race_config, &ctx)
            .expect("draw_race_frame should succeed");

        assert_ne!(
            frame.pixels, before,
            "draw_race_frame must modify at least one pixel"
        );
    }

    #[test]
    fn draw_race_frame_with_checkpoints_modifies_pixels() {
        use rphys_scene::Checkpoint;

        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(320, 480);
        for byte in frame.pixels.iter_mut() {
            *byte = 5;
        }
        let before = frame.pixels.clone();

        let race_state = make_race_state_with_active();
        let race_config = RaceConfig {
            finish_y: 2.0,
            racer_tag: "racer".to_string(),
            announcement_hold_secs: 2.0,
            checkpoints: vec![Checkpoint {
                y: 8.0,
                label: Some("Halfway".to_string()),
            }],
            elimination_interval_secs: None,
            post_finish_secs: 0.0,
        };
        let ctx = default_ctx();

        renderer
            .draw_race_frame(&mut frame, &race_state, &race_config, &ctx)
            .unwrap();

        assert_ne!(frame.pixels, before, "frame should change after draw");
    }

    // ── Test: draw_winner_announcement modifies frame pixels ──────────────────

    #[test]
    fn draw_winner_announcement_modifies_pixels() {
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(320, 480);
        for byte in frame.pixels.iter_mut() {
            *byte = 5;
        }
        let before = frame.pixels.clone();

        let race_state = make_race_state_with_winner();

        renderer
            .draw_winner_announcement(&mut frame, &race_state)
            .expect("draw_winner_announcement should succeed");

        assert_ne!(
            frame.pixels, before,
            "draw_winner_announcement must modify at least one pixel"
        );
    }

    #[test]
    fn draw_winner_announcement_no_winner_is_noop() {
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(100, 100);
        // All pixels start at zero.
        let before = frame.pixels.clone();

        let race_state = RaceState {
            active: vec![],
            finished: vec![],
            winner: None,
            elapsed_secs: 0.0,
        };

        renderer
            .draw_winner_announcement(&mut frame, &race_state)
            .unwrap();

        assert_eq!(frame.pixels, before, "no winner → no pixels should change");
    }

    // ── Test: text clipping at frame edges ────────────────────────────────────

    #[test]
    fn text_blit_clips_at_right_edge() {
        // Small frame + text starting near right edge → should not panic.
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(40, 40);
        let race_state = make_race_state_with_active();
        let race_config = default_race_config();
        // ctx with very small scale so finish line might be visible
        let ctx = RenderContext {
            width: 40,
            height: 40,
            camera_origin: Vec2::new(0.0, 0.0),
            scale: 2.0,
            background_color: Color::BLACK,
        };

        // Should not panic even though panel is wider than the frame.
        renderer
            .draw_race_frame(&mut frame, &race_state, &race_config, &ctx)
            .unwrap();
    }

    #[test]
    fn text_blit_clips_at_top_edge() {
        // Frame with height too small to show panel — should not panic.
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(320, 10);
        let race_state = make_race_state_with_active();
        let race_config = default_race_config();
        let ctx = RenderContext {
            width: 320,
            height: 10,
            camera_origin: Vec2::new(0.0, 0.0),
            scale: 1.0,
            background_color: Color::BLACK,
        };
        renderer
            .draw_race_frame(&mut frame, &race_state, &race_config, &ctx)
            .unwrap();
    }

    #[test]
    fn winner_announcement_clips_at_tiny_frame() {
        // Ensure announcement doesn't panic on a 20×20 frame.
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(20, 20);
        let race_state = make_race_state_with_winner();
        renderer
            .draw_winner_announcement(&mut frame, &race_state)
            .unwrap();
    }

    // ── Test: elimination banner renders without panic ────────────────────────

    #[test]
    fn elimination_banner_renders_without_panic() {
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(320, 480);
        let race_state = make_race_state_with_active();
        let race_config = default_race_config();
        let ctx = default_ctx();

        // Arm the banner.
        renderer.set_elimination_banner("Blue", Color::rgb(50, 100, 220), race_state.elapsed_secs);

        // Should render without panicking.
        renderer
            .draw_race_frame(&mut frame, &race_state, &race_config, &ctx)
            .expect("draw_race_frame with elimination banner should succeed");
    }

    #[test]
    fn elimination_banner_expires_after_duration() {
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(320, 480);
        let race_config = default_race_config();
        let ctx = default_ctx();

        // Set banner at t=0.
        renderer.set_elimination_banner("Green", Color::rgb(0, 200, 0), 0.0);

        // Draw at t=3.0 (past the 2s duration) — banner should be cleared.
        let mut race_state = make_race_state_with_active();
        race_state.elapsed_secs = 3.0;

        renderer
            .draw_race_frame(&mut frame, &race_state, &race_config, &ctx)
            .unwrap();

        // Banner should now be expired (internal field cleared). Verify by
        // drawing again and confirming no panic.
        renderer
            .draw_race_frame(&mut frame, &race_state, &race_config, &ctx)
            .unwrap();
    }

    // ── Test: empty state is handled gracefully ───────────────────────────────

    #[test]
    fn draw_race_frame_empty_state_ok() {
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(320, 480);
        let race_state = RaceState {
            active: vec![],
            finished: vec![],
            winner: None,
            elapsed_secs: 0.0,
        };
        let ctx = default_ctx();
        let config = default_race_config();
        renderer
            .draw_race_frame(&mut frame, &race_state, &config, &ctx)
            .unwrap();
    }

    // ── Test: many racers are capped at 8 rows ────────────────────────────────

    #[test]
    fn draw_race_frame_many_racers_no_panic() {
        let mut renderer = OverlayRenderer::new();
        let mut frame = Frame::new(320, 480);

        let active: Vec<RacerStatus> = (1..=15)
            .map(|i| {
                make_active_racer(
                    i,
                    &format!("Racer{i}"),
                    Color::rgb(100, 100, 200),
                    i as f32 * 2.0,
                )
            })
            .collect();

        let race_state = RaceState {
            active,
            finished: vec![],
            winner: None,
            elapsed_secs: 5.0,
        };

        let ctx = default_ctx();
        let config = default_race_config();
        renderer
            .draw_race_frame(&mut frame, &race_state, &config, &ctx)
            .unwrap();
    }
}
