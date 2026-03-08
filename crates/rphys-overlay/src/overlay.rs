//! [`OverlayRenderer`] — draws race UI elements into a [`Frame`] buffer.

use rphys_race::{FinishedEntry, RaceState, RacerStatus};
use rphys_renderer::{Frame, RenderContext};
use rphys_scene::{Color, RaceConfig};

use crate::{
    error::OverlayError,
    text::{blit_pixel, draw_hline, fill_rect, TextRenderer},
};

// ── Layout constants ──────────────────────────────────────────────────────────

/// Margin between the leaderboard panel and the frame edge (pixels).
const PANEL_MARGIN: i32 = 8;

/// Inner padding within the leaderboard panel (pixels).
const PANEL_PADDING: i32 = 8;

/// Height of each racer row in the leaderboard (pixels).
const ROW_HEIGHT: i32 = 24;

/// Width of the color chip square (pixels).
const CHIP_SIZE: i32 = 8;

/// Total panel width (pixels).
const PANEL_WIDTH: i32 = 220;

/// Font size for rank text in the leaderboard.
const RANK_FONT_SIZE: f32 = 18.0;

/// Font size for racer names in the leaderboard.
const NAME_FONT_SIZE: f32 = 14.0;

/// Font size for the winner name in the announcement panel.
const WINNER_FONT_SIZE: f32 = 36.0;

/// Font size for the subtitle in the announcement panel.
const SUBTITLE_FONT_SIZE: f32 = 20.0;

/// Background for the leaderboard panel: dark, semi-transparent black.
const PANEL_BG: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 180,
};

/// Background for the winner announcement: very dark, semi-transparent.
const WINNER_BG: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 210,
};

/// Color for the finish line (gold).
const FINISH_LINE_COLOR: Color = Color {
    r: 255,
    g: 215,
    b: 0,
    a: 220,
};

/// Color for checkpoint lines (translucent white).
const CHECKPOINT_LINE_COLOR: Color = Color {
    r: 255,
    g: 255,
    b: 255,
    a: 120,
};

/// Drop-shadow color for announcement text.
const SHADOW_COLOR: Color = Color {
    r: 0,
    g: 0,
    b: 0,
    a: 200,
};

// ── OverlayRenderer ───────────────────────────────────────────────────────────

/// Draws race UI elements directly into a [`Frame`] buffer.
///
/// Create once and reuse across frames. The embedded font is loaded once at
/// construction time — there is no I/O involved.
pub struct OverlayRenderer {
    text: TextRenderer,
}

impl OverlayRenderer {
    /// Construct a new overlay renderer, loading the bundled Roboto Bold font.
    ///
    /// This is cheap to call (the font is embedded at compile time; no I/O).
    pub fn new() -> Self {
        Self {
            text: TextRenderer::new(),
        }
    }

    /// Draw the full race overlay for a normal (in-progress) frame.
    ///
    /// Draws three layers onto `frame`:
    /// 1. Finish line — a gold dashed horizontal line at `race_config.finish_y`
    ///    in world space (may be off-screen if the camera hasn't reached it).
    /// 2. Checkpoint lines — translucent white lines at each checkpoint Y.
    /// 3. Rank leaderboard panel — top-right corner, dark semi-transparent
    ///    background, listing current standings with colored chips and names.
    pub fn draw_race_frame(
        &self,
        frame: &mut Frame,
        race_state: &RaceState,
        race_config: &RaceConfig,
        ctx: &RenderContext,
    ) -> Result<(), OverlayError> {
        // ── World-space lines (finish + checkpoints) ──────────────────────────
        self.draw_finish_line(frame, race_config.finish_y, ctx);

        for checkpoint in &race_config.checkpoints {
            self.draw_checkpoint_line(frame, checkpoint.y, checkpoint.label.as_deref(), ctx);
        }

        // ── Leaderboard panel ─────────────────────────────────────────────────
        self.draw_leaderboard(frame, race_state)?;

        Ok(())
    }

