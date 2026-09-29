//! The Moose test app: fly through a level.
//!
//! cargo run --release -p moose-app -- [options]
//!
//! Options:
//!   --level NAME          level in assets/levels (default shiny_rooms.mmp)
//!   --size WxH            framebuffer size (default 1280x720)
//!   --filter NAME         texture sampler: METHOD_mipmap_MIP, with METHOD nearest, bilinear
//!                         or dithered and MIP none, nearest, linear or dithered (default
//!                         bilinear_mipmap_linear)
//!   --fps N               frame rate cap (default 60; 0 starts uncapped, Tab toggles)
//!   --screenshot FILE     render one frame from the spawn point to a PNG and exit,
//!                         without opening a window
//!   --at X,Y,Z,YAW,PITCH  camera for --screenshot (degrees)
//!   --lock-flashlight X,Y,Z,YAW,PITCH  start with the flashlight locked where it would be
//!                         on a player standing there (U remounts it)
//!   --bounces N           how many reflections deep mirrors go (default 1)
//!   --f0 X                reflectance of shiny surfaces seen head-on, 0-1 (default 0.15)
//!   --fade M              how far past a textured shiny surface its reflection fades out,
//!                         in meters (default 5; 0 for no fade)
//!   --floor-texture NAME  texture for shiny surfaces, in assets/textures (default
//!                         metal_tile.png, falling back to test_floor.png); used when the
//!                         level has uvs
//!   --water               shiny floors start as water, not plain reflective tiles (V toggles);
//!                         water ripples like Half-Life's software renderer's
//!   --no-flashlight       start with the player's flashlight off (H toggles)
//!   --no-shadows          start with the flashlight's shadows off (Z toggles)
//!   --level-lights        start with the level's own lights on (N toggles; off by default,
//!                         leaving the flashlight and the ambient light)
//!   --time T              seconds into the water's animation, for --screenshot
//!   --show-samples        overlay where shading is sampled (sample rows red, sample
//!                         points green; O toggles)
//!   --min-step N          smallest sample spacing on steep surfaces, in pixels ([ ] halve
//!                         and double it)
//!   --light-spacing N     widest sample spacing on lit surfaces, in pixels (- = halve and
//!                         double it)
//!   --steep-limit X       how much depth may change across a cell held to the minimum
//!                         spacing before steep surfaces go finer anyway (default 0.25;
//!                         "off" never does; ; cycles 1/8, 1/4, 1/2, 1, off)
//!   --penumbra-threshold X  how much a spot light's cone may fade across a cell of sample
//!                         points where its penumbra crosses (default 0.125; 0 for no
//!                         limit; J cycles 1/4, 1/8, 1/16, off)
//!   --penumbra X          spot lights' penumbra as a multiple of their own (default 1;
//!                         9 and 0 shrink and grow it)
//!   --step-threshold X    how much depth may change across a cell before perspective asks
//!                         for finer spacing (default 1/16 = 0.0625; , . halve and double it)
//!
//! Controls: WASD move, mouse/trackpad or arrows look, Q/E roll, Space/C up/down, Shift faster,
//! R back to spawn, [ ] minimum sample interval, - = light sample spacing, ; steep surface
//! limit, , . perspective threshold, F floor reflectance (F0), G reflection fade
//! range, L texture sampler (the twelve of --filter; Shift+L back), B reflection bounces (0-4), V
//! water floors on/off, T
//! translucent crates, P per-pixel crates, O sample lattice overlay, Tab frame cap on/off, F12
//! screenshot, M mouse smoothing (off, 50, 100, 150 ms), K lights on/off, H flashlight on/off, U lock the
//! flashlight where it is (again: back on the shoulder), Z flashlight shadows on/off, N level
//! lights on/off,
//! 9 0 spot light penumbra narrower and wider, Esc quit.

use std::path::Path;
use std::time::Instant;

