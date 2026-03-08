//! Scene parsing, validation, and conversion.
//!
//! The public entry points are:
//! - [`parse_scene`]  — parse a YAML string
//! - [`parse_scene_file`] — parse a file, resolving relative asset paths

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::de::{
    RawEndCondition, RawEnvironment, RawObject, RawRaceConfig, RawScene, RawSceneAudio,
    RawWallConfig,
};
use crate::types::{
    BodyType, Checkpoint, Color, Destructible, EndCondition, Environment, Material, ObjectAudio,
    RaceConfig, Scene, SceneAudio, SceneMeta, SceneObject, ShapeKind, Vec2, WallConfig,
    WorldBounds,
};

// ── Error types ───────────────────────────────────────────────────────────────

/// Formats a list of [`ValidationError`]s as an indented bullet list.
pub(crate) fn format_validation_errors(errors: &[ValidationError]) -> String {
    errors
        .iter()
        .map(|e| format!("  - {e}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Top-level error returned by [`parse_scene`] and [`parse_scene_file`].
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// An I/O error occurred while reading the scene file.
    #[error("IO error reading scene file: {0}")]
    Io(#[from] std::io::Error),

    /// The YAML is syntactically invalid.
    #[error("YAML syntax error at line {line}: {message}")]
    Syntax { line: usize, message: String },

    /// One or more semantic validation rules failed.
    #[error("Validation failed:\n{}", format_validation_errors(.0))]
    Validation(Vec<ValidationError>),

    /// The file is empty or contains only whitespace.
    #[error("Empty scene file — nothing to simulate")]
    EmptyScene,

    /// The `version` field is present but not supported.
    #[error("Unsupported schema version '{version}' (expected '1')")]
    UnsupportedVersion { version: String },
}

/// A single semantic validation failure.
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    /// A required field is absent on a specific object.
    #[error("Object '{name}': missing required field '{field}'")]
    MissingField { name: String, field: &'static str },

    /// The `shape` field contains an unrecognised value.
    #[error("Object '{name}': unknown shape type '{shape}'")]
    UnknownShape { name: String, shape: String },

    /// A field value is outside its allowed range or format.
    #[error("Object '{name}': {message}")]
    InvalidValue { name: String, message: String },

    /// Two objects share the same name.
    #[error("Duplicate object name '{name}'")]
    DuplicateName { name: String },

    /// An audio file path cannot be found on disk.
    #[error("Audio file not found: '{path}'")]
    AudioFileNotFound { path: PathBuf },

    /// An end condition references an object name that does not exist.
    #[error("End condition references unknown object '{name}'")]
    UnknownObjectReference { name: String },
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Parse and validate a YAML string into a [`Scene`].
///
/// Returns structured errors — never raw serde panics.
///
/// Audio file existence is **not** checked when parsing from a string because
/// there is no base directory to resolve relative paths against.
///
/// # Errors
///
/// - [`ParseError::EmptyScene`] if `yaml` is empty or whitespace.
/// - [`ParseError::Syntax`] if the YAML is malformed.
/// - [`ParseError::UnsupportedVersion`] if `version` is not `"1"`.
/// - [`ParseError::Validation`] if semantic checks fail.
pub fn parse_scene(yaml: &str) -> Result<Scene, ParseError> {
    parse_scene_inner(yaml, None)
}

/// Parse from a file path.
///
/// Resolves relative asset paths (audio files) against the scene file's
/// directory, and validates that referenced audio files exist on disk.
///
/// # Errors
///
/// See [`parse_scene`] plus [`ParseError::Io`] for file-read failures.
pub fn parse_scene_file(path: &Path) -> Result<Scene, ParseError> {
    let yaml = std::fs::read_to_string(path)?;
    let base_dir = path.parent().map(Path::to_path_buf);
    parse_scene_inner(&yaml, base_dir.as_deref())
}

// ── Internal implementation ───────────────────────────────────────────────────

/// Shared implementation for both `parse_scene` and `parse_scene_file`.
fn parse_scene_inner(yaml: &str, base_dir: Option<&Path>) -> Result<Scene, ParseError> {
    // Check for empty input.
    if yaml.trim().is_empty() {
        return Err(ParseError::EmptyScene);
    }

    // Parse YAML into raw intermediate structs.
    let raw: RawScene = serde_yml::from_str(yaml).map_err(|e| {
        let line = e.location().map_or(0, |loc| loc.line());
        ParseError::Syntax {
            line,
            message: e.to_string(),
        }
    })?;

    // Version check.
    if raw.version != "1" {
        return Err(ParseError::UnsupportedVersion {
            version: raw.version,
        });
    }

    // Collect all validation errors before bailing — better UX than stopping
    // at the first error.
    let mut errors: Vec<ValidationError> = Vec::new();

    // Count name occurrences — used for both duplicate detection and
    // end-condition reference validation.  Names that appear more than once
    // are excluded from `named_objects` so that end-condition checks also
    // fail for ambiguous (duplicated) names.
    let name_counts: HashMap<&str, usize> = raw
        .objects
        .iter()
        .filter_map(|obj| obj.name.as_deref())
        .fold(HashMap::new(), |mut map, name| {
            *map.entry(name).or_insert(0) += 1;
            map
        });
    let named_objects: HashSet<String> = name_counts
        .iter()
        .filter(|(_, &count)| count == 1)
        .map(|(name, _)| name.to_string())
        .collect();

    // Validate and convert all objects.
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut objects: Vec<SceneObject> = Vec::new();

    for raw_obj in &raw.objects {
        match convert_object(raw_obj, base_dir, &mut seen_names, &mut errors) {
            Some(obj) => objects.push(obj),
            None => { /* errors already recorded */ }
        }
    }

    // Validate environment.
    let environment = convert_environment(&raw.environment, base_dir, &mut errors);

    // Validate and convert end condition.
    let end_condition = raw
        .end_condition
        .as_ref()
        .and_then(|ec| convert_end_condition(ec, &named_objects, &mut errors));

    // Validate meta.duration_hint (must be >= 0 if provided).
    if let Some(hint) = raw.meta.duration_hint {
        if hint < 0.0 {
            errors.push(ValidationError::InvalidValue {
                name: "meta.duration_hint".to_string(),
                message: format!("duration_hint must be >= 0, got {hint}"),
            });
        }
    }

    // Convert global audio.
    let audio = convert_scene_audio(raw.audio.as_ref(), base_dir, &mut errors);

    // Convert race config (optional).
    let race = raw
        .race
        .as_ref()
        .and_then(|rc| convert_race_config(rc, &mut errors));

    if !errors.is_empty() {
        return Err(ParseError::Validation(errors));
    }

    // SAFETY: `convert_environment` returns `None` only when it pushes a
    // validation error. Since `errors` is empty here, `environment` must be
    // `Some`. The explicit match guards against future regressions where
    // `convert_environment` might return `None` without recording an error.
    let environment = match environment {
        Some(e) => e,
        None => return Err(ParseError::Validation(errors)),
    };

    Ok(Scene {
        version: raw.version,
        meta: SceneMeta {
            name: raw.meta.name,
            description: raw.meta.description,
            author: raw.meta.author,
            duration_hint: raw.meta.duration_hint,
        },
        environment,
        objects,
        end_condition,
        audio,
        race,
    })
}

// ── Conversion helpers ────────────────────────────────────────────────────────

/// Convert a `RawObject` into a `SceneObject`, recording validation errors.
///
/// Returns `None` if conversion fails (errors are appended to `errors`).
fn convert_object(
    raw: &RawObject,
    base_dir: Option<&Path>,
    seen_names: &mut HashSet<String>,
    errors: &mut Vec<ValidationError>,
) -> Option<SceneObject> {
    // Use name for error messages; fall back to "<unnamed>" for display only.
    let display_name = raw.name.clone().unwrap_or_else(|| "<unnamed>".to_string());

    // Check for duplicate names.
    if let Some(ref name) = raw.name {
        if !seen_names.insert(name.clone()) {
            errors.push(ValidationError::DuplicateName { name: name.clone() });
        }
    }

    // Require `position`.
    let position = match raw.position {
        Some(p) => Vec2::new(p[0], p[1]),
        None => {
            errors.push(ValidationError::MissingField {
                name: display_name.clone(),
                field: "position",
            });
            return None;
        }
    };

    // Require `shape`.
    let shape_str = match &raw.shape {
        Some(s) => s.as_str(),
        None => {
            errors.push(ValidationError::MissingField {
                name: display_name.clone(),
                field: "shape",
            });
            return None;
        }
    };

    // Convert shape.
    let shape = match shape_str {
        "circle" => {
            let radius = match raw.radius {
                Some(r) => r,
                None => {
                    errors.push(ValidationError::MissingField {
                        name: display_name.clone(),
                        field: "radius",
                    });
                    return None;
                }
            };
            if radius <= 0.0 {
                errors.push(ValidationError::InvalidValue {
                    name: display_name.clone(),
                    message: format!("radius must be > 0, got {radius}"),
                });
                return None;
            }
            ShapeKind::Circle { radius }
        }
        "rectangle" => {
            let size = match raw.size {
                Some(s) => s,
                None => {
                    errors.push(ValidationError::MissingField {
                        name: display_name.clone(),
                        field: "size",
                    });
                    return None;
                }
            };
            if size[0] <= 0.0 || size[1] <= 0.0 {
                errors.push(ValidationError::InvalidValue {
                    name: display_name.clone(),
                    message: format!(
                        "rectangle size must be positive [w, h], got [{}, {}]",
                        size[0], size[1]
                    ),
                });
                return None;
            }
            ShapeKind::Rectangle {
                width: size[0],
                height: size[1],
            }
        }
        "polygon" => {
            let raw_verts = match &raw.vertices {
                Some(v) => v,
                None => {
                    errors.push(ValidationError::MissingField {
                        name: display_name.clone(),
                        field: "vertices",
                    });
                    return None;
                }
            };
            if raw_verts.len() < 3 {
                errors.push(ValidationError::InvalidValue {
                    name: display_name.clone(),
                    message: format!(
                        "polygon must have at least 3 vertices, got {}",
                        raw_verts.len()
                    ),
                });
                return None;
            }
            let vertices = raw_verts
                .iter()
                .map(|v| Vec2::new(v[0], v[1]))
                .collect::<Vec<_>>();
            ShapeKind::Polygon { vertices }
        }
        unknown => {
            errors.push(ValidationError::UnknownShape {
                name: display_name.clone(),
                shape: unknown.to_string(),
            });
            return None;
        }
    };

    // Convert optional fields with defaults.
    let velocity = raw.velocity.map_or(Vec2::ZERO, |v| Vec2::new(v[0], v[1]));

    // YAML stores degrees; internal representation is radians.
    let rotation = raw.rotation.map_or(0.0, |deg| deg.to_radians());
    let angular_velocity = raw.angular_velocity.map_or(0.0, |deg_s| deg_s.to_radians());

    let body_type = match raw.body_type.as_deref() {
        None | Some("dynamic") => BodyType::Dynamic,
        Some("static") => BodyType::Static,
        Some("kinematic") => BodyType::Kinematic,
        Some(unknown) => {
            errors.push(ValidationError::InvalidValue {
                name: display_name.clone(),
                message: format!(
                    "unknown body_type '{unknown}' (expected 'dynamic', 'static', or 'kinematic')"
                ),
            });
            return None;
        }
    };

    // Convert material — validate all three fields before returning so all
    // material errors are reported at once rather than stopping at the first.
    let material = match &raw.material {
        None => Material::default(),
        Some(rm) => {
            let restitution = rm.restitution.unwrap_or(0.5);
            let friction = rm.friction.unwrap_or(0.5);
            let density = rm.density.unwrap_or(1.0);

            let mut material_ok = true;
            if !(0.0..=1.0).contains(&restitution) {
                errors.push(ValidationError::InvalidValue {
                    name: display_name.clone(),
                    message: format!("restitution must be between 0.0 and 1.0, got {restitution}"),
                });
                material_ok = false;
            }
            if friction < 0.0 {
                errors.push(ValidationError::InvalidValue {
                    name: display_name.clone(),
                    message: format!("friction must be >= 0.0, got {friction}"),
                });
                material_ok = false;
            }
            if density <= 0.0 {
                errors.push(ValidationError::InvalidValue {
                    name: display_name.clone(),
                    message: format!("density must be > 0.0, got {density}"),
                });
                material_ok = false;
            }
            if !material_ok {
                return None;
            }
            Material {
                restitution,
                friction,
                density,
            }
        }
    };

    // Convert color.
    let color = match &raw.color {
        None => Color::WHITE,
        Some(hex) => match parse_hex_color(hex) {
            Ok(c) => c,
            Err(msg) => {
                errors.push(ValidationError::InvalidValue {
                    name: display_name.clone(),
                    message: msg,
                });
                return None;
            }
        },
    };

    let tags = raw.tags.clone().unwrap_or_default();

    let destructible = match &raw.destructible {
        None => None,
        Some(d) => {
            if d.min_impact_force <= 0.0 {
                errors.push(ValidationError::InvalidValue {
                    name: display_name.clone(),
                    message: format!(
                        "destructible.min_impact_force must be > 0, got {}",
                        d.min_impact_force
                    ),
                });
                return None;
            }
            Some(Destructible {
                min_impact_force: d.min_impact_force,
            })
        }
    };

    let audio = convert_object_audio(raw.audio.as_ref(), base_dir, errors);

    Some(SceneObject {
        name: raw.name.clone(),
        shape,
        position,
        velocity,
        rotation,
        angular_velocity,
        body_type,
        material,
        color,
        tags,
        destructible,
        audio,
    })
}

/// Convert `RawEnvironment` into [`Environment`], recording errors.
///
/// Returns `None` only if a required color parse fails.
fn convert_environment(
    raw: &RawEnvironment,
    _base_dir: Option<&Path>,
    errors: &mut Vec<ValidationError>,
) -> Option<Environment> {
    let gravity = Vec2::new(raw.gravity[0], raw.gravity[1]);

    let background_color = match parse_hex_color(&raw.background_color) {
        Ok(c) => c,
        Err(msg) => {
            errors.push(ValidationError::InvalidValue {
                name: "environment.background_color".to_string(),
                message: msg,
            });
            return None;
        }
    };

    if raw.world_bounds.width <= 0.0 {
        errors.push(ValidationError::InvalidValue {
            name: "environment.world_bounds.width".to_string(),
            message: format!(
                "world_bounds.width must be > 0, got {}",
                raw.world_bounds.width
            ),
        });
        return None;
    }
    if raw.world_bounds.height <= 0.0 {
        errors.push(ValidationError::InvalidValue {
            name: "environment.world_bounds.height".to_string(),
            message: format!(
                "world_bounds.height must be > 0, got {}",
                raw.world_bounds.height
            ),
        });
        return None;
    }

    let walls = convert_wall_config(&raw.walls, errors)?;

    Some(Environment {
        gravity,
        background_color,
        world_bounds: WorldBounds {
            width: raw.world_bounds.width,
            height: raw.world_bounds.height,
        },
        walls,
    })
}

/// Convert `RawWallConfig` into [`WallConfig`], recording errors.
fn convert_wall_config(
    raw: &RawWallConfig,
    errors: &mut Vec<ValidationError>,
) -> Option<WallConfig> {
    let visible = raw.visible.unwrap_or(true);
    let color = match &raw.color {
        None => Color::WHITE,
        Some(hex) => match parse_hex_color(hex) {
            Ok(c) => c,
            Err(msg) => {
                errors.push(ValidationError::InvalidValue {
                    name: "environment.walls.color".to_string(),
                    message: msg,
                });
                return None;
            }
        },
    };
    let thickness = raw.thickness.unwrap_or(0.3);
    if thickness <= 0.0 {
        errors.push(ValidationError::InvalidValue {
            name: "environment.walls.thickness".to_string(),
            message: format!("walls.thickness must be > 0, got {thickness}"),
        });
        return None;
    }
    Some(WallConfig {
        visible,
        color,
        thickness,
    })
}

/// Convert a `RawEndCondition` to an [`EndCondition`], recording errors.
///
/// Returns `None` if any referenced object name is not in `named_objects`.
fn convert_end_condition(
    raw: &RawEndCondition,
    named_objects: &HashSet<String>,
    errors: &mut Vec<ValidationError>,
) -> Option<EndCondition> {
    match raw {
        RawEndCondition::TimeLimit { seconds } => {
            if *seconds <= 0.0 {
                errors.push(ValidationError::InvalidValue {
                    name: "end_condition.time_limit".to_string(),
                    message: format!("time_limit.seconds must be > 0, got {seconds}"),
                });
                return None;
            }
            Some(EndCondition::TimeLimit { seconds: *seconds })
        }
        RawEndCondition::AllTaggedDestroyed { tag } => {
            Some(EndCondition::AllTaggedDestroyed { tag: tag.clone() })
        }
        RawEndCondition::ObjectEscaped { name } => {
            if !named_objects.contains(name) {
                errors.push(ValidationError::UnknownObjectReference { name: name.clone() });
                return None;
            }
            Some(EndCondition::ObjectEscaped { name: name.clone() })
        }
        RawEndCondition::ObjectsCollided { name_a, name_b } => {
            let mut ok = true;
            if !named_objects.contains(name_a) {
                errors.push(ValidationError::UnknownObjectReference {
                    name: name_a.clone(),
                });
                ok = false;
            }
            if !named_objects.contains(name_b) {
                errors.push(ValidationError::UnknownObjectReference {
                    name: name_b.clone(),
                });
                ok = false;
            }
            if ok {
                Some(EndCondition::ObjectsCollided {
                    name_a: name_a.clone(),
                    name_b: name_b.clone(),
                })
            } else {
                None
            }
        }
        RawEndCondition::TagsCollided { tag_a, tag_b } => Some(EndCondition::TagsCollided {
            tag_a: tag_a.clone(),
            tag_b: tag_b.clone(),
        }),
        RawEndCondition::And { conditions } => {
            let converted: Vec<EndCondition> = conditions
                .iter()
                .filter_map(|c| convert_end_condition(c, named_objects, errors))
                .collect();
            // If any sub-condition emitted errors, `converted` may be shorter.
            // We still emit the And with whatever converted successfully, but
            // the outer validation error list will catch this.
            if converted.len() == conditions.len() {
                Some(EndCondition::And {
                    conditions: converted,
                })
            } else {
                None
            }
        }
        RawEndCondition::Or { conditions } => {
            let converted: Vec<EndCondition> = conditions
                .iter()
                .filter_map(|c| convert_end_condition(c, named_objects, errors))
                .collect();
            if converted.len() == conditions.len() {
                Some(EndCondition::Or {
                    conditions: converted,
                })
            } else {
                None
            }
        }
        RawEndCondition::FirstToReach { finish_y, tag } => {
            if *finish_y < 0.0 {
                errors.push(ValidationError::InvalidValue {
                    name: "end_condition.first_to_reach".to_string(),
                    message: format!("first_to_reach.finish_y must be >= 0, got {finish_y}"),
                });
                return None;
            }
            Some(EndCondition::FirstToReach {
                finish_y: *finish_y,
                tag: tag.clone().unwrap_or_else(|| "racer".to_string()),
            })
        }
    }
}

/// Convert per-object audio overrides.
fn convert_object_audio(
    raw: Option<&crate::de::RawObjectAudio>,
    base_dir: Option<&Path>,
    errors: &mut Vec<ValidationError>,
) -> ObjectAudio {
    let raw = match raw {
        None => return ObjectAudio::default(),
        Some(r) => r,
    };

    let bounce = raw.bounce.as_ref().map(|p| {
        let path = resolve_path(p, base_dir);
        if let Some(dir) = base_dir {
            if !dir.join(p).exists() {
                errors.push(ValidationError::AudioFileNotFound { path: dir.join(p) });
            }
        }
        path
    });

    let destroy = raw.destroy.as_ref().map(|p| {
        let path = resolve_path(p, base_dir);
        if let Some(dir) = base_dir {
            let full = dir.join(p);
            if !full.exists() {
                errors.push(ValidationError::AudioFileNotFound { path: full });
            }
        }
        path
    });

    ObjectAudio { bounce, destroy }
}

/// Convert global scene audio config.
fn convert_scene_audio(
    raw: Option<&RawSceneAudio>,
    base_dir: Option<&Path>,
    errors: &mut Vec<ValidationError>,
) -> SceneAudio {
    let raw = match raw {
        None => {
            return SceneAudio {
                default_bounce: None,
                default_destroy: None,
                master_volume: 1.0,
            }
        }
        Some(r) => r,
    };

    let default_bounce = raw.default_bounce.as_ref().map(|p| {
        let path = resolve_path(p, base_dir);
        if let Some(dir) = base_dir {
            let full = dir.join(p);
            if !full.exists() {
                errors.push(ValidationError::AudioFileNotFound { path: full });
            }
        }
        path
    });

    let default_destroy = raw.default_destroy.as_ref().map(|p| {
        let path = resolve_path(p, base_dir);
        if let Some(dir) = base_dir {
            let full = dir.join(p);
            if !full.exists() {
                errors.push(ValidationError::AudioFileNotFound { path: full });
            }
        }
        path
    });

    let master_volume = raw.master_volume.unwrap_or(1.0);
    if !(0.0..=1.0).contains(&master_volume) {
        errors.push(ValidationError::InvalidValue {
            name: "audio.master_volume".to_string(),
            message: format!("master_volume must be between 0.0 and 1.0, got {master_volume}"),
        });
    }

    SceneAudio {
        default_bounce,
        default_destroy,
        master_volume,
    }
}

/// Convert a `RawRaceConfig` into a [`RaceConfig`], recording validation errors.
///
/// Returns `None` if any validation rule is violated.
fn convert_race_config(
    raw: &RawRaceConfig,
    errors: &mut Vec<ValidationError>,
) -> Option<RaceConfig> {
    let finish_y = raw.finish_y;
    let racer_tag = raw.racer_tag.clone().unwrap_or_else(|| "racer".to_string());
    let announcement_hold_secs = raw.announcement_hold_secs.unwrap_or(2.0);

    let mut ok = true;

    if finish_y < 0.0 {
        errors.push(ValidationError::InvalidValue {
            name: "race.finish_y".to_string(),
            message: format!("race.finish_y must be >= 0, got {finish_y}"),
        });
        ok = false;
    }

    if announcement_hold_secs <= 0.0 {
        errors.push(ValidationError::InvalidValue {
            name: "race.announcement_hold_secs".to_string(),
            message: format!(
                "race.announcement_hold_secs must be > 0, got {announcement_hold_secs}"
            ),
        });
        ok = false;
    }

    let mut checkpoints: Vec<Checkpoint> = Vec::new();
    for (i, raw_cp) in raw.checkpoints.iter().enumerate() {
        if raw_cp.y <= finish_y {
            errors.push(ValidationError::InvalidValue {
                name: format!("race.checkpoints[{i}]"),
                message: format!(
                    "checkpoint y ({}) must be > finish_y ({finish_y})",
                    raw_cp.y
                ),
            });
            ok = false;
        } else {
            checkpoints.push(Checkpoint {
                y: raw_cp.y,
                label: raw_cp.label.clone(),
            });
        }
    }

    if !ok {
        return None;
    }

    Some(RaceConfig {
        finish_y,
        racer_tag,
        announcement_hold_secs,
        checkpoints,
    })
}

// ── Low-level helpers ─────────────────────────────────────────────────────────

/// Parse a `"#RRGGBB"` or `"#RRGGBBAA"` hex color string into a [`Color`].
///
/// The `#` prefix is required; strings without it are rejected.
pub(crate) fn parse_hex_color(s: &str) -> Result<Color, String> {
    let hex = s
        .strip_prefix('#')
        .ok_or_else(|| format!("color must be '#RRGGBB' or '#RRGGBBAA' hex, got '{s}'"))?;
    match hex.len() {
        6 => {
            let r = parse_hex_byte(&hex[0..2], s)?;
            let g = parse_hex_byte(&hex[2..4], s)?;
            let b = parse_hex_byte(&hex[4..6], s)?;
            Ok(Color::rgb(r, g, b))
        }
        8 => {
            let r = parse_hex_byte(&hex[0..2], s)?;
            let g = parse_hex_byte(&hex[2..4], s)?;
            let b = parse_hex_byte(&hex[4..6], s)?;
            let a = parse_hex_byte(&hex[6..8], s)?;
            Ok(Color::rgba(r, g, b, a))
        }
        _ => Err(format!(
            "color must be '#RRGGBB' or '#RRGGBBAA' hex, got '{s}'"
        )),
    }
}

/// Parse a two-character hex byte.
fn parse_hex_byte(hex2: &str, full_input: &str) -> Result<u8, String> {
    u8::from_str_radix(hex2, 16).map_err(|_| format!("invalid hex color '{full_input}'"))
}

/// Resolve a path string against an optional base directory.
///
/// If `base_dir` is `None`, returns the path as-is.
/// Absolute paths are returned unchanged.
fn resolve_path(p: &str, base_dir: Option<&Path>) -> PathBuf {
    let path = PathBuf::from(p);
    if path.is_absolute() {
        return path;
    }
    match base_dir {
        Some(dir) => dir.join(&path),
        None => path,
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid YAML snippet (no objects, no end condition).
    fn minimal_yaml() -> &'static str {
        r##"
version: "1"
meta:
  name: "Test Scene"
environment:
  gravity: [0.0, -9.81]
  background_color: "#1a1a2e"
  world_bounds:
    width: 20.0
    height: 35.56
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects: []
"##
    }

    // ── Happy-path tests ──────────────────────────────────────────────────────

    #[test]
    fn test_parse_minimal_scene() {
        let scene = parse_scene(minimal_yaml()).expect("should parse");
        assert_eq!(scene.version, "1");
        assert_eq!(scene.meta.name, "Test Scene");
        assert!(scene.objects.is_empty());
        assert_eq!(scene.environment.gravity, Vec2::new(0.0, -9.81));
    }

    #[test]
    fn test_parse_scene_meta_optional_fields() {
        let yaml = r##"
version: "1"
meta:
  name: "Full Meta"
  description: "A test scene"
  author: "rphys"
  duration_hint: 30.0
environment:
  gravity: [0.0, 0.0]
  background_color: "#000000"
  world_bounds:
    width: 10.0
    height: 10.0
  walls:
    visible: false
    color: "#333333"
    thickness: 0.5
objects: []
"##;
        let scene = parse_scene(yaml).expect("should parse");
        assert_eq!(scene.meta.description.as_deref(), Some("A test scene"));
        assert_eq!(scene.meta.author.as_deref(), Some("rphys"));
        assert_eq!(scene.meta.duration_hint, Some(30.0));
    }

    #[test]
    fn test_parse_circle_object() {
        let yaml = r##"
version: "1"
meta:
  name: "Circle Test"
environment:
  gravity: [0.0, -9.81]
  background_color: "#1a1a2e"
  world_bounds:
    width: 20.0
    height: 35.56
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    radius: 0.8
    position: [10.0, 20.0]
    velocity: [2.0, 0.0]
    color: "#ff0000"
    tags: ["bouncy"]
    material:
      restitution: 0.9
      friction: 0.1
      density: 1.5
"##;
        let scene = parse_scene(yaml).expect("should parse circle");
        assert_eq!(scene.objects.len(), 1);
        let obj = &scene.objects[0];
        assert_eq!(obj.name.as_deref(), Some("ball"));
        assert!(matches!(obj.shape, ShapeKind::Circle { radius } if (radius - 0.8).abs() < 1e-6));
        assert_eq!(obj.position, Vec2::new(10.0, 20.0));
        assert_eq!(obj.velocity, Vec2::new(2.0, 0.0));
        assert_eq!(obj.tags, vec!["bouncy".to_string()]);
        assert_eq!(obj.color, Color::rgb(0xff, 0x00, 0x00));
        assert!((obj.material.restitution - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_parse_rectangle_object() {
        let yaml = r##"
version: "1"
meta:
  name: "Rect Test"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "box"
    shape: rectangle
    size: [3.0, 2.0]
    position: [5.0, 10.0]
    body_type: static
"##;
        let scene = parse_scene(yaml).expect("should parse rectangle");
        let obj = &scene.objects[0];
        assert!(matches!(
            obj.shape,
            ShapeKind::Rectangle { width, height } if (width - 3.0).abs() < 1e-6 && (height - 2.0).abs() < 1e-6
        ));
        assert_eq!(obj.body_type, BodyType::Static);
    }

    #[test]
    fn test_parse_polygon_object() {
        let yaml = r##"
version: "1"
meta:
  name: "Poly Test"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "tri"
    shape: polygon
    vertices:
      - [0.0, 1.0]
      - [1.0, -0.5]
      - [-1.0, -0.5]
    position: [10.0, 15.0]
"##;
        let scene = parse_scene(yaml).expect("should parse polygon");
        let obj = &scene.objects[0];
        if let ShapeKind::Polygon { vertices } = &obj.shape {
            assert_eq!(vertices.len(), 3);
        } else {
            panic!("expected polygon");
        }
    }

    #[test]
    fn test_rotation_converted_to_radians() {
        let yaml = r##"
version: "1"
meta:
  name: "Rotation Test"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "tilted"
    shape: circle
    radius: 1.0
    position: [10.0, 10.0]
    rotation: 90.0
    angular_velocity: 180.0
"##;
        let scene = parse_scene(yaml).expect("should parse");
        let obj = &scene.objects[0];
        // 90 degrees = π/2 radians
        assert!((obj.rotation - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        // 180 degrees/s = π rad/s
        assert!((obj.angular_velocity - std::f32::consts::PI).abs() < 1e-5);
    }

    #[test]
    fn test_parse_end_condition_time_limit() {
        let yaml = r##"
version: "1"
meta:
  name: "EC Test"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects: []
end_condition:
  type: time_limit
  seconds: 30.0
"##;
        let scene = parse_scene(yaml).expect("should parse");
        assert!(matches!(
            scene.end_condition,
            Some(EndCondition::TimeLimit { seconds }) if (seconds - 30.0).abs() < 1e-6
        ));
    }

    #[test]
    fn test_parse_end_condition_or_composite() {
        let yaml = r##"
version: "1"
meta:
  name: "Composite EC"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    radius: 0.5
    position: [10.0, 10.0]
end_condition:
  type: or
  conditions:
    - type: time_limit
      seconds: 60.0
    - type: object_escaped
      name: "ball"
"##;
        let scene = parse_scene(yaml).expect("should parse composite EC");
        if let Some(EndCondition::Or { conditions }) = &scene.end_condition {
            assert_eq!(conditions.len(), 2);
            assert!(matches!(conditions[0], EndCondition::TimeLimit { .. }));
            assert!(matches!(conditions[1], EndCondition::ObjectEscaped { .. }));
        } else {
            panic!("expected Or end condition");
        }
    }

    #[test]
    fn test_parse_destructible() {
        let yaml = r##"
version: "1"
meta:
  name: "Destructible"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "brick"
    shape: rectangle
    size: [2.0, 0.8]
    position: [5.0, 20.0]
    body_type: static
    destructible:
      min_impact_force: 5.0
"##;
        let scene = parse_scene(yaml).expect("should parse destructible");
        let obj = &scene.objects[0];
        let d = obj.destructible.as_ref().expect("should have destructible");
        assert!((d.min_impact_force - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_parse_color_rgba() {
        let c = parse_hex_color("#ff8800cc").expect("valid rgba color");
        assert_eq!(c, Color::rgba(0xff, 0x88, 0x00, 0xcc));
    }

    #[test]
    fn test_parse_color_rgb() {
        let c = parse_hex_color("#1a2b3c").expect("valid rgb color");
        assert_eq!(c, Color::rgb(0x1a, 0x2b, 0x3c));
    }

    #[test]
    fn test_parse_color_without_hash_is_rejected() {
        // Spec requires '#' prefix — strings without it must be rejected.
        let err = parse_hex_color("ff0000").unwrap_err();
        assert!(err.contains("'#RRGGBB' or '#RRGGBBAA'"), "got: {err}");
    }

    // ── Error-case tests ──────────────────────────────────────────────────────

    #[test]
    fn test_empty_yaml_returns_empty_scene_error() {
        let err = parse_scene("").unwrap_err();
        assert!(matches!(err, ParseError::EmptyScene));
    }

    #[test]
    fn test_whitespace_only_returns_empty_scene_error() {
        let err = parse_scene("   \n\t  ").unwrap_err();
        assert!(matches!(err, ParseError::EmptyScene));
    }

    #[test]
    fn test_unsupported_version() {
        let yaml = r##"
version: "2"
meta:
  name: "Future"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects: []
"##;
        let err = parse_scene(yaml).unwrap_err();
        assert!(matches!(
            err,
            ParseError::UnsupportedVersion { version } if version == "2"
        ));
    }

    #[test]
    fn test_syntax_error() {
        let yaml = "version: [\ninvalid: yaml: structure:";
        let err = parse_scene(yaml).unwrap_err();
        assert!(matches!(err, ParseError::Syntax { .. }));
    }

    #[test]
    fn test_missing_shape_field() {
        let yaml = r##"
version: "1"
meta:
  name: "Missing Shape"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    position: [10.0, 10.0]
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::MissingField { name, field }
                    if name == "ball" && *field == "shape"
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_unknown_shape_type() {
        let yaml = r##"
version: "1"
meta:
  name: "Unknown Shape"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "weird"
    shape: sphere
    position: [10.0, 10.0]
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::UnknownShape { name, shape }
                    if name == "weird" && shape == "sphere"
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_circle_missing_radius() {
        let yaml = r##"
version: "1"
meta:
  name: "No Radius"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    position: [10.0, 10.0]
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::MissingField { name, field }
                    if name == "ball" && *field == "radius"
            )));
        } else {
            panic!("expected Validation error");
        }
    }

    #[test]
    fn test_rectangle_missing_size() {
        let yaml = r##"
version: "1"
meta:
  name: "No Size"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "box"
    shape: rectangle
    position: [10.0, 10.0]
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::MissingField { name, field }
                    if name == "box" && *field == "size"
            )));
        } else {
            panic!("expected Validation error");
        }
    }

    #[test]
    fn test_duplicate_object_name() {
        let yaml = r##"
version: "1"
meta:
  name: "Duplicate Names"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    radius: 0.5
    position: [5.0, 10.0]
  - name: "ball"
    shape: circle
    radius: 0.5
    position: [15.0, 10.0]
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::DuplicateName { name } if name == "ball"
            )));
        } else {
            panic!("expected Validation error");
        }
    }

    #[test]
    fn test_invalid_restitution_out_of_range() {
        let yaml = r##"
version: "1"
meta:
  name: "Bad Restitution"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    radius: 0.5
    position: [10.0, 10.0]
    material:
      restitution: 1.5
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, message }
                    if name == "ball" && message.contains("restitution")
            )));
        } else {
            panic!("expected Validation error");
        }
    }

    #[test]
    fn test_end_condition_references_unknown_object() {
        let yaml = r##"
version: "1"
meta:
  name: "Bad Reference"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects: []
end_condition:
  type: object_escaped
  name: "nonexistent"
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::UnknownObjectReference { name }
                    if name == "nonexistent"
            )));
        } else {
            panic!("expected Validation error");
        }
    }

    #[test]
    fn test_invalid_hex_color_returns_error() {
        let err = parse_hex_color("#xyz123").unwrap_err();
        assert!(err.contains("invalid hex"));
    }

    #[test]
    fn test_invalid_hex_color_wrong_length() {
        let err = parse_hex_color("#ff00").unwrap_err();
        assert!(err.contains("'#RRGGBB' or '#RRGGBBAA'"));
    }

    #[test]
    fn test_parse_tags_collided_end_condition() {
        let yaml = r##"
version: "1"
meta:
  name: "Tags EC"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects: []
end_condition:
  type: tags_collided
  tag_a: "ball"
  tag_b: "goal"
"##;
        let scene = parse_scene(yaml).expect("should parse tags_collided");
        assert!(matches!(
            scene.end_condition,
            Some(EndCondition::TagsCollided { tag_a, tag_b })
                if tag_a == "ball" && tag_b == "goal"
        ));
    }

    #[test]
    fn test_all_tagged_destroyed_end_condition() {
        let yaml = r##"
version: "1"
meta:
  name: "Destroy All"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects: []
end_condition:
  type: all_tagged_destroyed
  tag: "enemy"
"##;
        let scene = parse_scene(yaml).expect("should parse");
        assert!(matches!(
            scene.end_condition,
            Some(EndCondition::AllTaggedDestroyed { tag }) if tag == "enemy"
        ));
    }

    #[test]
    fn test_material_defaults_when_omitted() {
        let yaml = r##"
version: "1"
meta:
  name: "Default Material"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - shape: circle
    radius: 1.0
    position: [10.0, 10.0]
"##;
        let scene = parse_scene(yaml).expect("should parse");
        let mat = &scene.objects[0].material;
        assert!((mat.restitution - 0.5).abs() < 1e-6);
        assert!((mat.friction - 0.5).abs() < 1e-6);
        assert!((mat.density - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_scene_audio_defaults() {
        let scene = parse_scene(minimal_yaml()).expect("should parse");
        assert!((scene.audio.master_volume - 1.0).abs() < 1e-6);
        assert!(scene.audio.default_bounce.is_none());
        assert!(scene.audio.default_destroy.is_none());
    }

    #[test]
    fn test_parse_scene_file() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().expect("temp file");
        tmp.write_all(minimal_yaml().as_bytes()).expect("write");
        let scene = super::super::parse_scene_file(tmp.path()).expect("should parse from file");
        assert_eq!(scene.version, "1");
        assert_eq!(scene.meta.name, "Test Scene");
    }

    #[test]
    fn test_parse_end_condition_and_composite() {
        let yaml = r##"
version: "1"
meta:
  name: "And EC"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    radius: 0.5
    position: [10.0, 10.0]
end_condition:
  type: and
  conditions:
    - type: time_limit
      seconds: 60.0
    - type: object_escaped
      name: "ball"
"##;
        let scene = parse_scene(yaml).expect("should parse And EC");
        if let Some(EndCondition::And { conditions }) = &scene.end_condition {
            assert_eq!(conditions.len(), 2);
            assert!(matches!(conditions[0], EndCondition::TimeLimit { .. }));
            assert!(matches!(conditions[1], EndCondition::ObjectEscaped { .. }));
        } else {
            panic!("expected And end condition");
        }
    }

    #[test]
    fn test_parse_nested_composite_end_condition() {
        let yaml = r##"
version: "1"
meta:
  name: "Nested EC"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    radius: 0.5
    position: [10.0, 10.0]
end_condition:
  type: or
  conditions:
    - type: time_limit
      seconds: 30.0
    - type: and
      conditions:
        - type: object_escaped
          name: "ball"
        - type: all_tagged_destroyed
          tag: "enemy"
"##;
        let scene = parse_scene(yaml).expect("should parse nested EC");
        if let Some(EndCondition::Or { conditions }) = &scene.end_condition {
            assert_eq!(conditions.len(), 2);
            assert!(matches!(conditions[1], EndCondition::And { .. }));
        } else {
            panic!("expected nested Or/And end condition");
        }
    }

    #[test]
    fn test_missing_position_field() {
        let yaml = r##"
version: "1"
meta:
  name: "Missing Position"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    radius: 0.5
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::MissingField { name, field }
                    if name == "ball" && *field == "position"
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_polygon_too_few_vertices() {
        let yaml = r##"
version: "1"
meta:
  name: "Bad Polygon"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "bad"
    shape: polygon
    vertices:
      - [0.0, 1.0]
      - [1.0, -1.0]
    position: [10.0, 10.0]
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, message }
                    if name == "bad" && message.contains("at least 3 vertices")
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_circle_negative_radius() {
        let yaml = r##"
version: "1"
meta:
  name: "Negative Radius"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "bad"
    shape: circle
    radius: -1.0
    position: [10.0, 10.0]
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, message }
                    if name == "bad" && message.contains("radius")
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_invalid_body_type() {
        let yaml = r##"
version: "1"
meta:
  name: "Bad Body Type"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "bad"
    shape: circle
    radius: 1.0
    position: [10.0, 10.0]
    body_type: "flying"
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, message }
                    if name == "bad" && message.contains("body_type")
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_multiple_object_errors_accumulate() {
        // Two separate objects both have errors — verify both are reported.
        let yaml = r##"
version: "1"
meta:
  name: "Multi Error"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "a"
    shape: circle
    radius: -1.0
    position: [5.0, 5.0]
  - name: "b"
    shape: circle
    radius: -2.0
    position: [15.0, 15.0]
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            let a_err = errs.iter().any(|e| {
                matches!(
                    e,
                    ValidationError::InvalidValue { name, .. } if name == "a"
                )
            });
            let b_err = errs.iter().any(|e| {
                matches!(
                    e,
                    ValidationError::InvalidValue { name, .. } if name == "b"
                )
            });
            assert!(a_err, "expected error for object 'a'");
            assert!(b_err, "expected error for object 'b'");
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_objects_collided_end_condition_valid() {
        let yaml = r##"
version: "1"
meta:
  name: "Collided EC"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "a"
    shape: circle
    radius: 0.5
    position: [5.0, 10.0]
  - name: "b"
    shape: circle
    radius: 0.5
    position: [15.0, 10.0]
end_condition:
  type: objects_collided
  name_a: "a"
  name_b: "b"
"##;
        let scene = parse_scene(yaml).expect("should parse objects_collided");
        assert!(matches!(
            scene.end_condition,
            Some(EndCondition::ObjectsCollided { name_a, name_b })
                if name_a == "a" && name_b == "b"
        ));
    }

    #[test]
    fn test_objects_collided_end_condition_unknown_name() {
        let yaml = r##"
version: "1"
meta:
  name: "Collided EC Bad"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "a"
    shape: circle
    radius: 0.5
    position: [5.0, 10.0]
end_condition:
  type: objects_collided
  name_a: "a"
  name_b: "ghost"
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::UnknownObjectReference { name } if name == "ghost"
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_scene_json_schema_is_valid_json() {
        use crate::scene_json_schema;
        let schema = scene_json_schema();
        let _: serde_json::Value =
            serde_json::from_str(schema).expect("scene_json_schema() must be valid JSON");
    }

    #[test]
    fn test_wall_config_defaults() {
        // walls block with no sub-fields — all should use defaults.
        let yaml = r##"
version: "1"
meta:
  name: "Wall Defaults"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls: {}
objects: []
"##;
        let scene = parse_scene(yaml).expect("should parse with empty walls block");
        assert!(scene.environment.walls.visible);
        assert_eq!(scene.environment.walls.color, Color::WHITE);
        assert!((scene.environment.walls.thickness - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_object_audio_parsed_from_yaml() {
        // Parse a scene with object audio paths — no base_dir so no file check.
        let yaml = r##"
version: "1"
meta:
  name: "Object Audio"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "ball"
    shape: circle
    radius: 0.5
    position: [10.0, 10.0]
    audio:
      bounce: "sounds/bounce.wav"
      destroy: "sounds/pop.wav"
"##;
        let scene = parse_scene(yaml).expect("should parse object audio");
        let audio = &scene.objects[0].audio;
        assert!(audio.bounce.is_some());
        assert!(audio.destroy.is_some());
    }

    #[test]
    fn test_time_limit_seconds_must_be_positive() {
        let yaml = r##"
version: "1"
meta:
  name: "Bad Seconds"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects: []
end_condition:
  type: time_limit
  seconds: -10.0
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { message, .. }
                    if message.contains("seconds")
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_master_volume_out_of_range() {
        let yaml = r##"
version: "1"
meta:
  name: "Bad Volume"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects: []
audio:
  master_volume: 2.5
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, .. }
                    if name == "audio.master_volume"
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_wall_thickness_must_be_positive() {
        let yaml = r##"
version: "1"
meta:
  name: "Bad Wall"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: -0.5
objects: []
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, .. }
                    if name == "environment.walls.thickness"
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    // ── Race / Sprint 2 tests ─────────────────────────────────────────────────

    /// Helper: build a minimal YAML string with an optional suffix appended.
    fn minimal_yaml_with(extra: &str) -> String {
        format!("{}{}", minimal_yaml(), extra)
    }

    #[test]
    fn test_parse_race_config_full() {
        let yaml = minimal_yaml_with(
            r#"race:
  finish_y: 2.0
  racer_tag: "racer"
  announcement_hold_secs: 3.0
  checkpoints:
    - y: 28.0
      label: "Checkpoint 1"
    - y: 15.0
"#,
        );
        let scene = parse_scene(&yaml).expect("should parse race config");
        let race = scene.race.as_ref().expect("race should be Some");
        assert!((race.finish_y - 2.0).abs() < 1e-6);
        assert_eq!(race.racer_tag, "racer");
        assert!((race.announcement_hold_secs - 3.0).abs() < 1e-6);
        assert_eq!(race.checkpoints.len(), 2);
        assert!((race.checkpoints[0].y - 28.0).abs() < 1e-6);
        assert_eq!(race.checkpoints[0].label.as_deref(), Some("Checkpoint 1"));
        assert!((race.checkpoints[1].y - 15.0).abs() < 1e-6);
        assert!(race.checkpoints[1].label.is_none());
    }

    #[test]
    fn test_parse_race_config_defaults() {
        // racer_tag and announcement_hold_secs should use defaults when omitted.
        let yaml = minimal_yaml_with(
            r#"race:
  finish_y: 0.0
"#,
        );
        let scene = parse_scene(&yaml).expect("should parse race with defaults");
        let race = scene.race.as_ref().expect("race should be Some");
        assert_eq!(race.racer_tag, "racer");
        assert!((race.announcement_hold_secs - 2.0).abs() < 1e-6);
        assert!(race.checkpoints.is_empty());
    }

    #[test]
    fn test_parse_race_config_absent_gives_none() {
        // Scenes without a race: section should have race = None.
        let scene = parse_scene(minimal_yaml()).expect("should parse");
        assert!(scene.race.is_none());
    }

    #[test]
    fn test_race_finish_y_must_not_be_negative() {
        let yaml = minimal_yaml_with(
            r#"race:
  finish_y: -1.0
"#,
        );
        let err = parse_scene(&yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, message }
                    if name == "race.finish_y" && message.contains("finish_y")
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_race_announcement_hold_secs_must_be_positive() {
        let yaml = minimal_yaml_with(
            r#"race:
  finish_y: 2.0
  announcement_hold_secs: 0.0
"#,
        );
        let err = parse_scene(&yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, .. }
                    if name == "race.announcement_hold_secs"
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_race_checkpoint_y_must_exceed_finish_y() {
        let yaml = minimal_yaml_with(
            r#"race:
  finish_y: 10.0
  checkpoints:
    - y: 5.0
      label: "Below finish line"
"#,
        );
        let err = parse_scene(&yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, message }
                    if name.starts_with("race.checkpoints") && message.contains("finish_y")
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_parse_end_condition_first_to_reach() {
        let yaml = minimal_yaml_with(
            r#"end_condition:
  type: first_to_reach
  finish_y: 2.0
  tag: "racer"
"#,
        );
        let scene = parse_scene(&yaml).expect("should parse first_to_reach");
        assert!(matches!(
            scene.end_condition,
            Some(EndCondition::FirstToReach { finish_y, ref tag })
                if (finish_y - 2.0).abs() < 1e-6 && tag == "racer"
        ));
    }

    #[test]
    fn test_parse_first_to_reach_default_tag() {
        // tag is optional, defaults to "racer".
        let yaml = minimal_yaml_with(
            r#"end_condition:
  type: first_to_reach
  finish_y: 0.0
"#,
        );
        let scene = parse_scene(&yaml).expect("should parse first_to_reach with default tag");
        if let Some(EndCondition::FirstToReach { tag, .. }) = &scene.end_condition {
            assert_eq!(tag, "racer");
        } else {
            panic!("expected FirstToReach end condition");
        }
    }

    #[test]
    fn test_first_to_reach_negative_finish_y_rejected() {
        let yaml = minimal_yaml_with(
            r#"end_condition:
  type: first_to_reach
  finish_y: -5.0
"#,
        );
        let err = parse_scene(&yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, message }
                    if name == "end_condition.first_to_reach" && message.contains("finish_y")
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_first_to_reach_inside_or_composite() {
        // FirstToReach can be nested inside an Or composite.
        let yaml = minimal_yaml_with(
            r#"end_condition:
  type: or
  conditions:
    - type: first_to_reach
      finish_y: 2.0
    - type: time_limit
      seconds: 120.0
"#,
        );
        let scene = parse_scene(&yaml).expect("should parse Or with FirstToReach");
        if let Some(EndCondition::Or { conditions }) = &scene.end_condition {
            assert_eq!(conditions.len(), 2);
            assert!(matches!(
                conditions[0],
                EndCondition::FirstToReach { finish_y, ref tag }
                    if (finish_y - 2.0).abs() < 1e-6 && tag == "racer"
            ));
            assert!(matches!(conditions[1], EndCondition::TimeLimit { .. }));
        } else {
            panic!("expected Or end condition");
        }
    }

    #[test]
    fn test_checkpoint_label_is_optional() {
        let yaml = minimal_yaml_with(
            r#"race:
  finish_y: 1.0
  checkpoints:
    - y: 20.0
    - y: 10.0
      label: "Halfway"
"#,
        );
        let scene = parse_scene(&yaml).expect("should parse checkpoints");
        let checkpoints = &scene.race.as_ref().unwrap().checkpoints;
        assert_eq!(checkpoints.len(), 2);
        assert!(checkpoints[0].label.is_none());
        assert_eq!(checkpoints[1].label.as_deref(), Some("Halfway"));
    }

    #[test]
    fn test_race_config_finish_y_at_zero_is_valid() {
        // finish_y = 0.0 is exactly at the boundary and should be accepted.
        let yaml = minimal_yaml_with(
            r#"race:
  finish_y: 0.0
"#,
        );
        let scene = parse_scene(&yaml).expect("finish_y=0.0 should be valid");
        assert!((scene.race.unwrap().finish_y).abs() < 1e-6);
    }

    #[test]
    fn test_first_to_reach_finish_y_at_zero_is_valid() {
        let yaml = minimal_yaml_with(
            r#"end_condition:
  type: first_to_reach
  finish_y: 0.0
"#,
        );
        let scene = parse_scene(&yaml).expect("first_to_reach finish_y=0.0 should be valid");
        assert!(matches!(
            scene.end_condition,
            Some(EndCondition::FirstToReach { finish_y, .. }) if finish_y.abs() < 1e-6
        ));
    }

    #[test]
    fn test_race_and_first_to_reach_together() {
        // Real-world usage: race config + first_to_reach end condition.
        let yaml = minimal_yaml_with(
            r#"race:
  finish_y: 2.0
  racer_tag: "racer"
  checkpoints:
    - y: 28.0
      label: "Checkpoint 1"
end_condition:
  type: first_to_reach
  finish_y: 2.0
  tag: "racer"
"#,
        );
        let scene = parse_scene(&yaml).expect("should parse race + first_to_reach");
        assert!(scene.race.is_some());
        assert!(matches!(
            scene.end_condition,
            Some(EndCondition::FirstToReach { .. })
        ));
    }

    #[test]
    fn test_destructible_min_impact_force_must_be_positive() {
        let yaml = r##"
version: "1"
meta:
  name: "Bad Destructible"
environment:
  gravity: [0.0, -9.81]
  background_color: "#000000"
  world_bounds:
    width: 20.0
    height: 35.0
  walls:
    visible: true
    color: "#ffffff"
    thickness: 0.3
objects:
  - name: "bad"
    shape: circle
    radius: 1.0
    position: [10.0, 10.0]
    destructible:
      min_impact_force: -5.0
"##;
        let err = parse_scene(yaml).unwrap_err();
        if let ParseError::Validation(errs) = err {
            assert!(errs.iter().any(|e| matches!(
                e,
                ValidationError::InvalidValue { name, message }
                    if name == "bad" && message.contains("min_impact_force")
            )));
        } else {
            panic!("expected Validation error, got {err:?}");
        }
    }

    #[test]
    fn test_parse_full_bouncing_ball_example() {
        // Full example from scene-schema.md
        let yaml = r##"
version: "1"

meta:
  name: "Bouncing Ball"
  description: "A single ball bouncing in a box"
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

end_condition:
  type: time_limit
  seconds: 20.0

audio:
  master_volume: 0.8
"##;
        let scene = parse_scene(yaml).expect("should parse bouncing ball example");
        assert_eq!(scene.meta.name, "Bouncing Ball");
        assert_eq!(scene.objects.len(), 1);
        assert_eq!(scene.objects[0].name.as_deref(), Some("ball"));
        assert!((scene.audio.master_volume - 0.8).abs() < 1e-6);
    }
}
