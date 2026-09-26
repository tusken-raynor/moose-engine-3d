//! The Moose test app: fly through a level.
//!
//! cargo run --release -p moose-app -- [options]
//!
//! Options:
//!   --level NAME          level in assets/levels (default shiny_rooms.mmp)
//!   --size WxH            framebuffer size (default 1280x720)
//!   --filter NAME         texture filtering: trilinear (default, mipmapped), anisotropic
//!                         (2x, mipmapped), bilinear or nearest
//!   --fps N               frame rate cap (default 60; 0 starts uncapped, Tab toggles)
//!   --screenshot FILE     render one frame from the spawn point to a PNG and exit,
//!                         without opening a window
//!   --at X,Y,Z,YAW,PITCH  camera for --screenshot (degrees)
//!   --bounces N           how many reflections deep mirrors go (default 1)
//!   --f0 X                reflectance of shiny surfaces seen head-on, 0-1 (default 0.15)
//!   --fade M              how far past a textured shiny surface its reflection fades out,
//!                         in meters (default 5; 0 for no fade)
//!   --floor-texture NAME  texture for shiny surfaces, in assets/textures (default
//!                         metal_tile.png, falling back to test_floor.png); used when the
//!                         level has uvs
//!
//! Controls: WASD move, mouse/trackpad or arrows look, Q/E roll, Space/C up/down, Shift faster,
//! R back to spawn, [ ] minimum sample interval, F floor reflectance (F0), G reflection fade
//! range, L texture filtering (nearest, bilinear, trilinear, anisotropic), B reflection bounces (0-4), T
//! translucent crates, P per-pixel crates, Tab frame cap on/off, F12 screenshot, Esc quit.

use std::path::Path;
use std::time::Instant;

use glam::Vec3;
use moose_assets::{Assets, Texture, TextureId};
use moose_present::{Display, Key};
use moose_raster::shaders::{
    CubeReflection, Textured, TexturedAnisotropic, TexturedBilinear, TexturedFresnel,
    TexturedFresnelAnisotropic, TexturedFresnelBilinear, TexturedFresnelNearest, TexturedNearest,
    VertexColor, VertexColorFresnel, VertexColorTranslucent,
};
use moose_raster::{RasterConfig, RasterPath, Renderer, ShaderId, Surface, Target};
use moose_scene::{Camera, Viewport, World};
use moose_view::{PolygonSource, ViewGeometry};

const EYE_HEIGHT: f32 = 1.7;
const MOVE_SPEED: f32 = 3.0; // m/s
const FAST: f32 = 3.0;
const TURN_SPEED: f32 = 2.0; // rad/s
const ROLL_SPEED: f32 = 1.5;
/// Mouse look, as in v1: radians per pixel the pointer moves (pi * 1.5 / 1000).
const MOUSE_TURN: f32 = std::f32::consts::PI * 1.5 / 1000.0;
const RADIUS: f32 = 0.25; // how close the camera may get to a wall
/// Texture filtering choices, in the order L cycles through them.
const FILTERS: [&str; 4] = ["nearest", "bilinear", "trilinear", "anisotropic"];
/// Default frame rate cap (`--fps`).
const MAX_FPS: u32 = 60;
/// Reflectance of shiny surfaces seen head-on (Schlick's F0), cycled with F. 0.04 is about
/// glass or polished stone; 1 is a perfect mirror.
const REFLECTANCE: [f32; 4] = [0.04, 0.15, 0.4, 1.0];
/// How far past a textured shiny surface its reflection fades out, in meters (0 = no fade),
/// cycled with G.
const FADE_RANGE: [f32; 5] = [0.0, 1.0, 2.5, 5.0, 10.0];
/// Texture for shiny surfaces (in levels with uvs), in assets/textures.
const DEFAULT_FLOOR_TEXTURE: &str = "metal_tile.png";
/// Entities with this model are mirror balls: each gets a static cube map of its
/// surroundings, baked at load.
const BALL_MODEL: &str = "ball.obj";
/// Mirror ball cube map faces are this many texels across.
const CUBE_SIZE: u32 = 128;

