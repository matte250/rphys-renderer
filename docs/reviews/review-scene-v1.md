# Code Review — `rphys-scene` v1

**Reviewer:** Code Reviewer Agent  
**Date:** 2026-03-08  
**Scope:** `crates/rphys-scene/src/` (types.rs, de.rs, parse.rs, schema.rs, lib.rs) + Cargo.toml  
**Verdict:** Approve with required fixes — solid foundation, a handful of real issues to address before merge.

---

## Summary

The `rphys-scene` crate is well-structured and largely on-spec. The two-layer architecture (raw `de::Raw*` types for serde, clean `types::*` domain types) is the right call and much better than the spec's suggestion to derive `serde::Deserialize` directly on the public types. Error accumulation across all objects before bailing is good UX. Doc comments are thorough. The 30-test suite covers the happy path and most error cases well.

There are four issues that need to be fixed before merge, plus several quality issues that should be addressed.

---

## File: `src/types.rs`

### ✅ Good
- Clean domain types with no serde noise — exactly right.
- `Vec2::ZERO` and `Color::WHITE/BLACK` constants are handy and well-documented.
- `Color` derives `Eq` (correct — all `u8` fields, unlike `f32`-containing types).
- All public types derive `Debug, Clone, PartialEq` as per CLAUDE.md convention.
- `Material::default()` values (`restitution=0.5, friction=0.5, density=1.0`) match both the spec and JSON Schema defaults exactly.
- `BodyType` uses `#[default]` attribute cleanly.
- Doc comments on every public field — very thorough.

### ⚠️ Issues
1. **`SceneAudio::master_volume` lacks a documented default.** The struct derives `Default`, which will zero-initialise `master_volume`. But the schema default is `1.0`, not `0.0`. `Default::default()` on `SceneAudio` would produce a silenced scene. The spec's `impl Default` in `modules.md` isn't shown for `SceneAudio`, but users calling `SceneAudio::default()` elsewhere (e.g., tests, other crates) will get `master_volume = 0.0`. Either override `Default` to return `1.0` or remove the derive and add an explicit impl.

### 🔧 Suggestions
- Consider adding `impl From<[f32; 2]> for Vec2` — it would clean up the parse.rs call-sites where `Vec2::new(arr[0], arr[1])` appears six times.

---

## File: `src/de.rs`

### ✅ Good
- Module-level doc comment clearly explains the design intent (raw intermediates for serde).
- `#[serde(tag = "type", rename_all = "snake_case")]` on `RawEndCondition` is the correct serde pattern for internally-tagged enums and avoids hand-rolling the dispatch.
- All raw types are `pub(crate)` — no accidental API surface leakage.
- The choice to flatten shape-specific fields (`radius`, `size`, `vertices`) onto `RawObject` is pragmatically correct and matches the YAML format.
- `rotation` and `angular_velocity` comments document the degrees→radians conversion responsibility — good.

### ⚠️ Issues
2. **`RawScene` missing `#[serde(deny_unknown_fields)]`.** The spec says "unknown fields produce warnings." Neither warning nor error is currently issued for unknown top-level keys (e.g., `typo_field: 123` silently passes). This is a spec gap. At minimum, this behaviour should be documented. If serde warnings aren't feasible, `deny_unknown_fields` on raw structs would turn them into errors, which is arguably safer for a declarative format. `RawObject` is the most important one to protect — typos in field names (e.g., `raduis: 0.5`) currently silently produce a missing-field error rather than a helpful "did you mean `radius`?" message.

---

## File: `src/parse.rs`

### ✅ Good
- Clear separation of parsing, validation, and conversion into focused functions.
- Error accumulation pattern is well-executed — all objects are validated before returning, producing a complete error list rather than stopping at the first problem.
- `named_objects` pre-pass for end-condition reference validation is smart.
- `convert_end_condition` correctly handles the composite `And`/`Or` cases: if any sub-condition fails validation, the parent returns `None` while still accumulating child errors.
- `parse_hex_color` and `parse_hex_byte` are clean, small, and testable (both are tested directly).
- `resolve_path` handles the absolute-path identity case correctly.
- All functions are well within the 50-line preference from CLAUDE.md.
- The `convert_environment` function correctly validates world_bounds positivity.

### 🚫 Blocking Issues

