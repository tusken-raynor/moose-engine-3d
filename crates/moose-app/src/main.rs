//! The Moose test app: fly through a level.
//!
//! cargo run --release -p moose-app -- [options]
//!
//! Options (all but the screenshot ones can also be changed in the options menu, Esc):
//!   --level NAME          level in assets/levels (default shiny_rooms.mmp)
//!   --size WxH            framebuffer size (default 1280x720)
//!   --filter NAME         texture sampler: METHOD_mipmap_MIP, with METHOD nearest, bilinear
//!                         or dithered and MIP none, nearest, linear or dithered (default
//!                         bilinear_mipmap_linear)
//!   --fps N               frame rate cap (default 60; 0 starts uncapped)
//!   --screenshot FILE     render one frame from the spawn point to a PNG and exit,
//!                         without opening a window
//!   --at X,Y,Z,YAW,PITCH[,ROLL]  camera for --screenshot (degrees)
//!   --edit                start in the level editor (Tab switches)
//!   --view V, --wire, --zoom S  the editor's view (3d, top, front or side; F6), wireframe
//!                         over the 3D view (F5), and 2D views' pixels per meter
//!   --select NAME         in the editor, start with the entity NAME (or surface:N,
//!                         sector:N, vertex:N, light:N) selected
//!   --edit-model          in the editor, open the mesh editor on the selected entity's
//!                         model; --polygon N also selects its polygon N
//!   --lock-flashlight X,Y,Z,YAW,PITCH  start with the flashlight locked where it would be
//!                         on a player standing there
//!   --flashlight-at X,Y,Z,DX,DY,DZ  start with the flashlight locked at X,Y,Z, aimed along
//!                         DX,DY,DZ
//!   --translucent-crates, --per-pixel-crates, --unlit  start with translucent or
//!                         per-pixel crates, or with lighting off
//!   --bounces N           how many reflections deep mirrors go (default 1)
//!   --f0 X                reflectance of shiny surfaces seen head-on, 0-1 (default 0.15)
//!   --fade M              how far past a textured shiny surface its reflection fades out,
//!                         in meters (default 5; 0 for no fade)
//!   --floor-texture NAME  texture for shiny surfaces, in assets/textures (default
//!                         metal_tile.png, falling back to test_floor.png); used when the
//!                         level has uvs
//!   --water               shiny floors start as water, not plain reflective tiles;
//!                         water ripples like Half-Life's software renderer's
//!   --no-flashlight       start with the player's flashlight off
//!   --fullscreen          start fullscreen (Alt+Enter switches)
//!   --cone MODE           how the flashlight's cone is drawn: beam (faded pixel by pixel
//!                         with its shadow; the default), sampled (lit at sample points)
//!                         or soft (lit at sample points as they are, fading over all 20
//!                         degrees: the cheapest)
//!   --no-shadows          start with shadows off
//!   --no-sun              start with the level's directional lights off
//!   --light-scale K       the level's point and spot lights' source sizes (their shadows'
//!                         softness) times K (default 1)
//!   --no-shadow-cache     carve static lights' shadows every frame (to compare)
//!   --no-dynamic-shadows  only baked shadows: none from the flashlight or moving lights,
//!                         moving occluders, or on moving surfaces (no carving per frame)
//!   --hud, --menu PAGE    for --screenshot: draw the debug HUD, or a menu page (main,
//!                         lighting, flashlight, rendering, sampling, controls), over it
//!   --sun-angle A         directional lights' source size in degrees, instead of the
//!                         level's
//!   --light-radius R      radius of the flashlight's source in meters: its shadows soften
//!                         over the part of it an occluder covers (default 0.05; 0 is hard)
//!   --flashlight-fade D   how wide the flashlight's cone fades, in degrees, inside its 20
//!                         degree edge (default 8; 0 is a hard edge)
//!   --dither-beam         draw the flashlight beam's fade as a stipple (an ordered dither)
//!   --level-lights        start with the level's own lights on (off by default, leaving the
//!                         flashlight and the ambient light)
//!   --time T              seconds into the water's animation, for --screenshot
//!   --show-samples        overlay where shading is sampled (sample rows red, sample
//!                         points green)
//!   --min-step N          smallest sample spacing on steep surfaces, in pixels
//!   --light-spacing N     widest sample spacing on lit surfaces, in pixels
//!   --steep-limit X       how much depth may change across a cell held to the minimum
//!                         spacing before steep surfaces go finer anyway (default 0.25;
//!                         "off" never does)
//!   --penumbra-threshold X  how much a spot light's cone may fade across a cell of sample
//!                         points where its penumbra crosses (default 0.125; 0 for no
//!                         limit)
//!   --penumbra X          spot lights' penumbra as a multiple of their own (default 1)
//!   --step-threshold X    how much depth may change across a cell before perspective asks
//!                         for finer spacing (default 1/16 = 0.0625)
//!
//! Controls: WASD move, mouse/trackpad or arrows look, Q/E roll, Space/C up/down, Shift
//! faster, U lock the flashlight in place (again: back on the shoulder), Esc options menu
//! (arrows choose and change, Enter picks, Backspace goes back), F1 print a command line that
//! reproduces this view, F3 debug HUD, Alt+Enter fullscreen, F12 screenshot. Every other setting
//! is in the menu. While playing, the cursor is locked (hidden) for mouse look; in the menu
//! it is free.

mod editor;
mod mesh_edit;
mod ui;
mod wire;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::Vec3;
use moose_assets::{Assets, LevelDoc, Light, MeshId, ModelDoc, RIPPLE_SIZE, Ripples, Texture, TextureId};
use moose_present::{Display, Key, MouseButton};
use moose_raster::shaders::{
    CubeReflection, Textured, TexturedFresnel, TexturedTranslucent, UnlitColor, VertexColor,
    VertexColorFresnel, VertexColorTranslucent, Water, filter,
};
use moose_raster::{
    MaterialId, Params, RasterConfig, RasterPath, Renderer, Surface, Target, register_per_filter,
};
use moose_scene::{Camera, Viewport, World};
use moose_view::{MAX_SHADOW_SLOTS, PolygonSource, ViewGeometry};

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

/// Penumbra thresholds the menu steps through (see `RasterConfig::penumbra_threshold`; 0 is
/// off).
const PENUMBRA_THRESHOLDS: [f32; 4] = [0.25, 0.125, 0.0625, 0.0];

/// How much the menu shrinks and grows spot lights' penumbra, each step.
const PENUMBRA_STEP: f32 = 1.25;

/// The player's flashlight: a spot light mounted on their right shoulder, at
/// this offset from the eye in camera space (x right, y up, -z forward), aimed where they
/// look.
const FLASHLIGHT_OFFSET: Vec3 = Vec3::new(0.25, -0.2, 0.0);
const FLASHLIGHT_COLOR: Vec3 = Vec3::new(3.4, 3.5, 3.9);
/// How far it reaches, in meters.
const FLASHLIGHT_RANGE: f32 = 16.0;
/// Its cone's outer half-angle, in degrees.
const FLASHLIGHT_OUTER: f32 = 20.0;
/// How the flashlight's cone is drawn: a beam (its cone drawn pixel by pixel with its
/// shadow; see `Light::beam`), lit at sample points, or lit at them as they are and fading
/// over all of it (see `Light::coarse`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cone {
    Beam,
    Sampled,
    Soft,
}

/// How wide its cone fades inside that, in degrees, as the menu steps through them
/// (`--flashlight-fade`): the inner half-angle is the outer less this. 0 is a hard edge.
const FLASHLIGHT_FADES: [f32; 6] = [0.0, 2.0, 4.0, 8.0, 14.0, 20.0];
/// Multiples of the level's point and spot lights' source sizes (their `radius=`) the
/// menu steps through: 0 casts hard shadows, 1 is as authored.
const LIGHT_SCALES: [f32; 5] = [0.0, 0.5, 1.0, 2.0, 4.0];
/// Angular sizes of directional lights' sources the menu steps through, in degrees (the sun is
/// about 0.53): their shadows soften over the part of it an occluder covers.
const SUN_ANGLES: [f32; 5] = [0.0, 0.53, 2.0, 5.0, 10.0];
/// Sizes of its source the menu steps through (radius in meters, `--light-radius`): its shadows
/// soften over the part of it an occluder covers; 0 casts hard shadows.
const FLASHLIGHT_RADII: [f32; 5] = [0.0, 0.02, 0.05, 0.1, 0.2];

/// Steep surface limits the menu steps through (see `RasterConfig::steep_limit`).
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

#[derive(Clone)]
struct Options {
    level: String,
    width: u32,
    height: u32,
    screenshot: Option<String>,
    /// Start in the editor, with an entity (by name) or a surface (`surface:N`) selected.
    edit: bool,
    select: Option<String>,
    /// Open the mesh editor on the selected entity's model, with polygon N selected.
    edit_model: Option<Option<usize>>,
    /// The editor's view, its wireframe over the 3D view, and its 2D views' scale.
    view: editor::ViewMode,
    wire: bool,
    zoom: Option<f32>,
    at: Option<[f32; 6]>,
    lock_flashlight: Option<[f32; 6]>,
    /// The flashlight locked at this position, aimed this way.
    flashlight_at: Option<[f32; 6]>,
    translucent_crates: bool,
    per_pixel_crates: bool,
    unlit: bool,
    bounces: u8,
    f0: f32,
    fade: f32,
    fps: u32,
    /// Index into `filter::ALL`.
    filter: usize,
    floor_texture: String,
    water: bool,
    no_flashlight: bool,
    fullscreen: bool,
    cone: Cone,
    level_lights: bool,
    no_shadows: bool,
    no_sun: bool,
    light_scale: f32,
    /// For screenshots: draw the debug HUD, or a menu page, over the frame.
    hud: bool,
    menu: Option<Page>,
    no_shadow_cache: bool,
    no_dynamic_shadows: bool,
    sun_angle: Option<f32>,
    light_radius: f32,
    flashlight_fade: f32,
    time: f32,
    show_samples: bool,
    dither_beam: bool,
    /// `RasterConfig` spacing limits, if given.
    min_step: Option<u32>,
    light_spacing: Option<u32>,
    steep_limit: Option<f32>,
    step_threshold: Option<f32>,
    penumbra_threshold: Option<f32>,
    penumbra: f32,
}

/// `X,Y,Z,YAW,PITCH[,ROLL]` for `option` (roll 0 if not given).
fn pose(text: &str, option: &str) -> Result<[f32; 6], String> {
    let mut v = numbers(text, option)?;
    if v.len() == 5 {
        v.push(0.0);
    }
    v.try_into().map_err(|_| format!("{option} is X,Y,Z,YAW,PITCH[,ROLL]"))
}

/// Comma-separated numbers for `option`.
fn numbers(text: &str, option: &str) -> Result<Vec<f32>, String> {
    text.split(',')
        .map(|n| n.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("bad {option}"))
}

fn parse_args() -> Result<Options, String> {
    parse_options(std::env::args().skip(1))
}