struct Options {
    level: String,
    width: u32,
    height: u32,
    screenshot: Option<String>,
    at: Option<[f32; 5]>,
    bounces: u8,
    f0: f32,
    fade: f32,
    fps: u32,
    /// Index into `FILTERS`.
    filter: usize,
    floor_texture: String,
}

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        level: "shiny_rooms.mmp".into(),
        width: 1280,
        height: 720,
        screenshot: None,
        at: None,
        bounces: 2,
        f0: 0.15,
        fade: 5.0,
        fps: MAX_FPS,
        filter: 2,
        floor_texture: DEFAULT_FLOOR_TEXTURE.into(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--level" => o.level = value()?,
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or("--size is WxH, like 1280x720")?;
                o.width = w.parse().map_err(|_| "bad width")?;
                o.height = h.parse().map_err(|_| "bad height")?;
            }
            "--screenshot" => o.screenshot = Some(value()?),
            "--at" => {
                let v: Vec<f32> = value()?
                    .split(',')
                    .map(|n| n.trim().parse::<f32>())
                    .collect::<Result<_, _>>()
                    .map_err(|_| "bad --at")?;
                o.at = Some(v.try_into().map_err(|_| "--at is X,Y,Z,YAW,PITCH")?);
            }
            "--bounces" => o.bounces = value()?.parse().map_err(|_| "bad --bounces")?,
            "--f0" => o.f0 = value()?.parse().map_err(|_| "bad --f0")?,
            "--fade" => o.fade = value()?.parse().map_err(|_| "bad --fade")?,
            "--fps" => o.fps = value()?.parse().map_err(|_| "bad --fps")?,
            "--filter" => {
                let name = value()?;
                o.filter = FILTERS
                    .iter()
                    .position(|&f| f == name)
                    .ok_or(format!("--filter is one of {}", FILTERS.join(", ")))?;
            }
            "--floor-texture" => o.floor_texture = value()?,
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(o)
}

/// Runtime settings toggled from the keyboard.
struct Settings {
    translucent_crates: bool,
    per_pixel_crates: bool,
    /// F0 of shiny surfaces; F steps through `REFLECTANCE`.
    reflectance: f32,
    /// Fade range of textured shiny surfaces' reflections; G steps through `FADE_RANGE`.
    fade_range: f32,
    /// Texture filtering, an index into `FILTERS`; L cycles.
    filter: usize,
}

/// A mirror ball's baked surroundings.
#[derive(Clone, Copy)]
struct CubeMap {
    texture: TextureId,
    /// The ball's radius, for picking the cube map's level of detail.
    radius: f32,
}

struct App {
    assets: Assets,
    world: World,
    camera: Camera,
    geometry: ViewGeometry,
    renderer: Renderer,
    opaque: ShaderId,
    translucent: ShaderId,
    fresnel: ShaderId,
    /// The textured shaders, one per filter in `FILTERS`.
    textured: [ShaderId; 4],
    textured_fresnel: [ShaderId; 4],
    /// The shiny surfaces' texture, if the level has uvs to map it with.
    floor_texture: Option<TextureId>,
    cube_reflection: ShaderId,
    /// Per entity, its cube map if it is a mirror ball.
    cube_maps: Vec<Option<CubeMap>>,
    settings: Settings,
    pixels: Vec<u32>,
    width: u32,
    height: u32,
}

