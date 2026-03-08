# STORY-007: CLI Interface

## Description
As a user, I want a clean CLI interface so I can easily preview and export scenes from the terminal.

## Acceptance Criteria
- [ ] Given no arguments, when running `rphys`, then a help message is shown with available commands
- [ ] Given `rphys preview <file>`, then live preview mode starts
- [ ] Given `rphys render <file> -o <output>`, then export mode starts
- [ ] Given `rphys render <file> --preset tiktok`, then export uses TikTok preset (1080x1920, 60fps)
- [ ] Given `rphys render <file> --preset youtube`, then export uses YouTube preset (1920x1080, 60fps)
- [ ] Given `rphys validate <file>`, then the YAML is parsed and validated without running simulation (fast feedback)
- [ ] Given `rphys schema`, then the JSON Schema is printed to stdout (for LLM/tooling use)
- [ ] Given an invalid file path, when running any command, then a clear error is shown
- [ ] Given `--verbose`, when running any command, then debug output is shown (physics stats, frame timing)

## Priority: High
## Depends on: STORY-001, STORY-004, STORY-005
## Estimated complexity: Small

## Notes
- Use `clap` for argument parsing
- Subcommand pattern: `rphys <command> [options] <file>`
- `validate` and `schema` commands are cheap wins for the AI-friendly goal
- Consider `rphys init` to scaffold an example scene.yaml (nice-to-have)
