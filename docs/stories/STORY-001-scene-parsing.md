# STORY-001: YAML Scene Parsing

## Description
As a user, I want to define a physics scene in a YAML file so that the application can load and validate it.

## Acceptance Criteria
- [ ] Given a valid YAML file with scene objects, when parsed, then a structured Scene is returned with all objects, environment settings, and metadata
- [ ] Given a YAML file with an invalid shape type, when parsed, then a clear error message indicates the line and what's wrong
- [ ] Given a YAML file missing required fields (e.g., object without position), when parsed, then validation fails with a specific error
- [ ] Given an empty YAML file, when parsed, then a clear "empty scene" error is returned
- [ ] Given a YAML file, the parser supports these object properties: shape (circle/rectangle/polygon), position, velocity, size/radius, material (restitution, friction, density), color, and optional name/tag
- [ ] Given a YAML file, environment settings include: gravity vector, world bounds, background color
- [ ] Given a YAML file, end conditions are parsed: time_limit, all_destroyed, object_escaped, object_collided, and AND/OR combinators
- [ ] A JSON Schema exists that documents the full YAML structure for LLM/tooling consumption

## Priority: High
## Depends on: None
## Estimated complexity: Medium

## Notes
- YAML format must be both human-writable and AI-generatable
- Use `serde` + `serde_yaml` for deserialization
- Validate with good error messages (not raw serde errors)
- See CLAUDE.md for syntax design principles
