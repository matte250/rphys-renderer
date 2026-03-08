# STORY-008: Example Scenes

## Description
As a new user, I want example scene files so I can understand the YAML format and start creating my own scenes quickly.

## Acceptance Criteria
- [ ] An `examples/` directory exists with at least 3 example scenes
- [ ] Example: `bouncing-ball.yaml` — a single ball bouncing in a box (simplest possible scene)
- [ ] Example: `breakout.yaml` — a ball destroying rows of bricks (breakout-style)
- [ ] Example: `escape.yaml` — an object escaping a layered circular enclosure
- [ ] Each example file has YAML comments explaining the key sections
- [ ] Each example can be run with `rphys preview examples/<file>.yaml` successfully
- [ ] Each example uses end conditions (not infinite)
- [ ] A `README.md` in `examples/` describes each scene

## Priority: Medium
## Depends on: STORY-001 through STORY-004
## Estimated complexity: Small

## Notes
- Examples serve double duty: user documentation AND test fixtures
- Keep them simple — showcase one concept each
- These also serve as reference for LLMs generating new scenes