/// Options from command-line arguments (without the program's name).
fn parse_options(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut o = Options {
        level: "shiny_rooms.mmp".into(),
        width: 1280,
        height: 720,
        screenshot: None,
        edit: false,
        select: None,
        edit_model: None,
        view: editor::ViewMode::Perspective,
        wire: false,
        zoom: None,
        at: None,
        lock_flashlight: None,
        flashlight_at: None,
        translucent_crates: false,
        per_pixel_crates: false,
        unlit: false,
        bounces: 2,
        f0: 0.15,
        fade: 5.0,
        fps: MAX_FPS,
        filter: sampler_index(filter::BILINEAR_MIPMAP_LINEAR),
        floor_texture: DEFAULT_FLOOR_TEXTURE.into(),
        water: false,
        no_flashlight: false,
        fullscreen: false,
        cone: Cone::Beam,
        no_shadows: false,
        no_sun: false,
        light_scale: 1.0,
        hud: false,
        menu: None,
        no_shadow_cache: false,
        no_dynamic_shadows: false,
        sun_angle: None,
        light_radius: FLASHLIGHT_RADII[2],
        flashlight_fade: FLASHLIGHT_FADES[3],
        level_lights: false,
        time: 0.0,
        show_samples: false,
        dither_beam: false,
        min_step: None,
        light_spacing: None,
        steep_limit: None,
        step_threshold: None,
        penumbra_threshold: None,
        penumbra: 1.0,
    };
    let mut args = args.into_iter();
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
            "--edit" => o.edit = true,
            "--wire" => o.wire = true,
            "--view" => {
                o.view = match value()?.as_str() {
                    "3d" => editor::ViewMode::Perspective,
                    "top" => editor::ViewMode::Ortho(wire::Axis::Top),
                    "front" => editor::ViewMode::Ortho(wire::Axis::Front),
                    "side" => editor::ViewMode::Ortho(wire::Axis::Side),
                    _ => return Err("--view is 3d, top, front or side".into()),
                }
            }
            "--zoom" => o.zoom = Some(value()?.parse().map_err(|_| "bad --zoom")?),
            "--select" => o.select = Some(value()?),
            "--edit-model" => o.edit_model = Some(None),
            "--polygon" => {
                o.edit_model = Some(Some(value()?.parse().map_err(|_| "bad --polygon")?));
            }
            "--at" => o.at = Some(pose(&value()?, "--at")?),
            "--lock-flashlight" => o.lock_flashlight = Some(pose(&value()?, "--lock-flashlight")?),
            "--flashlight-at" => {
                let v = numbers(&value()?, "--flashlight-at")?;
                o.flashlight_at =
                    Some(v.try_into().map_err(|_| "--flashlight-at is X,Y,Z,DX,DY,DZ")?);
            }
            "--translucent-crates" => o.translucent_crates = true,
            "--per-pixel-crates" => o.per_pixel_crates = true,
            "--unlit" => o.unlit = true,
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
            "--fullscreen" => o.fullscreen = true,
            "--cone" => {
                o.cone = match value()?.as_str() {
                    "beam" => Cone::Beam,
                    "sampled" => Cone::Sampled,
                    "soft" => Cone::Soft,
                    _ => return Err("--cone is beam, sampled or soft".into()),
                }
            }
            "--no-shadows" => o.no_shadows = true,
            "--no-sun" => o.no_sun = true,
            "--hud" => o.hud = true,
            "--menu" => {
                let name = value()?;
                o.menu = Some(Page::named(&name).ok_or(format!(
                    "--menu is one of {}",
                    Page::ALL.map(Page::name).join(", ")
                ))?);
            }
            "--light-scale" => {
                o.light_scale = value()?.parse().map_err(|_| "bad --light-scale")?
            }
            "--no-shadow-cache" => o.no_shadow_cache = true,
            "--no-dynamic-shadows" => o.no_dynamic_shadows = true,
            "--sun-angle" => o.sun_angle = Some(value()?.parse().map_err(|_| "bad --sun-angle")?),
            "--light-radius" => {
                o.light_radius = value()?.parse().map_err(|_| "bad --light-radius")?
            }
            "--flashlight-fade" => {
                o.flashlight_fade = value()?.parse().map_err(|_| "bad --flashlight-fade")?
            }
            "--level-lights" => o.level_lights = true,
            "--show-samples" => o.show_samples = true,
            "--dither-beam" => o.dither_beam = true,
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
    /// F0 of shiny surfaces, one of `REFLECTANCE`.
    reflectance: f32,
    /// Fade range of textured shiny surfaces' reflections, one of `FADE_RANGE`.
    fade_range: f32,
    /// Texture sampler, an index into `filter::ALL`.
    filter: usize,
    /// Shiny floors are water.
    water: bool,
    /// Lighting is on (off, surfaces show their full color).
    lit: bool,
    /// Spot lights' penumbra (the angle over which their cone fades) as a multiple of the
    /// level's, around the middle of the fade.
    penumbra: f32,
    /// The player's flashlight is on.
    flashlight: bool,
    /// The level's own lights are on (the flashlight and ambient light aside).
    level_lights: bool,
    /// Lights cast shadows.
    shadows: bool,
    /// The level's directional lights (the sun) are on.
    sun: bool,
    /// Their source's angular size in degrees, if not the level's (one of `SUN_ANGLES`).
    sun_angle: Option<f32>,
    /// The level's point and spot lights' source sizes, as a multiple of the level's (one
    /// of `LIGHT_SCALES`).
    light_scale: f32,
    /// The radius of the flashlight's source, in meters (one of `FLASHLIGHT_RADII`).
    light_radius: f32,
    /// How wide the flashlight's cone fades inside its edge, in degrees (one of
    /// `FLASHLIGHT_FADES`).
    flashlight_fade: f32,
    /// How the flashlight's cone is drawn.
    cone: Cone,
    /// Where the flashlight was left when it was locked in place (sector, position,
    /// direction); `None` while it is on the player's shoulder.
    flashlight_lock: Option<(u32, Vec3, Vec3)>,
    /// Mouse look smoothing, an index into `MOUSE_SMOOTHING`.
    smoothing: usize,
    /// The frame rate is capped, at `cap` frames a second.
    capped: bool,
    cap: u32,
    /// The debug HUD is showing; F3 toggles.
    hud: bool,
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
    /// Sky surfaces: their vertex colors, unlit.
    unlit: MaterialId,
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
    /// Cube map textures no longer used (the level was rebuilt), to draw the next ones in.
    spare_cubes: Vec<TextureId>,
    /// The level editor (Tab), with the level's tables.
    editor: editor::Editor,
    /// The model files in assets/models, for the editor.
    models: Vec<String>,
    /// Files to reload when they change (the level, loaded models and textures), and when
    /// each was last changed as far as the app knows.
    watched: std::collections::HashMap<PathBuf, std::time::SystemTime>,
    settings: Settings,
    /// The levels in assets/levels (file names, sorted), and the one loaded.
    levels: Vec<String>,
    level: String,
    /// Seconds since the start: for the water's ripples and moving lights.
    time: f32,
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
            beam_dither: options.dither_beam,
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
        let unlit = renderer.register_material::<UnlitColor>();
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
        let level_path = Path::new(root).join("levels").join(&options.level);
        let level_src = std::fs::read_to_string(&level_path).map_err(|e| e.to_string())?;
        let doc = LevelDoc::parse(&level_path, &level_src).map_err(|e| e.to_string())?;
        let mut app = App {
            assets,
            lights: (world.lights().to_vec(), world.ambient),
            world,
            camera,
            geometry: {
                let mut g = ViewGeometry::new();
                g.config.max_reflections = options.bounces;
                g.config.cache_shadows = !options.no_shadow_cache;
                g.config.dynamic_shadows = !options.no_dynamic_shadows;
                g
            },
            renderer,
            opaque,
            unlit,
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
            spare_cubes: Vec::new(),
            editor: editor::Editor::new(doc, level_path),
            models: model_files(root),
            watched: std::collections::HashMap::new(),
            settings: Settings {
                translucent_crates: options.translucent_crates,
                per_pixel_crates: options.per_pixel_crates,
                reflectance: options.f0.clamp(0.0, 1.0),
                fade_range: options.fade.max(0.0),
                filter: options.filter,
                water: options.water,
                lit: !options.unlit,
                penumbra: options.penumbra.clamp(1.0 / 64.0, 64.0),
                flashlight: !options.no_flashlight,
                level_lights: true,
                shadows: !options.no_shadows,
                sun: !options.no_sun,
                light_scale: options.light_scale.max(0.0),
                sun_angle: options.sun_angle.map(|a| a.clamp(0.0, 45.0)),
                light_radius: options.light_radius.max(0.0),
                flashlight_fade: options.flashlight_fade.clamp(0.0, FLASHLIGHT_OUTER),
                cone: options.cone,
                flashlight_lock: None,
                smoothing: 2,
                capped: options.fps != 0,
                cap: if options.fps == 0 { MAX_FPS } else { options.fps },
                hud: options.hud || options.screenshot.is_none(),
            },
            time: 0.0,
            levels: level_files(root),
            level: options.level.clone(),
            pixels: vec![0; (options.width * options.height) as usize],
            width: options.width,
            height: options.height,
        };
        app.editor.learn_animations(&app.world, &app.assets);
        // Mirror balls' cube maps see the level's lights only, not the flashlight where the
        // player happens to start.
        app.apply_lights(false);
        // The static lights' shadows on static surfaces, carved now rather than as each
        // surface is first seen.
        let started = Instant::now();
        let baked = app.geometry.bake_shadows(&app.world, &app.assets);
        println!(
            "baked {baked} static light shadows in {:.1} ms",
            started.elapsed().as_secs_f64() * 1000.0
        );
        app.bake_cube_maps()?;
        app.settings.level_lights = options.level_lights;
        Ok(app)
    }

    /// Draws the frame: the 3D view, or the editor's 2D view (a wireframe on a grid).
    /// Returns the view and raster times (none for a 2D view).
    fn frame(&mut self) -> Result<(f64, f64), String> {
        let (w, h) = (self.width as usize, self.height as usize);
        if self.editor.on
            && let Some(ortho) = self.editor.ortho(w, h)
        {
            let mut canvas = ui::Canvas {
                pixels: &mut self.pixels,
                width: w,
                height: h,
            };
            self.editor.draw_2d(&mut canvas, &ortho, &self.world, &self.assets);
            return Ok((0.0, 0.0));
        }
        self.render()
    }

    /// Rebuilds the level from the editor's tables: loads their text (which checks it),
    /// and redoes what depends on the level (its lights, baked shadows and cube maps).
    /// The camera stays where it is. On an error the level is left as it was.
    fn rebuild(&mut self) -> Result<(), String> {
        let text = self.editor.doc.to_text();
        let level = self
            .assets
            .parse_level(&self.level, &text)
            .map_err(|e| e.to_string())?;
        let mut world = World::new(level, &self.assets);
        self.world.hand_over_copies(&mut world);
        world.animate(&mut self.assets, self.time);
        self.camera.sector = world
            .find_sector(self.camera.position)
            .or_else(|| world.spawn_points.first().map(|s| s.sector))
            .unwrap_or(0);
        self.world = world;
        self.lights = (self.world.lights().to_vec(), self.world.ambient);
        while self.wall_textures.len() < self.world.sectors.len() {
            let i = self.wall_textures.len();
            let texture = match self.floor_texture {
                Some(_) => Some(
                    self.assets
                        .load_texture(WALL_TEXTURES[i % WALL_TEXTURES.len()])
                        .map_err(|e| e.to_string())?,
                ),
                None => None,
            };
            self.wall_textures.push(texture);
        }
        self.spare_cubes
            .extend(self.cube_maps.drain(..).flatten().map(|c| c.texture));
        self.cube_maps = vec![None; self.world.entities.len()];
        // Baked as at load: with the level's lights, and no flashlight.
        let level_lights = self.settings.level_lights;
        self.settings.level_lights = true;
        self.apply_lights(false);
        self.geometry.clear_shadows();
        self.geometry.bake_shadows(&self.world, &self.assets);
        let baked = self.bake_cube_maps();
        self.settings.level_lights = level_lights;
        self.editor.learn_animations(&self.world, &self.assets);
        baked
    }

    /// An edit of the editor's selection (see `editor::Editor::change`): made in the
    /// tables and the level rebuilt, or undone with the reason if the level rejects it.
    fn edit(&mut self, field: editor::Field, dir: f32) {
        if field == editor::Field::EditModel {
            self.toggle_model();
            return;
        }
        if self.editor.model.is_some() {
            self.edit_model(field, dir);
            return;
        }
        if !field.is_edit() {
            self.editor.command(field, dir);
            return;
        }
        let before = self.editor.begin();
        let result = self
            .editor
            .change(field, dir, &self.world, &self.models)
            .and_then(|what| self.rebuild().map(|()| what));
        match result {
            Ok(what) => self.editor.commit(before, &what),
            Err(why) => {
                self.editor.find_problem(&why);
                self.editor.revert(before, &why);
            }
        }
    }

    /// Opens the mesh editor on the selected entity's model, or leaves it. Leaving with
    /// unsaved changes is refused once; the second time drops them (the model is read from
    /// its file again).
    fn toggle_model(&mut self) {
        if let Some(model) = &mut self.editor.model {
            if model.dirty() && !model.warned {
                model.warned = true;
                self.editor.say("the model has unsaved changes: Ctrl+S saves them, leaving again drops them");
                return;
            }
            let (file, dirty) = (model.file.clone(), model.dirty());
            self.editor.model = None;
            if dirty {
                let result = self
                    .assets
                    .reload_mesh(&file)
                    .map_err(|e| e.to_string())
                    .and_then(|_| self.rebuild());
                match result {
                    Ok(()) => self.editor.say(format!("dropped the changes to {file}")),
                    Err(e) => self.editor.say(e),
                }
            }
            return;
        }
        let Some(editor::Selection::Entity(row)) = self.editor.selection else {
            self.editor.say("select an entity with a .mmdl model first");
            return;
        };
        let Some(file) = self.editor.doc.entities[row].model.clone().filter(|m| m.ends_with(".mmdl")) else {
            self.editor.say("the mesh editor edits .mmdl models (export one from Blender)");
            return;
        };
        let Some(entity) = self.editor.world_entity(row) else {
            return;
        };
        let path = self.assets.root().join("models").join(&file);
        let doc = std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|src| ModelDoc::parse(&path, &src).map_err(|e| e.to_string()));
        match doc {
            Ok(doc) => {
                self.editor.model = Some(mesh_edit::ModelEdit::new(file.clone(), path, doc, entity));
                self.editor.say(format!("editing {file}"));
            }
            Err(e) => self.editor.say(e),
        }
    }

    /// A mesh editor change: made in the model's tables, loaded (which checks them) and the
    /// level rebuilt with it; or undone, with the reason.
    fn edit_model(&mut self, field: editor::Field, dir: f32) {
        let Some(model) = &mut self.editor.model else {
            return;
        };
        if !field.is_edit() {
            model.command(field, dir);
            return;
        }
        let before = model.begin();
        let changed = model.change(field, dir);
        let (file, text) = (model.file.clone(), model.doc.to_text());
        let mut loaded = false;
        let result = changed.and_then(|what| {
            self.assets.set_model_text(&file, &text).map_err(|e| e.to_string())?;
            loaded = true;
            self.rebuild()?;
            Ok(what)
        });
        let model = self.editor.model.as_mut().unwrap();
        match result {
            Ok(what) => {
                model.commit(before);
                self.editor.say(what);
            }
            Err(why) => {
                let old = before.0.to_text();
                model.revert(before);
                if loaded {
                    // The model loaded, but the level refused it: back as it was.
                    let _ = self.assets.set_model_text(&file, &old);
                    let _ = self.rebuild();
                }
                self.editor.say(why);
            }
        }
    }

    /// Writes the mesh editor's model to its file.
    fn save_model(&mut self) {
        let Some(model) = &mut self.editor.model else {
            return;
        };
        let path = model.path.clone();
        match std::fs::write(&path, model.doc.to_text()) {
            Ok(()) => {
                if let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) {
                    self.watched.insert(path.clone(), modified);
                }
                model.mark_saved();
                self.editor.say(format!("saved {}", path.display()));
            }
            Err(e) => self.editor.say(format!("cannot save {}: {e}", path.display())),
        }
    }

    /// Undoes (or redoes) the last edit (of the model, in the mesh editor).
    fn travel(&mut self, redo: bool) {
        if let Some(model) = &mut self.editor.model {
            if !model.travel(redo) {
                self.editor.say(if redo { "nothing to redo" } else { "nothing to undo" });
                return;
            }
            let (file, text) = (model.file.clone(), model.doc.to_text());
            let result = self
                .assets
                .set_model_text(&file, &text)
                .map_err(|e| e.to_string())
                .and_then(|()| self.rebuild());
            match result {
                Ok(()) => self.editor.say(if redo { "redone" } else { "undone" }),
                Err(e) => self.editor.say(e),
            }
            return;
        }
        if !self.editor.travel(redo) {
            self.editor.say(if redo { "nothing to redo" } else { "nothing to undo" });
            return;
        }
        match self.rebuild() {
            Ok(()) => self.editor.say(if redo { "redone" } else { "undone" }),
            Err(e) => self.editor.say(e),
        }
    }

    /// Reloads what changed on disk since the last look: models and textures (then
    /// rebuilds the level, as entities' shapes may have changed), and the level's own file
    /// unless the editor has changes of its own.
    fn hot_reload(&mut self) {
        let root = self.assets.root().to_path_buf();
        let (models, textures) = self.assets.loaded_files();
        let files = models
            .iter()
            .map(|m| (root.join("models").join(m), Some((m.clone(), true))))
            .chain(textures.iter().map(|t| (root.join("textures").join(t), Some((t.clone(), false)))))
            .chain(std::iter::once((self.editor.path.clone(), None)));
        let mut changed = Vec::new();
        for (path, what) in files {
            let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
                continue;
            };
            match self.watched.insert(path.clone(), modified) {
                Some(before) if before != modified => changed.push((path, what)),
                _ => {}
            }
        }
        let mut rebuild = false;
        for (path, what) in changed {
            let result = match what {
                Some((name, true)) => self.assets.reload_mesh(&name).map(|_| rebuild = true),
                Some((name, false)) => self.assets.reload_texture(&name).map(|_| ()),
                None if self.editor.dirty() => {
                    self.editor.say("the level's file changed on disk; your edits here are kept");
                    continue;
                }
                None => match std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|src| LevelDoc::parse(&path, &src).map_err(|e| e.to_string()))
                {
                    Ok(doc) => {
                        self.editor.doc = doc;
                        self.editor.mark_saved();
                        self.editor.selection = None;
                        rebuild = true;
                        Ok(())
                    }
                    Err(e) => {
                        self.editor.say(e);
                        continue;
                    }
                },
            };
            match result {
                Ok(()) => self.editor.say(format!("reloaded {}", path.display())),
                Err(e) => self.editor.say(e.to_string()),
            }
        }
        if rebuild && let Err(e) = self.rebuild() {
            self.editor.say(e);
        }
    }

    /// Writes the editor's tables to the level's file.
    fn save_level(&mut self) {
        let path = self.editor.path.clone();
        match std::fs::write(&path, self.editor.doc.to_text()) {
            Ok(()) => {
                // Not a change to reload.
                if let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) {
                    self.watched.insert(path.clone(), modified);
                }
                self.editor.mark_saved();
                self.editor.say(format!("saved {}", path.display()));
            }
            Err(e) => self.editor.say(format!("cannot save {}: {e}", path.display())),
        }
    }

    /// Moves the animated models and the water to `time` seconds, redrawing the water's
    /// textures if the ripples moved.
    fn set_time(&mut self, time: f32) {
        self.time = time;
        self.world.animate(&mut self.assets, time);
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
            let texture = match self.spare_cubes.pop() {
                Some(id) => {
                    *self.assets.texture_mut(id) = texture;
                    id
                }
                None => self.assets.add_texture(texture),
            };
            self.cube_maps[i] = Some(CubeMap { texture, radius });
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
            let cos = |degrees: f32| degrees.clamp(0.0, 180.0).to_radians().cos();
            Light {
                cos_inner: cos(middle - half),
                cos_outer: cos(middle + half),
                ..l
            }
        };
        // The level's lights (N) and its directional lights (I, with the angle Y sets).
        // Those that cast shadows have fixed shadow slots after the flashlight's (their
        // place in the level plus 1), so their cached shadows stay theirs whatever is on.
        let s = &self.settings;
        let mut lights: Vec<Light> = Vec::new();
        for (i, &l) in self.lights.0.iter().enumerate() {
            let mut l = scaled(l);
            // A moving light where it is now, in the sector it is in.
            if l.motion.is_some() {
                l.position = l.at_time(self.time);
                l.sector = self.world.find_sector(l.position).unwrap_or(l.sector);
            }
            if l.directional {
                if !s.sun {
                    continue;
                }
                if let Some(angle) = s.sun_angle {
                    l.radius = (angle.to_radians() / 2.0).sin();
                }
            } else if !s.level_lights {
                continue;
            } else {
                l.radius *= s.light_scale;
            }
            if s.shadows && l.shadows && i + 1 < MAX_SHADOW_SLOTS as usize {
                l.shadow = Some(i as u8 + 1);
            }
            lights.push(l);
        }
        if flashlight && self.settings.flashlight {
            let mut light = scaled(self.flashlight());
            // Shadow slot 0: the view carves its shadows into polygons.
            light.shadow = self.settings.shadows.then_some(0);
            light.radius = self.settings.light_radius;
            light.beam = self.settings.cone == Cone::Beam;
            light.coarse = self.settings.cone == Cone::Soft;
            lights.push(light);
        }
        self.world.set_lights(lights, self.lights.1);
    }

    /// The player's flashlight: where it was locked, or on their shoulder.
    fn flashlight(&self) -> Light {
        let (sector, position, direction) = self.settings.flashlight_lock.unwrap_or_else(|| self.mount());
        Light::spot(
            sector,
            position,
            FLASHLIGHT_COLOR,
            FLASHLIGHT_RANGE,
            direction,
            FLASHLIGHT_OUTER - self.flashlight_fade(),
            FLASHLIGHT_OUTER,
        )
    }

    /// How the flashlight's cone is drawn, as `--cone` names it.
    fn cone_name(&self) -> &'static str {
        match self.settings.cone {
            Cone::Beam => "beam",
            Cone::Sampled => "sampled",
            Cone::Soft => "soft",
        }
    }

    /// How wide the flashlight's cone fades, in degrees: all of it for a soft cone.
    fn flashlight_fade(&self) -> f32 {
        match self.settings.cone {
            Cone::Soft => FLASHLIGHT_OUTER,
            _ => self.settings.flashlight_fade,
        }
    }

    /// Puts the camera at `[x, y, z, yaw, pitch]` (degrees); `option` names it in errors.
    fn place_camera(&mut self, [x, y, z, yaw, pitch, roll]: [f32; 6], option: &str) -> Result<(), String> {
        self.camera.position = Vec3::new(x, y, z);
        self.camera.sector = self
            .world
            .find_sector(self.camera.position)
            .ok_or(format!("{option} is outside the level"))?;
        (self.camera.yaw, self.camera.pitch, self.camera.roll) =
            (yaw.to_radians(), pitch.to_radians(), roll.to_radians());
        Ok(())
    }

    /// A menu page's rows: each item's label and, for a setting, its value now.
    fn rows(&self, page: Page) -> Vec<ui::Row> {
        self.items(page)
            .iter()
            .map(|&item| match item {
                Item::Load(i) => {
                    let name = &self.levels[i as usize];
                    ui::Row {
                        label: name.trim_end_matches(".mmp").to_string(),
                        value: (*name == self.level).then(|| "loaded".to_string()),
                    }
                }
                _ => ui::Row {
                    label: item.label().to_string(),
                    value: match item {
                        Item::Set(s) => Some(self.value(s)),
                        _ => None,
                    },
                },
            })
            .collect()
    }

    /// A menu page's items: the page's own, or for the level list, one per level file.
    fn items(&self, page: Page) -> Vec<Item> {
        match page {
            Page::Levels => (0..self.levels.len() as u16).map(Item::Load).collect(),
            _ => page.items().to_vec(),
        }
    }

    /// A setting's value, as the menu shows it.
    fn value(&self, setting: Setting) -> String {
        let (s, cfg) = (&self.settings, &self.renderer.config);
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        let fraction = |t: f32| if t > 0.0 { format!("1/{}", (1.0 / t).round()) } else { "off".into() };
        match setting {
            Setting::Lit => on(s.lit),
            Setting::LevelLights => {
                let n = self.lights.0.iter().filter(|l| !l.directional).count();
                format!("{} ({n})", on(s.level_lights))
            }
            Setting::LevelLightSize => format!("x{}", s.light_scale),
            Setting::Sun => match self.lights.0.iter().any(|l| l.directional) {
                true => on(s.sun),
                false => "none here".into(),
            },
            Setting::SunSize => match self.lights.0.iter().any(|l| l.directional) {
                true => format!("{}°", s.sun_angle.unwrap_or(self.level_sun_angle())),
                false => "-".into(),
            },
            Setting::SpotPenumbra => format!("x{:.2}", s.penumbra),
            Setting::Shadows => on(s.shadows),
            Setting::DynamicShadows => on(self.geometry.config.dynamic_shadows),
            Setting::Flashlight => on(s.flashlight),
            Setting::FlashlightMount => {
                if s.flashlight_lock.is_some() { "locked here" } else { "shoulder" }.into()
            }
            Setting::FlashlightSize => format!("{} cm", (s.light_radius * 100.0).round()),
            Setting::FlashlightFade => match (s.cone, s.flashlight_fade) {
                (Cone::Soft, _) => format!("{FLASHLIGHT_OUTER}° (soft)"),
                (_, 0.0) => "hard".into(),
                (_, fade) => format!("{fade}°"),
            },
            Setting::FlashlightDither => on(cfg.beam_dither),
            Setting::FlashlightBeam => self.cone_name().into(),
            Setting::Filter => filter::name(filter::ALL[s.filter]).replace("_mipmap_", " / "),
            Setting::Water => on(s.water),
            Setting::Bounces => self.geometry.config.max_reflections.to_string(),
            Setting::Reflectance => s.reflectance.to_string(),
            Setting::Fade => if s.fade_range > 0.0 { format!("{} m", s.fade_range) } else { "off".into() },
            Setting::TranslucentCrates => on(s.translucent_crates),
            Setting::PerPixelCrates => on(s.per_pixel_crates),
            Setting::FrameCap => if s.capped { format!("{} fps", s.cap) } else { "off".into() },
            Setting::Overlay => on(cfg.show_samples),
            Setting::MinStep => format!("{} px", cfg.min_step),
            Setting::LightSpacing => format!("{} px", cfg.light_spacing),
            Setting::SteepLimit => {
                if cfg.steep_limit.is_finite() { cfg.steep_limit.to_string() } else { "off".into() }
            }
            Setting::StepThreshold => fraction(cfg.step_threshold),
            Setting::PenumbraThreshold => fraction(cfg.penumbra_threshold),
            Setting::MouseSmoothing => match MOUSE_SMOOTHING[s.smoothing] {
                0.0 => "off".into(),
                w => format!("{} ms", (w * 1000.0).round()),
            },
            Setting::Hud => on(s.hud),
        }
    }

    /// The level's directional lights' angular size in degrees (0 without any).
    fn level_sun_angle(&self) -> f32 {
        self.lights
            .0
            .iter()
            .find(|l| l.directional)
            .map_or(0.0, |l| (l.radius.asin() * 2.0).to_degrees())
    }

    /// Changes a setting one step (`dir` +1 or -1; on/off settings just toggle).
    fn change(&mut self, setting: Setting, dir: i32) {
        let (s, cfg) = (&mut self.settings, &mut self.renderer.config);
        let wrap = |i: usize, n: usize| (i as i32 + dir).rem_euclid(n as i32) as usize;
        match setting {
            Setting::Lit => s.lit = !s.lit,
            Setting::LevelLights => s.level_lights = !s.level_lights,
            Setting::LevelLightSize => s.light_scale = cycle(&LIGHT_SCALES, s.light_scale, dir),
            Setting::Sun => s.sun = !s.sun,
            Setting::SunSize => {
                let level = self.level_sun_angle();
                let s = &mut self.settings;
                s.sun_angle = Some(cycle(&SUN_ANGLES, s.sun_angle.unwrap_or(level), dir));
            }
            Setting::SpotPenumbra => {
                s.penumbra = if dir > 0 {
                    (s.penumbra * PENUMBRA_STEP).min(64.0)
                } else {
                    (s.penumbra / PENUMBRA_STEP).max(1.0 / 64.0)
                }
            }
            Setting::Shadows => s.shadows = !s.shadows,
            Setting::DynamicShadows => {
                let d = &mut self.geometry.config.dynamic_shadows;
                *d = !*d;
            }
            Setting::Flashlight => s.flashlight = !s.flashlight,
            Setting::FlashlightMount => {
                let mount = self.mount();
                let s = &mut self.settings;
                s.flashlight_lock = match s.flashlight_lock {
                    Some(_) => None,
                    None => Some(mount),
                };
            }
            Setting::FlashlightSize => s.light_radius = cycle(&FLASHLIGHT_RADII, s.light_radius, dir),
            Setting::FlashlightFade => {
                s.flashlight_fade = cycle(&FLASHLIGHT_FADES, s.flashlight_fade, dir)
            }
            Setting::FlashlightBeam => {
                const CONES: [Cone; 3] = [Cone::Beam, Cone::Sampled, Cone::Soft];
                let i = CONES.iter().position(|&c| c == s.cone).unwrap_or(0);
                s.cone = CONES[wrap(i, CONES.len())];
            }
            Setting::FlashlightDither => cfg.beam_dither = !cfg.beam_dither,
            Setting::Filter => s.filter = wrap(s.filter, filter::ALL.len()),
            Setting::Water => s.water = !s.water,
            Setting::Bounces => {
                let b = &mut self.geometry.config.max_reflections;
                *b = wrap(*b as usize, 5) as u8;
            }
            Setting::Reflectance => s.reflectance = cycle(&REFLECTANCE, s.reflectance, dir),
            Setting::Fade => s.fade_range = cycle(&FADE_RANGE, s.fade_range, dir),
            Setting::TranslucentCrates => s.translucent_crates = !s.translucent_crates,
            Setting::PerPixelCrates => s.per_pixel_crates = !s.per_pixel_crates,
            Setting::FrameCap => s.capped = !s.capped,
            Setting::Overlay => cfg.show_samples = !cfg.show_samples,
            Setting::MinStep => {
                cfg.min_step = cycle(&PIXEL_STEPS, cfg.min_step as f32, dir) as u32;
            }
            Setting::LightSpacing => {
                cfg.light_spacing = cycle(&PIXEL_STEPS, cfg.light_spacing as f32, dir) as u32;
            }
            Setting::SteepLimit => cfg.steep_limit = cycle(&STEEP_LIMITS, cfg.steep_limit, dir),
            Setting::StepThreshold => {
                cfg.step_threshold = cycle(&STEP_THRESHOLDS, cfg.step_threshold, dir);
            }
            Setting::PenumbraThreshold => {
                cfg.penumbra_threshold = cycle(&PENUMBRA_THRESHOLDS, cfg.penumbra_threshold, -dir);
            }
            Setting::MouseSmoothing => s.smoothing = wrap(s.smoothing, MOUSE_SMOOTHING.len()),
            Setting::Hud => s.hud = !s.hud,
        }
    }

    /// The debug HUD's lines: frame rate and times, where the camera is, and what's on.
    /// `present_ms` is showing the last frames (see `PresentTimes`: copying, handing it to
    /// the system, and the window's events, without the frame rate cap's wait).
    fn hud(&self, fps: f64, view_ms: f64, raster_ms: f64, present_ms: f64) -> Vec<String> {
        let (c, s) = (&self.camera, &self.settings);
        let on = |b: bool, name: &str| if b { name.to_string() } else { format!("{name} off") };
        vec![
            format!(
                "{fps:.0} fps{}   view {view_ms:.2} ms   raster {raster_ms:.2} ms   present {present_ms:.2} ms",
                if s.capped { format!(" (cap {})", s.cap) } else { String::new() },
            ),
            format!(
                "{}  ({:.1}, {:.1}, {:.1})  yaw {:.0}  pitch {:.0}",
                self.world.sectors[c.sector as usize].name,
                c.position.x,
                c.position.y,
                c.position.z,
                c.yaw.to_degrees().rem_euclid(360.0),
                c.pitch.to_degrees(),
            ),
            format!(
                "{} polygons   {} mirrors   {}",
                self.geometry.polygons.len(),
                self.geometry.mirrors.len(),
                filter::name(filter::ALL[s.filter]),
            ),
            if s.lit {
                format!(
                    "{}   {}   {}{}   {}",
                    on(s.level_lights, "level lights"),
                    on(s.sun, "sun"),
                    on(s.flashlight, "flashlight"),
                    if s.flashlight_lock.is_some() { " (locked)" } else { "" },
                    on(s.shadows, "shadows"),
                )
            } else {
                "lighting off".into()
            },
        ]
    }

    /// A command line that starts the app (as a screenshot) exactly where the camera is,
    /// with the current settings and the water at `time` seconds: for reporting what's on
    /// screen. Options the app can't be started with are listed after it.
    fn command_line(&self, options: &Options, time: f32) -> String {
        let (c, s, cfg) = (&self.camera, &self.settings, &self.renderer.config);
        let mut line = format!(
            "cargo run --release -p moose-app -- --level {} --size {}x{} --at {},{},{},{},{},{}",
            options.level,
            self.width,
            self.height,
            c.position.x,
            c.position.y,
            c.position.z,
            c.yaw.to_degrees(),
            c.pitch.to_degrees(),
            c.roll.to_degrees(),
        );
        let mut add = |option: String| {
            line.push(' ');
            line.push_str(&option);
        };
        if let Some((_, p, d)) = s.flashlight_lock {
            add(format!("--flashlight-at {},{},{},{},{},{}", p.x, p.y, p.z, d.x, d.y, d.z));
        }
        add(format!("--filter {}", filter::name(filter::ALL[s.filter])));
        add(format!("--bounces {}", self.geometry.config.max_reflections));
        add(format!("--f0 {} --fade {}", s.reflectance, s.fade_range));
        add(format!(
            "--light-radius {} --flashlight-fade {} --light-scale {} --penumbra {}",
            s.light_radius, s.flashlight_fade, s.light_scale, s.penumbra
        ));
        if let Some(angle) = s.sun_angle {
            add(format!("--sun-angle {angle}"));
        }
        add(format!(
            "--min-step {} --light-spacing {} --steep-limit {} --step-threshold {} --penumbra-threshold {}",
            cfg.min_step,
            cfg.light_spacing,
            if cfg.steep_limit.is_finite() { cfg.steep_limit.to_string() } else { "off".into() },
            cfg.step_threshold,
            cfg.penumbra_threshold,
        ));
        add(format!("--cone {}", self.cone_name()));
        add(format!("--time {time}"));
        for (on, flag) in [
            (s.water, "--water"),
            (!s.flashlight, "--no-flashlight"),
            (s.level_lights, "--level-lights"),
            (!s.shadows, "--no-shadows"),
            (!self.geometry.config.dynamic_shadows, "--no-dynamic-shadows"),
            (!s.sun, "--no-sun"),
            (!s.lit, "--unlit"),
            (s.translucent_crates, "--translucent-crates"),
            (s.per_pixel_crates, "--per-pixel-crates"),
            (cfg.show_samples, "--show-samples"),
            (cfg.beam_dither, "--dither-beam"),
        ] {
            if on {
                add(flag.to_string());
            }
        }
        add("--screenshot report.png".to_string());
        line
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
        let (opaque, unlit, translucent, fresnel, s) =
            (self.opaque, self.unlit, self.translucent, self.fresnel, &self.settings);
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
                        if p.flags.sky() {
                            return Surface::new(unlit);
                        }
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
                            // Floors (shiny or not) get the floor texture.
                            None => [
                                floor_texture
                                    .filter(|_| p.flags.reflective() || n.y > 0.9)
                                    .or(wall),
                                None,
                            ],
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

/// The level files (`.mmp`) in the assets' levels folder, sorted.
/// The model files in assets/models (sorted).
fn model_files(assets: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(Path::new(assets).join("models"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".obj") || n.ends_with(".mmdl"))
        .collect();
    names.sort();
    names
}

fn level_files(assets: &str) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(Path::new(assets).join("levels"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".mmp"))
        .collect();
    names.sort();
    names
}

