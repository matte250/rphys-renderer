//! Static JSON Schema for the rphys scene format.
//!
//! The schema is hand-authored to match the specification in
//! `docs/architecture/scene-schema.md`.  It validates scene files against
//! JSON Schema draft-07.
//!
//! Use [`scene_json_schema`] to retrieve the schema string.

/// Return the JSON Schema string for the rphys scene format (version 1).
///
/// The returned string is a valid JSON Schema (draft-07) that can be used
/// to validate `.yaml` scene files with any JSON Schema validator.
///
/// # Example
///
/// ```rust
/// use rphys_scene::scene_json_schema;
///
/// let schema = scene_json_schema();
/// assert!(schema.contains("\"$schema\""));
/// assert!(schema.contains("rphys-scene"));
/// ```
pub fn scene_json_schema() -> &'static str {
    SCENE_SCHEMA
}

static SCENE_SCHEMA: &str = r##"{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "$id": "https://github.com/rphys-renderer/rphys-renderer/schemas/scene.schema.json",
  "title": "rphys-scene",
  "description": "Schema for rphys 2D physics simulation scene files (version 1).",
  "type": "object",
  "required": ["version", "meta", "environment", "objects"],
  "additionalProperties": false,
  "properties": {

    "version": {
      "type": "string",
      "const": "1",
      "description": "Schema version. Currently always \"1\"."
    },

    "meta": {
      "type": "object",
      "description": "Scene metadata.",
      "required": ["name"],
      "additionalProperties": false,
      "properties": {
        "name":          { "type": "string", "description": "Human-readable scene name." },
        "description":   { "type": "string", "description": "What this scene demonstrates." },
        "author":        { "type": "string", "description": "Scene author." },
        "duration_hint": {
          "type": "number",
          "minimum": 0,
          "description": "Suggested export duration in seconds."
        }
      }
    },

    "environment": {
      "type": "object",
      "description": "World environment settings.",
      "required": ["gravity", "background_color", "world_bounds", "walls"],
      "additionalProperties": false,
      "properties": {

        "gravity": {
          "type": "array",
          "description": "Gravity vector [x, y] in m/s². Earth standard: [0.0, -9.81].",
          "items": { "type": "number" },
          "minItems": 2,
          "maxItems": 2
        },

        "background_color": {
          "type": "string",
          "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$",
          "description": "Background fill color (#RRGGBB or #RRGGBBAA)."
        },

        "world_bounds": {
          "type": "object",
          "description": "Axis-aligned world boundary rectangle.",
          "required": ["width", "height"],
          "additionalProperties": false,
          "properties": {
            "width":  { "type": "number", "exclusiveMinimum": 0, "description": "World width in meters." },
            "height": { "type": "number", "exclusiveMinimum": 0, "description": "World height in meters." }
          }
        },

        "walls": {
          "type": "object",
          "description": "Boundary wall configuration.",
          "additionalProperties": false,
          "properties": {
            "visible": {
              "type": "boolean",
              "default": true,
              "description": "Whether walls are drawn."
            },
            "color": {
              "type": "string",
              "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$",
              "default": "#ffffff",
              "description": "Wall color."
            },
            "thickness": {
              "type": "number",
              "exclusiveMinimum": 0,
              "default": 0.3,
              "description": "Wall thickness in meters."
            }
          }
        }
      }
    },

    "objects": {
      "type": "array",
      "description": "List of simulated bodies in the scene.",
      "items": { "$ref": "#/definitions/SceneObject" }
    },

    "end_condition": {
      "$ref": "#/definitions/EndCondition",
      "description": "Optional condition that terminates the simulation."
    },

    "audio": {
      "type": "object",
      "description": "Global audio defaults.",
      "additionalProperties": false,
      "properties": {
        "default_bounce":  { "type": "string", "description": "Fallback bounce sound path." },
        "default_destroy": { "type": "string", "description": "Fallback destroy sound path." },
        "master_volume": {
          "type": "number",
          "minimum": 0.0,
          "maximum": 1.0,
          "default": 1.0,
          "description": "Global volume multiplier."
        }
      }
    },

    "camera": {
      "type": "object",
      "description": "Optional camera configuration. Controls which camera mode is used during export.",
      "additionalProperties": false,
      "properties": {
        "mode": {
          "type": "string",
          "enum": ["static", "race", "follow_leader"],
          "default": "race",
          "description": "Camera mode: 'static' (fixed), 'race' (smooth-follow), or 'follow_leader' (dynamic with shake and zoom)."
        },
        "follow_lerp": {
          "type": "number",
          "minimum": 0.0,
          "maximum": 1.0,
          "default": 0.08,
          "description": "Position smoothing factor per frame (0.0 = instant snap, 1.0 = camera never moves)."
        },
        "look_ahead": {
          "type": "number",
          "default": 2.0,
          "description": "Meters ahead of the leader in the travel direction to offset the camera."
        },
        "shake_on_impact": {
          "type": "boolean",
          "default": true,
          "description": "Enable camera shake on collision events."
        },
        "shake_intensity": {
          "type": "number",
          "minimum": 0.0,
          "default": 0.3,
          "description": "Maximum shake displacement in meters."
        },
        "shake_decay": {
          "type": "number",
          "minimum": 0.0,
          "maximum": 1.0,
          "default": 0.85,
          "description": "Per-frame shake decay multiplier (e.g. 0.85 halves shake in ~4 frames)."
        },
        "zoom": {
          "type": "number",
          "exclusiveMinimum": 0,
          "default": 1.0,
          "description": "Extra zoom multiplier on top of base scale (1.0 = no change)."
        },
        "finish_zoom": {
          "type": "boolean",
          "default": true,
          "description": "Zoom in when a winner is decided."
        },
        "finish_zoom_factor": {
          "type": "number",
          "exclusiveMinimum": 0,
          "default": 1.5,
          "description": "Zoom multiplier applied on race complete."
        },
        "finish_zoom_lerp": {
          "type": "number",
          "minimum": 0.0,
          "maximum": 1.0,
          "default": 0.04,
          "description": "Per-frame smoothing factor for the finish zoom transition."
        },
        "lock_horizontal": {
          "type": "boolean",
          "default": true,
          "description": "Lock horizontal position to world centre so both side walls remain visible. Set false to allow full 2-D panning."
        }
      }
    },

    "race": {
      "type": "object",
      "description": "Optional race-mode configuration. Presence enables race tracking and overlay.",
      "required": ["finish_y"],
      "additionalProperties": false,
      "properties": {
        "finish_y": {
          "type": "number",
          "minimum": 0,
          "description": "World Y coordinate of the finish line. Race ends when any racer's Y ≤ this value."
        },
        "racer_tag": {
          "type": "string",
          "default": "racer",
          "description": "Tag that identifies racer bodies."
        },
        "announcement_hold_secs": {
          "type": "number",
          "exclusiveMinimum": 0,
          "default": 2.0,
          "description": "How long (in seconds) to hold the winner frame at the end of export."
        },
        "elimination_interval_secs": {
          "type": ["number", "null"],
          "exclusiveMinimum": 0,
          "description": "When set, the last-place racer is eliminated every this many seconds. Omit or null to disable."
        },
        "post_finish_secs": {
          "type": "number",
          "minimum": 0.0,
          "default": 0.0,
          "description": "Simulation seconds to continue running after the first racer crosses the finish line so that subsequent racers can also finish and be ranked. Because the export loop applies a 4× slow-motion effect near the finish line (SLOWDOWN_FACTOR = 0.25), the resulting video plays for approximately post_finish_secs × 4 wall-clock seconds. For example, post_finish_secs: 8.0 produces roughly 32 video-seconds of post-finish footage. 0 (default) stops immediately on the first finish."
        },
        "checkpoints": {
          "type": "array",
          "description": "Optional milestone Y-coordinates shown as horizontal lines with labels.",
          "items": {
            "type": "object",
            "required": ["y"],
            "additionalProperties": false,
            "properties": {
              "y": {
                "type": "number",
                "description": "Y coordinate of the checkpoint line. Must be > finish_y."
              },
              "label": {
                "type": "string",
                "description": "Display label shown on the checkpoint line."
              }
            }
          }
        }
      }
    }

  },

  "definitions": {

    "Color": {
      "type": "string",
      "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$",
      "description": "Hex color string: #RRGGBB or #RRGGBBAA."
    },

    "Vec2": {
      "type": "array",
      "items": { "type": "number" },
      "minItems": 2,
      "maxItems": 2,
      "description": "[x, y] 2D vector."
    },

    "Material": {
      "type": "object",
      "description": "Physical material properties.",
      "additionalProperties": false,
      "properties": {
        "restitution": {
          "type": "number",
          "minimum": 0.0,
          "maximum": 1.0,
          "default": 0.5,
          "description": "Bounciness (0 = absorbs all, 1 = perfectly elastic)."
        },
        "friction": {
          "type": "number",
          "minimum": 0.0,
          "default": 0.5,
          "description": "Surface friction coefficient."
        },
        "density": {
          "type": "number",
          "exclusiveMinimum": 0,
          "default": 1.0,
          "description": "Density in kg/m². Determines mass from shape area."
        }
      }
    },

    "Destructible": {
      "type": "object",
      "description": "Enables destruction of an object on high-impulse collision.",
      "required": ["min_impact_force"],
      "additionalProperties": false,
      "properties": {
        "min_impact_force": {
          "type": "number",
          "exclusiveMinimum": 0,
          "description": "Minimum collision impulse (N·s) to trigger destruction."
        }
      }
    },

    "BoostConfig": {
      "type": "object",
      "description": "Speed-boost configuration. When a dynamic body contacts this object, an impulse is applied in the given direction.",
      "required": ["direction", "impulse"],
      "additionalProperties": false,
      "properties": {
        "direction": {
          "$ref": "#/definitions/Vec2",
          "description": "Unit vector [x, y] (world space) indicating the impulse direction."
        },
        "impulse": {
          "type": "number",
          "exclusiveMinimum": 0,
          "description": "Impulse magnitude in N·s applied per contact frame."
        }
      }
    },

    "GravityWellConfig": {
      "type": "object",
      "description": "Gravity-well attractor/repulsor zone. Dynamic bodies within `radius` meters are continuously pulled toward (attractor) or pushed away from (repulsor) the well center.",
      "required": ["radius", "strength"],
      "additionalProperties": false,
      "properties": {
        "radius": {
          "type": "number",
          "exclusiveMinimum": 0,
          "description": "Influence radius in meters. Bodies inside this distance are affected."
        },
        "strength": {
          "type": "number",
          "exclusiveMinimum": 0,
          "description": "Force magnitude in N applied per physics step (scales linearly with proximity)."
        },
        "repulsor": {
          "type": "boolean",
          "default": false,
          "description": "false = attractor (pulls toward center), true = repulsor (pushes away)."
        }
      }
    },

    "ObjectAudio": {
      "type": "object",
      "description": "Per-object audio overrides.",
      "additionalProperties": false,
      "properties": {
        "bounce":  { "type": "string", "description": "Sound to play on bounce." },
        "destroy": { "type": "string", "description": "Sound to play on destruction." }
      }
    },

    "SceneObject": {
      "type": "object",
      "description": "A single simulated body.",
      "required": ["shape", "position"],
      "additionalProperties": false,
      "properties": {
        "name":     { "type": "string", "description": "Unique identifier (optional)." },
        "shape": {
          "type": "string",
          "enum": ["circle", "rectangle", "polygon"],
          "description": "Shape type."
        },
        "radius": {
          "type": "number",
          "exclusiveMinimum": 0,
          "description": "Circle radius in meters. Required when shape=circle."
        },
        "size": {
          "$ref": "#/definitions/Vec2",
          "description": "Rectangle [width, height] in meters. Required when shape=rectangle."
        },
        "vertices": {
          "type": "array",
          "items": { "$ref": "#/definitions/Vec2" },
          "minItems": 3,
          "description": "Polygon vertex offsets from center. Required when shape=polygon."
        },
        "position": {
          "$ref": "#/definitions/Vec2",
          "description": "Initial position in meters from world origin (bottom-left)."
        },
        "velocity": {
          "$ref": "#/definitions/Vec2",
          "default": [0.0, 0.0],
          "description": "Initial velocity in m/s."
        },
        "rotation": {
          "type": "number",
          "default": 0.0,
          "description": "Initial rotation in degrees (counter-clockwise positive)."
        },
        "angular_velocity": {
          "type": "number",
          "default": 0.0,
          "description": "Initial angular velocity in degrees/s."
        },
        "body_type": {
          "type": "string",
          "enum": ["dynamic", "static", "kinematic"],
          "default": "dynamic",
          "description": "Simulation mode."
        },
        "material": { "$ref": "#/definitions/Material" },
        "color": {
          "$ref": "#/definitions/Color",
          "default": "#ffffff",
          "description": "Fill color."
        },
        "tags": {
          "type": "array",
          "items": { "type": "string" },
          "default": [],
          "description": "Labels for grouping and end conditions."
        },
        "destructible": { "$ref": "#/definitions/Destructible" },
        "boost": { "$ref": "#/definitions/BoostConfig" },
        "gravity_well": { "$ref": "#/definitions/GravityWellConfig" },
        "audio": { "$ref": "#/definitions/ObjectAudio" }
      }
    },

    "EndCondition": {
      "type": "object",
      "description": "A condition that terminates the simulation when satisfied.",
      "required": ["type"],
      "properties": {
        "type": {
          "type": "string",
          "enum": [
            "time_limit",
            "all_tagged_destroyed",
            "object_escaped",
            "objects_collided",
            "tags_collided",
            "and",
            "or"
          ]
        }
      },
      "oneOf": [
        {
          "additionalProperties": false,
          "properties": {
            "type": { "const": "time_limit" },
            "seconds": { "type": "number", "exclusiveMinimum": 0 }
          },
          "required": ["seconds"]
        },
        {
          "additionalProperties": false,
          "properties": {
            "type": { "const": "all_tagged_destroyed" },
            "tag": { "type": "string" }
          },
          "required": ["tag"]
        },
        {
          "additionalProperties": false,
          "properties": {
            "type": { "const": "object_escaped" },
            "name": { "type": "string" }
          },
          "required": ["name"]
        },
        {
          "additionalProperties": false,
          "properties": {
            "type": { "const": "objects_collided" },
            "name_a": { "type": "string" },
            "name_b": { "type": "string" }
          },
          "required": ["name_a", "name_b"]
        },
        {
          "additionalProperties": false,
          "properties": {
            "type": { "const": "tags_collided" },
            "tag_a": { "type": "string" },
            "tag_b": { "type": "string" }
          },
          "required": ["tag_a", "tag_b"]
        },
        {
          "additionalProperties": false,
          "properties": {
            "type": { "const": "and" },
            "conditions": {
              "type": "array",
              "items": { "$ref": "#/definitions/EndCondition" },
              "minItems": 1
            }
          },
          "required": ["conditions"]
        },
        {
          "additionalProperties": false,
          "properties": {
            "type": { "const": "or" },
            "conditions": {
              "type": "array",
              "items": { "$ref": "#/definitions/EndCondition" },
              "minItems": 1
            }
          },
          "required": ["conditions"]
        }
      ]
    }

  }
}
"##;
