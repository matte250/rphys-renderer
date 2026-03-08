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
          "properties": {
            "type": { "const": "time_limit" },
            "seconds": { "type": "number", "exclusiveMinimum": 0 }
          },
          "required": ["seconds"]
        },
        {
          "properties": {
            "type": { "const": "all_tagged_destroyed" },
            "tag": { "type": "string" }
          },
          "required": ["tag"]
        },
        {
          "properties": {
            "type": { "const": "object_escaped" },
            "name": { "type": "string" }
          },
          "required": ["name"]
        },
        {
          "properties": {
            "type": { "const": "objects_collided" },
            "name_a": { "type": "string" },
            "name_b": { "type": "string" }
          },
          "required": ["name_a", "name_b"]
        },
        {
          "properties": {
            "type": { "const": "tags_collided" },
            "tag_a": { "type": "string" },
            "tag_b": { "type": "string" }
          },
          "required": ["tag_a", "tag_b"]
        },
        {
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