/// Numbers the menu steps through for pixel spacings.
const PIXEL_STEPS: [f32; 6] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0];
/// Perspective thresholds the menu steps through (see `RasterConfig::step_threshold`).
const STEP_THRESHOLDS: [f32; 9] = [
    1.0 / 256.0,
    1.0 / 128.0,
    1.0 / 64.0,
    1.0 / 32.0,
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    1.0,
];

/// The value `dir` steps from `now` in `list`, wrapping around: from `now`'s place in it,
/// or if it isn't there, from the nearest value beyond it that way.
fn cycle(list: &[f32], now: f32, dir: i32) -> f32 {
    let same = |x: f32| x == now || (x - now).abs() <= 1e-4 * x.abs().max(1e-3);
    let n = list.len() as i32;
    let i = match list.iter().position(|&x| same(x)) {
        Some(i) => i as i32 + dir,
        None if dir > 0 => list.iter().position(|&x| x > now).map_or(n, |i| i as i32),
        None => list.iter().rposition(|&x| x < now).map_or(-1, |i| i as i32),
    };
    list[i.rem_euclid(n) as usize]
}

/// A page of the options menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Main,
    Levels,
    Lighting,
    Flashlight,
    Rendering,
    Sampling,
    Controls,
}

/// A row of a menu page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Item {
    Resume,
    Respawn,
    Open(Page),
    Quit,
    Set(Setting),
    /// Load a level: an index into `App::levels`.
    Load(u16),
}

