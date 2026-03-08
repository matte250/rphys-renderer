# STORY-004: Live Preview Mode

## Description
As a user, I want to run `rphys preview scene.yaml` and see a window showing my simulation in real-time, with hot-reload when I edit the YAML.

## Acceptance Criteria
- [ ] Given a valid YAML file, when running `rphys preview scene.yaml`, then a window opens showing the simulation
- [ ] Given the preview is running and I edit the YAML file, when saved, then the simulation restarts with the new scene (hot-reload)
- [ ] Given the preview is running, when I press Escape or close the window, then the application exits cleanly
- [ ] Given a YAML file with errors during hot-reload, when saved, then an error is displayed (in terminal or overlay) without crashing — the previous valid scene continues
- [ ] Given end conditions are met during preview, when triggered, then the simulation pauses or loops (configurable)
- [ ] The preview window title shows the scene file name

## Priority: High
## Depends on: STORY-003 (needs renderer)
## Estimated complexity: Medium

## Notes
- Use `winit` for windowing or integrate with the rendering backend's windowing
- File watching via `notify` crate
- Hot-reload = re-parse YAML → rebuild scene → restart simulation
- Graceful degradation on parse errors during reload
