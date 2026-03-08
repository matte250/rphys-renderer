# STORY-006: Event-Based Audio System

## Description
As a content creator, I want satisfying sound effects to play when physics events occur (collisions, destructions, bounces) so that my videos sound as good as they look.

## Acceptance Criteria
- [ ] Given a scene with audio configured, when a collision occurs, then the mapped sound effect plays
- [ ] Given an object with a custom bounce sound in YAML, when it bounces, then that specific sound plays
- [ ] Given a destruction event, when an object is destroyed, then the destruction sound plays
- [ ] Given multiple simultaneous collisions, when they occur in the same frame, then sounds are mixed together
- [ ] Given no audio configured for an event, when it occurs, then no sound plays (silent by default)
- [ ] Given the live preview mode, when sounds trigger, then they play in real-time through speakers
- [ ] Given the export mode, when encoding video, then audio is mixed into the MP4 output
- [ ] Given a sound file path in YAML, when the file doesn't exist, then a clear error is shown at parse time
- [ ] Default/bundled sound effects are available so users don't need to provide their own files

## Priority: Medium
## Depends on: STORY-002 (needs physics events)
## Estimated complexity: Medium

## Notes
- Audio events emitted by physics engine: collision, bounce (wall), destruction
- YAML maps events to sound files: `audio: { bounce: "sounds/bonk.wav", destroy: "sounds/break.wav" }`
- Consider `rodio` for audio playback, `symphonia` for decoding
- For video export, render audio to a WAV buffer and let ffmpeg mux it
- Bundle a small set of default sounds (CC0 licensed)
- Volume could scale with collision force (nice-to-have for MVP)