/// A setting the menu shows and changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Lit,
    LevelLights,
    LevelLightSize,
    Sun,
    SunSize,
    SpotPenumbra,
    Shadows,
    DynamicShadows,
    Flashlight,
    FlashlightMount,
    FlashlightSize,
    FlashlightBeam,
    FlashlightFade,
    FlashlightDither,
    Filter,
    Water,
    Bounces,
    Reflectance,
    Fade,
    TranslucentCrates,
    PerPixelCrates,
    FrameCap,
    Overlay,
    MinStep,
    LightSpacing,
    SteepLimit,
    StepThreshold,
    PenumbraThreshold,
    MouseSmoothing,
    Hud,
}

impl Page {
    const ALL: [Page; 7] = [
        Page::Main,
        Page::Levels,
        Page::Lighting,
        Page::Flashlight,
        Page::Rendering,
        Page::Sampling,
        Page::Controls,
    ];

    fn name(self) -> &'static str {
        match self {
            Page::Main => "main",
            Page::Levels => "levels",
            Page::Lighting => "lighting",
            Page::Flashlight => "flashlight",
            Page::Rendering => "rendering",
            Page::Sampling => "sampling",
            Page::Controls => "controls",
        }
    }

    fn named(name: &str) -> Option<Page> {
        Page::ALL.into_iter().find(|p| p.name() == name)
    }

    fn title(self) -> &'static str {
        match self {
            Page::Main => "Moose",
            Page::Levels => "Levels",
            Page::Lighting => "Lighting",
            Page::Flashlight => "Flashlight",
            Page::Rendering => "Rendering",
            Page::Sampling => "Sampling (debug)",
            Page::Controls => "Controls",
        }
    }

    fn items(self) -> &'static [Item] {
        use Item::*;
        use Setting::*;
        match self {
            Page::Main => &[
                Resume,
                Open(Page::Levels),
                Open(Page::Lighting),
                Open(Page::Flashlight),
                Open(Page::Rendering),
                Open(Page::Sampling),
                Open(Page::Controls),
                Respawn,
                Quit,
            ],
            Page::Lighting => &[
                Set(Lit),
                Set(Shadows),
                Set(DynamicShadows),
                Set(LevelLights),
                Set(LevelLightSize),
                Set(Sun),
                Set(SunSize),
                Set(SpotPenumbra),
            ],
            Page::Flashlight => &[
                Set(Flashlight),
                Set(FlashlightMount),
                Set(FlashlightSize),
                Set(FlashlightBeam),
                Set(FlashlightFade),
                Set(FlashlightDither),
            ],
            Page::Rendering => &[
                Set(Filter),
                Set(Water),
                Set(Bounces),
                Set(Reflectance),
                Set(Fade),
                Set(TranslucentCrates),
                Set(PerPixelCrates),
                Set(FrameCap),
            ],
            Page::Sampling => &[
                Set(Overlay),
                Set(MinStep),
                Set(LightSpacing),
                Set(SteepLimit),
                Set(StepThreshold),
                Set(PenumbraThreshold),
            ],
            Page::Controls => &[Set(MouseSmoothing), Set(Hud)],
            // The levels found in assets/levels (see `App::items`).
            Page::Levels => &[],
        }
    }

    /// Lines shown under a page's rows.
    fn notes(self) -> &'static [&'static str] {
        match self {
            Page::Controls => &[
                "WASD move, mouse or arrows look, Q/E roll",
                "Space/C up/down, Shift faster",
                "U lock the flashlight in place / remount it",
                "Esc menu, F1 print a command line for this view",
                "F3 debug HUD, Alt+Enter fullscreen, F12 screenshot",
            ],
            Page::Flashlight => &[
                "Locking leaves it where it is: walk around",
                "to see its shadows. U does it from anywhere.",
            ],
            Page::Sampling => &["How shading is sampled: for tuning and debugging."],
            Page::Levels => &["Settings carry over; you start at its spawn."],
            _ => &[],
        }
    }
}

