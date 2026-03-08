# STORY-003: 2D Renderer

## Description
As a user, I want to see my physics simulation rendered visually so that I can preview scenes and export videos.

## Acceptance Criteria
- [ ] Given a running simulation, when rendering, then all objects are drawn at their current positions with correct shapes and colors
- [ ] Given a scene with background color configured, when rendered, then the background matches the specification
- [ ] Given circle objects, when rendered, then they appear as circles (not approximated polygons visible to the eye)
- [ ] Given rectangle objects, when rendered, then they appear as rectangles with correct dimensions and rotation
- [ ] Given polygon objects, when rendered, then they are drawn with correct vertices
- [ ] Given world boundary walls, when rendered, then walls are visible (configurable color/visibility)
- [ ] Given the "clean/minimal" style, when rendered, then objects are solid colors on a solid background with no effects
- [ ] The renderer supports drawing at arbitrary resolution independent of the physics simulation
- [ ] The renderer provides a frame-by-frame interface (render frame N → image) suitable for both live preview and video export

## Priority: High
## Depends on: STORY-002 (needs physics state to render)
## Estimated complexity: Large

## Notes
- Start with clean/minimal style only (solid shapes, solid background)
- Renderer should be backend-agnostic (trait-based) to support future styles
- Consider `wgpu` for GPU rendering or `tiny-skia` for CPU software rendering
- Frame-by-frame API enables both real-time preview and offline video export
- Neon/glow and stylized themes are future work, but the architecture should support them
