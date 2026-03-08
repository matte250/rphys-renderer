# Scene Schema — rphys-renderer YAML Format

This document is the authoritative specification for the `.yaml` scene files consumed by `rphys`.  
It is written for **three audiences**:
1. **Human authors** who want to write scenes by hand
2. **AI systems** that generate scenes programmatically
3. **Developers** implementing the `rphys-scene` parser

A JSON Schema (machine-readable) is generated at build time and available via `rphys schema`.

---

## Design Principles

- **One obvious way to express each concept.** No aliases, no shorthand.
- **Explicit over implicit.** Every meaningful field is stated; nothing is inferred from context.
- **Self-describing field names.** `restitution` not `rest`; `background_color` not `bg`.
- **Consistent types.** Positions are always `[x, y]` arrays. Colors are always `"#RRGGBB"` hex strings. Sizes are always in meters (physics units).
- **Fail loudly on errors.** Unknown fields produce warnings; missing required fields produce errors with line numbers.

---

## Top-Level Structure

```yaml
version: "1"          # required — schema version; currently always "1"

meta:                 # required — scene metadata
  name: "Scene Name"
  description: "What this scene demonstrates"  # optional
  author: "Your Name"                           # optional
  duration_hint: 15.0                           # optional: hint for export (seconds)

environment:          # required — world settings
  gravity: [0.0, -9.81]
  background_color: "#1a1a2e"
  world_bounds:
    width: 20.0       # meters — horizontal extent
    height: 35.5      # meters — vertical extent (use 20×35.5 for 9:16 at ~54 px/m)
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3    # meters

objects:              # required — list of simulated bodies (can be empty list)
  - ...               # see Objects section below

end_condition:        # optional — when to stop the simulation
  type: or
  conditions:
    - ...

audio:                # optional — global audio config
  default_bounce: "assets/sounds/bounce.wav"
  default_destroy: "assets/sounds/destroy.wav"
  master_volume: 1.0
```

---

## Field Reference

### `version` _(string, required)_
Schema version. Currently always `"1"`. The parser rejects unknown versions.

---

### `meta` _(object, required)_

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Human-readable scene name |
| `description` | string | no | What this scene demonstrates |
| `author` | string | no | Scene author |
| `duration_hint` | float | no | Suggested export duration in seconds. Used when no end condition fires and `--duration` is not given. |

---

### `environment` _(object, required)_

#### `gravity` _(array [x, y], required)_
Gravity vector in m/s². Standard Earth gravity pointing down: `[0.0, -9.81]`.  
Zero gravity: `[0.0, 0.0]`. Sideways: `[9.81, 0.0]`.

#### `background_color` _(hex string, required)_
Background fill color. Format: `"#RRGGBB"` or `"#RRGGBBAA"`.

#### `world_bounds` _(object, required)_
Defines the axis-aligned rectangle of the simulation world. Objects outside this box trigger `ObjectEscaped` end conditions and are culled from physics.

| Field | Type | Description |
|---|---|---|
| `width` | float | World width in meters |
| `height` | float | World height in meters |

The world origin `(0, 0)` is at the **bottom-left**. Y increases upward (standard math convention, not screen convention). The renderer flips Y for display.

**Recommended sizes for social media formats:**
- 9:16 vertical (TikTok/Shorts): `width: 20.0, height: 35.56` at ~54 px/m → 1080×1920
- 16:9 landscape (YouTube): `width: 35.56, height: 20.0`

#### `walls` _(object, required)_
Invisible-or-visible boundary walls on all four sides.

| Field | Type | Default | Description |
|---|---|---|---|
| `visible` | bool | `true` | Whether walls are drawn |
| `color` | hex string | `"#ffffff"` | Wall color (if visible) |
| `thickness` | float | `0.3` | Wall thickness in meters |

Walls are always **static** colliders, regardless of any other setting.

---

### `objects` _(list, required)_

Each object in the list is a simulated body. At least one object is recommended (an empty list is valid but produces a blank simulation).

#### Common Fields (all objects)

| Field | Type | Required | Default | Description |
|---|---|---|---|---|
| `name` | string | no | — | Unique identifier. Required if referenced by end conditions. |
| `shape` | string | yes | — | `"circle"`, `"rectangle"`, or `"polygon"` |
| `position` | [x, y] | yes | — | Initial position in meters from world origin (bottom-left) |
| `velocity` | [x, y] | no | `[0.0, 0.0]` | Initial velocity in m/s |
| `rotation` | float | no | `0.0` | Initial rotation in degrees (positive = counter-clockwise) |
| `angular_velocity` | float | no | `0.0` | Initial angular velocity in degrees/s |
| `body_type` | string | no | `"dynamic"` | `"dynamic"`, `"static"`, or `"kinematic"` |
| `material` | object | no | defaults | Physical material properties |
| `color` | hex string | no | `"#ffffff"` | Fill color |
| `tags` | list of strings | no | `[]` | Arbitrary labels for grouping and end conditions |
| `destructible` | object | no | null | If present, object can be destroyed |
| `audio` | object | no | defaults | Per-object sound overrides |

