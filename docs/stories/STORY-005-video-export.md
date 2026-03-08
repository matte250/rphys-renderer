# STORY-005: Video Export Mode

## Description
As a content creator, I want to export my simulation as an MP4 video file suitable for uploading to TikTok, YouTube Shorts, and similar platforms.

## Acceptance Criteria
- [ ] Given a valid YAML file, when running `rphys render scene.yaml -o output.mp4`, then an MP4 file is produced
- [ ] Given end conditions in the scene, when the condition is met, then the video ends at that frame
- [ ] Given `--preset tiktok`, then the output is 1080x1920 (9:16 vertical) at 60fps
- [ ] Given `--preset youtube`, then the output is 1920x1080 (16:9 landscape) at 60fps
- [ ] Given `--fps 30`, then the output is rendered at 30fps (physics simulation identical to 60fps version)
- [ ] Given `--resolution 1080x1920`, then custom resolution is supported
- [ ] Given a scene with audio events, when exported, then audio is mixed into the MP4
- [ ] The output uses H.264 codec (universally compatible)
- [ ] Progress is shown during export (frame count, percentage, ETA)
- [ ] Given a scene with no end condition, when `--duration 10s` is provided, then the video runs for 10 seconds

## Priority: High
## Depends on: STORY-003 (renderer), STORY-006 (audio)
## Estimated complexity: Medium

## Notes
- Use `ffmpeg` via CLI or a Rust binding for encoding
- The renderer produces frames; the exporter encodes them into video
- Fixed timestep means physics is identical regardless of output FPS
- Consider pipe-to-ffmpeg approach (render frames → pipe raw pixels → ffmpeg encodes)