    /// Draw the winner announcement overlay for the final held frames.
    ///
    /// Composites a semi-transparent dark panel in the lower 40 % of the frame
    /// with the winner's name and finish time. If no winner is set yet, this
    /// is a no-op.
    pub fn draw_winner_announcement(
        &self,
        frame: &mut Frame,
        race_state: &RaceState,
    ) -> Result<(), OverlayError> {
        let winner = match &race_state.winner {
            Some(w) => w,
            None => return Ok(()),
        };

        let fw = frame.width as i32;
        let fh = frame.height as i32;

        // Panel: lower 40 % of the frame.
        let panel_h = (fh as f32 * 0.4) as i32;
        let panel_y = fh - panel_h;

        fill_rect(frame, 0, panel_y, fw, panel_h, WINNER_BG);

        // ── Winner name (large, centered) ─────────────────────────────────────
        let winner_text = format!("{} wins!", winner.display_name);
        let (tw, _th) = self.text.measure(&winner_text, WINNER_FONT_SIZE);
        let text_x = ((fw - tw as i32) / 2).max(PANEL_PADDING);
        let text_y = panel_y + PANEL_PADDING * 2;

        // Drop shadow (1 px offset, black).
        self.text.draw_text(
            frame,
            &winner_text,
            text_x + 1,
            text_y + 1,
            WINNER_FONT_SIZE,
            SHADOW_COLOR,
            0.85,
        );
        // Foreground in winner's color.
        self.text
            .draw_text(frame, &winner_text, text_x, text_y, WINNER_FONT_SIZE, winner.color, 1.0);

        // ── Subtitle: finish time ─────────────────────────────────────────────
        let subtitle = format!("Time: {:.2}s", winner.finish_time_secs);
        let (sw, _) = self.text.measure(&subtitle, SUBTITLE_FONT_SIZE);
        let sub_x = ((fw - sw as i32) / 2).max(PANEL_PADDING);
        let (_, wh) = self.text.measure(&winner_text, WINNER_FONT_SIZE);
        let sub_y = text_y + wh as i32 + PANEL_PADDING;

        self.text.draw_text(
            frame,
            &subtitle,
            sub_x + 1,
            sub_y + 1,
            SUBTITLE_FONT_SIZE,
            SHADOW_COLOR,
            0.85,
        );
        self.text
            .draw_text(frame, &subtitle, sub_x, sub_y, SUBTITLE_FONT_SIZE, Color::WHITE, 1.0);

        // ── Additional finishers ──────────────────────────────────────────────
        let mut line_y = sub_y + self.text.measure(&subtitle, SUBTITLE_FONT_SIZE).1 as i32
            + PANEL_PADDING;

        for entry in &race_state.finished {
            if entry.body_id == winner.body_id {
                continue; // winner already shown above
            }
            if line_y >= fh - PANEL_PADDING {
                break;
            }
            let entry_text = format!("{}. {}  {:.2}s", entry.finish_rank, entry.display_name, entry.finish_time_secs);
            let (ew, _) = self.text.measure(&entry_text, NAME_FONT_SIZE);
            let entry_x = ((fw - ew as i32) / 2).max(PANEL_PADDING);
            self.text
                .draw_text(frame, &entry_text, entry_x, line_y, NAME_FONT_SIZE, entry.color, 1.0);
            line_y += self.text.measure(&entry_text, NAME_FONT_SIZE).1 as i32 + 4;
        }

        Ok(())
    }

    // ── Private helpers ───────────────────────────────────────────────────────

    /// Draw the finish line at the given world Y coordinate.
    fn draw_finish_line(&self, frame: &mut Frame, finish_y: f32, ctx: &RenderContext) {
        let pixel_y = world_y_to_pixel(finish_y, ctx);
        if pixel_y < 0 || pixel_y >= frame.height as i32 {
            return;
        }

        draw_hline(
            frame,
            pixel_y,
            0,
            frame.width as i32,
            FINISH_LINE_COLOR,
            3,
        );

        // "FINISH" label 8 px above the line.
        let label = "FINISH";
        let (lw, _) = self.text.measure(label, 24.0);
        let label_x = ((frame.width as i32 - lw as i32) / 2).max(0);
        let label_y = (pixel_y - 32).max(0);
        self.text
            .draw_text(frame, label, label_x, label_y, 24.0, FINISH_LINE_COLOR, 1.0);
    }

    /// Draw a checkpoint line at the given world Y coordinate.
    fn draw_checkpoint_line(
        &self,
        frame: &mut Frame,
        checkpoint_y: f32,
        label: Option<&str>,
        ctx: &RenderContext,
    ) {
        let pixel_y = world_y_to_pixel(checkpoint_y, ctx);
        if pixel_y < 0 || pixel_y >= frame.height as i32 {
            return;
        }

        draw_hline(
            frame,
            pixel_y,
            0,
            frame.width as i32,
            CHECKPOINT_LINE_COLOR,
            2,
        );

        if let Some(label_text) = label {
            let (lw, _) = self.text.measure(label_text, 16.0);
            let label_x = (frame.width as i32 - lw as i32 - PANEL_MARGIN).max(0);
            let label_y = (pixel_y - 20).max(0);
            self.text.draw_text(
                frame,
                label_text,
                label_x,
                label_y,
                16.0,
                CHECKPOINT_LINE_COLOR,
                1.0,
            );
        }
    }