#### Shape-Specific Fields

**`shape: circle`**
```yaml
shape: circle
radius: 0.5     # required — radius in meters
```

**`shape: rectangle`**
```yaml
shape: rectangle
size: [2.0, 1.0]  # required — [width, height] in meters
```

**`shape: polygon`**
```yaml
shape: polygon
vertices:           # required — list of [x, y] offsets from object center (meters)
  - [0.0, 1.0]
  - [1.0, -0.5]
  - [-1.0, -0.5]
```
Vertices are in counter-clockwise order. The polygon must be convex (non-convex polygons are decomposed automatically by rapier2d, but results may be unexpected — keep simple shapes for MVP).

#### `material` _(object, optional)_

| Field | Type | Default | Description |
|---|---|---|---|
| `restitution` | float (0–1) | `0.5` | Bounciness. 0 = absorbs all energy; 1 = perfectly elastic. |
| `friction` | float (0–∞) | `0.5` | Surface friction. 0 = ice; 1 = rubber. |
| `density` | float (> 0) | `1.0` | Density in kg/m². Determines mass from shape area. |

#### `body_type` _(string)_

| Value | Behaviour |
|---|---|
| `dynamic` | Affected by gravity and collisions (default) |
| `static` | Never moves; infinite mass. Used for walls, floors, platforms. |
| `kinematic` | Position set programmatically (future feature — use `static` for MVP) |

#### `destructible` _(object, optional)_
If present, the object can be destroyed during simulation.

```yaml
destructible:
  min_impact_force: 5.0   # N·s — minimum impulse to destroy this object
```

When the impulse of a collision exceeds `min_impact_force`, the body is removed from the simulation and a `Destroyed` physics event is emitted.

#### `audio` _(object, per-object override)_

| Field | Type | Description |
|---|---|---|
| `bounce` | string (path) | Sound to play on bounce. Overrides `audio.default_bounce`. |
| `destroy` | string (path) | Sound to play on destruction. Overrides `audio.default_destroy`. |

Paths are relative to the **scene file's directory**.

---

### `end_condition` _(object, optional)_

Defines when the simulation ends. If omitted, the simulation runs until the `duration_hint` or until stopped manually.

End conditions can be **simple** (one condition) or **composite** (boolean combinators).

#### Simple Condition Types

**`time_limit`** — stop after N seconds of simulation time
```yaml
end_condition:
  type: time_limit
  seconds: 30.0
```

**`all_tagged_destroyed`** — stop when every object with a given tag has been destroyed
```yaml
end_condition:
  type: all_tagged_destroyed
  tag: "brick"
```

**`object_escaped`** — stop when the named object leaves the world bounds
```yaml
end_condition:
  type: object_escaped
  name: "ball"
```

**`objects_collided`** — stop when two specific named objects first touch
```yaml
end_condition:
  type: objects_collided
  name_a: "ball"
  name_b: "target"
```

**`tags_collided`** — stop when any object with `tag_a` touches any object with `tag_b`
```yaml
end_condition:
  type: tags_collided
  tag_a: "ball"
  tag_b: "goal"
```

#### Composite Conditions

**`or`** — triggers when ANY sub-condition is met
```yaml
end_condition:
  type: or
  conditions:
    - type: time_limit
      seconds: 60.0
    - type: all_tagged_destroyed
      tag: "brick"
```

**`and`** — triggers only when ALL sub-conditions are simultaneously met
```yaml
end_condition:
  type: and
  conditions:
    - type: all_tagged_destroyed
      tag: "enemy"
    - type: object_escaped
      name: "hero"
```

Nesting is supported: `or` can contain `and` conditions and vice versa.

---

### `audio` _(object, optional)_

Global audio defaults. Per-object `audio` blocks override these.

| Field | Type | Default | Description |
|---|---|---|---|
| `default_bounce` | string (path) | null | Fallback sound for any bounce event |
| `default_destroy` | string (path) | null | Fallback sound for any destruction event |
| `master_volume` | float (0–1) | `1.0` | Global volume multiplier |

Paths relative to the scene file's directory.

If neither a per-object audio mapping nor a global default exists for an event, the event is silent.

---

## Complete Examples

### Example 1: Bouncing Ball (minimal)

```yaml
version: "1"

meta:
  name: "Bouncing Ball"
  description: "A single ball bouncing in a box — the hello world of physics"
  author: "rphys"

environment:
  gravity: [0.0, -9.81]
  background_color: "#0d0d0d"
  world_bounds:
    width: 20.0
    height: 35.56
  walls:
    visible: true
    color: "#333333"
    thickness: 0.3

objects:
  - name: "ball"
    shape: circle
    radius: 0.8
    position: [10.0, 25.0]
    velocity: [3.0, 0.0]
    material:
      restitution: 0.85
      friction: 0.1
      density: 1.0
    color: "#e94560"
    tags: ["ball"]
    audio:
      bounce: "assets/sounds/bonk.wav"

end_condition:
  type: time_limit
  seconds: 20.0

audio:
  master_volume: 0.8
```

---

### Example 2: Breakout (with destructibles and composite end condition)

