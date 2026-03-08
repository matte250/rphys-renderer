# STORY-002: 2D Physics Engine

## Description
As a user, I want objects in my scene to obey realistic 2D physics so that simulations look natural and satisfying.

## Acceptance Criteria
- [ ] Given objects with gravity enabled, when the simulation runs, then objects accelerate downward (or in the configured gravity direction)
- [ ] Given two objects on a collision course, when they collide, then they bounce according to their restitution values
- [ ] Given an object hitting a world boundary wall, when it collides, then it bounces correctly
- [ ] Given a destructible object marked as a "brick", when hit with sufficient force, then it is removed from the simulation
- [ ] Given a scene, the simulation uses a fixed timestep (e.g., 1/240s) independent of rendering FPS
- [ ] Given the same scene run twice, the simulation produces identical results (deterministic)
- [ ] Given objects with friction, when sliding against surfaces, then friction slows them appropriately
- [ ] Supported shapes for collision: circle-circle, circle-rectangle, rectangle-rectangle, circle-polygon, rectangle-polygon
- [ ] Given end conditions defined in the scene, when a condition is met, then the simulation signals completion

## Priority: High
## Depends on: STORY-001 (needs parsed scene data)
## Estimated complexity: Large

## Notes
- Fixed timestep with accumulator pattern for determinism
- Consider using `rapier2d` as the physics backend (well-maintained, deterministic, Rust-native)
- End condition evaluation runs each frame after physics step
- Objects need tags/names for condition targeting ("when ball touches goal")