impl Item {
    fn label(self) -> &'static str {
        match self {
            Item::Resume => "Resume",
            Item::Respawn => "Back to spawn",
            Item::Quit => "Quit",
            Item::Load(_) => "Load level",
            Item::Open(page) => match page {
                Page::Lighting => "Lighting",
                Page::Flashlight => "Flashlight",
                Page::Rendering => "Rendering",
                Page::Sampling => "Sampling (debug)",
                Page::Controls => "Controls",
                Page::Levels => "Levels",
                Page::Main => "Back",
            },
            Item::Set(s) => match s {
                Setting::Lit => "All lighting",
                Setting::LevelLights => "Level lights",
                Setting::LevelLightSize => "Level light size",
                Setting::Sun => "Sun",
                Setting::SunSize => "Sun size",
                Setting::SpotPenumbra => "Spot cone edge",
                Setting::Shadows => "Shadows",
                Setting::DynamicShadows => "Dynamic shadows",
                Setting::Flashlight => "Flashlight",
                Setting::FlashlightMount => "Mount",
                Setting::FlashlightSize => "Size",
                Setting::FlashlightBeam => "Cone",
                Setting::FlashlightFade => "Fade",
                Setting::FlashlightDither => "Dither",
                Setting::Filter => "Texture filter",
                Setting::Water => "Water floors",
                Setting::Bounces => "Reflection bounces",
                Setting::Reflectance => "Floor reflectance",
                Setting::Fade => "Reflection fade",
                Setting::TranslucentCrates => "Translucent crates",
                Setting::PerPixelCrates => "Per-pixel crates",
                Setting::FrameCap => "Frame cap",
                Setting::Overlay => "Sample overlay",
                Setting::MinStep => "Minimum step",
                Setting::LightSpacing => "Light spacing",
                Setting::SteepLimit => "Steep limit",
                Setting::StepThreshold => "Perspective threshold",
                Setting::PenumbraThreshold => "Penumbra threshold",
                Setting::MouseSmoothing => "Mouse smoothing",
                Setting::Hud => "Debug HUD",
            },
        }
    }
}

/// The options menu: open or not, the page showing, the selected row on it, and the pages
/// it was opened from.
struct Menu {
    open: bool,
    page: Page,
    selected: usize,
    back: Vec<(Page, usize)>,
}

/// Draws the menu page or the HUD over the app's frame, as the settings say.
fn draw_ui(app: &mut App, menu: Option<(Page, usize)>, hud: Option<Vec<String>>) {
    let rows = menu.map(|(page, _)| app.rows(page));
    let mut canvas = ui::Canvas {
        pixels: &mut app.pixels,
        width: app.width as usize,
        height: app.height as usize,
    };
    if app.editor.on && menu.is_none() {
        let camera = app.camera.view();
        app.editor.draw(&mut canvas, &camera, &app.world, &app.assets);
    }
    if let Some(lines) = hud {
        ui::draw_hud(&mut canvas, &lines);
    }
    if let (Some((page, selected)), Some(rows)) = (menu, rows) {
        let hint = match page {
            Page::Main => "Up/Down choose   Enter pick   Esc close",
            Page::Levels => "Up/Down choose   Enter load   Backspace back",
            _ => "Up/Down choose   Left/Right change   Backspace back",
        };
        ui::draw_menu(&mut canvas, page.title(), &rows, selected, page.notes(), hint);
    }
}