impl App {
    fn new(options: &Options) -> Result<App, String> {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        let mut assets = Assets::new(root);
        let level = assets
            .load_level(&options.level)
            .map_err(|e| e.to_string())?;
        let world = World::new(level, &assets);
        let viewport = Viewport {
            x: 0,
            y: 0,
            width: options.width,
            height: options.height,
        };
        let spawn = world
            .spawn_points
            .first()
            .ok_or("the level has no spawn point")?;
        let mut camera = Camera::at_spawn(spawn, viewport);
        camera.move_to(&world, camera.position + Vec3::Y * EYE_HEIGHT);
        let mut renderer = Renderer::new(RasterConfig::default());
        let opaque = renderer.register_shader::<VertexColor>();
        let translucent = renderer.register_shader::<VertexColorTranslucent>();
        let fresnel = renderer.register_shader::<VertexColorFresnel>();
        let textured = [
            renderer.register_shader::<TexturedNearest>(),
            renderer.register_shader::<TexturedBilinear>(),
            renderer.register_shader::<Textured>(),
            renderer.register_shader::<TexturedAnisotropic>(),
        ];
        let textured_fresnel = [
            renderer.register_shader::<TexturedFresnelNearest>(),
            renderer.register_shader::<TexturedFresnelBilinear>(),
            renderer.register_shader::<TexturedFresnel>(),
            renderer.register_shader::<TexturedFresnelAnisotropic>(),
        ];
        let cube_reflection = renderer.register_shader::<CubeReflection>();
        let has_uvs = assets
            .mesh(world.geometry)
            .attribs
            .iter()
            .any(|a| a.name == "uv");
        let floor_texture = if has_uvs {
            let fallback = "test_floor.png";
            let texture = assets.load_texture(&options.floor_texture).or_else(|e| {
                if options.floor_texture == DEFAULT_FLOOR_TEXTURE {
                    eprintln!("{e}; using {fallback}");
                    assets.load_texture(fallback)
                } else {
                    Err(e)
                }
            });
            Some(texture.map_err(|e| e.to_string())?)
        } else {
            None
        };
        let cube_maps = vec![None; world.entities.len()];
        let mut app = App {
            assets,
            world,
            camera,
            geometry: {
                let mut g = ViewGeometry::new();
                g.config.max_reflections = options.bounces;
                g
            },
            renderer,
            opaque,
            translucent,
            fresnel,
            textured,
            textured_fresnel,
            floor_texture,
            cube_reflection,
            cube_maps,
            settings: Settings {
                translucent_crates: false,
                per_pixel_crates: false,
                reflectance: options.f0.clamp(0.0, 1.0),
                fade_range: options.fade.max(0.0),
                filter: options.filter,
            },
            pixels: vec![0; (options.width * options.height) as usize],
            width: options.width,
            height: options.height,
        };
        app.bake_cube_maps()?;
        Ok(app)
    }

    /// Bakes a cube map for each mirror ball: the level rendered six times from the ball's
    /// center, a 90 degree square view down each axis. The ball itself is not in them: from
    /// inside, all of its faces face away.
    fn bake_cube_maps(&mut self) -> Result<(), String> {
        let Some(ball) = self.assets.mesh_id(BALL_MODEL) else {
            return Ok(());
        };
        for i in 0..self.world.entities.len() {
            let e = &self.world.entities[i];
            if e.mesh != ball {
                continue;
            }
            let (name, position, sector) = (e.name.clone(), e.position, e.sector);
            let radius = (e.bounds.max.x - e.bounds.min.x) / 2.0;
            let t0 = Instant::now();
            let mut faces: [Vec<u32>; 6] = Default::default();
            for (face, pixels) in faces.iter_mut().enumerate() {
                let camera = Camera::cube_face(position, sector, face, CUBE_SIZE);
                *pixels = vec![0; (CUBE_SIZE * CUBE_SIZE) as usize];
                self.draw(&camera, pixels, CUBE_SIZE, CUBE_SIZE)?;
            }
            let texture = Texture::cube(&name, CUBE_SIZE, faces)?;
            self.cube_maps[i] = Some(CubeMap {
                texture: self.assets.add_texture(texture),
                radius,
            });
            println!(
                "baked a {CUBE_SIZE}x{CUBE_SIZE} cube map for {name} in {:.1} ms",
                t0.elapsed().as_secs_f64() * 1000.0
            );
        }
        Ok(())
    }

