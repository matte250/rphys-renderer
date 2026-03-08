# rphys-renderer — Project Conventions

## What This Is
A CLI tool for creating physics simulation videos from declarative YAML scene definitions. Target audience: content creators making short-form satisfying physics videos (TikTok, YouTube Shorts, Reels).

## Language & Tooling
- **Rust** (latest stable)
- `cargo fmt` — run before every commit
- `cargo clippy -- -D warnings` — must pass clean
- All public APIs get `///` doc comments

## Dependencies
- **ALWAYS check crates.io for the latest version** before adding any dependency
- Never use hardcoded versions from memory — LLM training data is stale
- Run `cargo search <crate>` or check https://crates.io/<crate> to verify
- Pin to the latest stable major version (e.g., `"2"` not `"1"` if 2.x exists)
- If a crate API has changed in a newer version, use the NEW API, not the old one

## Code Standards
- No `.unwrap()` in production code — use proper error handling
- `thiserror` for library error types, `anyhow` for CLI/application errors
- Small, focused functions (< 50 lines preferred)
- Descriptive names — no single-letter variables except iterators
- Derive `Debug`, `Clone`, `PartialEq` on data types where sensible
- Prefer borrowing over cloning

## Architecture
- Modular design — clear boundaries between physics, rendering, audio, scene parsing
- Traits at module boundaries for testability
- Deterministic physics — fixed timestep, frame-rate independent
- See `docs/architecture/` for full design

## YAML Syntax Design Principles
- Human-readable AND AI-friendly (LLMs should generate valid scenes easily)
- Consistent, predictable key names — no magic or implicit behavior
- Self-describing — clear names, not cryptic abbreviations
- JSON Schema for validation
- Good error messages on invalid input

## Testing
- Unit tests in-module (`#[cfg(test)]`)
- Integration tests in `tests/`
- Every public function gets at least one test
- Test names describe behavior: `test_parser_rejects_invalid_shape`

## Git
- Conventional commits preferred
- Feature branches → PR → merge
- Don't commit generated files (videos, build artifacts)