    /// Draw the leaderboard panel in the top-right corner.
    fn draw_leaderboard(&self, frame: &mut Frame, race_state: &RaceState) -> Result<(), OverlayError> {
        let total_racers = race_state.finished.len() + race_state.active.len();
        if total_racers == 0 {
            return Ok(());
        }

        // Cap display at 8 rows to prevent panel overflow.
        let display_count = total_racers.min(8);
        let panel_h = display_count as i32 * ROW_HEIGHT + PANEL_PADDING * 2;
        let panel_x = frame.width as i32 - PANEL_WIDTH - PANEL_MARGIN;
        let panel_y = PANEL_MARGIN;

        fill_rect(frame, panel_x, panel_y, PANEL_WIDTH, panel_h, PANEL_BG);

        // Build ordered list: finished (by rank) then active (by rank).
        // We iterate finished first, then active. Both are already sorted.
        let mut drawn = 0usize;

        for entry in &race_state.finished {
            if drawn >= display_count {
                break;
            }
            let row_y = panel_y + PANEL_PADDING + drawn as i32 * ROW_HEIGHT;
            draw_leaderboard_row_finished(frame, &self.text, panel_x, row_y, entry);
            drawn += 1;
        }

        for racer in &race_state.active {
            if drawn >= display_count {
                break;
            }
            let row_y = panel_y + PANEL_PADDING + drawn as i32 * ROW_HEIGHT;
            draw_leaderboard_row_active(frame, &self.text, panel_x, row_y, racer);
            drawn += 1;
        }

        // If total racers exceeded 8, show "…" indicator.
        if total_racers > 8 {
            let row_y = panel_y + PANEL_PADDING + 8 * ROW_HEIGHT;
            if row_y < frame.height as i32 {
                let more_text = format!("+ {} more", total_racers - 8);
                self.text.draw_text(
                    frame,
                    &more_text,
                    panel_x + PANEL_PADDING,
                    row_y,
                    NAME_FONT_SIZE,
                    Color::rgba(200, 200, 200, 200),
                    0.8,
                );
            }
        }

        Ok(())
    }
}

impl Default for OverlayRenderer {
    fn default() -> Self {
        Self::new()
    }
}

// ── Row-drawing helpers ───────────────────────────────────────────────────────

/// Draw a leaderboard row for a racer who has already finished.
fn draw_leaderboard_row_finished(
    frame: &mut Frame,
    text: &TextRenderer,
    panel_x: i32,
    row_y: i32,
    entry: &FinishedEntry,
) {
    // Color chip.
    let chip_x = panel_x + PANEL_PADDING;
    let chip_y = row_y + (ROW_HEIGHT - CHIP_SIZE) / 2;
    draw_color_chip(frame, chip_x, chip_y, entry.color);

    // Rank + name + time.
    let label = format!(
        "{}. {}  {:.1}s",
        entry.finish_rank, entry.display_name, entry.finish_time_secs
    );
    let text_x = chip_x + CHIP_SIZE + 6;
    let text_y = row_y + (ROW_HEIGHT - NAME_FONT_SIZE as i32) / 2;

    // Dimmed to indicate the racer is done.
    let dimmed_color = Color::rgba(entry.color.r, entry.color.g, entry.color.b, 160);
    text.draw_text(frame, &label, text_x, text_y, NAME_FONT_SIZE, dimmed_color, 0.85);
}

/// Draw a leaderboard row for an active (still-racing) racer.
fn draw_leaderboard_row_active(
    frame: &mut Frame,
    text: &TextRenderer,
    panel_x: i32,
    row_y: i32,
    racer: &RacerStatus,
) {
    // Color chip.
    let chip_x = panel_x + PANEL_PADDING;
    let chip_y = row_y + (ROW_HEIGHT - CHIP_SIZE) / 2;
    draw_color_chip(frame, chip_x, chip_y, racer.color);

    // Rank + name.
    let label = format!("{}. {}", racer.rank, racer.display_name);
    let text_x = chip_x + CHIP_SIZE + 6;
    let text_y = row_y + (ROW_HEIGHT - RANK_FONT_SIZE as i32) / 2;

    text.draw_text(frame, &label, text_x, text_y, RANK_FONT_SIZE, racer.color, 1.0);
}

/// Draw an 8×8 filled color chip at `(x, y)`.
fn draw_color_chip(frame: &mut Frame, x: i32, y: i32, color: Color) {
    let fw = frame.width as i32;
    let fh = frame.height as i32;

    for dy in 0..CHIP_SIZE {
        let py = y + dy;
        if py < 0 || py >= fh {
            continue;
        }
        for dx in 0..CHIP_SIZE {
            let px = x + dx;
            if px < 0 || px >= fw {
                continue;
            }
            blit_pixel(frame, px as u32, py as u32, color, 1.0);
        }
    }
}

// ── Coordinate conversion ─────────────────────────────────────────────────────

/// Convert a world-space Y coordinate to a pixel-space row index.
///
/// Mirrors the renderer's coordinate convention:
/// ```text
/// pixel_y = frame_height - (world_y - camera_origin.y) * scale
/// ```
fn world_y_to_pixel(world_y: f32, ctx: &RenderContext) -> i32 {
    (ctx.height as f32 - (world_y - ctx.camera_origin.y) * ctx.scale) as i32
}