    fn reset(&mut self) {
        let spawn = &self.world.spawn_points[0];
        self.camera = Camera::at_spawn(spawn, self.camera.viewport);
        self.camera
            .move_to(&self.world, self.camera.position + Vec3::Y * EYE_HEIGHT);
    }

    /// Renders one frame into `pixels`, returning (view ms, raster ms).
    fn render(&mut self) -> Result<(f64, f64), String> {
        let mut pixels = std::mem::take(&mut self.pixels);
        let camera = self.camera.clone();
        let times = self.draw(&camera, &mut pixels, self.width, self.height);
        self.pixels = pixels;
        times
    }

    /// Renders what `camera` sees into `pixels` (`width` x `height`, holding the camera's
    /// viewport), returning (view ms, raster ms).
    fn draw(
        &mut self,
        camera: &Camera,
        pixels: &mut [u32],
        width: u32,
        height: u32,
    ) -> Result<(f64, f64), String> {
        let t0 = Instant::now();
        let view = camera.view();
        self.geometry.build(&self.world, &self.assets, &view);
        let t1 = Instant::now();
        let (opaque, translucent, fresnel, s) =
            (self.opaque, self.translucent, self.fresnel, &self.settings);
        let (textured, textured_fresnel, floor_texture) = (
            self.textured[s.filter],
            self.textured_fresnel[s.filter],
            self.floor_texture,
        );
        let level = self.assets.mesh(self.world.geometry);
        let (cube_reflection, cube_maps, entities) =
            (self.cube_reflection, &self.cube_maps, &self.world.entities);
        let (eye, mirrors) = (camera.position, &self.geometry.mirrors);
        let mut target = Target {
            pixels,
            width,
            height,
        };
        self.renderer
            .render(
                &mut target,
                camera.viewport,
                &self.geometry,
                &self.assets,
                |p| {
                    if let PolygonSource::World { polygon, .. } = p.source {
                        // Shiny surfaces are textured, if there is a texture to map.
                        let texture = floor_texture.filter(|_| p.flags.reflective());
                        // A shiny surface whose reflection was drawn is drawn over it. Past the
                        // bounce limit (or with its reflection not drawn), it is plain.
                        if p.reflection.is_none() {
                            return Surface {
                                texture,
                                ..Surface::new(if texture.is_some() { textured } else { opaque })
                            };
                        }
                        // Seen in a mirror, its view vectors come from that mirror's eye.
                        let eye = p.mirror.map_or(eye, |m| mirrors[m as usize].eye);
                        let n = level.polygons[polygon as usize].plane.normal;
                        let mut surface = Surface {
                            texture,
                            ..Surface::new(if texture.is_some() {
                                textured_fresnel
                            } else {
                                fresnel
                            })
                        };
                        surface.uniforms.values[..7].copy_from_slice(&[
                            eye.x,
                            eye.y,
                            eye.z,
                            n.x,
                            n.y,
                            n.z,
                            s.reflectance,
                        ]);
                        surface.uniforms.values[7] = s.fade_range;
                        return surface;
                    }
                    if let PolygonSource::Entity { entity, .. } = p.source
                        && let Some(cube) = cube_maps[entity as usize]
                    {
                        let eye = p.mirror.map_or(eye, |m| mirrors[m as usize].eye);
                        let distance = entities[entity as usize].position.distance(eye);
                        // Face texels per pixel at the middle of the ball, where the view
                        // turns least across it: a pixel spans 1 / r of the normal's angle (r
                        // the ball's radius in pixels), so 2 / r of the reflection's, and a
                        // face texel spans about (pi / 2) / size.
                        let r = view.focal * cube.radius / distance.max(1e-3);
                        let lod = (4.0 * CUBE_SIZE as f32 / (std::f32::consts::PI * r)).log2();
                        let mut surface = Surface {
                            texture: Some(cube.texture),
                            ..Surface::new(cube_reflection)
                        };
                        surface.uniforms.values[..4].copy_from_slice(&[eye.x, eye.y, eye.z, lod]);
                        return surface;
                    }
                    let mut surface = Surface::new(if s.translucent_crates {
                        translucent
                    } else {
                        opaque
                    });
                    surface.uniforms.values[0] = 0.5;
                    if s.per_pixel_crates {
                        surface.path_override = Some(RasterPath::PerPixel);
                    }
                    surface
                },
            )
            .map_err(|e| e.to_string())?;
        let t2 = Instant::now();
        Ok((
            (t1 - t0).as_secs_f64() * 1000.0,
            (t2 - t1).as_secs_f64() * 1000.0,
        ))
    }