use glam::Vec3;
use moose_assets::{Assets, Light, MeshId, RIPPLE_SIZE, Ripples, Texture, TextureId};
use moose_present::{Display, Key};
use moose_raster::shaders::{
    CubeReflection, Textured, TexturedFresnel, TexturedTranslucent, VertexColor,
    VertexColorFresnel, VertexColorTranslucent, Water, filter,
};
use moose_raster::{
    MaterialId, Params, RasterConfig, RasterPath, Renderer, Surface, Target, register_per_filter,
};
use moose_scene::{Camera, Occluder, Viewport, World};
use moose_view::{PolygonSource, ViewGeometry};

const EYE_HEIGHT: f32 = 1.7;
const MOVE_SPEED: f32 = 3.0; // m/s
const FAST: f32 = 3.0;
const TURN_SPEED: f32 = 2.0; // rad/s
const ROLL_SPEED: f32 = 1.5;
/// Mouse look, as in v1: radians per pixel the pointer moves (pi * 1.5 / 1000).
const MOUSE_TURN: f32 = std::f32::consts::PI * 1.5 / 1000.0;
/// Mouse look smoothing, cycled with M: a window in seconds (0 for none). The pointer's
/// motion reaches each frame unevenly (the system reports it at its own rate, not the
/// frame's), so each frame's motion is turned evenly over the window after it, and a frame
/// turns by the parts of all the windows it overlaps: a moving average of the pointer's
/// speed. Steady hand movement becomes steady turning at any frame rate, all of it is
/// turned in the end, and it lags by half the window on average.
const MOUSE_SMOOTHING: [f64; 4] = [0.0, 0.05, 0.1, 0.15];
const RADIUS: f32 = 0.25; // how close the camera may get to a wall
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
/// Wall textures (in levels with uvs), in assets/textures: sector i gets the i-th, cycling,
/// so the test levels' rooms and hallway each have their own.
const WALL_TEXTURES: [&str; 3] = ["brick_wall.png", "panel_wall.png", "stone_wall.png"];
/// Walls with this texture get close-up detail noise, `DETAIL` strong, masked by its alpha
/// (none on the mortar).
const DETAIL_TEXTURE: &str = "brick_wall.png";
const DETAIL: f32 = 0.1;
/// Where sampler `f` is in `filter::ALL`.
fn sampler_index(f: u8) -> usize {
    filter::ALL.iter().position(|&g| g == f).expect("a sampler")
}

/// Penumbra thresholds J cycles through (see `RasterConfig::penumbra_threshold`; 0 is
/// off).
const PENUMBRA_THRESHOLDS: [f32; 4] = [0.25, 0.125, 0.0625, 0.0];

/// How much 9 and 0 shrink and grow spot lights' penumbra, each press.
const PENUMBRA_STEP: f32 = 1.25;

/// The player's flashlight (H toggles): a spot light mounted on their right shoulder, at
/// this offset from the eye in camera space (x right, y up, -z forward), aimed where they
/// look.
const FLASHLIGHT_OFFSET: Vec3 = Vec3::new(0.25, -0.2, 0.0);
const FLASHLIGHT_COLOR: Vec3 = Vec3::new(3.4, 3.5, 3.9);
/// How far it reaches, in meters.
const FLASHLIGHT_RANGE: f32 = 16.0;
/// Its cone's inner and outer half-angles, in degrees.
const FLASHLIGHT_CONE: (f32, f32) = (6.0, 20.0);

/// Steep surface limits `;` cycles through (see `RasterConfig::steep_limit`).
const STEEP_LIMITS: [f32; 5] = [0.125, 0.25, 0.5, 1.0, f32::INFINITY];

/// Entities with this model are crates, textured with `CRATE_TEXTURE` (in assets/textures):
/// Jedi Knight's crt4 crate, as version 1 used it.
const CRATE_MODEL: &str = "crate.obj";
const CRATE_TEXTURE: &str = "metal_crate.png";
/// Entities with this model are mirror balls: each gets a static cube map of its
/// surroundings, baked at load.
const BALL_MODEL: &str = "ball.obj";
/// Mirror ball cube map faces are this many texels across.
const CUBE_SIZE: u32 = 128;
/// Meters per texture repeat on the test levels' floors (`UV_TILE` in
/// tools/gen_test_assets.py): with the water texture's size, the size of a ripple's shift.
const WATER_TILE: f32 = 2.0;