/// The editor's mouse and keys, while it is on and the menu is closed: picking and the
/// panel with the mouse, and the keys for edits, undo and saving. Returns whether the
/// arrows (and PageUp/PageDown) move the selection, rather than turn the view.
fn edit_input(app: &mut App, display: &Display) -> bool {
    use editor::{Field, Selection};
    let (w, h) = (app.width as usize, app.height as usize);
    let down = |keys: &[Key]| keys.iter().any(|&k| display.key_down(k));
    let ctrl = down(&[Key::LeftCtrl, Key::RightCtrl, Key::LeftSuper, Key::RightSuper]);
    let shift = down(&[Key::LeftShift, Key::RightShift]);
    let cursor = display.cursor_position();
    let over_panel = cursor.is_some_and(|(x, _)| app.editor.over_panel(x, w, h));
    if display.key_pressed(Key::F5) {
        app.editor.wire = !app.editor.wire;
    }
    if display.key_pressed(Key::F6) {
        if app.editor.view == editor::ViewMode::Perspective {
            app.editor.ortho_center = app.camera.position;
        }
        app.editor.view = app.editor.view.next();
        app.editor.hover = None;
    }
    if !ctrl && display.key_pressed(Key::V) {
        app.editor.vertices = !app.editor.vertices;
        app.editor.hover = None;
    }
    let ortho = app.editor.ortho(w, h);
    let camera = app.camera.view();
    let projection = match &ortho {
        Some(o) => wire::Projection::Ortho(o),
        None => wire::Projection::Perspective(&camera),
    };
    app.editor.anchor = match &ortho {
        Some(o) => o.center,
        None => app.camera.position + app.camera.forward() * 3.0,
    };
    app.editor.pointer = match (&ortho, cursor) {
        (Some(o), Some((x, y))) => Some(o.to_world(x, y)),
        _ => None,
    };
    if app.editor.model.is_some() {
        model_input(app, display, &projection, ortho.is_some(), cursor, over_panel, (ctrl, shift));
        return false;
    }
    app.editor.hover = match cursor {
        Some((x, y)) if !over_panel => {
            let at = glam::Vec2::new(x, y);
            if app.editor.vertices {
                app.editor.pick_vertex(&projection, at)
            } else if let Some(marker) = app.editor.pick_marker(&projection, at) {
                Some(marker)
            } else if let Some(o) = &ortho {
                app.editor.pick_2d(o, &app.world, at)
            } else {
                app.geometry
                    .pick(x, y)
                    .and_then(|(source, _)| app.editor.selection_of(source))
            }
        }
        _ => None,
    };
    // The panel row under the pointer, and what it changes.
    let row_field = cursor
        .filter(|_| over_panel)
        .and_then(|(x, y)| app.editor.row_at(x, y, w, h))
        .and_then(|k| app.editor.panel()[k].field);
    if display.mouse_clicked(MouseButton::Left) {
        if over_panel {
            if let Some(field) = row_field {
                app.edit(field, 1.0);
            }
        } else if app.editor.cutting.is_some() {
            match (app.editor.pointer, cursor) {
                (Some(p), Some(_)) => {
                    if app.editor.cut_point(p) {
                        app.edit(Field::Cut, 1.0);
                    }
                }
                _ => app.editor.say("cut in a 2D view (F6)"),
            }
        } else if cursor.is_some() {
            app.editor.selection = app.editor.hover;
        }
    }
    let scroll = display.scroll();
    if scroll != 0.0
        && let Some(field) = row_field
        && !matches!(field, Field::Duplicate | Field::Delete)
    {
        app.edit(field, scroll.signum());
    }
    // A 2D view zooms about the pointer.
    if scroll != 0.0
        && !over_panel
        && let (Some(o), Some((x, y))) = (&ortho, cursor)
    {
        let before = o.to_world(x, y);
        app.editor.ortho_scale = (app.editor.ortho_scale * 1.25f32.powf(scroll)).clamp(2.0, 2000.0);
        let after = app.editor.ortho(w, h).unwrap().to_world(x, y);
        app.editor.ortho_center += before - after;
    }
    if ctrl {
        if display.key_pressed(Key::Z) {
            app.travel(shift);
        }
        if display.key_pressed(Key::Y) {
            app.travel(true);
        }
        if display.key_pressed(Key::S) {
            app.save_level();
        }
        if display.key_pressed(Key::D) {
            app.edit(Field::Duplicate, 1.0);
        }
        if display.key_pressed(Key::C) {
            app.editor.copy();
        }
        if display.key_pressed(Key::V) {
            app.edit(Field::Paste, 1.0);
        }
        for (k, key) in [Key::Key1, Key::Key2, Key::Key3, Key::Key4].into_iter().enumerate() {
            if display.key_pressed(key) {
                let c = &app.camera;
                app.editor.bookmarks[k] = Some((c.position, c.yaw, c.pitch));
                app.editor.say(format!("bookmark {}: here", k + 1));
            }
        }
        return false;
    }
    for (k, key) in [Key::Key1, Key::Key2, Key::Key3, Key::Key4].into_iter().enumerate() {
        if display.key_pressed(key)
            && let Some((position, yaw, pitch)) = app.editor.bookmarks[k]
            && let Some(sector) = app.world.find_sector(position)
        {
            let c = &mut app.camera;
            (c.position, c.sector, c.yaw, c.pitch) = (position, sector, yaw, pitch);
            app.editor.ortho_center = position;
        }
    }
    if display.key_pressed(Key::M) {
        app.edit(Field::EditModel, 1.0);
    }
    if display.key_pressed(Key::G) {
        app.editor.step = (app.editor.step + 1) % editor::STEPS.len();
        let step = app.editor.step();
        app.editor.say(format!("grid {} m", moose_assets::number(step)));
    }
    if display.key_pressed(Key::Delete) || display.key_pressed(Key::Backspace) {
        app.edit(Field::Delete, 1.0);
    }
    if let Some(Selection::Surface(_)) = app.editor.selection {
        for (key, dir) in [(Key::PageUp, 1.0), (Key::PageDown, -1.0)] {
            if display.key_repeated(key) {
                app.edit(Field::Push, dir);
            }
        }
    }
    if !matches!(
        app.editor.selection,
        Some(Selection::Entity(_) | Selection::Vertex(_) | Selection::Light(_))
    ) {
        return false;
    }
    // Along the screen's axes in a 2D view; in the 3D view, along the world axes nearest
    // the view's forward and right.
    let axis_field = |v: Vec3| {
        let a = v.abs();
        if a.x >= a.y && a.x >= a.z {
            (Field::X, v.x.signum())
        } else if a.y >= a.z {
            (Field::Y, v.y.signum())
        } else {
            (Field::Z, v.z.signum())
        }
    };
    let (forward, right) = match &ortho {
        Some(o) => {
            let (right, up) = o.axis.basis();
            (axis_field(up), axis_field(right))
        }
        None => {
            let f = app.camera.forward();
            (axis_field(Vec3::new(f.x, 0.0, f.z)), axis_field(Vec3::new(-f.z, 0.0, f.x)))
        }
    };
    let moves = [
        (Key::Up, forward),
        (Key::Down, (forward.0, -forward.1)),
        (Key::Right, right),
        (Key::Left, (right.0, -right.1)),
        (Key::PageUp, (Field::Y, 1.0)),
        (Key::PageDown, (Field::Y, -1.0)),
        (Key::LeftBracket, (Field::Yaw, 1.0)),
        (Key::RightBracket, (Field::Yaw, -1.0)),
    ];
    for (key, (field, dir)) in moves {
        if display.key_repeated(key) {
            app.edit(field, dir);
        }
    }
    true
}