    /// Moves the camera by `delta` (world space), one axis at a time so it slides along
    /// walls, then keeps it clear of them.
    fn fly(&mut self, delta: Vec3) {
        for axis in [Vec3::X, Vec3::Y, Vec3::Z] {
            let step = axis * delta.dot(axis);
            if step != Vec3::ZERO {
                self.camera
                    .move_to(&self.world, self.camera.position + step);
            }
        }
        let clear = self
            .world
            .keep_clear(self.camera.sector, self.camera.position, RADIUS);
        self.camera.move_to(&self.world, clear);
    }

    fn save_png(&self, path: &Path) -> Result<(), String> {
        let file = std::fs::File::create(path)
            .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let rgb: Vec<u8> = self
            .pixels
            .iter()
            .flat_map(|&c| [(c >> 16) as u8, (c >> 8) as u8, c as u8])
            .collect();
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgb).map_err(|e| e.to_string())
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = parse_args()?;
    let mut app = App::new(&options)?;

    if let Some(path) = &options.screenshot {
        if let Some([x, y, z, yaw, pitch]) = options.at {
            app.camera.position = Vec3::new(x, y, z);
            app.camera.sector = app
                .world
                .find_sector(app.camera.position)
                .ok_or("--at is outside the level")?;
            (app.camera.yaw, app.camera.pitch) = (yaw.to_radians(), pitch.to_radians());
        }
        let (view_ms, raster_ms) = app.render()?;
        app.save_png(Path::new(path))?;
        println!(
            "wrote {path} ({}x{}): view {view_ms:.3} ms, raster {raster_ms:.3} ms",
            app.width, app.height
        );
        return Ok(());
    }