```yaml
version: "1"

meta:
  name: "Breakout"
  description: "Ball destroys bricks — stops when all bricks gone or time runs out"
  author: "rphys"

environment:
  gravity: [0.0, 0.0]           # zero gravity — bricks float in place
  background_color: "#1a0533"
  world_bounds:
    width: 20.0
    height: 35.56
  walls:
    visible: true
    color: "#4444aa"
    thickness: 0.5

objects:
  # The ball
  - name: "ball"
    shape: circle
    radius: 0.5
    position: [10.0, 8.0]
    velocity: [4.0, 6.0]
    material:
      restitution: 1.0          # perfectly elastic
      friction: 0.0
      density: 0.8
    color: "#ffdd44"
    tags: ["ball"]
    audio:
      bounce: "assets/sounds/ping.wav"

  # Row 1 — bricks (use repeating pattern)
  - name: "brick_r1_c1"
    shape: rectangle
    size: [2.8, 0.8]
    position: [1.9, 28.0]
    body_type: static
    material:
      restitution: 0.3
      friction: 0.5
      density: 2.0
    color: "#ff4455"
    tags: ["brick"]
    destructible:
      min_impact_force: 3.0
    audio:
      destroy: "assets/sounds/crack.wav"

  - name: "brick_r1_c2"
    shape: rectangle
    size: [2.8, 0.8]
    position: [4.9, 28.0]
    body_type: static
    material:
      restitution: 0.3
      friction: 0.5
      density: 2.0
    color: "#ff7744"
    tags: ["brick"]
    destructible:
      min_impact_force: 3.0
    audio:
      destroy: "assets/sounds/crack.wav"

  # ... (more bricks follow the same pattern)

end_condition:
  type: or
  conditions:
    - type: all_tagged_destroyed
      tag: "brick"
    - type: time_limit
      seconds: 60.0

audio:
  default_bounce: "assets/sounds/bounce.wav"
  default_destroy: "assets/sounds/crack.wav"
  master_volume: 1.0
```

---

### Example 3: Escape (object exits world bounds)

```yaml
version: "1"

meta:
  name: "The Escape"
  description: "A ball carves its way through obstacles and escapes out the top"
  author: "rphys"
  duration_hint: 30.0

environment:
  gravity: [0.0, -4.0]          # lighter gravity for floaty feel
  background_color: "#0a0a1a"
  world_bounds:
    width: 20.0
    height: 35.56
  walls:
    visible: false               # no side walls — only floor
    color: "#222244"
    thickness: 0.5

objects:
  - name: "hero"
    shape: circle
    radius: 0.6
    position: [10.0, 2.0]
    velocity: [0.5, 8.0]         # launched upward
    material:
      restitution: 0.7
      friction: 0.2
      density: 1.0
    color: "#44ffaa"
    tags: ["hero", "ball"]

  - name: "barrier_1"
    shape: rectangle
    size: [14.0, 0.6]
    position: [3.0, 12.0]       # horizontal barrier
    body_type: static
    material:
      restitution: 0.5
      friction: 0.3
      density: 3.0
    color: "#ff3366"
    tags: ["barrier", "obstacle"]
    destructible:
      min_impact_force: 8.0

  - name: "barrier_2"
    shape: rectangle
    size: [14.0, 0.6]
    position: [3.0, 22.0]
    body_type: static
    material:
      restitution: 0.5
      friction: 0.3
      density: 3.0
    color: "#ff6633"
    tags: ["barrier", "obstacle"]
    destructible:
      min_impact_force: 8.0

end_condition:
  type: or
  conditions:
    - type: object_escaped
      name: "hero"
    - type: time_limit
      seconds: 30.0

audio:
  default_bounce: "assets/sounds/thud.wav"
  default_destroy: "assets/sounds/shatter.wav"
  master_volume: 1.0
```

---

## Common Mistakes & Error Messages

| Mistake | Error message |
|---|---|
| Missing `shape` field | `Object 'ball': missing required field 'shape'` |
| Unknown shape type | `Object 'ball': unknown shape type 'sphere'` |
| `circle` without `radius` | `Object 'ball': missing required field 'radius'` |
| `rectangle` without `size` | `Object 'box': missing required field 'size'` |
| Duplicate object names | `Duplicate object name 'ball'` |
| End condition references unknown name | `End condition references unknown object 'target'` |
| Audio file not found | `Audio file not found: 'assets/sounds/missing.wav'` |
| `restitution` outside 0–1 | `Object 'ball': restitution must be between 0.0 and 1.0, got 1.5` |
| Empty YAML file | `Empty scene file — nothing to simulate` |

---

## JSON Schema Location

The canonical JSON Schema lives at `schemas/scene.schema.json` and is regenerated from code.

To print it:
```
rphys schema
```

To validate a scene file against the schema using external tooling:
```
npx ajv-cli validate -s schemas/scene.schema.json -d my-scene.yaml
```

---

## Versioning Policy

- `version: "1"` — current and only version
- Future breaking changes bump the version to `"2"`
- The parser rejects files with an unknown version with an actionable error message
- Non-breaking additions (new optional fields) do not bump the version