**Issue 1 — `.expect()` in production path (line 177)**
```rust
let environment = environment.expect("environment is required and we error on None");
```
CLAUDE.md is explicit: "No `.unwrap()` in production code." `.expect()` is `unwrap()` with a message — it will still panic. While the logic is sound (any `None` from `convert_environment` always appends an error, so `errors` will be non-empty and we'd have returned on line 173), this depends on an implicit invariant that is not enforced by the type system. A future change to `convert_environment` that returns `None` without pushing an error would cause a silent panic in production.

Fix: return an error explicitly instead of panicking:
```rust
let environment = match environment {
    Some(e) => e,
    None => {
        // Invariant violation: convert_environment returned None
        // without recording an error. Treat as an internal error
        // rather than panicking.
        return Err(ParseError::Validation(errors));
    }
};
```
Or change `convert_environment` to return `Result<Environment, ()>` and push the error itself, then use `?` — this enforces the invariant structurally.

**Issue 2 — `parse_hex_color` accepts colors without `#` prefix, conflicting with spec (line 675)**
```rust
let hex = s.strip_prefix('#').unwrap_or(s);
```
The spec says colors MUST be `"#RRGGBB"` or `"#RRGGBBAA"`. The JSON Schema has `pattern: "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$"` which requires the `#`. There is even a test (`test_parse_color_without_hash`) that validates this undocumented lenient behaviour, creating a discrepancy between what the schema says is valid and what the parser accepts.

This matters because: scenes generated by AI or tooling that validate against the JSON Schema will always include `#`. Silently accepting `"ff0000"` means the parser is more permissive than the declared contract, which could mask authoring errors.

Fix: reject strings without the `#` prefix:
```rust
let hex = s.strip_prefix('#').ok_or_else(|| {
    format!("color must be '#RRGGBB' or '#RRGGBBAA' hex, got '{s}'")
})?;
```
Remove `test_parse_color_without_hash` or convert it to test that the format is rejected.

### ⚠️ Issues

**Issue 3 — Missing validation for several schema-constrained numeric fields**

The JSON Schema defines `exclusiveMinimum: 0` or range constraints on several fields that the Rust parser does not enforce. These constraints exist in the schema spec but have no runtime check:

| Field | Schema constraint | Rust check |
|---|---|---|
| `walls.thickness` (line 495) | `exclusiveMinimum: 0` | None — accepts `thickness: -1.0` |
| `destructible.min_impact_force` | `exclusiveMinimum: 0` | None — accepts `min_impact_force: -5.0` |
| `audio.master_volume` (line 662) | `minimum: 0, maximum: 1` | None — accepts `master_volume: 99.0` |
| `end_condition.time_limit.seconds` | `exclusiveMinimum: 0` | None — accepts `seconds: -30.0` |
| `meta.duration_hint` | `minimum: 0` | None — accepts `duration_hint: -1.0` |

The material fields (`restitution`, `friction`, `density`) ARE validated — these others should be too, for consistency and for the "Fail loudly" design principle.

**Issue 4 — Double `resolve_path` call in `convert_object_audio` for `bounce` (lines 597–607)**
```rust
let bounce = raw.bounce.as_ref().map(|p| {
    let path = resolve_path(p, base_dir);      // call 1 — `path` is the resolved PathBuf
    if let Some(dir) = base_dir {
        let full = dir.join(&path);            // join with already-absolute path
        if !full.exists() { ... }
    }
    let _ = obj_name;                          // (see Issue 5)
    resolve_path(p, base_dir)                  // call 2 — redundant, should be `path`
});
```
The closure resolves the path twice and discards the first result. The `dir.join(&path)` on line 599 also joins an already-absolute path to `dir` — this works by accident on Unix (joining an absolute path replaces the base), but is confusing. Compare to the `destroy` block (lines 609–617), which uses `dir.join(p)` (the raw string) for the check and returns `path` — a simpler, cleaner pattern.

Fix: mirror the `destroy` pattern:
```rust
let bounce = raw.bounce.as_ref().map(|p| {
    let path = resolve_path(p, base_dir);
    if let Some(dir) = base_dir {
        if !dir.join(p).exists() {
            errors.push(ValidationError::AudioFileNotFound { path: dir.join(p) });
        }
    }
    path
});
```

**Issue 5 — Dead `let _ = obj_name;` in `convert_object_audio` (line 605)**
```rust
let _ = obj_name; // used only for context, already in error above
```
This is a no-op. In Rust, unused function parameters do not trigger `unused_variable` warnings — only unused local bindings do. This line suppresses no warning and does nothing. The comment is misleading: `obj_name` is not included in the `AudioFileNotFound` error (that error type has no `name` field). If the intent was to include the object name for context in audio errors, the implementation doesn't achieve that — the error type `ValidationError::AudioFileNotFound { path }` has no object name field. Remove this line or, better, consider whether the object name should be in the error.

**Issue 6 — `ValidationError::InvalidValue` used for environment fields produces garbled messages (lines 434–436, 443–450, 487–490)**

`ValidationError::InvalidValue` renders as `"Object '{name}': {message}"`. When used for environment fields:
```rust
errors.push(ValidationError::InvalidValue {
    name: "environment.background_color".to_string(),
    message: msg,
});
```
...the resulting error reads: `"Object 'environment.background_color': color must be '#RRGGBB'..."`.  
The word "Object" is semantically wrong here — this is not an object, it's an environment field. This is the only `ValidationError` variant available for generic value errors, so the misuse is understandable, but it's noticeable to users.

Consider either adding an `Environment`-specific variant or renaming `InvalidValue.name` to `context` with a doc comment clarifying it can refer to either an object or a path within the scene.

**Issue 7 — `named_objects` set includes duplicate names, making end-condition reference validation pass for duplicated names (lines 144–148)**

```rust
let named_objects: HashSet<String> = raw
    .objects
    .iter()
    .filter_map(|obj| obj.name.clone())
    .collect();
```

If two objects share the name `"ball"`, both a `DuplicateName` error and validation of any end condition referencing `"ball"` happen independently. The end-condition check will succeed (the name is in the set) even though the duplicate-name error means the name is ambiguous. A user sees two errors: `DuplicateName "ball"` AND then (at runtime) discovers their `object_escaped: "ball"` end condition is ambiguous. Better UX: when building `named_objects`, deduplicate by only including names that appear exactly once, so the end-condition reference check also fails for names that are duplicated.

**Issue 8 — Early `return None` in `convert_object` stops accumulating errors for that object after the first material validation failure (line ~350)**

When `restitution` is invalid, `convert_object` pushes the error and returns `None`, skipping the `friction` and `density` checks. If all three material values are wrong, only the first is reported. This could frustrate users who prefer to see all problems at once. Consider validating all material fields and only returning `None` after all checks complete.

### 🔧 Suggestions
- The `convert_environment` and `convert_wall_config` functions return early (`return None`) on any single validation failure, which stops accumulating further environment errors. This is less of a problem than in `convert_object` (environment failures are rare), but the pattern is inconsistent.
- `display_name` in `convert_object` allocates a `String` via `unwrap_or_else` even when the name is present. Minor, but could use `raw.name.as_deref().unwrap_or("<unnamed>")` as a `&str` for most of the function, only cloning where `String` is required.

---

## File: `src/schema.rs`

### ✅ Good
- Hand-authored JSON Schema is complete and accurate.
- Draft-07 is the right choice for broad validator compatibility.
- `additionalProperties: false` is set on `meta`, `environment`, `world_bounds`, `walls`, `Material`, `Destructible`, `ObjectAudio`, and `audio` — good defence against typos.
- The `oneOf` structure for `EndCondition` is correct and will catch missing required fields per condition type.
- The doc comment includes a usage example for `rphys schema` and `ajv-cli`.
- The `scene_json_schema()` function has a rustdoc example that validates the return value — good.

### ⚠️ Issues

**Issue 9 — `SceneObject` definition is missing `additionalProperties: false` (schema.rs, `SceneObject` definition)**

Every other object definition in the schema has `additionalProperties: false`, but `SceneObject` does not. A scene with a typo like `raduis: 0.5` will pass schema validation (field silently ignored) and then fail at runtime with a less helpful "missing required field 'radius'" error. Adding `additionalProperties: false` to `SceneObject` makes the schema catch typos at validation time.

**Issue 10 — JSON Schema `EndCondition` allows extra properties alongside required ones**

The `EndCondition` `oneOf` schema doesn't set `additionalProperties: false` on the sub-schemas. A condition like:
```yaml
type: time_limit
seconds: 30.0
tag: "oops"
```
passes schema validation. The Rust parser ignores the extra field (serde doesn't error on unknown fields for tagged enums), but schema validation should catch it. Each `oneOf` branch should include `additionalProperties: false`.

---

## File: `src/lib.rs`

### ✅ Good
- Clean re-export surface — everything the caller needs is at the crate root.
- Module-level doc comment is thorough: overview, example, crate-level links. The example uses `r##"..."##` with a comment explaining why — nice attention to detail.
- `format_validation_errors` is not re-exported (correctly kept `pub(crate)`).
- The example in the doc comment compiles (uses `parse_scene`, checks a field).

### 🔧 Suggestions
- The example in the crate-level doc calls `unwrap()` — acceptable in examples, but could use `?` with a `fn main() -> Result<...>` form to demonstrate proper error handling as well.

---

## Test Coverage

**Count:** 30 tests, all in `parse.rs #[cfg(test)]`.

### ✅ Good Coverage
- Happy-path tests for all three shape types (circle, rectangle, polygon) with field verification.
- Rotation/angular_velocity degree→radian conversion verified precisely.
- Both composite end condition types verified (`Or` — test 8).
- All five simple end condition types have at least one test.
- Error cases cover: empty input, whitespace-only input, unsupported version, YAML syntax error, missing `shape`, unknown shape, missing `radius`, missing `size`, duplicate names, `restitution` out of range, end condition references unknown object, invalid hex color (2 tests).
- `parse_hex_color` tested directly (good unit isolation).
- Full end-to-end test using the example from `scene-schema.md` (test 30).
- Default values tested: material defaults (test 28), audio defaults (test 29).

### ⚠️ Missing Test Cases

These are gaps that should be filled:

1. **`parse_scene_file` has zero tests.** CLAUDE.md: "Every public function gets at least one test." This is a public API with filesystem interaction and path resolution — it needs a test. Use `tempfile` or write a YAML file to a temp dir, then call `parse_scene_file`.

2. **`and` composite end condition is not tested.** Only `Or` is tested. Add a test for `And`.

3. **Nested composite end conditions are not tested.** The `Or` test has only one level. A nested `or { and { ... } }` should be tested to confirm recursion works.

4. **Missing `position` field.** There is a test for missing `shape` but not for missing `position`. This is another required field — test that it produces `MissingField { field: "position" }`.

5. **Polygon with fewer than 3 vertices.** The code validates this (`raw_verts.len() < 3`) but there is no test for it.

6. **Negative/zero radius.** The code validates `radius <= 0.0` — no test.

7. **Invalid `body_type` string.** The code handles unknown body types with `InvalidValue` — no test.

8. **Multiple validation errors accumulate.** There is no test that verifies two objects both have errors and both are reported. This would catch any regression in the accumulation logic.

9. **`objects_collided` end condition — both valid and invalid.** No test for the happy path or for the case where one name is known and one isn't.

10. **`scene_json_schema` return value.** The function is tested only via its doc example. A unit test verifying it's parseable as JSON (using `serde_json::from_str`) would catch any accidental corruption of the static string.

11. **`WallConfig` defaults** (`visible: true`, `color: WHITE`, `thickness: 0.3`) when the `walls` block has no sub-fields — not tested.

12. **`ObjectAudio` parsed from YAML** (without filesystem check). Currently no test for parsing a scene with `audio: { bounce: "foo.wav" }` via `parse_scene` (string path, no base_dir, so no file existence check).

---

## Cargo.toml

### ✅ Good
- Minimal and correct: `serde`, `serde_yml`, `thiserror` are exactly what's needed.
- Workspace-level version pinning is used (no hardcoded versions at crate level).
- No stray dev-dependencies or features.

### 🔧 Suggestions
- Add `[dev-dependencies]` with `tempfile` now — it will be needed once `parse_scene_file` tests are added, and it's easier to set up ahead of time.

---

## Architecture Spec Compliance (`modules.md`)

| Spec item | Status |
|---|---|
| `Vec2` public type with `x: f32, y: f32` | ✅ |
| `Color` with `r,g,b,a: u8` and hex YAML | ✅ |
| `ShapeKind` enum (Circle/Rectangle/Polygon) | ✅ |
| `Material` with correct defaults | ✅ |
| `BodyType` with `#[default] Dynamic` | ✅ |
| `Destructible`, `ObjectAudio`, `SceneObject` | ✅ |
| `WorldBounds`, `WallConfig`, `Environment` | ✅ |
| `EndCondition` all 7 variants | ✅ |
| `SceneAudio`, `SceneMeta`, `Scene` | ✅ |
| `parse_scene(yaml: &str) -> Result<Scene, ParseError>` | ✅ |
| `parse_scene_file(path: &Path) -> Result<Scene, ParseError>` | ✅ |
| `scene_json_schema() -> &'static str` | ✅ |
| `ParseError` all 5 variants with correct signatures | ✅ |
| `ValidationError` all 6 variants with correct signatures | ✅ |
| `serde::Deserialize` on `Vec2` (modules.md) | ⚠️ intentionally omitted — better design, see note |

> **Note on `Vec2 serde::Deserialize`:** The spec says to derive it on `Vec2` directly. The implementation omits it in favour of the raw-types pattern in `de.rs`. This is a conscious and better design decision — it avoids polluting the domain type with serde attributes. The spec should be updated to reflect this approach.

---

## Required Actions Before Merge

1. 🚫 **Line 177** — Replace `.expect()` with an explicit error path. No panics in production.
2. 🚫 **Line 675** — Enforce `#` prefix on color strings. Remove or invert `test_parse_color_without_hash`.
3. ⚠️ **Lines 495, 662 + destructible, end conditions** — Add validation for `thickness > 0`, `min_impact_force > 0`, `master_volume ∈ [0,1]`, `seconds > 0`.
4. ⚠️ **Missing `parse_scene_file` test** — CLAUDE.md convention: every public function needs a test.

Everything else is non-blocking but should be tracked and addressed soon. The core architecture is sound — this crate just needs its rough edges filed down.