struct Options {
    level: String,
    width: u32,
    height: u32,
    screenshot: Option<String>,
    at: Option<[f32; 5]>,
    lock_flashlight: Option<[f32; 5]>,
    bounces: u8,
    f0: f32,
    fade: f32,
    fps: u32,
    /// Index into `filter::ALL`.
    filter: usize,
    floor_texture: String,
    water: bool,
    no_flashlight: bool,
    level_lights: bool,
    no_shadows: bool,
    time: f32,
    show_samples: bool,
    /// `RasterConfig` spacing limits, if given.
    min_step: Option<u32>,
    light_spacing: Option<u32>,
    steep_limit: Option<f32>,
    step_threshold: Option<f32>,
    penumbra_threshold: Option<f32>,
    penumbra: f32,
}

/// `X,Y,Z,YAW,PITCH` for `option`.
fn pose(text: &str, option: &str) -> Result<[f32; 5], String> {
    let v: Vec<f32> = text
        .split(',')
        .map(|n| n.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("bad {option}"))?;
    v.try_into().map_err(|_| format!("{option} is X,Y,Z,YAW,PITCH"))
}

fn parse_args() -> Result<Options, String> {
    let mut o = Options {
        level: "shiny_rooms.mmp".into(),
        width: 1280,
        height: 720,
        screenshot: None,
        at: None,
        lock_flashlight: None,
        bounces: 2,
        f0: 0.15,
        fade: 5.0,
        fps: MAX_FPS,
        filter: sampler_index(filter::BILINEAR_MIPMAP_LINEAR),
        floor_texture: DEFAULT_FLOOR_TEXTURE.into(),
        water: false,
        no_flashlight: false,
        no_shadows: false,
        level_lights: false,
        time: 0.0,
        show_samples: false,
        min_step: None,
        light_spacing: None,
        steep_limit: None,
        step_threshold: None,
        penumbra_threshold: None,
        penumbra: 1.0,
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
            "--at" => o.at = Some(pose(&value()?, "--at")?),
            "--lock-flashlight" => o.lock_flashlight = Some(pose(&value()?, "--lock-flashlight")?),
            "--bounces" => o.bounces = value()?.parse().map_err(|_| "bad --bounces")?,
            "--f0" => o.f0 = value()?.parse().map_err(|_| "bad --f0")?,
            "--fade" => o.fade = value()?.parse().map_err(|_| "bad --fade")?,
            "--fps" => o.fps = value()?.parse().map_err(|_| "bad --fps")?,
            "--filter" => {
                let name = value()?;
                o.filter = filter::named(&name).map(sampler_index).ok_or_else(|| {
                    let names: Vec<_> = filter::ALL.map(filter::name).into();
                    format!("--filter is one of {}", names.join(", "))
                })?;
            }
            "--floor-texture" => o.floor_texture = value()?,
            "--water" => o.water = true,
            "--no-flashlight" => o.no_flashlight = true,
            "--no-shadows" => o.no_shadows = true,
            "--level-lights" => o.level_lights = true,
            "--show-samples" => o.show_samples = true,
            "--min-step" => o.min_step = Some(value()?.parse().map_err(|_| "bad --min-step")?),
            "--light-spacing" => {
                o.light_spacing = Some(value()?.parse().map_err(|_| "bad --light-spacing")?)
            }
            "--steep-limit" => {
                let v = value()?;
                o.steep_limit = Some(if v == "off" {
                    f32::INFINITY
                } else {
                    v.parse().map_err(|_| "bad --steep-limit")?
                })
            }
            "--penumbra" => o.penumbra = value()?.parse().map_err(|_| "bad --penumbra")?,
            "--penumbra-threshold" => {
                o.penumbra_threshold =
                    Some(value()?.parse().map_err(|_| "bad --penumbra-threshold")?)
            }
            "--step-threshold" => {
                o.step_threshold = Some(value()?.parse().map_err(|_| "bad --step-threshold")?)
            }
            "--time" => o.time = value()?.parse().map_err(|_| "bad --time")?,
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
    /// Texture sampler, an index into `filter::ALL`; L cycles (Shift+L backward).
    filter: usize,
    /// Shiny floors are water; V toggles.
    water: bool,
    /// The level's lights are on; K toggles (off, surfaces show their full color).
    lit: bool,
    /// Spot lights' penumbra (the angle over which their cone fades) as a multiple of the
    /// level's, around the middle of the fade; 9 and 0 shrink and grow it.
    penumbra: f32,
    /// The player's flashlight is on; H toggles.
    flashlight: bool,
    /// The level's own lights are on (the flashlight and ambient light aside); N toggles.
    level_lights: bool,
    /// The flashlight casts shadows; Z toggles.
    shadows: bool,
    /// Where the flashlight was left when U locked it in place (sector, position, direction);
    /// `None` while it is on the player's shoulder. U again remounts it.
    flashlight_lock: Option<(u32, Vec3, Vec3)>,
    /// Mouse look smoothing, an index into `MOUSE_SMOOTHING`; M cycles.
    smoothing: usize,
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
    /// The level's lights and ambient light, for switching them back on (K).
    lights: (Vec<Light>, Vec3),
    camera: Camera,
    geometry: ViewGeometry,
    renderer: Renderer,
    opaque: MaterialId,
    translucent: MaterialId,
    fresnel: MaterialId,
    /// The textured shaders, one per sampler in `filter::ALL`.
    textured: [MaterialId; 12],
    textured_fresnel: [MaterialId; 12],
    /// The water shader, one per sampler in `filter::ALL`.
    water: [MaterialId; 12],
    /// The water's ripples, and the floor texture rippled by them with its height map
    /// (redrawn as they move).
    ripples: Ripples,
    water_textures: Option<[TextureId; 2]>,
    /// The shiny surfaces' texture, if the level has uvs to map it with.
    floor_texture: Option<TextureId>,
    /// Per sector, its walls' texture, if the level has uvs.
    wall_textures: Vec<Option<TextureId>>,
    /// The texture whose walls get detail noise, if loaded.
    detail_texture: Option<TextureId>,
    /// The crate model and its texture, if the level has crates.
    crate_texture: Option<(MeshId, TextureId)>,
    /// The translucent textured shader (crates with T), one per sampler in `filter::ALL`.
    textured_translucent: [MaterialId; 12],
    /// The mirror ball shader, one per sampler in `filter::ALL`.
    cube_reflection: [MaterialId; 12],
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
        let defaults = RasterConfig::default();
        let mut renderer = Renderer::new(RasterConfig {
            show_samples: options.show_samples,
            min_step: options.min_step.unwrap_or(defaults.min_step),
            light_spacing: options.light_spacing.unwrap_or(defaults.light_spacing),
            steep_limit: options.steep_limit.unwrap_or(defaults.steep_limit),
            step_threshold: options.step_threshold.unwrap_or(defaults.step_threshold),
            penumbra_threshold: options
                .penumbra_threshold
                .unwrap_or(defaults.penumbra_threshold),
            ..defaults
        });
        let opaque = renderer.register_material::<VertexColor>();
        let translucent = renderer.register_material::<VertexColorTranslucent>();
        let fresnel = renderer.register_material::<VertexColorFresnel>();
        let textured = register_per_filter!(renderer, Textured);
        let textured_fresnel = register_per_filter!(renderer, TexturedFresnel);
        let water = register_per_filter!(renderer, Water);
        let textured_translucent = register_per_filter!(renderer, TexturedTranslucent);
        let cube_reflection = register_per_filter!(renderer, CubeReflection);
        let crate_texture = match assets.mesh_id(CRATE_MODEL) {
            Some(mesh) => Some((
                mesh,
                assets
                    .load_texture(CRATE_TEXTURE)
                    .map_err(|e| e.to_string())?,
            )),
            None => None,
        };
        // What casts shadows: crates as themselves (boxes), mirror balls as spheres.
        let mut world = world;
        let ball = assets.mesh_id(BALL_MODEL);
        for entity in &mut world.entities {
            if crate_texture.is_some_and(|(mesh, _)| mesh == entity.mesh) {
                entity.occluder = Occluder::Mesh;
            } else if Some(entity.mesh) == ball {
                let b = assets.mesh(entity.mesh).bounds;
                entity.occluder = Occluder::Sphere {
                    center: (b.min + b.max) * 0.5,
                    radius: (b.max.x - b.min.x) * 0.5,
                };
            }
        }
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
        let wall_textures = (0..world.sectors.len())
            .map(|i| {
                has_uvs
                    .then(|| assets.load_texture(WALL_TEXTURES[i % WALL_TEXTURES.len()]))
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        let detail_texture = assets.texture_id(DETAIL_TEXTURE);
        let ripples = Ripples::new(1);
        let water_textures = floor_texture.map(|floor| {
            let water = ripples.texture("water", assets.texture(floor).base());
            [
                assets.add_texture(water),
                assets.add_texture(ripples.heights("water heights")),
            ]
        });
        let cube_maps = vec![None; world.entities.len()];
        let mut app = App {
            assets,
            lights: (world.lights().to_vec(), world.ambient),
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
            water,
            ripples,
            water_textures,
            floor_texture,
            wall_textures,
            detail_texture,
            crate_texture,
            textured_translucent,
            cube_reflection,
            cube_maps,
            settings: Settings {
                translucent_crates: false,
                per_pixel_crates: false,
                reflectance: options.f0.clamp(0.0, 1.0),
                fade_range: options.fade.max(0.0),
                filter: options.filter,
                water: options.water,
                lit: true,
                penumbra: options.penumbra.clamp(1.0 / 64.0, 64.0),
                flashlight: !options.no_flashlight,
                level_lights: true,
                shadows: !options.no_shadows,
                flashlight_lock: None,
                smoothing: 2,
            },
            pixels: vec![0; (options.width * options.height) as usize],
            width: options.width,
            height: options.height,
        };
        // Mirror balls' cube maps see the level's lights only, not the flashlight where the
        // player happens to start.
        app.apply_lights(false);
        app.bake_cube_maps()?;
        app.settings.level_lights = options.level_lights;
        Ok(app)
    }

    /// Moves the water's animation to `time` seconds, redrawing its textures if the ripples
    /// moved.
    fn set_time(&mut self, time: f32) {
        if let (Some([water, heights]), Some(floor)) = (self.water_textures, self.floor_texture)
            && self.ripples.advance_to(time as f64)
        {
            let rippled = self
                .ripples
                .texture("water", self.assets.texture(floor).base());
            *self.assets.texture_mut(water) = rippled;
            *self.assets.texture_mut(heights) = self.ripples.heights("water heights");
        }
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

    /// Gives the world the level's lights and, if `flashlight` and it is on, the player's
    /// flashlight; or no lights (K). Spot lights' penumbra is scaled by the setting: their
    /// fade keeps its middle angle and spans `penumbra` times their own angle, within 0 to
    /// 180 degrees.
    fn apply_lights(&mut self, flashlight: bool) {
        if !self.settings.lit {
            self.world.set_lights(Vec::new(), Vec3::ONE);
            return;
        }
        let k = self.settings.penumbra;
        let scaled = |l: Light| {
            if l.is_point() {
                return l;
            }
            let (inner, outer) = (
                l.cos_inner.clamp(-1.0, 1.0).acos().to_degrees(),
                l.cos_outer.clamp(-1.0, 1.0).acos().to_degrees(),
            );
            let (middle, half) = ((inner + outer) / 2.0, (outer - inner) / 2.0 * k);
            Light::spot(
                l.sector,
                l.position,
                l.color,
                l.range,
                l.direction,
                (middle - half).clamp(0.0, 180.0),
                (middle + half).clamp(0.0, 180.0),
            )
        };
        let mut lights: Vec<Light> = match self.settings.level_lights {
            true => self.lights.0.iter().map(|&l| scaled(l)).collect(),
            false => Vec::new(),
        };
        if flashlight && self.settings.flashlight {
            let mut light = scaled(self.flashlight());
            // Shadow slot 0: the view carves its shadows into polygons.
            light.shadow = self.settings.shadows.then_some(0);
            lights.push(light);
        }
        self.world.set_lights(lights, self.lights.1);
    }

    /// The player's flashlight: where U locked it, or on their shoulder.
    fn flashlight(&self) -> Light {
        let (sector, position, direction) = self.settings.flashlight_lock.unwrap_or_else(|| self.mount());
        Light::spot(
            sector,
            position,
            FLASHLIGHT_COLOR,
            FLASHLIGHT_RANGE,
            direction,
            FLASHLIGHT_CONE.0,
            FLASHLIGHT_CONE.1,
        )
    }

    /// Puts the camera at `[x, y, z, yaw, pitch]` (degrees); `option` names it in errors.
    fn place_camera(&mut self, [x, y, z, yaw, pitch]: [f32; 5], option: &str) -> Result<(), String> {
        self.camera.position = Vec3::new(x, y, z);
        self.camera.sector = self
            .world
            .find_sector(self.camera.position)
            .ok_or(format!("{option} is outside the level"))?;
        (self.camera.yaw, self.camera.pitch) = (yaw.to_radians(), pitch.to_radians());
        Ok(())
    }

    /// Where the flashlight sits on the player's shoulder (or as far toward it as the walls
    /// let it go, so it stays in the level), and where it points: where they look. As
    /// (sector, position, direction).
    fn mount(&self) -> (u32, Vec3, Vec3) {
        let c = &self.camera;
        let mount = self
            .world
            .trace(c.sector, c.position, c.position + c.rotation() * FLASHLIGHT_OFFSET);
        (mount.sector, mount.position, c.forward())
    }

    fn reset(&mut self) {
        let spawn = &self.world.spawn_points[0];
        self.camera = Camera::at_spawn(spawn, self.camera.viewport);
        self.camera
            .move_to(&self.world, self.camera.position + Vec3::Y * EYE_HEIGHT);
    }

    /// Renders one frame into `pixels`, returning (view ms, raster ms). The lights are set
    /// first, so the flashlight follows the camera.
    fn render(&mut self) -> Result<(f64, f64), String> {
        self.apply_lights(true);
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
        let (water, water_textures) = (self.water[s.filter], self.water_textures);
        let (wall_textures, detail_texture) = (&self.wall_textures, self.detail_texture);
        let (crate_texture, textured_translucent) =
            (self.crate_texture, self.textured_translucent[s.filter]);
        let level = self.assets.mesh(self.world.geometry);
        let (cube_reflection, cube_maps) = (self.cube_reflection[s.filter], &self.cube_maps);
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
                    if let PolygonSource::World { sector, polygon } = p.source {
                        // Shiny surfaces are textured, if there is a texture to map, and so
                        // are other walls, with their sector's texture.
                        let n = level.polygons[polygon as usize].plane.normal;
                        let wall = wall_textures[sector as usize]
                            .filter(|_| !p.flags.reflective() && n.y.abs() < 0.5);
                        // Shiny textured floors are water, if it is on: the rippling texture
                        // and its height map.
                        let water_textures =
                            water_textures.filter(|_| s.water && p.flags.reflective() && n.y > 0.9);
                        let is_water = water_textures.is_some();
                        let textures = match water_textures {
                            Some([water, heights]) => [Some(water), Some(heights)],
                            None => [floor_texture.filter(|_| p.flags.reflective()).or(wall), None],
                        };
                        let texture = textures[0];
                        // A shiny surface whose reflection was drawn is drawn over it. Past the
                        // bounce limit (or with its reflection not drawn), it is plain.
                        if p.reflection.is_none() {
                            let detail = if wall.is_some() && wall == detail_texture {
                                DETAIL
                            } else {
                                0.0
                            };
                            return Surface {
                                textures,
                                params: Params::new(&[detail]),
                                ..Surface::new(if texture.is_some() { textured } else { opaque })
                            };
                        }
                        return Surface {
                            textures,
                            // Water shifts what is behind it by its texels' size (meters per
                            // repeat over texels per repeat).
                            params: Params::new(&[
                                s.reflectance,
                                s.fade_range,
                                WATER_TILE / RIPPLE_SIZE as f32,
                            ]),
                            ..Surface::new(match texture {
                                Some(_) if is_water => water,
                                Some(_) => textured_fresnel,
                                None => fresnel,
                            })
                        };
                    }
                    if let PolygonSource::Entity { entity, .. } = p.source
                        && let Some(cube) = cube_maps[entity as usize]
                    {
                        return Surface {
                            textures: [Some(cube.texture), None],
                            params: Params::new(&[cube.radius, CUBE_SIZE as f32]),
                            ..Surface::new(cube_reflection)
                        };
                    }
                    // Crates are textured; other entities keep their vertex colors.
                    let texture = crate_texture
                        .filter(|&(mesh, _)| p.mesh == mesh)
                        .map(|(_, texture)| texture);
                    let mut surface = Surface::new(match (texture, s.translucent_crates) {
                        (Some(_), true) => textured_translucent,
                        (Some(_), false) => textured,
                        (None, true) => translucent,
                        (None, false) => opaque,
                    });
                    surface.textures = [texture, None];
                    // Translucent crates' opacity; opaque ones (textured) get no detail.
                    surface.params.values[0] = if s.translucent_crates { 0.5 } else { 0.0 };
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
    if let Some(at) = options.lock_flashlight {
        // The flashlight locked where it would be on a player standing there.
        let camera = app.camera.clone();
        app.place_camera(at, "--lock-flashlight")?;
        app.settings.flashlight_lock = Some(app.mount());
        app.camera = camera;
    }

    if let Some(path) = &options.screenshot {
        if let Some(at) = options.at {
            app.place_camera(at, "--at")?;
        }
        app.set_time(options.time);
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
    let started = last;
    let (mut title_at, mut frames, mut view_sum, mut raster_sum) = (Instant::now(), 0u32, 0.0, 0.0);
    // Pointer motion still being turned (see `MOUSE_SMOOTHING`): when its window starts,
    // and the motion, in pixels. And the time of this frame's start, in seconds.
    let mut look: Vec<(f64, (f32, f32))> = Vec::new();
    let mut clock = 0.0f64;
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
        // Mouse look is always on (no button), as in v1, smoothed over time: this frame's
        // motion (made since the last) is turned evenly over the window from then on.
        let (mx, my) = display.mouse_delta();
        let window = MOUSE_SMOOTHING[app.settings.smoothing];
        let (t0, t1) = (clock, (now - started).as_secs_f64());
        clock = t1;
        let (tx, ty) = if window > 0.0 {
            if (mx, my) != (0.0, 0.0) {
                look.push((t0, (mx, my)));
            }
            let mut turn = (0.0f32, 0.0f32);
            for &(start, (x, y)) in &look {
                let share = ((t1.min(start + window) - t0.max(start)).max(0.0) / window) as f32;
                turn = (turn.0 + x * share, turn.1 + y * share);
            }
            look.retain(|&(start, _)| start + window > t1);
            turn
        } else {
            look.clear();
            (mx, my)
        };
        c.yaw -= tx * MOUSE_TURN;
        c.pitch -= ty * MOUSE_TURN;
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
            look.clear();
        }
        if display.key_pressed(Key::LeftBracket) {
            app.renderer.config.min_step = (app.renderer.config.min_step / 2).max(1);
        }
        if display.key_pressed(Key::RightBracket) {
            app.renderer.config.min_step = (app.renderer.config.min_step * 2).min(32);
        }
        if display.key_pressed(Key::J) {
            // The next penumbra threshold down, wrapping around.
            let t = &mut app.renderer.config.penumbra_threshold;
            *t = PENUMBRA_THRESHOLDS
                .into_iter()
                .find(|&x| x < *t)
                .unwrap_or(PENUMBRA_THRESHOLDS[0]);
        }
        if display.key_pressed(Key::Semicolon) {
            // The next limit up, wrapping around.
            let limit = &mut app.renderer.config.steep_limit;
            *limit = STEEP_LIMITS
                .into_iter()
                .find(|&l| l > *limit)
                .unwrap_or(STEEP_LIMITS[0]);
        }
        if display.key_pressed(Key::Comma) {
            let t = &mut app.renderer.config.step_threshold;
            *t = (*t / 2.0).max(1.0 / 256.0);
        }
        if display.key_pressed(Key::Period) {
            let t = &mut app.renderer.config.step_threshold;
            *t = (*t * 2.0).min(1.0);
        }
        if display.key_pressed(Key::Minus) {
            let s = &mut app.renderer.config.light_spacing;
            *s = (*s / 2).max(1);
        }
        if display.key_pressed(Key::Equal) {
            let s = &mut app.renderer.config.light_spacing;
            *s = (*s * 2).min(32);
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
            // The next sampler, or with Shift the one before, wrapping around.
            let n = filter::ALL.len();
            let back = display.key_down(Key::LeftShift) || display.key_down(Key::RightShift);
            let f = &mut app.settings.filter;
            *f = if back { (*f + n - 1) % n } else { (*f + 1) % n };
        }
        if display.key_pressed(Key::K) {
            app.settings.lit = !app.settings.lit;
        }
        if display.key_pressed(Key::Key9) {
            app.settings.penumbra = (app.settings.penumbra / PENUMBRA_STEP).max(1.0 / 64.0);
        }
        if display.key_pressed(Key::Key0) {
            app.settings.penumbra = (app.settings.penumbra * PENUMBRA_STEP).min(64.0);
        }
        if display.key_pressed(Key::H) {
            app.settings.flashlight = !app.settings.flashlight;
        }
        if display.key_pressed(Key::U) {
            app.settings.flashlight_lock = match app.settings.flashlight_lock {
                Some(_) => None,
                None => Some(app.mount()),
            };
        }
        if display.key_pressed(Key::Z) {
            app.settings.shadows = !app.settings.shadows;
        }
        if display.key_pressed(Key::N) {
            app.settings.level_lights = !app.settings.level_lights;
        }
        if display.key_pressed(Key::M) {
            app.settings.smoothing = (app.settings.smoothing + 1) % MOUSE_SMOOTHING.len();
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
        if display.key_pressed(Key::V) {
            app.settings.water = !app.settings.water;
        }
        if display.key_pressed(Key::T) {
            app.settings.translucent_crates = !app.settings.translucent_crates;
        }
        if display.key_pressed(Key::P) {
            app.settings.per_pixel_crates = !app.settings.per_pixel_crates;
        }
        if display.key_pressed(Key::O) {
            let config = &mut app.renderer.config;
            config.show_samples = !config.show_samples;
        }
        if display.key_pressed(Key::Tab) {
            capped = !capped;
            display.set_max_fps(if capped { cap } else { 0 });
        }

        app.set_time(started.elapsed().as_secs_f32());
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
                "Moose | {:.0} fps{} | view {:.2} ms, raster {:.2} ms | {sector} ({:.1}, {:.1}, {:.1}) | bounces {} mirrors {} F0 {} fade {} m | {}{} | min_step {} light {} steep {} threshold 1/{} | crates {}{} | smoothing {} ms | {} level lights {}{}, flashlight {}{}{} | penumbra x{:.2}, fade/cell {}",
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
                filter::name(filter::ALL[app.settings.filter]),
                if app.settings.water { " | water" } else { "" },
                cfg.min_step,
                cfg.light_spacing,
                if cfg.steep_limit.is_finite() {
                    format!("{}", cfg.steep_limit)
                } else {
                    "off".to_string()
                },
                (1.0 / cfg.step_threshold).round(),
                if app.settings.translucent_crates { "translucent" } else { "opaque" },
                if app.settings.per_pixel_crates { ", per-pixel" } else { "" },
                MOUSE_SMOOTHING[app.settings.smoothing] * 1000.0,
                app.lights.0.len(),
                if app.settings.level_lights { "on" } else { "off" },
                if app.settings.lit { "" } else { " (all off)" },
                if app.settings.flashlight { "on" } else { "off" },
                if app.settings.flashlight_lock.is_some() { " (locked)" } else { "" },
                if app.settings.flashlight && app.settings.shadows { ", shadows" } else { "" },
                app.settings.penumbra,
                cfg.penumbra_threshold,
            ));
            (title_at, frames, view_sum, raster_sum) = (Instant::now(), 0, 0.0, 0.0);
        }
    }
    Ok(())
}
