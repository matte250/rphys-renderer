/// Quick demo: load a scene, step physics once, render a frame, save as PNG.
use rphys_physics::{PhysicsConfig, PhysicsEngine};
use rphys_renderer::{RenderContext, Renderer, TinySkiaRenderer};
use rphys_scene::{parse_scene, Color, Vec2};
use std::path::Path;

const SCENE: &str = r##"
version: "1"
meta:
  name: "Demo Scene"
  duration_hint: 3.0

environment:
  gravity: [0.0, -9.81]
  world_bounds:
    width: 20.0
    height: 20.0
  walls:
    visible: false
  background_color: "#1a1a2e"

objects:
  - name: ball
    shape: circle
    radius: 0.8
    position: [10.0, 14.0]
    body_type: dynamic
    color: "#e94560"
    material:
      restitution: 0.85
      friction: 0.2
      density: 1.0

  - name: ball2
    shape: circle
    radius: 0.5
    position: [8.0, 12.0]
    body_type: dynamic
    color: "#53d8fb"
    material:
      restitution: 0.7
      friction: 0.3
      density: 1.5

  - name: platform
    shape: rectangle
    size: [8.0, 0.4]
    position: [10.0, 8.0]
    rotation: -10.0
    body_type: static
    color: "#533483"

  - name: ground
    shape: rectangle
    size: [20.0, 0.5]
    position: [10.0, 0.25]
    body_type: static
    color: "#16213e"

  - name: left_wall
    shape: rectangle
    size: [0.3, 20.0]
    position: [0.15, 10.0]
    body_type: static
    color: "#16213e"

  - name: right_wall
    shape: rectangle
    size: [0.3, 20.0]
    position: [19.85, 10.0]
    body_type: static
    color: "#16213e"

end_condition:
  type: time_limit
  seconds: 3.0
"##;

fn main() {
    let scene = parse_scene(SCENE).expect("scene parse failed");

    // Use unlimited steps per call so advance_to() actually reaches the target time.
    // (Default max_steps_per_call=8 is designed for real-time preview, not offline use.)
    let config = PhysicsConfig { max_steps_per_call: u32::MAX, ..PhysicsConfig::default() };
    let mut engine = PhysicsEngine::new(&scene, config).expect("physics init failed");

    // Step to t=0.6s so the balls are mid-fall
    engine.advance_to(0.6).expect("physics step failed");
    let state = engine.state();

    let ctx = RenderContext {
        width: 800,
        height: 800,
        camera_origin: Vec2 { x: 0.0, y: 0.0 },
        scale: 40.0,
        background_color: Color { r: 26, g: 26, b: 46, a: 255 },
    };

    let renderer = TinySkiaRenderer;
    let frame = renderer.render(&state, &ctx);

    // Write RGBA → PNG (png crate is a transitive dep via tiny-skia)
    let out_path = Path::new("/tmp/rphys-demo.png");
    let file = std::fs::File::create(out_path).expect("create output file");
    let mut encoder = png::Encoder::new(file, frame.width, frame.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("write PNG header");
    writer.write_image_data(&frame.pixels).expect("write PNG data");

    println!("✓ Rendered frame → {}", out_path.display());
    println!("  Bodies: {}", state.bodies.len());
    for b in &state.bodies {
        println!(
            "    {:?}  pos=({:.2}, {:.2})  angle={:.1}°",
            b.shape,
            b.position.x,
            b.position.y,
            b.rotation.to_degrees()
        );
    }
}