/// The mesh editor's mouse and keys (see [`edit_input`]): picking the model's polygons in
/// the 3D view, the panel, undo and saving.
fn model_input(
    app: &mut App,
    display: &Display,
    projection: &wire::Projection,
    two_d: bool,
    cursor: Option<(f32, f32)>,
    over_panel: bool,
    (ctrl, shift): (bool, bool),
) {
    use editor::Field;
    let (w, h) = (app.width as usize, app.height as usize);
    let model = app.editor.model.as_ref().unwrap();
    let hover = match cursor {
        Some((x, y)) if !over_panel && !two_d => {
            if model.proxies {
                model.pick_proxy(projection, app.camera.position, glam::Vec2::new(x, y), &app.world, &app.assets)
            } else {
                app.geometry.pick(x, y).and_then(|(source, _)| match source {
                    PolygonSource::Entity { entity, polygon } if entity as usize == model.entity => {
                        Some(polygon as usize)
                    }
                    _ => None,
                })
            }
        }
        _ => None,
    };
    app.editor.model.as_mut().unwrap().hover = hover;
    let row_field = cursor
        .filter(|_| over_panel)
        .and_then(|(x, y)| app.editor.row_at(x, y, w, h))
        .and_then(|k| app.editor.panel()[k].field);
    if display.mouse_clicked(MouseButton::Left) {
        if over_panel {
            if let Some(field) = row_field {
                app.edit(field, 1.0);
            }
        } else if two_d {
            app.editor.say("pick the model's polygons in the 3D view (F6)");
        } else if cursor.is_some() {
            app.editor.model.as_mut().unwrap().polygon = hover;
        }
    }
    let scroll = display.scroll();
    if scroll != 0.0
        && let Some(field) = row_field
        && !matches!(field, Field::Delete | Field::EditModel | Field::ProxyBox | Field::RemoveProxies)
    {
        app.edit(field, scroll.signum());
    }
    if ctrl {
        if display.key_pressed(Key::Z) {
            app.travel(shift);
        }
        if display.key_pressed(Key::Y) {
            app.travel(true);
        }
        if display.key_pressed(Key::S) {
            app.save_model();
        }
        return;
    }
    if display.key_pressed(Key::P) {
        app.edit(Field::PickProxies, 1.0);
    }
    if display.key_pressed(Key::M) {
        app.edit(Field::EditModel, 1.0);
    }
    if display.key_pressed(Key::Delete) || display.key_pressed(Key::Backspace) {
        app.edit(Field::Delete, 1.0);
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut options = parse_args()?;
    let mut app = App::new(&options)?;
    if let Some(at) = options.lock_flashlight {
        // The flashlight locked where it would be on a player standing there.
        let camera = app.camera.clone();
        app.place_camera(at, "--lock-flashlight")?;
        app.settings.flashlight_lock = Some(app.mount());
        app.camera = camera;
    }
    if let Some([x, y, z, dx, dy, dz]) = options.flashlight_at {
        let position = Vec3::new(x, y, z);
        let sector = app
            .world
            .find_sector(position)
            .ok_or("--flashlight-at is outside the level")?;
        app.settings.flashlight_lock = Some((sector, position, Vec3::new(dx, dy, dz).normalize()));
    }

    app.editor.on = options.edit;
    (app.editor.view, app.editor.wire) = (options.view, options.wire);
    if let Some(zoom) = options.zoom {
        app.editor.ortho_scale = zoom;
    }
    if let Some(name) = &options.select {
        let index = |n: &str| n.parse::<usize>().map_err(|_| "bad --select".to_string());
        app.editor.selection = Some(match name.split_once(':') {
            Some(("surface", n)) => editor::Selection::Surface(index(n)?),
            Some(("sector", n)) => editor::Selection::Sector(index(n)?),
            Some(("vertex", n)) => editor::Selection::Vertex(index(n)?),
            Some(("light", n)) => editor::Selection::Light(index(n)?),
            _ => editor::Selection::Entity(
                app.editor
                    .doc
                    .entities
                    .iter()
                    .position(|e| &e.name == name)
                    .ok_or(format!("--select: no entity '{name}'"))?,
            ),
        });
    }
    if let Some(polygon) = options.edit_model {
        app.edit(editor::Field::EditModel, 1.0);
        let model = app.editor.model.as_mut().ok_or("--edit-model: select an entity with a .mmdl model")?;
        model.polygon = polygon;
    }
    if let Some(path) = &options.screenshot {
        if let Some(at) = options.at {
            app.place_camera(at, "--at")?;
        }
        app.editor.ortho_center = app.camera.position;
        app.set_time(options.time);
        let (view_ms, raster_ms) = app.frame()?;
        let hud = app.settings.hud.then(|| app.hud(0.0, view_ms, raster_ms, 0.0));
        draw_ui(&mut app, options.menu.map(|page| (page, 0)), hud);
        app.save_png(Path::new(path))?;
        println!(
            "wrote {path} ({}x{}): view {view_ms:.3} ms, raster {raster_ms:.3} ms",
            app.width, app.height
        );
        return Ok(());
    }

    // The frame cap (the Rendering page's setting); `--fps 0` starts uncapped with the
    // default cap to switch to.
    let title = format!("Moose - {}", app.world.name);
    let s = &app.settings;
    let mut display =
        Display::open(&title, app.width, app.height, if s.capped { s.cap } else { 0 })?;
    display.set_fullscreen(options.fullscreen);
    let mut capped = s.capped;
    let mut shots = 0;
    let mut last = Instant::now();
    let started = last;
    let mut menu = Menu {
        open: false,
        page: Page::Main,
        selected: 0,
        back: Vec::new(),
    };
    // Frame rate and times, averaged over half a second for the HUD.
    let (mut stats_at, mut frames, mut view_sum, mut raster_sum) = (Instant::now(), 0u32, 0.0, 0.0);
    let mut stats = (0.0, 0.0, 0.0, 0.0);
    let mut present_sum = 0.0;
    // Pointer motion still being turned (see `MOUSE_SMOOTHING`): when its window starts,
    // and the motion, in pixels. And the time of this frame's start, in seconds.
    let mut look: Vec<(f64, (f32, f32))> = Vec::new();
    // When files were last looked at for changes (see `App::hot_reload`).
    let mut reload_at = Instant::now();
    app.hot_reload();
    let mut clock = 0.0f64;
    'frames: while display.is_open() {
        let now = Instant::now();
        let dt = (now - last).as_secs_f32().min(0.1);
        last = now;

        if display.key_pressed(Key::Escape) {
            // In the editor, Esc stops a cut, then drops the selection, first; in the mesh
            // editor, drops the polygon, then leaves it.
            if app.editor.on && !menu.open && let Some(model) = &mut app.editor.model {
                if model.polygon.is_some() {
                    model.polygon = None;
                } else {
                    app.edit(editor::Field::EditModel, 1.0);
                }
            } else if app.editor.on && !menu.open && app.editor.cutting.is_some() {
                app.editor.cutting = None;
                app.editor.say("cut stopped");
            } else if app.editor.on && !menu.open && app.editor.selection.is_some() {
                app.editor.selection = None;
            } else {
                menu.open = !menu.open;
                (menu.page, menu.selected) = (Page::Main, 0);
                menu.back.clear();
            }
        }
        if !menu.open && display.key_pressed(Key::Tab) {
            app.editor.on = !app.editor.on;
            app.editor.hover = None;
        }
        // Fullscreen: Alt+Enter (Option+Return on a Mac). F11 too, where the system
        // doesn't take it (macOS shows the desktop). Not Ctrl+Cmd+F: macOS takes that for
        // its own fullscreen, which fights ours and hangs the window.
        let alt = display.key_down(Key::LeftAlt) || display.key_down(Key::RightAlt);
        if (alt && display.key_pressed(Key::Enter)) || display.key_pressed(Key::F11) {
            display.set_fullscreen(!display.is_fullscreen());
        }
        // Mouse look while playing, and in the editor while the right button is held; a
        // free pointer otherwise.
        let looking = !menu.open && (!app.editor.on || display.mouse_down(MouseButton::Right));
        display.set_cursor_locked(looking);
        if menu.open {
            // The menu has the keys; the view holds still (the pointer's motion is dropped).
            display.mouse_delta();
            look.clear();
            let items = app.items(menu.page);
            if display.key_repeated(Key::Up) {
                menu.selected = (menu.selected + items.len() - 1) % items.len();
            }
            if display.key_repeated(Key::Down) {
                menu.selected = (menu.selected + 1) % items.len();
            }
            let item = items[menu.selected];
            let dir = if display.key_repeated(Key::Left) {
                -1
            } else if display.key_repeated(Key::Right) {
                1
            } else {
                0
            };
            if let (Item::Set(setting), true) = (item, dir != 0) {
                app.change(setting, dir);
            }
            if display.key_pressed(Key::Enter) && !alt {
                match item {
                    Item::Resume => menu.open = false,
                    Item::Respawn => {
                        app.reset();
                        menu.open = false;
                    }
                    Item::Quit => break 'frames,
                    Item::Open(page) => {
                        menu.back.push((menu.page, menu.selected));
                        (menu.page, menu.selected) = (page, 0);
                    }
                    Item::Set(setting) => app.change(setting, 1),
                    Item::Load(i) => {
                        // A new app on the level, with this one's settings.
                        let mut next = options.clone();
                        next.level = app.levels[i as usize].clone();
                        (next.at, next.lock_flashlight, next.flashlight_at) = (None, None, None);
                        match App::new(&next) {
                            Ok(mut loaded) => {
                                loaded.settings = Settings {
                                    flashlight_lock: None,
                                    ..app.settings
                                };
                                loaded.renderer.config = app.renderer.config;
                                loaded.geometry.config = app.geometry.config;
                                app = loaded;
                                options = next;
                                display.set_title(&format!("Moose - {}", app.world.name));
                                menu.open = false;
                                look.clear();
                            }
                            Err(e) => eprintln!("cannot load {}: {e}", next.level),
                        }
                    }
                }
            }
            if display.key_pressed(Key::Backspace) {
                match menu.back.pop() {
                    Some((page, selected)) => (menu.page, menu.selected) = (page, selected),
                    None => menu.open = false,
                }
            }
        } else {
            // The editor's mouse and keys; the arrows move its selection, if it has one.
            let editing = app.editor.on && edit_input(&mut app, &display);
            // A 2D view pans (dragged with the right button, or WASD), and the 3D camera
            // holds still.
            let two_d = app.editor.on && app.editor.view != editor::ViewMode::Perspective;
            if two_d {
                let (w, h) = (app.width as usize, app.height as usize);
                let o = app.editor.ortho(w, h).unwrap();
                let (right, up) = o.axis.basis();
                let (mx, my) = display.mouse_delta();
                let mut pan = if looking { (right * -mx + up * my) / o.scale } else { Vec3::ZERO };
                let key = |k| if display.key_down(k) { 1.0 } else { 0.0 };
                let ctrl = [Key::LeftCtrl, Key::RightCtrl, Key::LeftSuper, Key::RightSuper]
                    .into_iter()
                    .any(|k| display.key_down(k));
                if !ctrl {
                    let speed = 600.0 * dt / o.scale;
                    pan += (right * (key(Key::D) - key(Key::A)) + up * (key(Key::W) - key(Key::S))) * speed;
                }
                app.editor.ortho_center += pan;
                look.clear();
            }
            if !two_d {
                // Look.
                let turn = TURN_SPEED * dt;
                let c = &mut app.camera;
                if !editing {
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
                }
                if display.key_down(Key::Q) {
                    c.roll += ROLL_SPEED * dt;
                }
                if display.key_down(Key::E) {
                    c.roll -= ROLL_SPEED * dt;
                }
                // Mouse look is always on (no button), as in v1, smoothed over time: this
                // frame's motion (made since the last) is turned evenly over the window from
                // then on.
                let (mx, my) = display.mouse_delta();
                let (mx, my) = if looking { (mx, my) } else { (0.0, 0.0) };
                let window = MOUSE_SMOOTHING[app.settings.smoothing];
                let (t0, t1) = (clock, (now - started).as_secs_f64());
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
                // (Not with Ctrl: Ctrl+S saves in the editor.)
                let ctrl = [Key::LeftCtrl, Key::RightCtrl, Key::LeftSuper, Key::RightSuper]
                    .into_iter()
                    .any(|k| display.key_down(k));
                let key = |k| if display.key_down(k) && !ctrl { 1.0 } else { 0.0 };
                local.z -= key(Key::W) - key(Key::S);
                local.x += key(Key::D) - key(Key::A);
                let up = key(Key::Space) - key(Key::C);
                let fast = display.key_down(Key::LeftShift) || display.key_down(Key::RightShift);
                let speed = MOVE_SPEED * dt * if fast { FAST } else { 1.0 };
                let delta = (app.camera.rotation() * local + Vec3::Y * up) * speed;
                if delta != Vec3::ZERO {
                    app.fly(delta);
                }
            }
        }
        if reload_at.elapsed() >= Duration::from_secs(1) {
            reload_at = Instant::now();
            app.hot_reload();
        }
        // (While the menu is open too, so closing it doesn't turn by the time it was open.)
        clock = (now - started).as_secs_f64();
        if display.key_pressed(Key::F3) {
            app.settings.hud = !app.settings.hud;
        }
        if !menu.open && display.key_pressed(Key::U) {
            app.change(Setting::FlashlightMount, 1);
        }
        if app.settings.capped != capped {
            capped = app.settings.capped;
            display.set_max_fps(if capped { app.settings.cap } else { 0 });
        }

        let time = started.elapsed().as_secs_f32();
        if display.key_pressed(Key::F1) {
            println!("{}", app.command_line(&options, time));
        }
        app.set_time(time);
        let (view_ms, raster_ms) = app.frame()?;
        if display.key_pressed(Key::F12) {
            shots += 1;
            let path = format!("moose-{shots}.png");
            app.save_png(Path::new(&path))?;
            println!("saved {path}");
        }

        frames += 1;
        view_sum += view_ms;
        raster_sum += raster_ms;
        let elapsed = stats_at.elapsed().as_secs_f64();
        if elapsed >= 0.5 {
            let n = frames as f64;
            stats = (n / elapsed, view_sum / n, raster_sum / n, present_sum / n);
            (stats_at, frames, view_sum, raster_sum, present_sum) = (Instant::now(), 0, 0.0, 0.0, 0.0);
        }
        let hud = app.settings.hud.then(|| app.hud(stats.0, stats.1, stats.2, stats.3));
        draw_ui(&mut app, menu.open.then_some((menu.page, menu.selected)), hud);
        display.present(&app.pixels)?;
        let t = display.present_times();
        present_sum += t.copy + t.show + t.events;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small app on `level`, for tests.
    fn test_app(level: &str) -> App {
        let args = ["--level", level, "--size", "64x36"].map(String::from);
        App::new(&parse_options(args).unwrap()).unwrap()
    }

    #[test]
    fn editing_moves_an_entity_and_undo_brings_it_back() {
        use editor::{Field, Selection};
        let mut app = test_app("shiny_rooms.mmp");
        let row = app.editor.doc.entities.iter().position(|e| e.name == "crate_a3").unwrap();
        app.editor.selection = Some(Selection::Entity(row));
        let world_index = |app: &App| app.world.entities.iter().position(|e| e.name == "crate_a3").unwrap();
        let start = app.world.entities[world_index(&app)].position;
        // One grid step (0.25 m) along x: the level is rebuilt with it there.
        app.edit(Field::X, 1.0);
        let moved = app.world.entities[world_index(&app)].position;
        assert_eq!(moved, start + Vec3::X * 0.25);
        assert_eq!(app.editor.doc.entities[row].position, moved);
        assert!(app.editor.dirty());
        // Past the room's wall (x = 4; an origin on it counts as inside): refused, and
        // nothing changes.
        app.editor.step = editor::STEPS.len() - 1;
        for _ in 0..3 {
            app.edit(Field::X, 1.0);
        }
        let stopped = app.world.entities[world_index(&app)].position.x;
        assert!(stopped <= 4.0, "left the level at {stopped}");
        assert_eq!(app.editor.doc.entities[row].position.x, stopped);
        // Undo all the way back, then redo one.
        while app.editor.dirty() {
            app.travel(false);
        }
        assert_eq!(app.world.entities[world_index(&app)].position, start);
        app.travel(true);
        assert_eq!(app.world.entities[world_index(&app)].position, moved);
    }

    #[test]
    fn sector_tools_grow_cut_and_add_rooms() {
        use editor::{Field, Selection, ViewMode};
        let mut app = test_app("two_rooms.mmp");
        let sectors = app.world.sectors.len();
        // room_a's back wall (z = 8, facing into the room) extruded 2 m: a new sector,
        // with its far end selected.
        let back = app
            .editor
            .doc
            .sector_surfaces(0)
            .find(|&i| {
                app.editor.doc.surfaces[i].adjoin.is_none()
                    && app.editor.doc.surface_normal(i).dot(Vec3::NEG_Z) > 0.99
            })
            .unwrap();
        app.editor.selection = Some(Selection::Surface(back));
        app.edit(Field::Extrude, 1.0);
        assert_eq!(app.world.sectors.len(), sectors + 1);
        assert!(matches!(app.editor.selection, Some(Selection::Surface(i)) if i != back));
        // Cut room_a in two in the top view, along x = 0.
        app.editor.selection = Some(Selection::Sector(0));
        app.editor.view = ViewMode::Ortho(wire::Axis::Top);
        app.edit(Field::Cleave, 1.0);
        assert!(!app.editor.cut_point(Vec3::new(0.0, 1.0, -5.0)));
        assert!(app.editor.cut_point(Vec3::new(0.0, 1.0, 20.0)));
        app.edit(Field::Cut, 1.0);
        assert_eq!(app.world.sectors.len(), sectors + 2);
        // A room of its own, away from the level.
        app.editor.selection = None;
        app.editor.anchor = Vec3::new(30.0, 1.7, 30.0);
        app.edit(Field::NewRoom, 1.0);
        assert_eq!(app.world.sectors.len(), sectors + 3);
        assert!(matches!(app.editor.selection, Some(Selection::Sector(_))));
        // All undone.
        while app.editor.dirty() {
            app.travel(false);
        }
        assert_eq!(app.world.sectors.len(), sectors);
    }

    #[test]
    fn animated_models_move_and_the_editor_picks_their_animation() {
        use editor::{Field, Selection};
        let mut app = test_app("walker_rooms.mmp");
        let walker = |app: &App| app.world.entities.iter().position(|e| e.name == "walker").unwrap();
        // Posed in a copy of its model, which moves on with time.
        app.set_time(0.0);
        let e = &app.world.entities[walker(&app)];
        assert_ne!(e.mesh, e.model);
        let at = |app: &App| app.assets.mesh(app.world.entities[walker(app)].mesh).positions.clone();
        let start = at(&app);
        app.set_time(0.25);
        assert_ne!(at(&app), start);
        // Rebuilds reuse the copy.
        let row = app.editor.doc.entities.iter().position(|e| e.name == "walker").unwrap();
        app.editor.selection = Some(Selection::Entity(row));
        let label = |app: &App| {
            app.editor.panel().into_iter().find(|r| r.label == "Animation").map(|r| r.value)
        };
        assert_eq!(label(&app).as_deref(), Some("walk"));
        let meshes = app.assets.meshes().len();
        app.edit(Field::Animation, 1.0);
        assert_eq!(label(&app).as_deref(), Some("idle"));
        app.edit(Field::Animation, 1.0);
        assert_eq!(label(&app).as_deref(), Some("none"));
        let e = &app.world.entities[walker(&app)];
        assert_eq!(e.mesh, e.model, "not animated: its model as it is");
        app.edit(Field::Animation, 1.0);
        assert_eq!(label(&app).as_deref(), Some("walk"));
        // One new level mesh per rebuild, and no more copies.
        assert_eq!(app.assets.meshes().len(), meshes + 3);
    }

    #[test]
    fn the_mesh_editor_sets_proxies_and_drops_unsaved_changes() {
        use editor::{Field, Selection};
        let mut app = test_app("walker_rooms.mmp");
        let row = app.editor.doc.entities.iter().position(|e| e.name == "walker").unwrap();
        app.editor.selection = Some(Selection::Entity(row));
        assert!(app.editor.panel().iter().any(|r| r.field == Some(Field::EditModel)));
        app.edit(Field::EditModel, 1.0);
        assert!(app.editor.model.is_some(), "opened on the walker");
        let model_id = app.assets.mesh_id("walker.mmdl").unwrap();
        let original = app.assets.mesh(model_id).clone();
        let proxies = |app: &App| app.assets.mesh(model_id).polygons.iter().filter(|p| p.flags.proxy()).count();
        // The first polygon (the hips' box, drawn) made a proxy: the model and the walker's
        // posed copy have it.
        app.edit(Field::PolygonPick, 1.0);
        assert_eq!(app.editor.model.as_ref().unwrap().polygon, Some(0));
        app.edit(Field::Proxy, 1.0);
        assert_eq!(proxies(&app), 43);
        let walker = app.world.entities.iter().find(|e| e.name == "walker").unwrap();
        assert!(app.assets.mesh(walker.mesh).polygons[0].flags.proxy());
        // Undone; then the hips' proxies removed and a box put back around them.
        app.travel(false);
        assert_eq!(proxies(&app), 42);
        app.edit(Field::RemoveProxies, 1.0);
        assert_eq!(proxies(&app), 36);
        app.edit(Field::PolygonPick, 1.0);
        app.edit(Field::ProxyBox, 1.0);
        assert_eq!(proxies(&app), 42);
        // Leaving asks first, then drops the changes: the model is as on disk.
        app.edit(Field::EditModel, 1.0);
        assert!(app.editor.model.is_some(), "refused once: unsaved changes");
        app.edit(Field::EditModel, 1.0);
        assert!(app.editor.model.is_none());
        assert_eq!(app.assets.mesh(model_id), &original);
        // Saving writes the tables (here to a scratch file).
        app.edit(Field::EditModel, 1.0);
        let path = std::env::temp_dir().join("moose_mesh_editor_test.mmdl");
        let model = app.editor.model.as_mut().unwrap();
        model.path = path.clone();
        model.polygon = Some(1);
        app.edit(Field::Proxy, 1.0);
        app.save_model();
        assert!(!app.editor.model.as_ref().unwrap().dirty());
        let saved = ModelDoc::parse(&path, &std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(saved.polygons[1].proxy());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn a_vertex_that_would_bend_a_surface_stays() {
        use editor::{Field, Selection};
        let mut app = test_app("two_rooms.mmp");
        // Vertex 0 is a corner of room_a's floor (-4, 0, 8): raised, the floor bends.
        app.editor.selection = Some(Selection::Vertex(0));
        let before = app.editor.doc.clone();
        app.edit(Field::Y, 1.0);
        assert_eq!(app.editor.doc, before, "refused: the floor isn't flat any more");
    }

    #[test]
    fn lights_and_things_are_placed_and_changed() {
        use editor::{Field, Selection};
        let mut app = test_app("two_rooms.mmp");
        let (lights, entities, spawns) = (
            app.lights.0.len(),
            app.world.entities.len(),
            app.world.spawn_points.len(),
        );
        // In the middle of room_a.
        app.editor.anchor = Vec3::new(0.0, 2.0, 4.0);
        for field in [Field::AddLight, Field::AddProp, Field::AddSpawn] {
            app.editor.selection = None;
            app.edit(field, 1.0);
        }
        assert_eq!(app.lights.0.len(), lights + 1);
        assert_eq!(app.world.entities.len(), entities + 1);
        assert_eq!(app.world.spawn_points.len(), spawns + 1);
        // The light: moved down the hallway it follows into, made a spot, aimed.
        let light = app.editor.doc.lights.len() - 1;
        app.editor.selection = Some(Selection::Light(light));
        app.editor.step = editor::STEPS.len() - 1; // 1 m
        for _ in 0..6 {
            app.edit(Field::Z, -1.0);
        }
        let hallway = app.editor.doc.sectors.iter().position(|s| s.name == "hallway").unwrap();
        assert_eq!(app.editor.doc.lights[light].sector, hallway);
        app.edit(Field::Spot, 1.0);
        app.edit(Field::Pitch, 1.0);
        let (dir, _, _) = app.editor.doc.lights[light].spot.unwrap();
        assert!(dir.y > -1.0 + 1e-3, "aimed up from straight down");
        assert!(app.lights.0.iter().any(|l| !l.is_point()));
        app.edit(Field::Delete, 1.0);
        assert_eq!(app.lights.0.len(), lights);
        // The level's ambient light.
        app.editor.selection = None;
        app.edit(Field::AmbientRed, 5.0);
        assert!(app.world.ambient.x > app.world.ambient.y);
    }

    #[test]
    fn the_sun_turns() {
        use editor::Field;
        let mut app = test_app("sunny_rooms.mmp");
        let before = app.editor.doc.directional[0].direction;
        app.editor.selection = None;
        app.edit(Field::SunYaw, 1.0);
        app.edit(Field::SunPitch, 1.0);
        let after = app.editor.doc.directional[0].direction;
        assert!(before.normalize().angle_between(after.normalize()) > 0.1);
        assert!(app.lights.0.iter().any(|l| l.directional && l.direction.angle_between(after.normalize()) < 1e-3));
    }

    #[test]
    fn textures_move_on_a_surface_and_map_flat_again() {
        use editor::{Field, Selection};
        let mut app = test_app("shiny_rooms.mmp");
        let uv = app.editor.doc.attributes.iter().position(|a| a.name == "uv").unwrap();
        let wall = app
            .editor
            .doc
            .surfaces
            .iter()
            .position(|s| s.adjoin.is_none() && s.corners.len() == 4)
            .unwrap();
        let uvs = |app: &App| -> Vec<Vec<String>> {
            app.editor.doc.surfaces[wall]
                .corners
                .iter()
                .map(|(_, rows)| app.editor.doc.attributes[uv].values[rows[uv]].clone())
                .collect()
        };
        let original = uvs(&app);
        app.editor.selection = Some(Selection::Surface(wall));
        app.edit(Field::TextureU, 1.0);
        let shifted = uvs(&app);
        assert_ne!(shifted, original);
        // Mapped flat again: as the level was made.
        app.edit(Field::TextureProject, 1.0);
        let flat: Vec<Vec<f32>> = uvs(&app)
            .iter()
            .map(|row| row.iter().map(|t| t.parse().unwrap()).collect())
            .collect();
        let made: Vec<Vec<f32>> = original
            .iter()
            .map(|row| row.iter().map(|t| t.parse().unwrap()).collect())
            .collect();
        assert_eq!(flat, made);
    }

    #[test]
    fn copies_paste_where_the_view_is() {
        use editor::{Field, Selection};
        let mut app = test_app("two_rooms.mmp");
        let crate_row = app
            .editor
            .doc
            .entities
            .iter()
            .position(|e| e.kind == moose_assets::EntityKind::Prop)
            .unwrap();
        app.editor.selection = Some(Selection::Entity(crate_row));
        app.editor.copy();
        let count = app.world.entities.len();
        app.editor.anchor = Vec3::new(1.0, 0.0, 5.0);
        app.edit(Field::Paste, 1.0);
        assert_eq!(app.world.entities.len(), count + 1);
        assert!(app.world.entities.iter().any(|e| e.position == Vec3::new(1.0, 0.0, 5.0)));
    }

    #[test]
    fn a_refused_edit_points_at_the_problem() {
        use editor::{Field, Selection};
        let mut app = test_app("two_rooms.mmp");
        // Raising a floor corner bends the floor: the loader names a surface.
        app.editor.selection = Some(Selection::Vertex(0));
        app.editor.find_problem("");
        app.edit(Field::Y, 1.0);
        let why = format!("{:?}", app.editor.problem_for_tests());
        assert!(why.contains("Surface"), "{why}");
    }

    #[test]
    fn a_level_needs_a_spawn_point() {
        use editor::{Field, Selection};
        let mut app = test_app("two_rooms.mmp");
        let spawn = app.editor.doc.entities.iter().position(|e| e.kind == moose_assets::EntityKind::Spawn).unwrap();
        app.editor.selection = Some(Selection::Entity(spawn));
        app.edit(Field::Delete, 1.0);
        assert!(app.editor.doc.entities.iter().any(|e| e.kind == moose_assets::EntityKind::Spawn));
        assert!(!app.editor.dirty());
    }

    #[test]
    fn duplicates_get_their_own_names() {
        use editor::{Field, Selection};
        let mut app = test_app("two_rooms.mmp");
        let row = app.editor.doc.entities.iter().position(|e| e.kind != moose_assets::EntityKind::Spawn).unwrap();
        let name = app.editor.doc.entities[row].name.clone();
        let count = app.world.entities.len();
        app.editor.selection = Some(Selection::Entity(row));
        app.edit(Field::Duplicate, 1.0);
        app.edit(Field::Duplicate, 1.0);
        assert_eq!(app.world.entities.len(), count + 2);
        let names: std::collections::HashSet<_> = app.editor.doc.entities.iter().map(|e| e.name.clone()).collect();
        assert_eq!(names.len(), app.editor.doc.entities.len(), "names are unique");
        assert!(names.contains(&format!("{}_2", name.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('_'))));
    }

    #[test]
    fn menu_values_step_and_wrap() {
        let list = [0.0, 0.5, 1.0, 2.0];
        assert_eq!(cycle(&list, 0.5, 1), 1.0);
        assert_eq!(cycle(&list, 0.5, -1), 0.0);
        assert_eq!(cycle(&list, 2.0, 1), 0.0);
        assert_eq!(cycle(&list, 0.0, -1), 2.0);
        // A value not in the list goes to the nearest one that way.
        assert_eq!(cycle(&list, 0.7, 1), 1.0);
        assert_eq!(cycle(&list, 0.7, -1), 0.5);
        assert_eq!(cycle(&list, 5.0, 1), 0.0);
        // Infinity (the steep limit's "off") is found too.
        assert_eq!(cycle(&STEEP_LIMITS, f32::INFINITY, 1), STEEP_LIMITS[0]);
        assert_eq!(cycle(&STEEP_LIMITS, f32::INFINITY, -1), 1.0);
    }

    #[test]
    fn the_level_list_finds_the_levels() {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        let levels = level_files(root);
        for name in ["shiny_rooms.mmp", "sunny_rooms.mmp", "two_rooms.mmp"] {
            assert!(levels.iter().any(|l| l == name), "{name} in {levels:?}");
        }
        assert!(levels.is_sorted());
    }

    #[test]
    fn every_page_is_reachable_and_named() {
        for page in Page::ALL {
            assert_eq!(Page::named(page.name()), Some(page));
            assert!(page == Page::Levels || !page.items().is_empty());
            if page != Page::Main {
                assert!(Page::Main.items().contains(&Item::Open(page)), "{page:?}");
            }
        }
    }
}