    // Tab switches between the cap and uncapped; `--fps 0` starts uncapped with the default
    // cap to switch to.
    let cap = if options.fps == 0 {
        MAX_FPS
    } else {
        options.fps
    };
    let mut capped = options.fps != 0;
    let mut display = Display::open("Moose", app.width, app.height, if capped { cap } else { 0 })?;
    let mut shots = 0;
    let mut last = Instant::now();
    let (mut title_at, mut frames, mut view_sum, mut raster_sum) = (Instant::now(), 0u32, 0.0, 0.0);
    while display.is_open() && !display.key_down(Key::Escape) {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        // Look.
        let turn = TURN_SPEED * dt;
        let c = &mut app.camera;
        if display.key_down(Key::Left) {
            c.yaw += turn;
        }
        if display.key_down(Key::Right) {
            c.yaw -= turn;
        }
        if display.key_down(Key::Up) {
            c.pitch += turn;
        }
        if display.key_down(Key::Down) {
            c.pitch -= turn;
        }
        if display.key_down(Key::Q) {
            c.roll += ROLL_SPEED * dt;
        }
        if display.key_down(Key::E) {
            c.roll -= ROLL_SPEED * dt;
        }
        // Mouse look is always on (no button), as in v1.
        let (mx, my) = display.mouse_delta();
        c.yaw -= mx * MOUSE_TURN;
        c.pitch -= my * MOUSE_TURN;
        c.pitch = c.pitch.clamp(-1.55, 1.55);

        // Move: WASD along the view, Space/C straight up and down.
        let mut local = Vec3::ZERO;
        let key = |k| if display.key_down(k) { 1.0 } else { 0.0 };
        local.z -= key(Key::W) - key(Key::S);
        local.x += key(Key::D) - key(Key::A);
        let up = key(Key::Space) - key(Key::C);
        let speed = MOVE_SPEED
            * dt
            * if display.key_down(Key::LeftShift) {
                FAST
            } else {
                1.0
            };
        let delta = (app.camera.rotation() * local + Vec3::Y * up) * speed;
        if delta != Vec3::ZERO {
            app.fly(delta);
        }

        // Settings.
        if display.key_pressed(Key::R) {
            app.reset();
        }
        if display.key_pressed(Key::LeftBracket) {
            app.renderer.config.min_step = (app.renderer.config.min_step / 2).max(1);
        }
        if display.key_pressed(Key::RightBracket) {
            app.renderer.config.min_step = (app.renderer.config.min_step * 2).min(32);
        }
        if display.key_pressed(Key::F) {
            // The next preset up, wrapping around.
            let r = &mut app.settings.reflectance;
            *r = REFLECTANCE
                .into_iter()
                .find(|&f| f > *r)
                .unwrap_or(REFLECTANCE[0]);
        }
        if display.key_pressed(Key::L) {
            app.settings.filter = (app.settings.filter + 1) % FILTERS.len();
        }
        if display.key_pressed(Key::G) {
            // The next preset up, wrapping around.
            let r = &mut app.settings.fade_range;
            *r = FADE_RANGE
                .into_iter()
                .find(|&f| f > *r)
                .unwrap_or(FADE_RANGE[0]);
        }
        if display.key_pressed(Key::B) {
            let bounces = &mut app.geometry.config.max_reflections;
            *bounces = (*bounces + 1) % 5;
        }
        if display.key_pressed(Key::T) {
            app.settings.translucent_crates = !app.settings.translucent_crates;
        }
        if display.key_pressed(Key::P) {
            app.settings.per_pixel_crates = !app.settings.per_pixel_crates;
        }
        if display.key_pressed(Key::Tab) {
            capped = !capped;
            display.set_max_fps(if capped { cap } else { 0 });
        }

        let (view_ms, raster_ms) = app.render()?;
        if display.key_pressed(Key::F12) {
            shots += 1;
            let path = format!("moose-{shots}.png");
            app.save_png(Path::new(&path))?;
            println!("saved {path}");
        }
        display.present(&app.pixels)?;

        frames += 1;
        view_sum += view_ms;
        raster_sum += raster_ms;
        let elapsed = title_at.elapsed().as_secs_f64();
        if elapsed >= 0.5 {
            let p = app.camera.position;
            let sector = &app.world.sectors[app.camera.sector as usize].name;
            let cfg = &app.renderer.config;
            display.set_title(&format!(
                "Moose | {:.0} fps{} | view {:.2} ms, raster {:.2} ms | {sector} ({:.1}, {:.1}, {:.1}) | bounces {} mirrors {} F0 {} fade {} m | {} | min_step {} | crates {}{}",
                frames as f64 / elapsed,
                if capped { format!(" (cap {cap})") } else { String::new() },
                view_sum / frames as f64,
                raster_sum / frames as f64,
                p.x,
                p.y,
                p.z,
                app.geometry.config.max_reflections,
                app.geometry.mirrors.len(),
                app.settings.reflectance,
                app.settings.fade_range,
                FILTERS[app.settings.filter],
                cfg.min_step,
                if app.settings.translucent_crates { "translucent" } else { "opaque" },
                if app.settings.per_pixel_crates { ", per-pixel" } else { "" },
            ));
            (title_at, frames, view_sum, raster_sum) = (Instant::now(), 0, 0.0, 0.0);
        }
    }
    Ok(())
}
