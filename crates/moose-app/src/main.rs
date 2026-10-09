//! The Moose test app: fly through a level.
//!
//! cargo run --release -p moose-app -- [options]
//!
//! Options (all but the screenshot ones can also be changed in the options menu, Esc):
//!   --level NAME          level in assets/levels (default shiny_rooms.mmp)
//!   --size WxH            framebuffer size (default 1280x720)
//!   --bump SHADER         bump mapping, for materials that have it: normal (the default)
//!                         or off
//!   --specular ON         specular highlights, for materials that have them: on (the
//!                         default) or off
//!   --sky STYLE           sky materials: box (the default) or flat (their simple_sky
//!                         variant)
//!   --bloom MODE          glow around light past white: sun (the sun's, the default),
//!                         bright (and any near-white pixel's), or off
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
//!   --softness K          every light's source size (its shadows' softness) times K: its
//!                         own size at 1 (the default), hard shadows at 0
//!   --dynamic-softness K  the same for shadows carved every frame, dynamic lights' (the
//!                         flashlight's) and moving occluders', at most --softness (default 1)
//!   --no-shadow-cache     carve static lights' shadows every frame (to compare)
//!   --blur-scale N        pixels per cell of the grid blurred shadows are blurred on, each
//!                         way (default 8; it must divide 8, the rows per band)
//!   --blur-half-rate      blur grid cells twice as wide as they are tall
//!   --no-dynamic-shadows  only baked shadows: none from the flashlight or moving lights,
//!                         moving occluders, or on moving surfaces (no carving per frame)
//!   --hud, --menu PAGE    for --screenshot: draw the debug HUD, or a menu page (main,
//!                         lighting, flashlight, rendering, sampling, controls), over it
//!   --flashlight-fade D   how wide the flashlight's cone fades, in degrees, inside its 20
//!                         degree edge (default 8; 0 is a hard edge)
//!   --dither-beam         draw the flashlight beam's fade as a stipple (an ordered dither)
//!   --level-lights        start with the level's own lights on (off by default, leaving the
//!                         flashlight and the ambient light)
//!   --time T              seconds into the water's animation, for --screenshot
//!   --bench N             for --screenshot: render the frame N more times and print the
//!                         average view and raster times (for measuring)
//!   --views FILE          for --screenshot: first render a frame from each camera in FILE
//!                         (one X,Y,Z,YAW,PITCH[,ROLL] a line), printing each before it's
//!                         drawn (for sweeping views for crashes)
//!   --show-shadow-mesh    overlay the shadow pieces' outlines (F4; see `draw_shadow_mesh`)
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
//! reproduces this view, F3 debug HUD, F4 shadow mesh, Alt+Enter fullscreen, F12 screenshot. Every other setting
//! is in the menu, where B on a setting binds a key to it (pressed while playing, it does
//! what Enter does there; kept in ~/.config/moose/keys.txt), and Delete unbinds it. While playing, the cursor is locked (hidden) for mouse look; in the menu
//! it is free.

mod editor;
mod material_edit;
mod materials;
mod mesh_edit;
mod ui;
mod wire;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use glam::Vec3;
use moose_assets::{Assets, LevelDoc, Light, MATERIAL_SLOTS, MaterialLibrary, ModelDoc, Ripples, Texture, TextureId};
use moose_present::{Display, Key, MouseButton};
use materials::{FrameSettings, Materials, ShaderIds};
use moose_raster::post::{Bloom, BloomConfig};
use moose_raster::{Params, RasterConfig, RasterPath, Renderer, Surface, Target};
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

/// Heights the Resolution setting steps through (widths from the starting size's shape).
const HEIGHTS: [u32; 8] = [360, 480, 540, 720, 900, 1080, 1440, 2160];
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
/// Multiples of every light's own source size the menu steps through, for all shadows and
/// again for those carved every frame (`--softness`, `--dynamic-softness`): shadows soften
/// over the part of the source an occluder covers; 0 casts hard shadows, 1 is the light's
/// own size. Each light's size is its own: a level light's `radius=`, the sun's angle in
/// the level, the flashlight's `FLASHLIGHT_RADIUS`.
const SOFTNESS: [f32; 5] = [0.0, 1.0, 2.0, 4.0, 6.0];
/// The radius of the flashlight's source, in meters.
const FLASHLIGHT_RADIUS: f32 = 0.05;

/// Steep surface limits the menu steps through (see `RasterConfig::steep_limit`).
const STEEP_LIMITS: [f32; 5] = [0.125, 0.25, 0.5, 1.0, f32::INFINITY];

/// How much wider the sun's disk is drawn than its light's angle (the real sun's 0.53
/// degrees is two pixels across at 720p).
const SUN_DISK_SCALE: f32 = 2.5;
/// Bloom: how strong the glow is, of light past white alone and with the bright pass, and
/// where the bright pass starts (the brightest channel, 0 to 1).
const BLOOM_STRENGTH: f32 = 1.0;
const BLOOM_BRIGHT_STRENGTH: f32 = 0.45;
const BLOOM_THRESHOLD: f32 = 0.85;
/// Entities drawn with a material that reads their own cube map (`@cube`, mirror balls)
/// get one baked at load, of their surroundings, faces this many texels across.
const CUBE_SIZE: u32 = 128;

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
    bump: Bump,
    specular: bool,
    sky: SkyStyle,
    bloom: BloomMode,
    water: bool,
    no_flashlight: bool,
    fullscreen: bool,
    cone: Cone,
    level_lights: bool,
    no_shadows: bool,
    no_sun: bool,
    softness: f32,
    dynamic_softness: f32,
    /// For screenshots: draw the debug HUD, or a menu page, over the frame.
    hud: bool,
    menu: Option<Page>,
    no_shadow_cache: bool,
    no_dynamic_shadows: bool,
    blur_scale: Option<u32>,
    blur_half_rate: bool,
    flashlight_fade: f32,
    time: f32,
    /// For --screenshot: render the frame this many times more, and report the average.
    bench: u32,
    /// For --screenshot: cameras to render a frame from first, one a line.
    views: Option<String>,
    show_samples: bool,
    /// Draw the shadow pieces' outlines over the frame (F4).
    show_shadow_mesh: bool,
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
        bump: Bump::Normal,
        sky: SkyStyle::Box,
        bloom: BloomMode::Sun,
        specular: true,
        water: false,
        no_flashlight: false,
        fullscreen: false,
        cone: Cone::Beam,
        no_shadows: false,
        no_sun: false,
        softness: 1.0,
        dynamic_softness: 1.0,
        hud: false,
        menu: None,
        no_shadow_cache: false,
        no_dynamic_shadows: false,
        blur_scale: None,
        blur_half_rate: false,
        flashlight_fade: FLASHLIGHT_FADES[3],
        level_lights: false,
        time: 0.0,
        bench: 0,
        views: None,
        show_samples: false,
        show_shadow_mesh: false,
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
            "--bump" => {
                let name = value()?;
                o.bump = Bump::ALL.into_iter().find(|b| b.name() == name).ok_or_else(|| {
                    format!("--bump is one of {}", Bump::ALL.map(Bump::name).join(", "))
                })?;
            }
            "--sky" => {
                let name = value()?;
                o.sky = SkyStyle::ALL.into_iter().find(|v| v.name() == name).ok_or_else(|| {
                    format!("--sky is one of {}", SkyStyle::ALL.map(SkyStyle::name).join(", "))
                })?;
            }
            "--bloom" => {
                let name = value()?;
                o.bloom = BloomMode::ALL.into_iter().find(|v| v.name() == name).ok_or_else(|| {
                    format!("--bloom is one of {}", BloomMode::ALL.map(BloomMode::name).join(", "))
                })?;
            }
            "--specular" => {
                o.specular = match value()?.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err("--specular is on or off".into()),
                }
            }
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
            "--softness" => o.softness = value()?.parse().map_err(|_| "bad --softness")?,
            "--dynamic-softness" => {
                o.dynamic_softness = value()?.parse().map_err(|_| "bad --dynamic-softness")?
            }
            "--no-shadow-cache" => o.no_shadow_cache = true,
            "--no-dynamic-shadows" => o.no_dynamic_shadows = true,
            "--blur-scale" => o.blur_scale = Some(value()?.parse().map_err(|_| "bad --blur-scale")?),
            "--blur-half-rate" => o.blur_half_rate = true,
            "--flashlight-fade" => {
                o.flashlight_fade = value()?.parse().map_err(|_| "bad --flashlight-fade")?
            }
            "--level-lights" => o.level_lights = true,
            "--show-samples" => o.show_samples = true,
            "--show-shadow-mesh" => o.show_shadow_mesh = true,
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
            "--bench" => o.bench = value()?.parse().map_err(|_| "bad --bench")?,
            "--views" => o.views = Some(value()?),
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
    /// Bump mapping and specular highlights, for materials that have them (their
    /// features).
    bump: Bump,
    specular: bool,
    /// Sky materials draw their `simple_sky` variant when flat.
    sky: SkyStyle,
    bloom: BloomMode,
    /// Materials draw their `water` variant (shiny floors are water).
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
    /// Every light's source size, as a multiple of its own (one of `SOFTNESS`).
    softness: f32,
    /// The same for shadows carved every frame (dynamic lights' and moving occluders'), at
    /// most `softness`: hard wherever that is.
    dynamic_softness: f32,
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
    /// The shadow pieces' outlines are drawn over the frame; F4 toggles.
    shadow_mesh: bool,
}

/// A mirror ball's baked surroundings.
#[derive(Clone, Copy)]
struct CubeMap {
    texture: TextureId,
    /// The ball's radius, for picking the cube map's level of detail.
    radius: f32,
}

/// Bump mapping, for materials with a `bump` feature (see `moose_raster::shaders::textured_lit`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Bump {
    Off,
    /// From the material's normal map.
    Normal,
}

impl Bump {
    const ALL: [Bump; 2] = [Bump::Off, Bump::Normal];

    fn name(self) -> &'static str {
        match self {
            Bump::Off => "off",
            Bump::Normal => "normal",
        }
    }
}

/// How sky surfaces are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SkyStyle {
    /// Their vertex colors, unlit.
    Flat,
    /// The sky cube map (`SKY_FACES`) seen from the eye, with the sun in it: `SkyBox`.
    Box,
}

impl SkyStyle {
    const ALL: [SkyStyle; 2] = [SkyStyle::Box, SkyStyle::Flat];

    fn name(self) -> &'static str {
        match self {
            SkyStyle::Flat => "flat",
            SkyStyle::Box => "box",
        }
    }
}

/// What glows (see `moose_raster::post::Bloom`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BloomMode {
    Off,
    /// Light past white: the sun's.
    Sun,
    /// That, and any pixel's brightest channel past `BLOOM_THRESHOLD`.
    Bright,
}

impl BloomMode {
    const ALL: [BloomMode; 3] = [BloomMode::Off, BloomMode::Sun, BloomMode::Bright];

    fn name(self) -> &'static str {
        match self {
            BloomMode::Off => "off",
            BloomMode::Sun => "sun",
            BloomMode::Bright => "bright",
        }
    }
}



struct App {
    assets: Assets,
    world: World,
    /// The level's lights and ambient light, for switching them back on (K).
    lights: (Vec<Light>, Vec3),
    camera: Camera,
    geometry: ViewGeometry,
    renderer: Renderer,
    /// The shaders materials can name, and the materials (`assets/materials`), checked
    /// and with their textures loaded.
    shaders: ShaderIds,
    materials: Materials,
    /// Sampler overrides (`filterN=`) of the level's bindings, and of each entity's.
    level_filters: Vec<[Option<u8>; MATERIAL_SLOTS]>,
    entity_filters: Vec<[Option<u8>; MATERIAL_SLOTS]>,
    /// The water's ripples, which the rippling textures (`@water:FILE`) follow.
    ripples: Ripples,
    /// Hotkeys: each key does what Enter does on its setting in the menu (see
    /// `App::bind`), kept in `bindings_path()`.
    bindings: Vec<(Key, Setting)>,
    /// A note shown at the bottom of the screen for a moment, and when it was made.
    toast: Option<(String, Instant)>,
    /// The bloom pass, and how long it took last frame (ms).
    bloom: Bloom,
    post_ms: f64,
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
    /// The material being made or changed on the Material page (Esc > Materials).
    material: Option<material_edit::Draft>,
    /// Seconds since the start: for the water's ripples and moving lights.
    time: f32,
    pixels: Vec<u32>,
    width: u32,
    height: u32,
    /// The size the app started at (`--size`): its shape is the Resolution setting's.
    start_size: (u32, u32),
}

impl App {
    fn new(options: &Options) -> Result<App, String> {
        App::open(options, None)
    }

    /// The app on `options.level`: its file in assets/levels, or `src` (a new level, not
    /// saved yet, to be saved there).
    fn open(options: &Options, src: Option<String>) -> Result<App, String> {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
        let mut assets = Assets::new(root);
        assets.load_materials().map_err(|e| e.to_string())?;
        let level_path = Path::new(root).join("levels").join(&options.level);
        let level_src = match src {
            Some(src) => src,
            None => std::fs::read_to_string(&level_path).map_err(|e| format!("{}: {e}", level_path.display()))?,
        };
        let level = assets
            .parse_level(&options.level, &level_src)
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
            blur_scale: options.blur_scale.unwrap_or(defaults.blur_scale),
            blur_half_rate: options.blur_half_rate,
            min_step: options.min_step.unwrap_or(defaults.min_step),
            light_spacing: options.light_spacing.unwrap_or(defaults.light_spacing),
            steep_limit: options.steep_limit.unwrap_or(defaults.steep_limit),
            step_threshold: options.step_threshold.unwrap_or(defaults.step_threshold),
            penumbra_threshold: options
                .penumbra_threshold
                .unwrap_or(defaults.penumbra_threshold),
            ..defaults
        });
        let shaders = ShaderIds::register(&mut renderer);
        let ripples = Ripples::new(1);
        let materials = Materials::compile(&mut assets, &ripples)?;
        let materials_names = materials.names().to_vec();
        let (level_filters, entity_filters) = binding_filters_of(&world)?;
        let cube_maps = vec![None; world.entities.len()];
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
            shaders,
            materials,
            level_filters,
            entity_filters,
            ripples,
            bindings: load_bindings(),
            toast: None,
            bloom: Bloom::default(),
            post_ms: 0.0,
            cube_maps,
            spare_cubes: Vec::new(),
            editor: {
                let mut editor = editor::Editor::new(doc, level_path);
                editor.materials = materials_names.clone();
                editor
            },
            models: model_files(root),
            watched: std::collections::HashMap::new(),
            settings: Settings {
                translucent_crates: options.translucent_crates,
                per_pixel_crates: options.per_pixel_crates,
                reflectance: options.f0.clamp(0.0, 1.0),
                fade_range: options.fade.max(0.0),
                bump: options.bump,
                sky: options.sky,
                bloom: options.bloom,
                specular: options.specular,
                water: options.water,
                lit: !options.unlit,
                penumbra: options.penumbra.clamp(1.0 / 64.0, 64.0),
                flashlight: !options.no_flashlight,
                level_lights: true,
                shadows: !options.no_shadows,
                sun: !options.no_sun,
                softness: options.softness.max(0.0),
                dynamic_softness: options.dynamic_softness.max(0.0),
                flashlight_fade: options.flashlight_fade.clamp(0.0, FLASHLIGHT_OUTER),
                cone: options.cone,
                flashlight_lock: None,
                smoothing: 2,
                capped: options.fps != 0,
                cap: if options.fps == 0 { MAX_FPS } else { options.fps },
                hud: options.hud || options.screenshot.is_none(),
                shadow_mesh: options.show_shadow_mesh,
            },
            time: 0.0,
            levels: level_files(root),
            level: options.level.clone(),
            material: None,
            pixels: vec![0; (options.width * options.height) as usize],
            width: options.width,
            height: options.height,
            start_size: (options.width, options.height),
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
        (self.level_filters, self.entity_filters) = binding_filters_of(&self.world)?;
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

    /// Sets editor field `field` to the number typed, `text` (a unit after it, `m` or `°`,
    /// is let be), as an edit: undone in one step, refused if the level refuses it.
    fn type_value(&mut self, field: editor::Field, text: &str) {
        let number = text.trim().trim_end_matches(['m', '°']).trim();
        if number.is_empty() {
            self.editor.say("not set");
            return;
        }
        let Some(value) = number.parse::<f32>().ok().filter(|v| v.is_finite()) else {
            self.editor.say(format!("'{number}' is not a number"));
            return;
        };
        let before = self.editor.begin();
        let result = self.editor.set_typed(field, value).and_then(|what| self.rebuild().map(|()| what));
        match result {
            Ok(what) => self.editor.commit(before, &what),
            Err(why) => self.editor.revert(before, &why),
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
        let doc = &self.editor.doc;
        let Some(file) = doc.model_file(&doc.entities[row]).filter(|m| m.ends_with(".mmdl")).map(String::from) else {
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

    /// Reloads what changed on disk since the last look: the materials, models and
    /// textures (then rebuilds the level, as entities' shapes may have changed), and the
    /// level's own file unless the editor has changes of its own.
    fn hot_reload(&mut self) {
        let root = self.assets.root().to_path_buf();
        let material_files: Vec<PathBuf> = std::fs::read_dir(root.join("materials"))
            .map(|entries| {
                entries
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().is_some_and(|x| x == "mmat"))
                    .collect()
            })
            .unwrap_or_default();
        let mut materials_changed = false;
        for path in material_files {
            let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
                continue;
            };
            if self.watched.insert(path, modified).is_some_and(|before| before != modified) {
                materials_changed = true;
            }
        }
        if materials_changed {
            match self.reload_materials() {
                Ok(()) => self.editor.say("reloaded the materials"),
                Err(e) => self.editor.say(e),
            }
        }
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

    /// Reads the material files anew, and rebuilds the level with them; if they are refused
    /// (or the level is, with them), keeps the materials as they were.
    fn reload_materials(&mut self) -> Result<(), String> {
        let library = MaterialLibrary::load(&self.assets.root().join("materials")).map_err(|e| e.to_string())?;
        self.apply_materials(library)
    }

    /// Draws with `library` from now on, and rebuilds the level with it; if it is refused
    /// (or the level is, with it), keeps the materials as they were.
    fn apply_materials(&mut self, library: MaterialLibrary) -> Result<(), String> {
        let old = self.assets.materials().clone();
        self.assets.set_materials(library);
        let result = Materials::compile(&mut self.assets, &self.ripples).and_then(|materials| {
            let before = std::mem::replace(&mut self.materials, materials);
            self.rebuild().inspect_err(|_| self.materials = before)
        });
        match result {
            Ok(()) => {
                self.editor.materials = self.materials.names().to_vec();
                Ok(())
            }
            Err(e) => {
                self.assets.set_materials(old);
                // The level as it was (it built with them before).
                let _ = self.rebuild();
                Err(e)
            }
        }
    }

    /// Saves the Material page's draft as its file, `NAME.mmat`: checked first with every
    /// other material (the files beside it), as they would be with it (an error leaves the
    /// files as they were), then drawn with from then on.
    fn save_material(&mut self) -> Result<String, String> {
        let draft = self.material.as_ref().ok_or("no material to save")?;
        let (def, comments, new) = (draft.cleaned(), draft.comments.clone(), draft.new);
        moose_assets::check_material_name(&def.name)?;
        if new && def.file.exists() {
            return Err(format!("there is already a material '{}' ({})", def.name, def.file.display()));
        }
        let text = def.to_text(&comments);
        let mut library = MaterialLibrary::default();
        let dir = def.file.parent().unwrap_or(Path::new("."));
        for file in material_files_in(dir).into_iter().filter(|f| *f != def.file) {
            let src = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            library.add_file(&file, &src).map_err(|e| e.to_string())?;
        }
        library.add_file(&def.file, &text).map_err(|e| e.to_string())?;
        self.apply_materials(library)?;
        std::fs::write(&def.file, &text).map_err(|e| format!("{}: {e}", def.file.display()))?;
        // Not a change to reload.
        if let Ok(modified) = std::fs::metadata(&def.file).and_then(|m| m.modified()) {
            self.watched.insert(def.file.clone(), modified);
        }
        // The draft as the file now has it.
        let library = self.assets.materials();
        let saved = library.get(library.id(&def.name).ok_or("the material went missing")?);
        let said = format!("saved {}.mmat", saved.name);
        self.material = Some(material_edit::Draft::of(saved, comments));
        Ok(said)
    }

    /// A draft of material `id`, for the Material page, with its file's top comments.
    fn material_draft(&self, id: u32) -> material_edit::Draft {
        let def = self.assets.materials().get(id);
        let comments = std::fs::read_to_string(&def.file).map(|src| moose_assets::material_comments(&src)).unwrap_or_default();
        material_edit::Draft::of(def, comments)
    }

    /// The meta keys and light names the level has, for the Material page's hints.
    fn known(&self) -> material_edit::Known {
        let keys = &self.world.meta_keys;
        material_edit::Known {
            keys: (0..keys.len() as u16).map(|k| keys.name(k).to_string()).collect(),
            lights: self.world.light_names.iter().flatten().cloned().collect(),
        }
    }

    /// Writes the editor's tables to the level's file, and says how it went.
    fn save_level(&mut self) -> String {
        let path = self.editor.path.clone();
        let said = match std::fs::write(&path, self.editor.doc.to_text()) {
            Ok(()) => {
                // Not a change to reload.
                if let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) {
                    self.watched.insert(path.clone(), modified);
                }
                self.editor.mark_saved();
                // A new level is in the list now.
                self.levels = level_files(&self.assets.root().to_string_lossy());
                format!("saved {}", path.display())
            }
            Err(e) => format!("cannot save {}: {e}", path.display()),
        };
        self.editor.say(said.clone());
        said
    }

    /// Moves the animated models and the water to `time` seconds, redrawing the water's
    /// textures if the ripples moved.
    fn set_time(&mut self, time: f32) {
        self.time = time;
        self.world.animate(&mut self.assets, time);
        // The rippling textures (`@water:FILE`, `@water_heights`), redrawn as the ripples move.
        let waters = &self.materials.waters;
        if (!waters.is_empty() || self.materials.heights.is_some()) && self.ripples.advance_to(time as f64) {
            for &(water, base) in waters {
                let rippled = self.ripples.texture("water", self.assets.texture(base).base());
                *self.assets.texture_mut(water) = rippled;
            }
            if let Some(heights) = self.materials.heights {
                *self.assets.texture_mut(heights) = self.ripples.heights("water heights");
            }
        }
    }

    /// Bakes a cube map for each mirror ball: the level rendered six times from the ball's
    /// center, a 90 degree square view down each axis. The ball itself is not in them: from
    /// inside, all of its faces face away.
    fn bake_cube_maps(&mut self) -> Result<(), String> {
        for i in 0..self.world.entities.len() {
            let e = &self.world.entities[i];
            // Those drawn with their own cube map (`@cube`).
            if !e.binding.as_ref().is_some_and(|b| self.materials.uses_entity_cube(b.material)) {
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
        // The level's lights (N) and its directional lights (I), their sources' sizes
        // scaled by the softness setting.
        // Those that cast shadows have fixed shadow slots after the flashlight's (their
        // place in the level plus 1), so their cached shadows stay theirs whatever is on.
        let s = &self.settings;
        // Shadows carved every frame: their own softness, at most the one for all (the
        // view scales the lights' sizes below by this).
        self.geometry.config.dynamic_softness =
            if s.softness > 0.0 { s.dynamic_softness.min(s.softness) / s.softness } else { 0.0 };
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
                // Its radius is the sine of its angular radius: the angle is scaled, up to
                // 22.5 degrees (45 across).
                let angle = l.radius.clamp(0.0, 1.0).asin() * s.softness;
                l.radius = angle.min(std::f32::consts::FRAC_PI_8).sin();
            } else if !s.level_lights {
                continue;
            } else {
                l.radius *= s.softness;
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
            light.radius = FLASHLIGHT_RADIUS * self.settings.softness;
            light.beam = self.settings.cone == Cone::Beam;
            light.coarse = self.settings.cone == Cone::Soft;
            light.id = moose_assets::FLASHLIGHT_ID;
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
                Item::NewLevel => ui::Row { label: "New level".into(), value: Some("10x5x10 m room".into()) },
                Item::SaveLevel => {
                    let file = self.editor.path.file_name().map_or(String::new(), |n| n.to_string_lossy().into_owned());
                    let state = if !self.editor.path.exists() {
                        "new, not saved"
                    } else if self.editor.dirty() {
                        "unsaved changes"
                    } else {
                        "saved"
                    };
                    ui::Row { label: format!("Save level ({file})"), value: Some(state.into()) }
                }
                Item::NewMaterial => ui::Row { label: "New material".into(), value: None },
                Item::EditMaterial(i) => {
                    let def = self.assets.materials().get(i as u32);
                    ui::Row { label: def.name.clone(), value: Some(def.shader.clone()) }
                }
                Item::MaterialRow(row) => match &self.material {
                    Some(draft) => ui::Row { label: draft.label(row), value: draft.value(row, &self.known()) },
                    None => ui::Row { label: String::new(), value: None },
                },
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
                        Item::Set(s) => Some(match self.binding(s) {
                            Some(key) => format!("{}  [{}]", self.value(s), key.name()),
                            None => self.value(s),
                        }),
                        _ => None,
                    },
                },
            })
            .collect()
    }

    /// A menu page's items: the page's own, or for the level list, one per level file.
    fn items(&self, page: Page) -> Vec<Item> {
        match page {
            Page::Levels => [Item::NewLevel, Item::SaveLevel]
                .into_iter()
                .chain((0..self.levels.len() as u16).map(Item::Load))
                .collect(),
            Page::Materials => std::iter::once(Item::NewMaterial)
                .chain((0..self.assets.materials().len() as u16).map(Item::EditMaterial))
                .collect(),
            Page::Material => self
                .material
                .as_ref()
                .map_or(Vec::new(), |d| d.rows().into_iter().map(Item::MaterialRow).collect()),
            _ => page.items().to_vec(),
        }
    }

    /// The resolutions the menu steps through: [`HEIGHTS`] in the shape the app started
    /// with (widths rounded to even), and the size it started at, smallest first.
    fn resolutions(&self) -> Vec<(u32, u32)> {
        let (w, h) = self.start_size;
        let mut sizes: Vec<(u32, u32)> = HEIGHTS
            .iter()
            .map(|&y| (((w as f32 * y as f32 / h as f32) / 2.0).round() as u32 * 2, y))
            .chain([(w, h)])
            .collect();
        sizes.sort_by_key(|&(w, h)| (h, w));
        sizes.dedup_by_key(|&mut (_, h)| h);
        sizes
    }

    /// Renders at `width` x `height` from the next frame (the window keeps its size).
    fn set_size(&mut self, width: u32, height: u32) {
        (self.width, self.height) = (width, height);
        self.pixels = vec![0; (width * height) as usize];
        self.camera.viewport = Viewport { x: 0, y: 0, width, height };
    }

    /// A setting's value, as the menu shows it.
    fn value(&self, setting: Setting) -> String {
        let (s, cfg) = (&self.settings, &self.renderer.config);
        let on = |b: bool| if b { "on" } else { "off" }.to_string();
        let fraction = |t: f32| if t > 0.0 { format!("1/{}", (1.0 / t).round()) } else { "off".into() };
        let softness = |k: f32| if k > 0.0 { format!("x{k}") } else { "hard".into() };
        match setting {
            Setting::Lit => on(s.lit),
            Setting::LevelLights => {
                let n = self.lights.0.iter().filter(|l| !l.directional).count();
                format!("{} ({n})", on(s.level_lights))
            }
            Setting::Sun => match self.lights.0.iter().any(|l| l.directional) {
                true => on(s.sun),
                false => "none here".into(),
            },
            Setting::Softness => softness(s.softness),
            Setting::DynamicSoftness => softness(s.dynamic_softness),
            Setting::SpotPenumbra => format!("x{:.2}", s.penumbra),
            Setting::Shadows => on(s.shadows),
            Setting::DynamicShadows => on(self.geometry.config.dynamic_shadows),
            Setting::Flashlight => on(s.flashlight),
            Setting::FlashlightMount => {
                if s.flashlight_lock.is_some() { "locked here" } else { "shoulder" }.into()
            }
            Setting::FlashlightFade => match (s.cone, s.flashlight_fade) {
                (Cone::Soft, _) => format!("{FLASHLIGHT_OUTER}° (soft)"),
                (_, 0.0) => "hard".into(),
                (_, fade) => format!("{fade}°"),
            },
            Setting::FlashlightDither => on(cfg.beam_dither),
            Setting::FlashlightBeam => self.cone_name().into(),
            Setting::Bump => s.bump.name().into(),
            Setting::Sky => s.sky.name().into(),
            Setting::Bloom => s.bloom.name().into(),
            Setting::Specular => on(s.specular),
            Setting::Water => on(s.water),
            Setting::Bounces => self.geometry.config.max_reflections.to_string(),
            Setting::Reflectance => s.reflectance.to_string(),
            Setting::Fade => if s.fade_range > 0.0 { format!("{} m", s.fade_range) } else { "off".into() },
            Setting::TranslucentCrates => on(s.translucent_crates),
            Setting::PerPixelCrates => on(s.per_pixel_crates),
            Setting::FrameCap => if s.capped { format!("{} fps", s.cap) } else { "off".into() },
            Setting::Resolution => format!("{}x{}", self.width, self.height),
            Setting::Overlay => on(cfg.show_samples),
            Setting::ShadowMesh => on(s.shadow_mesh),
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

    /// Changes a setting one step (`dir` +1 or -1; on/off settings just toggle).
    fn change(&mut self, setting: Setting, dir: i32) {
        let (s, cfg) = (&mut self.settings, &mut self.renderer.config);
        let wrap = |i: usize, n: usize| (i as i32 + dir).rem_euclid(n as i32) as usize;
        match setting {
            Setting::Lit => s.lit = !s.lit,
            Setting::LevelLights => s.level_lights = !s.level_lights,
            Setting::Sun => s.sun = !s.sun,
            Setting::Softness => s.softness = cycle(&SOFTNESS, s.softness, dir),
            Setting::DynamicSoftness => s.dynamic_softness = cycle(&SOFTNESS, s.dynamic_softness, dir),
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
            Setting::FlashlightFade => {
                s.flashlight_fade = cycle(&FLASHLIGHT_FADES, s.flashlight_fade, dir)
            }
            Setting::FlashlightBeam => {
                const CONES: [Cone; 3] = [Cone::Beam, Cone::Sampled, Cone::Soft];
                let i = CONES.iter().position(|&c| c == s.cone).unwrap_or(0);
                s.cone = CONES[wrap(i, CONES.len())];
            }
            Setting::FlashlightDither => cfg.beam_dither = !cfg.beam_dither,
            Setting::Sky => {
                let i = SkyStyle::ALL.iter().position(|&v| v == s.sky).unwrap_or(0);
                s.sky = SkyStyle::ALL[wrap(i, SkyStyle::ALL.len())];
            }
            Setting::Bloom => {
                let i = BloomMode::ALL.iter().position(|&v| v == s.bloom).unwrap_or(0);
                s.bloom = BloomMode::ALL[wrap(i, BloomMode::ALL.len())];
            }
            Setting::Bump => {
                let i = Bump::ALL.iter().position(|&b| b == s.bump).unwrap_or(0);
                s.bump = Bump::ALL[wrap(i, Bump::ALL.len())];
            }
            Setting::Specular => s.specular = !s.specular,
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
            Setting::Resolution => {
                let sizes = self.resolutions();
                let i = sizes.iter().position(|&wh| wh == (self.width, self.height)).unwrap_or(0);
                let (w, h) = sizes[wrap(i, sizes.len())];
                self.set_size(w, h);
            }
            Setting::Overlay => cfg.show_samples = !cfg.show_samples,
            Setting::ShadowMesh => s.shadow_mesh = !s.shadow_mesh,
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
                "{fps:.0} fps{}   view {view_ms:.2} ms   raster {raster_ms:.2} ms   post {:.2} ms   present {present_ms:.2} ms",
                if s.capped { format!(" (cap {})", s.cap) } else { String::new() },
                self.post_ms,
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
                "{} polygons   {} mirrors{}",
                self.geometry.polygons.len(),
                self.geometry.mirrors.len(),
                match self.world.terrain.is_empty() && self.world.blockers.is_empty() {
                    true => String::new(),
                    false => {
                        let v = &self.geometry.stats;
                        format!(
                            "   terrain {} drawn, {} blocked   {} entities blocked",
                            v.terrain_drawn, v.terrain_blocked, v.entities_blocked
                        )
                    }
                },
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
        if s.bump != Bump::Normal {
            add(format!("--bump {}", s.bump.name()));
        }
        if !s.specular {
            add("--specular off".into());
        }
        if s.sky != SkyStyle::Box {
            add(format!("--sky {}", s.sky.name()));
        }
        if s.bloom != BloomMode::Sun {
            add(format!("--bloom {}", s.bloom.name()));
        }
        add(format!("--bounces {}", self.geometry.config.max_reflections));
        add(format!("--f0 {} --fade {}", s.reflectance, s.fade_range));
        add(format!(
            "--softness {} --dynamic-softness {} --flashlight-fade {} --penumbra {}",
            s.softness, s.dynamic_softness, s.flashlight_fade, s.penumbra
        ));
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
            (s.shadow_mesh, "--show-shadow-mesh"),
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

    /// The key bound to `setting`, if any.
    fn binding(&self, setting: Setting) -> Option<Key> {
        self.bindings.iter().find(|&&(_, s)| s == setting).map(|&(k, _)| k)
    }

    /// Binds `key` to `setting` (each key does one setting, each setting has one key),
    /// unless the game uses it; saves the bindings. Returns what happened, to show.
    fn bind(&mut self, setting: Setting, key: Key) -> String {
        if RESERVED.contains(&key) {
            return format!("{} is taken: the game uses it", key.name());
        }
        self.bindings.retain(|&(k, s)| k != key && s != setting);
        self.bindings.push((key, setting));
        let mut said = format!("{} bound to {}", key.name(), Item::Set(setting).label());
        if let Err(e) = save_bindings(&self.bindings) {
            said = format!("{said} (not saved: {e})");
        }
        said
    }

    /// Unbinds `setting`'s key, if it has one, and saves the bindings.
    fn unbind(&mut self, setting: Setting) {
        let before = self.bindings.len();
        self.bindings.retain(|&(_, s)| s != setting);
        if self.bindings.len() != before {
            let said = match save_bindings(&self.bindings) {
                Ok(()) => format!("{} unbound", Item::Set(setting).label()),
                Err(e) => format!("not saved: {e}"),
            };
            self.toast = Some((said, Instant::now()));
        }
    }

    /// Renders one frame into `pixels`, returning (view ms, raster ms). The lights are set
    /// first, so the flashlight follows the camera.
    fn render(&mut self) -> Result<(f64, f64), String> {
        self.apply_lights(true);
        let mut pixels = std::mem::take(&mut self.pixels);
        let camera = self.camera.clone();
        let times = self.draw(&camera, &mut pixels, self.width, self.height);
        // The glow, over the finished frame (not cube maps).
        let post = Instant::now();
        if self.settings.bloom != BloomMode::Off {
            let bright = self.settings.bloom == BloomMode::Bright;
            let config = BloomConfig {
                strength: if bright { BLOOM_BRIGHT_STRENGTH } else { BLOOM_STRENGTH },
                threshold: if bright { BLOOM_THRESHOLD } else { 1.0 },
            };
            self.bloom.apply(&mut pixels, self.width as usize, self.height as usize, &config);
        }
        self.post_ms = post.elapsed().as_secs_f64() * 1000.0;
        self.pixels = pixels;
        times
    }

    /// What the player's settings say this frame, for resolving materials.
    fn frame_settings(&self) -> FrameSettings {
        let s = &self.settings;
        // The sky box's params: toward the sun, its tint and size, whether there is one,
        // whether to write how far past white for the bloom.
        let sun = self.world.directional.first().filter(|_| s.sun);
        let (toward, tint, radius) = match sun {
            Some(d) => (
                -d.direction.normalize(),
                d.color / d.color.max_element().max(1e-6),
                (d.angle * 0.5).to_radians() * SUN_DISK_SCALE,
            ),
            None => (Vec3::Y, Vec3::ONE, 0.0),
        };
        FrameSettings {
            water: s.water,
            translucent: s.translucent_crates,
            simple_sky: s.sky == SkyStyle::Flat,
            bump: s.bump == Bump::Normal,
            specular: s.specular,
            reflectance: s.reflectance,
            fade: s.fade_range,
            sun: Params::new(&[
                toward.x,
                toward.y,
                toward.z,
                tint.x,
                tint.y,
                tint.z,
                radius,
                if sun.is_some() { 1.0 } else { 0.0 },
                if s.bloom != BloomMode::Off { 1.0 } else { 0.0 },
            ]),
            // The level's lights as they are now: black while they're off (the sun with
            // its own setting).
            lights: self
                .lights
                .0
                .iter()
                .take(self.world.light_names.len())
                .map(|l| if s.lit && if l.directional { s.sun } else { s.level_lights } { l.color } else { Vec3::ZERO })
                .collect(),
        }
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
        let s = &self.settings;
        // Each material's surface this frame, under the player's settings.
        let table = self.materials.table(&self.shaders, &self.frame_settings(), &self.world);
        let (world, assets, shaders) = (&self.world, &self.assets, &self.shaders);
        let (level, cube_maps) = (assets.mesh(world.geometry), &self.cube_maps);
        let (level_filters, entity_filters) = (&self.level_filters, &self.entity_filters);
        let per_pixel = s.per_pixel_crates;
        let mut target = Target {
            pixels,
            width,
            height,
        };
        // What each polygon is drawn with: its binding's material in its scenario (its
        // reflection drawn under it, or seen in a mirror), or its vertex colors.
        let surface_of = |p: &moose_view::ViewPolygon| -> Surface {
            // Its binding, and where it is (for the inputs read there).
            let (binding, filters, entity, place) = match p.source {
                PolygonSource::World { polygon, sector } => {
                    let b = level.polygons[polygon as usize].material;
                    let place = materials::Place { face: Some(polygon), sector: Some(sector), entity: None };
                    (world.bindings.get(b as usize), level_filters.get(b as usize), None, place)
                }
                PolygonSource::Entity { entity, .. } | PolygonSource::Terrain { entity, .. } => {
                    let sector = match p.source {
                        PolygonSource::Terrain { sector, .. } => sector,
                        _ => world.entities[entity as usize].sector,
                    };
                    let place = materials::Place { face: None, sector: Some(sector), entity: Some(entity) };
                    (
                        world.entities[entity as usize].binding.as_ref(),
                        entity_filters.get(entity as usize),
                        Some(entity as usize),
                        place,
                    )
                }
            };
            let mut surface = match binding {
                Some(b) => {
                    let r = table.get(b.material, p.reflection.is_some(), p.mirror.is_some());
                    let mesh = assets.mesh(p.mesh);
                    let mut surface = table.surface(r, place, world);
                    if r.attribs.iter().any(|&a| mesh.attrib(a).is_none()) {
                        // A mesh without what its shader reads is drawn plain.
                        shaders.plain()
                    } else if r.entity_cube {
                        // Its own cube map, once baked.
                        match entity.and_then(|e| cube_maps[e]) {
                            Some(cube) => {
                                surface.textures[0] = Some(cube.texture);
                                surface.params = Params::new(&[cube.radius, CUBE_SIZE as f32]);
                                surface
                            }
                            None => shaders.plain(),
                        }
                    } else {
                        surface
                    }
                }
                None => shaders.plain(),
            };
            if let Some(filters) = filters {
                for (slot, f) in filters.iter().enumerate() {
                    if let Some(f) = f {
                        surface.filters[slot] = *f;
                    }
                }
            }
            // The lights it isn't lit by: its face's (with its sector's), or its entity's.
            surface.excluded_lights = match (place.face, entity) {
                (Some(f), _) => world.face_excluded_lights.get(f as usize).copied().unwrap_or(0),
                (None, Some(e)) => world.entities[e].excluded_lights,
                (None, None) => 0,
            };
            if per_pixel && entity.is_some() && !matches!(p.source, PolygonSource::Terrain { .. }) {
                surface.path_override = Some(RasterPath::PerPixel);
            }
            surface
        };
        self.renderer.time = self.time;
        self.renderer
            .render(&mut target, camera.viewport, &self.geometry, &self.assets, surface_of)
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

/// The material files (`.mmat`) in `dir`, sorted.
fn material_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "mmat"))
        .collect();
    files.sort();
    files
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
    /// The materials, to change one or make a new one.
    Materials,
    /// The material creator, on `App::material`.
    Material,
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
    /// Make a new level (named by typing), and save the one loaded.
    NewLevel,
    SaveLevel,
    /// Open the material creator on a new material, or on one in the library (an id).
    NewMaterial,
    EditMaterial(u16),
    /// A row of the material creator.
    MaterialRow(material_edit::Row),
}

/// A setting the menu shows and changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Lit,
    LevelLights,
    Sun,
    Softness,
    DynamicSoftness,
    SpotPenumbra,
    Shadows,
    DynamicShadows,
    Flashlight,
    FlashlightMount,
    FlashlightBeam,
    FlashlightFade,
    FlashlightDither,
    Bump,
    Sky,
    Bloom,
    Specular,
    Water,
    Bounces,
    Reflectance,
    Fade,
    TranslucentCrates,
    PerPixelCrates,
    FrameCap,
    Resolution,
    Overlay,
    ShadowMesh,
    MinStep,
    LightSpacing,
    SteepLimit,
    StepThreshold,
    PenumbraThreshold,
    MouseSmoothing,
    Hud,
}

impl Page {
    const ALL: [Page; 9] = [
        Page::Main,
        Page::Levels,
        Page::Materials,
        Page::Material,
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
            Page::Materials => "materials",
            Page::Material => "material",
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
            Page::Materials => "Materials",
            Page::Material => "Material",
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
                Open(Page::Materials),
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
                Set(Softness),
                Set(DynamicSoftness),
                Set(LevelLights),
                Set(Sun),
                Set(SpotPenumbra),
            ],
            Page::Flashlight => &[
                Set(Flashlight),
                Set(FlashlightMount),
                Set(FlashlightBeam),
                Set(FlashlightFade),
                Set(FlashlightDither),
            ],
            Page::Rendering => &[
                Set(Resolution),
                Set(Bump),
                Set(Specular),
                Set(Sky),
                Set(Bloom),
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
                Set(ShadowMesh),
                Set(MinStep),
                Set(LightSpacing),
                Set(SteepLimit),
                Set(StepThreshold),
                Set(PenumbraThreshold),
            ],
            Page::Controls => &[Set(MouseSmoothing), Set(Hud)],
            // The levels found in assets/levels, the materials, and a material's rows (see
            // `App::items`).
            Page::Levels | Page::Materials | Page::Material => &[],
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
                "B on a setting binds a key to it, Delete unbinds",
            ],
            Page::Flashlight => &[
                "Locking leaves it where it is: walk around",
                "to see its shadows. U does it from anywhere.",
            ],
            Page::Sampling => &["How shading is sampled: for tuning and debugging."],
            Page::Levels => &[
                "Settings carry over; you start at its spawn.",
                "Ctrl+S in the editor (Tab) saves the level too.",
            ],
            Page::Materials => &["Enter opens one in the material creator."],
            Page::Material => &[
                "A material is its own file, NAME.mmat.",
                "Left/Right steps an input's source: a number, a setting,",
                "or a face's, sector's, entity's, the level's or a light's",
                "value; Enter types its value, key or light's name.",
                "Renamed, a material is saved as a new one (a copy).",
            ],
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
            Item::NewLevel => "New level",
            Item::SaveLevel => "Save level",
            Item::NewMaterial => "New material",
            Item::EditMaterial(_) | Item::MaterialRow(_) => "Material",
            Item::Open(page) => match page {
                Page::Lighting => "Lighting",
                Page::Flashlight => "Flashlight",
                Page::Rendering => "Rendering",
                Page::Sampling => "Sampling (debug)",
                Page::Controls => "Controls",
                Page::Levels => "Levels",
                Page::Materials => "Materials",
                Page::Material => "Material",
                Page::Main => "Back",
            },
            Item::Set(s) => match s {
                Setting::Lit => "All lighting",
                Setting::LevelLights => "Level lights",
                Setting::Sun => "Sun",
                Setting::Softness => "Shadow softness",
                Setting::DynamicSoftness => "Dynamic softness",
                Setting::SpotPenumbra => "Spot cone edge",
                Setting::Shadows => "Shadows",
                Setting::DynamicShadows => "Dynamic shadows",
                Setting::Flashlight => "Flashlight",
                Setting::FlashlightMount => "Mount",
                Setting::FlashlightBeam => "Cone",
                Setting::FlashlightFade => "Fade",
                Setting::FlashlightDither => "Dither",
                Setting::Bump => "Bump mapping",
                Setting::Sky => "Sky",
                Setting::Bloom => "Bloom",
                Setting::Specular => "Specular highlights",
                Setting::Water => "Water floors",
                Setting::Bounces => "Reflection bounces",
                Setting::Reflectance => "Floor reflectance",
                Setting::Fade => "Reflection fade",
                Setting::TranslucentCrates => "Translucent crates",
                Setting::PerPixelCrates => "Per-pixel crates",
                Setting::FrameCap => "Frame cap",
                Setting::Resolution => "Resolution",
                Setting::Overlay => "Sample overlay",
                Setting::ShadowMesh => "Shadow mesh",
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

/// Keys the game, the editor or the menu use, which can't be hotkeys (B binds one in the
/// menu).
const RESERVED: &[Key] = &[
    Key::W, Key::A, Key::S, Key::D, Key::Q, Key::E, Key::C, Key::U, Key::V, Key::G, Key::M,
    Key::P, Key::Z, Key::Y, Key::B, Key::Space, Key::Tab, Key::Escape, Key::Enter,
    Key::Backspace, Key::Delete, Key::Up, Key::Down, Key::Left, Key::Right, Key::PageUp,
    Key::PageDown, Key::Key1, Key::Key2, Key::Key3, Key::Key4, Key::LeftBracket,
    Key::RightBracket, Key::F1, Key::F3, Key::F4, Key::F5, Key::F6, Key::F11, Key::F12,
    Key::LeftShift, Key::RightShift, Key::LeftCtrl, Key::RightCtrl, Key::LeftAlt,
    Key::RightAlt, Key::LeftSuper, Key::RightSuper,
];

/// Where hotkeys are kept: `~/.config/moose/keys.txt`, a line per key: its name, then
/// its setting's.
fn bindings_path() -> Option<PathBuf> {
    // (Tests keep their hands off the real file.)
    if cfg!(test) {
        return None;
    }
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/moose/keys.txt"))
}

/// Per binding, its slots' samplers (`None`: the material's).
type BindingFilters = Vec<[Option<u8>; MATERIAL_SLOTS]>;

/// Every setting the menu has.
/// Sampler overrides (`filterN=`) of the level's bindings and of each entity's, by slot.
fn binding_filters_of(world: &World) -> Result<(BindingFilters, BindingFilters), String> {
    let level = world.bindings.iter().map(materials::binding_filters).collect::<Result<Vec<_>, _>>()?;
    let entities = world
        .entities
        .iter()
        .map(|e| {
            e.binding
                .as_ref()
                .map_or(Ok([None; MATERIAL_SLOTS]), materials::binding_filters)
                .map_err(|m| format!("entity '{}': {m}", e.name))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok((level, entities))
}


fn all_settings() -> Vec<Setting> {
    Page::ALL
        .iter()
        .flat_map(|page| page.items())
        .filter_map(|item| if let Item::Set(s) = item { Some(*s) } else { None })
        .collect()
}

/// The saved hotkeys (see `bindings_path`); none if there are none. Lines that name no
/// key or setting (left from another version) are skipped.
fn load_bindings() -> Vec<(Key, Setting)> {
    let Some(text) = bindings_path().and_then(|p| std::fs::read_to_string(p).ok()) else {
        return Vec::new();
    };
    let settings = all_settings();
    text.lines()
        .filter_map(|line| {
            let (key, setting) = line.split_once(' ')?;
            let key = Key::named(key.trim()).filter(|k| !RESERVED.contains(k))?;
            let setting = settings.iter().find(|s| format!("{s:?}") == setting.trim())?;
            Some((key, *setting))
        })
        .collect()
}

/// Saves the hotkeys (see `bindings_path`).
fn save_bindings(bindings: &[(Key, Setting)]) -> Result<(), String> {
    let Some(path) = bindings_path() else {
        return if cfg!(test) { Ok(()) } else { Err("no home folder".into()) };
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut text = String::from("# Moose hotkeys: a key, then the setting it changes as Enter does in the menu.\n");
    for (key, setting) in bindings {
        text.push_str(&format!("{} {setting:?}\n", key.name()));
    }
    std::fs::write(&path, text).map_err(|e| e.to_string())
}

/// The options menu: open or not, the page showing, the selected row on it, and the pages
/// it was opened from.
struct Menu {
    open: bool,
    page: Page,
    selected: usize,
    back: Vec<(Page, usize)>,
    /// The setting waiting for a key to bind (B pressed on it).
    binding: Option<Setting>,
    /// A row being typed on (Enter on it), and the text so far.
    typing: Option<(Item, String)>,
}

/// Draws the menu page or the HUD over the app's frame, as the settings say.
fn draw_ui(
    app: &mut App,
    menu: Option<(Page, usize)>,
    binding: Option<Setting>,
    typing: Option<&str>,
    hud: Option<Vec<String>>,
) {
    let mut rows = menu.map(|(page, _)| app.rows(page));
    // The row being typed on shows the text so far.
    if let (Some(rows), Some((_, selected)), Some(text)) = (&mut rows, menu, typing)
        && let Some(row) = rows.get_mut(selected)
    {
        row.value = Some(format!("{text}_"));
    }
    let mut canvas = ui::Canvas {
        pixels: &mut app.pixels,
        width: app.width as usize,
        height: app.height as usize,
    };
    if app.settings.shadow_mesh && !(app.editor.on && app.editor.view != editor::ViewMode::Perspective) {
        draw_shadow_mesh(&mut canvas, &app.geometry);
    }
    if app.editor.on && menu.is_none() {
        let camera = app.camera.view();
        app.editor.draw(&mut canvas, &camera, &app.world, &app.assets);
    }
    if let Some(lines) = hud {
        ui::draw_hud(&mut canvas, &lines);
    }
    if let (Some((page, selected)), Some(rows)) = (menu, rows) {
        let hint = match (page, binding) {
            _ if typing.is_some() => "Type   Enter done   Esc cancel".into(),
            (_, Some(setting)) => format!("Press a key for {}   Esc cancels", Item::Set(setting).label()),
            (Page::Main, _) => "Up/Down choose   Enter pick   Esc close".into(),
            (Page::Levels, _) => "Up/Down choose   Enter load   Backspace back".into(),
            (Page::Materials, _) => "Up/Down choose   Enter open   Backspace back".into(),
            (Page::Material, _) => "Up/Down choose   Left/Right change   Enter type   Backspace back".into(),
            _ => "Up/Down choose   Left/Right change   B bind a key   Backspace back".into(),
        };
        ui::draw_menu(&mut canvas, page.title(), &rows, selected, page.notes(), &hint);
    }
    if let Some((text, at)) = &app.toast
        && at.elapsed() < Duration::from_secs(2)
    {
        ui::draw_toast(&mut canvas, text);
    }
}

/// Draws the last frame's shadow pieces (see `ViewGeometry::shadow_pieces`) over it, for
/// seeing how shadows are carved: each piece's outline, colored by what it is, and a dot
/// at each corner as bright as the light reaching it.
///
/// - Red: full shadow (none of the light reaches any corner; for a beam, also outside its
///   pyramid).
/// - Yellow: a soft edge (some of the light at some corner).
/// - Cyan: all of the light, inside a beam's pyramid (its cone is worked out per pixel).
/// - Green: all of the light (the lit parts a polygon is split into).
/// - Magenta: a blurred caster's hard shadow (see `ShadowKind::Blurred`), which the
///   renderer blurs.
///
/// Pieces on polygons seen in mirrors are drawn dimmer.
fn draw_shadow_mesh(canvas: &mut ui::Canvas, geometry: &ViewGeometry) {
    let dim = |c: u32| (c >> 1) & 0x7F7F7F;
    for polygon in &geometry.polygons {
        let pieces = &geometry.shadow_pieces[polygon.first_shadow as usize..][..polygon.shadow_count as usize];
        for piece in pieces {
            let vertices =
                &geometry.shadow_vertices[piece.first_vertex as usize..][..piece.vertex_count as usize];
            if vertices.is_empty() {
                continue;
            }
            let (lo, hi) = vertices
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| (lo.min(v.light), hi.max(v.light)));
            let color = if piece.occluded {
                0xE0_40_E0
            } else if hi <= 0.0 {
                0xE0_40_40
            } else if lo < 1.0 {
                0xF0_D0_40
            } else if piece.beam.is_some() {
                0x40_E0_E0
            } else {
                0x40_C0_40
            };
            let color = if polygon.mirror.is_some() { dim(color) } else { color };
            for (i, a) in vertices.iter().enumerate() {
                let b = &vertices[(i + 1) % vertices.len()];
                canvas.line(a.x, a.y, b.x, b.y, color);
            }
            for v in vertices {
                let g = (v.light.clamp(0.0, 1.0) * 255.0) as u32;
                canvas.fill_centered(v.x, v.y, 3, g << 16 | g << 8 | g);
            }
        }
    }
    // The legend, bottom left.
    let scale = if canvas.height >= 600 { 2 } else { 1 };
    let line = ui::Canvas::line_height(scale);
    let entries = [("full shadow", 0xE0_40_40), ("soft edge", 0xF0_D0_40), ("beam, lit", 0x40_E0_E0), ("lit", 0x40_C0_40), ("blurred", 0xE0_40_E0)];
    let gap = ui::Canvas::text_width("  ", scale);
    let width = entries.iter().map(|(t, _)| ui::Canvas::text_width(t, scale) + gap).sum::<usize>() + gap;
    let y = canvas.height.saturating_sub(2 * line);
    canvas.shade(0, y.saturating_sub(line / 2), width, 2 * line, 90);
    let mut x = gap / 2;
    for (text, color) in entries {
        canvas.text(x, y, text, color, scale);
        x += ui::Canvas::text_width(text, scale) + gap;
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
    // Alt (Option): texture rows step finely.
    app.editor.fine = down(&[Key::LeftAlt, Key::RightAlt]);
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
        app.editor.face_cut = None;
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
    // Where a cleave's end would go (and its preview runs to): on the grid, or with Ctrl
    // (Cmd) on the nearest vertex.
    app.editor.pointer = match (&ortho, cursor) {
        (Some(o), Some((x, y))) => Some(app.editor.cut_snap(o, glam::Vec2::new(x, y), ctrl)),
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
    // A line dragged across the selected surface in a 2D view cuts it (Ctrl: its ends on
    // vertices). The press only starts it; a release without a drag is a click.
    let mut deferred = false;
    if let (Some(o), Some((x, y))) = (&ortho, cursor) {
        let at = glam::Vec2::new(x, y);
        let snapped = app.editor.cut_snap(o, at, ctrl);
        let can_cut = matches!(app.editor.selection, Some(Selection::Surface(_)))
            && app.editor.picking.is_none()
            && app.editor.cutting.is_none()
            && !over_panel;
        if can_cut && display.mouse_clicked(MouseButton::Left) {
            app.editor.face_cut = Some(editor::FaceCut { from: snapped, to: snapped, pressed: at });
            deferred = true;
        } else if let Some(cut) = &mut app.editor.face_cut {
            if display.mouse_down(MouseButton::Left) {
                cut.to = snapped;
            } else if cut.pressed.distance(at) > 4.0 {
                app.edit(Field::CutFace, 1.0);
            } else {
                // Not dragged: a click, which selects.
                app.editor.face_cut = None;
                app.editor.selection = app.editor.hover;
            }
        }
    }
    // The panel row under the pointer, and what it changes.
    let row_field = cursor
        .filter(|_| over_panel)
        .and_then(|(x, y)| app.editor.row_at(x, y, w, h))
        .and_then(|k| app.editor.panel()[k].field);
    if display.mouse_clicked(MouseButton::Left) && !deferred {
        if over_panel {
            match row_field {
                // A number to type (Enter sets it, Esc drops it).
                Some(field) if field.typed() && app.editor.texture_mapping_selected() => {
                    app.editor.typing = Some((field, String::new()));
                    app.editor.say("type a number: Enter sets it, Esc drops it");
                }
                Some(field) => app.edit(field, 1.0),
                None => {}
            }
        } else if app.editor.picking.is_some() {
            // The surface clicked is the one to stitch from or merge in.
            app.edit(Field::Picked, 1.0);
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
    // Choosing the face to stitch from, a right click stitches mirrored (rather than
    // starting to look around).
    if display.mouse_clicked(MouseButton::Right) && !over_panel && cursor.is_some() && app.editor.picking.is_some() {
        app.edit(Field::PickedMirrored, 1.0);
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

/// Swaps `app` for a new one on `level` (its file, or `src` for a new level not saved
/// yet), with this one's settings.
fn switch_level(
    app: &mut App,
    options: &mut Options,
    level: String,
    src: Option<String>,
    display: &mut Display,
) -> Result<(), String> {
    let mut next = options.clone();
    next.level = level;
    (next.at, next.lock_flashlight, next.flashlight_at) = (None, None, None);
    let mut loaded = App::open(&next, src)?;
    loaded.settings = Settings {
        flashlight_lock: None,
        ..app.settings
    };
    loaded.renderer.config = app.renderer.config;
    loaded.geometry.config = app.geometry.config;
    loaded.start_size = app.start_size;
    loaded.set_size(app.width, app.height);
    *app = loaded;
    *options = next;
    display.set_title(&format!("Moose - {}", app.world.name));
    Ok(())
}

/// Starts a new level named `name` (its file `name.mmp` in assets/levels, written when it
/// is saved): one room of the default material (see `editor::new_level_doc`).
fn new_level(app: &mut App, options: &mut Options, name: &str, display: &mut Display) -> Result<(), String> {
    let name = name.trim().trim_end_matches(".mmp");
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')) {
        return Err("a level's name is letters, digits, '_' and '-'".into());
    }
    let file = format!("{name}.mmp");
    if app.assets.root().join("levels").join(&file).exists() {
        return Err(format!("there is already a level {file}"));
    }
    let src = editor::new_level_doc(name)?.to_text();
    switch_level(app, options, file.clone(), Some(src), display)?;
    app.toast = Some((format!("new level {file}: not saved yet (Levels > Save level)"), Instant::now()));
    Ok(())
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
        if let Some(file) = &options.views {
            app.set_time(options.time);
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                println!("view {line}");
                std::io::Write::flush(&mut std::io::stdout()).ok();
                match app.place_camera(pose(line, "--views")?, "--views") {
                    Ok(()) => drop(app.frame()?),
                    Err(e) => println!("skipped: {e}"),
                }
            }
        }
        if let Some(at) = options.at {
            app.place_camera(at, "--at")?;
        }
        app.editor.ortho_center = app.camera.position;
        app.set_time(options.time);
        let (view_ms, raster_ms) = app.frame()?;
        if options.bench > 0 {
            let (mut view, mut raster, mut post) = (0.0, 0.0, 0.0);
            for _ in 0..options.bench {
                let (v, r) = app.frame()?;
                (view, raster, post) = (view + v, raster + r, post + app.post_ms);
            }
            let n = options.bench as f64;
            println!(
                "{} frames: view {:.3} ms, raster {:.3} ms, post {:.3} ms on average",
                options.bench,
                view / n,
                raster / n,
                post / n
            );
        }
        let hud = app.settings.hud.then(|| app.hud(0.0, view_ms, raster_ms, 0.0));
        if options.menu == Some(Page::Material) {
            app.material = Some(material_edit::Draft::new_material(
                &app.assets.root().join("materials"),
                app.materials.names(),
            ));
        }
        draw_ui(&mut app, options.menu.map(|page| (page, 0)), None, None, hud);
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
        binding: None,
        typing: None,
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
            // editor, drops the polygon, then leaves it. While binding a key, it cancels.
            if menu.open && menu.binding.is_some() {
                menu.binding = None;
            } else if menu.open && menu.typing.is_some() {
                menu.typing = None;
            } else if app.editor.on && !menu.open && let Some(model) = &mut app.editor.model {
                if model.polygon.is_some() {
                    model.polygon = None;
                } else {
                    app.edit(editor::Field::EditModel, 1.0);
                }
            } else if app.editor.on && !menu.open && app.editor.typing.is_some() {
                app.editor.typing = None;
                app.editor.say("not set");
            } else if app.editor.on && !menu.open && app.editor.face_cut.is_some() {
                app.editor.face_cut = None;
                app.editor.say("cut stopped");
            } else if app.editor.on && !menu.open && app.editor.picking.is_some() {
                app.editor.picking = None;
                app.editor.say("stopped");
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
        if !menu.open && app.editor.typing.is_none() && display.key_pressed(Key::Tab) {
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
        // (Not while a face is being picked: there the right button is a click, a mirrored
        // stitch, which needs the pointer where it is.)
        let looking = !menu.open
            && (!app.editor.on || (display.mouse_down(MouseButton::Right) && app.editor.picking.is_none()));
        display.set_cursor_locked(looking);
        if let Some(setting) = menu.binding.filter(|_| menu.open) {
            // Waiting for a key to bind: the first one pressed (Esc cancels, above).
            display.mouse_delta();
            look.clear();
            if let Some(key) = display.pressed_keys().into_iter().find(|&k| k != Key::Escape) {
                let said = app.bind(setting, key);
                app.toast = Some((said, Instant::now()));
                menu.binding = None;
            }
        } else if menu.open && let Some((item, text)) = &mut menu.typing {
            // Typing on a row: Enter takes it, Esc drops it (above).
            display.mouse_delta();
            look.clear();
            text.push_str(display.text());
            if display.key_repeated(Key::Backspace) {
                text.pop();
            }
            if display.key_pressed(Key::Enter) && !alt {
                let (item, text) = (*item, std::mem::take(text));
                menu.typing = None;
                match item {
                    Item::NewLevel => match new_level(&mut app, &mut options, &text, &mut display) {
                        Ok(()) => {
                            menu.open = false;
                            look.clear();
                        }
                        Err(e) => app.toast = Some((e, Instant::now())),
                    },
                    Item::MaterialRow(row) => {
                        let names = app.materials.names().to_vec();
                        if let Some(draft) = &mut app.material {
                            let said = draft.typed(row, &text, &names);
                            draft.status = Some(said.unwrap_or_else(|e| e)).filter(|s| !s.is_empty());
                        }
                    }
                    _ => {}
                }
            }
        } else if menu.open {
            // The menu has the keys; the view holds still (the pointer's motion is dropped).
            display.mouse_delta();
            look.clear();
            let items = app.items(menu.page);
            if items.is_empty() {
                // A page with nothing on it (the material creator without a material).
                (menu.page, menu.selected) = menu.back.pop().unwrap_or((Page::Main, 0));
                continue;
            }
            // Rows come and go on the material page (its shaders read different things).
            menu.selected = menu.selected.min(items.len() - 1);
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
            if let (Item::MaterialRow(row), true) = (item, dir != 0) {
                let textures = material_edit::texture_choices(&app.assets.root().join("textures"));
                if let Some(draft) = &mut app.material {
                    draft.status = draft.step(row, dir, &textures).err();
                }
            }
            // B binds a key to the setting (as Enter on it), Delete unbinds it.
            if let Item::Set(setting) = item {
                if display.key_pressed(Key::B) {
                    menu.binding = Some(setting);
                }
                if display.key_pressed(Key::Delete) {
                    app.unbind(setting);
                }
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
                        let level = app.levels[i as usize].clone();
                        match switch_level(&mut app, &mut options, level.clone(), None, &mut display) {
                            Ok(()) => {
                                menu.open = false;
                                look.clear();
                            }
                            Err(e) => {
                                eprintln!("cannot load {level}: {e}");
                                app.toast = Some((format!("cannot load {level}"), Instant::now()));
                            }
                        }
                    }
                    Item::NewLevel => {
                        let name = (1..)
                            .map(|n| if n == 1 { "new_level".to_string() } else { format!("new_level_{n}") })
                            .find(|n| !app.levels.contains(&format!("{n}.mmp")))
                            .unwrap();
                        menu.typing = Some((item, name));
                    }
                    Item::SaveLevel => {
                        let said = app.save_level();
                        app.toast = Some((said, Instant::now()));
                    }
                    Item::NewMaterial | Item::EditMaterial(_) => {
                        app.material = Some(match item {
                            Item::EditMaterial(i) => app.material_draft(i as u32),
                            _ => material_edit::Draft::new_material(&app.assets.root().join("materials"), app.materials.names()),
                        });
                        menu.back.push((menu.page, menu.selected));
                        (menu.page, menu.selected) = (Page::Material, 0);
                    }
                    Item::MaterialRow(material_edit::Row::Save) => {
                        let said = app.save_material();
                        if let Some(draft) = &mut app.material {
                            draft.status = Some(said.unwrap_or_else(|e| e));
                        }
                    }
                    Item::MaterialRow(row) => {
                        let textures = material_edit::texture_choices(&app.assets.root().join("textures"));
                        if let Some(draft) = &mut app.material {
                            match draft.typing(row) {
                                Some(text) => menu.typing = Some((item, text)),
                                None => draft.status = draft.step(row, 1, &textures).err(),
                            }
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
        } else if app.editor.on && app.editor.typing.is_some() {
            // Typing a number into the editor's panel: the keys are the text's, the view
            // holds still.
            display.mouse_delta();
            look.clear();
            if let Some((_, text)) = &mut app.editor.typing {
                text.push_str(display.text());
                if display.key_repeated(Key::Backspace) {
                    text.pop();
                }
            }
            if display.key_pressed(Key::Enter) && !alt {
                let (field, text) = app.editor.typing.take().unwrap();
                app.type_value(field, &text);
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
        if display.key_pressed(Key::F4) {
            app.settings.shadow_mesh = !app.settings.shadow_mesh;
        }
        if !menu.open && display.key_pressed(Key::U) {
            app.change(Setting::FlashlightMount, 1);
        }
        // Hotkeys: as Enter on their settings in the menu.
        if !menu.open {
            for (key, setting) in app.bindings.clone() {
                if display.key_pressed(key) {
                    app.change(setting, 1);
                    let said = format!("{}: {}", Item::Set(setting).label(), app.value(setting));
                    app.toast = Some((said, Instant::now()));
                }
            }
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
        // A frame that can't be drawn says why (once, and on screen) and the app carries on.
        let (view_ms, raster_ms) = match app.frame() {
            Ok(times) => times,
            Err(e) => {
                if app.toast.as_ref().is_none_or(|(said, _)| *said != e) {
                    eprintln!("cannot draw the frame: {e}");
                }
                app.editor.say(e.clone());
                app.toast = Some((e, Instant::now()));
                (0.0, 0.0)
            }
        };
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
        let typing = menu.typing.as_ref().map(|(_, text)| text.as_str());
        draw_ui(&mut app, menu.open.then_some((menu.page, menu.selected)), menu.binding, typing, hud);
        display.set_size(app.width, app.height);
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
    fn a_new_level_is_a_room_of_the_default_material_and_saves() {
        let args = ["--level", "moose_new_level_test.mmp", "--size", "64x36"].map(String::from);
        let options = parse_options(args).unwrap();
        let src = editor::new_level_doc("moose_new_level_test").unwrap().to_text();
        let mut app = App::open(&options, Some(src)).unwrap();
        assert_eq!(app.world.sectors.len(), 1);
        assert_eq!(app.editor.doc.surfaces.len(), 6);
        assert!(app.editor.doc.surfaces.iter().all(|s| s.options == ["material=default"]));
        // Not saved yet: its file isn't there.
        assert!(app.editor.dirty());
        app.frame().unwrap();
        // The flashlight shows the wall ahead: neither black nor one flat color.
        let lit = app.pixels.iter().filter(|&&p| p & 0xFF_FF_FF > 0x10_10_10).count();
        assert!(lit > app.pixels.len() / 4, "{lit} of {} pixels lit", app.pixels.len());
        // Saved (here to a scratch file), it loads back the same.
        let path = std::env::temp_dir().join("moose_new_level_test.mmp");
        app.editor.path = path.clone();
        assert!(app.save_level().starts_with("saved"), "{}", app.save_level());
        assert!(!app.editor.dirty());
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(LevelDoc::parse(&path, &text).unwrap(), app.editor.doc);
        App::open(&options, Some(text)).unwrap();
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_line_dragged_across_a_surface_in_2d_cuts_only_it() {
        use editor::{FaceCut, Field, Selection, ViewMode};
        use wire::Axis;
        let mut app = test_app("shiny_rooms.mmp");
        app.editor.on = true;
        let before = app.editor.doc.clone();
        // The far wall (2, at z = 8) in the front view, a line down its middle.
        app.editor.view = ViewMode::Ortho(Axis::Front);
        app.editor.selection = Some(Selection::Surface(2));
        let (w, h) = (app.width as usize, app.height as usize);
        let o = app.editor.ortho(w, h).unwrap();
        // Ends on the grid, or with Ctrl on the vertex nearest (within a few pixels).
        let corner = o.to_screen(Vec3::new(4.0, 4.0, 8.0));
        // (Any vertex there on screen: the cut is a line on screen, square to the view.)
        let snapped = app.editor.cut_snap(&o, corner + glam::Vec2::new(3.0, 3.0), true);
        assert!(o.to_screen(snapped).distance(corner) < 1e-3, "{snapped}");
        let free = app.editor.cut_snap(&o, corner + glam::Vec2::new(3.0, 3.0), false);
        assert_eq!(free, free.map(|v| (v / app.editor.step()).round() * app.editor.step()));
        let (from, to) = (Vec3::new(0.0, -1.0, 8.0), Vec3::new(0.0, 5.0, 8.0));
        app.editor.face_cut = Some(FaceCut { from, to, pressed: glam::Vec2::ZERO });
        app.edit(Field::CutFace, 1.0);
        assert_eq!(app.editor.face_cut, None);
        assert_eq!(app.editor.doc.surfaces.len(), before.surfaces.len() + 1);
        assert_eq!(app.editor.doc.sectors.len(), before.sectors.len());
        assert_eq!(app.world.sectors.len(), before.sectors.len());
        app.frame().unwrap();
        // One undo puts it back; a line that misses the surface is refused.
        app.travel(false);
        assert_eq!(app.editor.doc, before);
        let (from, to) = (Vec3::new(9.0, -1.0, 8.0), Vec3::new(9.0, 5.0, 8.0));
        app.editor.face_cut = Some(FaceCut { from, to, pressed: glam::Vec2::ZERO });
        app.edit(Field::CutFace, 1.0);
        assert_eq!(app.editor.doc, before);
    }

    #[test]
    fn a_wall_stitched_to_the_floor_takes_its_material_and_texture() {
        use editor::{Field, Selection};
        let mut app = test_app("shiny_rooms.mmp");
        app.editor.on = true;
        let before = app.editor.doc.clone();
        // Stitch the far wall (2): then click the floor (0), the pointer over it.
        app.editor.selection = Some(Selection::Surface(2));
        app.edit(Field::Stitch, 1.0);
        assert_eq!(app.editor.picking, Some((2, editor::Pick::Stitch)));
        assert!(app.editor.panel().iter().any(|r| r.label == "Stitch from a touching face" && r.value == "click a face (right: mirrored)"));
        app.editor.hover = Some(Selection::Surface(0));
        app.edit(Field::Picked, 1.0);
        assert_eq!(app.editor.picking, None);
        assert!(app.editor.doc.surfaces[2].options.contains(&"material=metal_floor_shiny".to_string()));
        assert_eq!(app.editor.selection, Some(Selection::Surface(2)));
        app.frame().unwrap();
        // One undo puts it back.
        app.travel(false);
        assert_eq!(app.editor.doc, before);
        // Right-clicked: mirrored, the floor's texture reflected up the wall (v = (8 - h) / 2
        // for a corner h up, where continued it is (8 + h) / 2).
        app.edit(Field::Stitch, 1.0);
        assert!(app.editor.panel().iter().any(|r| r.label == "Stitch from a touching face" && r.value.contains("right: mirrored")));
        app.editor.hover = Some(Selection::Surface(0));
        app.edit(Field::PickedMirrored, 1.0);
        let uv = app.editor.doc.attributes.iter().position(|a| a.name == "uv").unwrap();
        for (v, rows) in &app.editor.doc.surfaces[2].corners {
            let (p, t) = (app.editor.doc.vertices[*v], &app.editor.doc.attributes[uv].values[rows[uv]]);
            let down: f32 = t[1].parse().unwrap();
            assert!((down - (8.0 - p.y) / 2.0).abs() < 1e-3, "{p}: {down}");
        }
        app.travel(false);
        assert_eq!(app.editor.doc, before);
        // A merge has no mirrored kind: a right click leaves it waiting for a left one.
        app.edit(Field::Merge, 1.0);
        app.editor.hover = Some(Selection::Surface(3));
        app.edit(Field::PickedMirrored, 1.0);
        assert_eq!(app.editor.picking, Some((2, editor::Pick::Merge)));
        app.editor.picking = None;
        assert_eq!(app.editor.doc, before);
        // A face that doesn't touch (the hallway's floor) is refused, and the stitch ends.
        app.edit(Field::Stitch, 1.0);
        app.editor.hover = Some(Selection::Surface(9));
        app.edit(Field::Picked, 1.0);
        assert_eq!((app.editor.picking, &app.editor.doc), (None, &before));
    }

    #[test]
    fn a_face_merged_with_its_neighbor_through_the_panel() {
        use editor::{FaceCut, Field, Pick, Selection, ViewMode};
        let mut app = test_app("shiny_rooms.mmp");
        app.editor.on = true;
        // The far wall cut in two (front view), then its halves merged back.
        app.editor.view = ViewMode::Ortho(wire::Axis::Front);
        app.editor.selection = Some(Selection::Surface(2));
        let (from, to) = (Vec3::new(0.0, -1.0, 8.0), Vec3::new(0.0, 5.0, 8.0));
        app.editor.face_cut = Some(FaceCut { from, to, pressed: glam::Vec2::ZERO });
        app.edit(Field::CutFace, 1.0);
        let cut = app.editor.doc.clone();
        app.edit(Field::Merge, 1.0);
        assert_eq!(app.editor.picking, Some((2, Pick::Merge)));
        assert!(app.editor.panel().iter().any(|r| r.label == "Merge with a touching face" && r.value == "click a face"));
        app.editor.hover = Some(Selection::Surface(3));
        app.edit(Field::Picked, 1.0);
        assert_eq!(app.editor.picking, None);
        assert_eq!(app.editor.doc.surfaces.len(), cut.surfaces.len() - 1);
        assert_eq!(app.editor.selection, Some(Selection::Surface(2)));
        app.frame().unwrap();
        // One undo brings the halves back; a face on another plane (the floor) is refused.
        app.travel(false);
        assert_eq!(app.editor.doc, cut);
        app.editor.selection = Some(Selection::Surface(2));
        app.edit(Field::Merge, 1.0);
        app.editor.hover = Some(Selection::Surface(0));
        app.edit(Field::Picked, 1.0);
        assert_eq!((app.editor.picking, &app.editor.doc), (None, &cut));
    }

    #[test]
    fn a_cleaved_sector_merged_back_through_the_panel() {
        use editor::{Field, Pick, Selection, ViewMode};
        let mut app = test_app("shiny_rooms.mmp");
        app.editor.on = true;
        let original = app.editor.doc.clone();
        // The hallway cleaved lengthwise in the top view, then its new part merged back.
        app.editor.view = ViewMode::Ortho(wire::Axis::Top);
        app.editor.selection = Some(Selection::Sector(1));
        app.editor.cutting = Some(vec![Vec3::new(0.25, 0.0, -20.0), Vec3::new(0.25, 0.0, 5.0)]);
        app.edit(Field::Cut, 1.0);
        let cut = app.editor.doc.clone();
        assert_eq!(cut.sectors.len(), original.sectors.len() + 1);
        let new = cut.sectors.len() - 1;
        app.editor.selection = Some(Selection::Sector(1));
        app.edit(Field::MergeSector, 1.0);
        assert_eq!(app.editor.picking, Some((1, Pick::MergeSector)));
        assert!(app.editor.panel().iter().any(|r| r.label == "Merge with a sector" && r.value == "click a face of it"));
        app.editor.hover = Some(Selection::Surface(cut.sector_surfaces(new).start));
        app.edit(Field::Picked, 1.0);
        assert_eq!(app.editor.picking, None);
        assert_eq!(app.editor.selection, Some(Selection::Sector(1)));
        assert_eq!(app.editor.doc.sectors.len(), original.sectors.len());
        assert_eq!(app.editor.doc.surfaces.len(), original.surfaces.len());
        app.frame().unwrap();
        // One undo brings the parts back; room_a with room_b (no opening between) is refused.
        app.travel(false);
        assert_eq!(app.editor.doc, cut);
        app.editor.selection = Some(Selection::Sector(0));
        app.edit(Field::MergeSector, 1.0);
        app.editor.hover = Some(Selection::Surface(cut.sector_surfaces(2).start));
        app.edit(Field::Picked, 1.0);
        assert_eq!((app.editor.picking, &app.editor.doc), (None, &cut));
    }

    #[test]
    fn a_light_kept_off_a_sector_and_a_face_through_the_panel() {
        use editor::{Excluder, Field, Pick, Selection};
        // A new level's room, lit by the flashlight: kept off the room, it's as if the
        // flashlight were off; one undo lights it again.
        let args = ["--level", "moose_exclusion_test.mmp", "--size", "64x36"].map(String::from);
        let src = editor::new_level_doc("moose_exclusion_test").unwrap().to_text();
        let mut app = App::open(&parse_options(args).unwrap(), Some(src)).unwrap();
        app.editor.on = true;
        // (The editor's panel and overlay off while measuring.)
        let brightness = |app: &mut App| {
            app.editor.on = false;
            app.frame().unwrap();
            app.editor.on = true;
            app.pixels.iter().map(|&p| (p >> 16 & 255) + (p >> 8 & 255) + (p & 255)).sum::<u32>()
        };
        let lit = brightness(&mut app);
        app.editor.selection = Some(Selection::Sector(0));
        assert!(app.editor.panel().iter().any(|r| r.label == "Lit by the flashlight" && r.value == "on"));
        app.edit(Field::FlashlightLights, 1.0);
        assert_eq!(app.editor.doc.sectors[0].options, ["exclude_lights=flashlight"]);
        assert!(app.editor.panel().iter().any(|r| r.label == "Lit by the flashlight" && r.value == "off"));
        // As dark as with the flashlight off (the room has only it and the ambient light).
        let dark = brightness(&mut app);
        app.settings.flashlight = false;
        let off = brightness(&mut app);
        app.settings.flashlight = true;
        assert!(dark < lit && dark == off, "{dark}, {off} with it off, of {lit}");
        app.travel(false);
        assert_eq!(brightness(&mut app), lit);

        // shiny_rooms: a face picks a level light to keep off, which is named for it;
        // clicked again, it lights the face again. Deleting a light takes it off the lists.
        let mut app = test_app("shiny_rooms.mmp");
        app.editor.on = true;
        app.editor.selection = Some(Selection::Surface(2));
        app.edit(Field::ExcludeLights, 1.0);
        assert_eq!(app.editor.picking, Some((2, Pick::Exclude(Excluder::Surface))));
        app.editor.hover = Some(Selection::Light(1));
        app.edit(Field::Picked, 1.0);
        assert!(app.editor.doc.lights[1].options.contains(&"name=light".to_string()));
        assert!(app.editor.doc.surfaces[2].options.contains(&"exclude_lights=light".to_string()));
        assert_eq!(app.world.face_excluded_lights[2], 1 << 1);
        // Still picking: the next click lights it again. (An edit drops what the pointer
        // is over; the next frame picks it again.)
        assert_eq!(app.editor.picking, Some((2, Pick::Exclude(Excluder::Surface))));
        app.editor.hover = Some(Selection::Light(1));
        app.edit(Field::Picked, 1.0);
        assert!(!app.editor.doc.surfaces[2].options.iter().any(|o| o.starts_with("exclude_lights")));
        app.editor.hover = Some(Selection::Light(1));
        app.edit(Field::Picked, 1.0);
        assert_eq!(app.world.face_excluded_lights[2], 1 << 1);
        app.editor.picking = None;
        app.editor.selection = Some(Selection::Light(1));
        app.edit(Field::Delete, 1.0);
        assert!(!app.editor.doc.surfaces[2].options.iter().any(|o| o.starts_with("exclude_lights")));
        assert_eq!(app.world.face_excluded_lights[2], 0);
        app.frame().unwrap();
    }

    #[test]
    fn undoing_what_the_pointer_is_over_drops_it() {
        use editor::{Field, Selection};
        let mut app = test_app("two_rooms.mmp");
        app.editor.on = true;
        let surfaces = app.editor.doc.surfaces.len();
        app.edit(Field::NewRoom, 1.0);
        assert_eq!(app.editor.doc.surfaces.len(), surfaces + 6);
        // The pointer over the new room's last surface, its sector selected: then undone in
        // the same frame (as Ctrl+Z is, after the pointer is picked), and drawn.
        let last = app.editor.doc.surfaces.len() - 1;
        app.editor.hover = Some(Selection::Surface(last));
        app.editor.selection = Some(Selection::Sector(app.editor.doc.sectors.len() - 1));
        app.travel(false);
        assert_eq!(app.editor.doc.surfaces.len(), surfaces);
        assert_eq!(app.editor.hover, None);
        draw_ui(&mut app, None, None, None, None);
        app.editor.panel();
        // Redone, the room is back, but what pointed into it isn't kept.
        app.travel(true);
        assert_eq!(app.editor.doc.surfaces.len(), surfaces + 6);
        draw_ui(&mut app, None, None, None, None);
        // A stale one set by hand isn't drawn or listed either.
        app.travel(false);
        app.editor.hover = Some(Selection::Surface(last));
        app.editor.selection = Some(Selection::Vertex(10_000));
        draw_ui(&mut app, None, None, None, None);
        app.editor.panel();
    }

    #[test]
    fn undo_and_redo_all_the_way_through_edits_that_add_and_remove_surfaces() {
        use editor::{Field, Selection};
        let mut app = test_app("two_rooms.mmp");
        app.editor.on = true;
        // Before every step, the pointer over the level's last surface and its last vertex
        // selected (as a frame picks them); after it, drawn (as the frame then is). What the
        // step took away must not be drawn.
        let aim = |app: &mut App| {
            let d = &app.editor.doc;
            app.editor.hover = Some(Selection::Surface(d.surfaces.len() - 1));
            app.editor.selection = Some(Selection::Vertex(d.vertices.len() - 1));
        };
        let draw = |app: &mut App| {
            draw_ui(app, None, None, None, None);
            app.editor.panel();
        };
        // The edits the level accepted.
        let mut made = 0;
        let mut edit = |app: &mut App, selection: Option<Selection>, field: Field| {
            aim(app);
            app.editor.selection = selection.or(app.editor.selection);
            let before = app.editor.doc.clone();
            app.edit(field, 1.0);
            made += (app.editor.doc != before) as usize;
            draw(app);
        };
        for k in 0..4 {
            edit(&mut app, None, Field::NewRoom);
            // Extrude a wall of the new room (selected first: New room selects the room).
            app.editor.selection = None;
            let room = app.editor.doc.sectors.len() - 1;
            let wall = app.editor.doc.sector_surfaces(room).start + 1;
            edit(&mut app, Some(Selection::Surface(wall)), Field::Extrude);
            // Every other time, delete the extension.
            if k % 2 == 1 {
                let last = app.editor.doc.sectors.len() - 1;
                edit(&mut app, Some(Selection::Sector(last)), Field::Delete);
            }
        }
        let start = test_app("two_rooms.mmp").editor.doc;
        let mut undone = 0;
        while app.editor.dirty() {
            aim(&mut app);
            app.travel(false);
            draw(&mut app);
            undone += 1;
        }
        assert_eq!(app.editor.doc, start);
        assert_eq!(undone, made);
        assert!(made >= 6, "{made} edits made");
        for _ in 0..undone {
            aim(&mut app);
            app.travel(true);
            draw(&mut app);
        }
    }

    #[test]
    fn every_material_draws_on_a_new_level_whatever_its_mesh_lacks() {
        // A new level's surfaces have texture coordinates only: no vertex colors or
        // normals. Each material, on all of them, still draws (what the mesh lacks reads 0).
        let args = ["--level", "moose_every_material_test.mmp", "--size", "64x36"].map(String::from);
        let options = parse_options(args).unwrap();
        let mut app = App::open(&options, Some(editor::new_level_doc("cheese").unwrap().to_text())).unwrap();
        for name in app.materials.names().to_vec() {
            for s in &mut app.editor.doc.surfaces {
                s.options = vec![format!("material={name}")];
            }
            app.rebuild().unwrap_or_else(|e| panic!("{name}: {e}"));
            app.frame().unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn the_material_creator_saves_a_material_as_its_own_file() {
        use material_edit::{Draft, Row};
        let mut app = test_app("two_rooms.mmp");
        // A scratch copy of the material files, so the real ones stay as they are.
        let dir = std::env::temp_dir().join("moose_material_creator_test");
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for file in material_files_in(&app.assets.root().join("materials")) {
            std::fs::copy(&file, dir.join(file.file_name().unwrap())).unwrap();
        }
        let mut draft = Draft::new_material(&dir, app.materials.names());
        draft.typed(Row::Name, "road", &[]).unwrap();
        draft.def.shader = "basic_bumpy".into();
        draft.step(Row::Source(0), 1, &[]).unwrap();
        draft.typed(Row::Value(0), "4", &[]).unwrap();
        draft.step(Row::Source(1), 1, &[]).unwrap();
        draft.step(Row::Source(1), 1, &[]).unwrap();
        draft.typed(Row::Value(1), "bump_far", &[]).unwrap();
        draft.typed(Row::Fallback(1), "6", &[]).unwrap();
        app.material = Some(draft);
        assert_eq!(app.save_material().unwrap(), "saved road.mmat");
        let text = std::fs::read_to_string(dir.join("road.mmat")).unwrap();
        assert!(text.starts_with("MOOSEMATERIAL 2\nshader             basic_bumpy\n"), "{text}");
        assert!(text.contains("short              4\nfar                face:bump_far|6\n"), "{text}");
        assert!(app.materials.names().contains(&"road".to_string()));
        // Saved again it is written over; a new one by the same name is refused.
        assert!(!app.material.as_ref().unwrap().new);
        app.material.as_mut().unwrap().typed(Row::Value(0), "3", &[]).unwrap();
        app.save_material().unwrap();
        assert!(std::fs::read_to_string(dir.join("road.mmat")).unwrap().contains("short              3"));
        let mut twin = Draft::new_material(&dir, &[]);
        twin.typed(Row::Name, "road", &[]).unwrap();
        app.material = Some(twin);
        assert!(app.save_material().unwrap_err().contains("already a material 'road'"));
        // One that can't be drawn is refused, and the file stays as it was.
        let before = std::fs::read_to_string(dir.join("road.mmat")).unwrap();
        let id = app.materials.names().iter().position(|n| n == "road").unwrap() as u32;
        app.material = Some(app.material_draft(id));
        app.material.as_mut().unwrap().def.textures[1] = Some(moose_assets::TextureSource::File("nope.png".into()));
        assert!(app.save_material().is_err());
        assert_eq!(std::fs::read_to_string(dir.join("road.mmat")).unwrap(), before);
        // A material from a file keeps the comments at its top.
        let brick = app.materials.names().iter().position(|n| n == "brick").unwrap() as u32;
        assert!(app.material_draft(brick).comments[0].starts_with("# Walls."));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// `library` with a material `name` added, from its file's text.
    fn with_material(library: &mut MaterialLibrary, name: &str, body: &str) {
        library.add_file(Path::new(&format!("{name}.mmat")), &format!("MOOSEMATERIAL 2\n{body}\n")).unwrap();
    }

    #[test]
    fn inputs_are_read_from_faces_sectors_entities_the_level_and_lights() {
        use materials::Place;
        let mut app = test_app("two_rooms.mmp");
        // Materials whose input reads each source (`textured`'s detail: 0-255, over 255).
        let mut library = app.assets.materials().clone();
        for (name, source) in [
            ("by_face", "face:dirt|10"),
            ("by_sector", "sector:dirt|20"),
            ("by_entity", "entity:dirt|30"),
            ("by_level", "level:dirt|40"),
            ("by_light", "light:lamp|50"),
        ] {
            with_material(&mut library, name, &format!("shader textured\ntexture0 default.png\ndetail {source}"));
        }
        app.apply_materials(library).unwrap();
        let id = |name: &str| app.materials.names().iter().position(|n| n == name).unwrap() as u32;
        // Values on face 2, sector 1, entity 0 and the level (as a script will set them).
        let key = app.world.meta_keys.intern("dirt");
        app.world.faces[2].set(key, vec![102.0]);
        app.world.sectors[1].meta.set(key, vec![153.0]);
        app.world.entities[0].meta.set(key, vec![204.0]);
        app.world.meta.set(key, vec![255.0]);
        let table = app.materials.table(&app.shaders, &app.frame_settings(), &app.world);
        let detail = |name: &str, place: Place| {
            let r = table.get(id(name), false, false);
            (table.surface(r, place, &app.world).params.values[0] * 255.0).round()
        };
        let at = |face, sector, entity| Place { face, sector, entity };
        assert_eq!(detail("by_face", at(Some(2), Some(0), None)), 102.0);
        assert_eq!(detail("by_face", at(Some(3), Some(0), None)), 10.0);
        assert_eq!(detail("by_face", at(None, Some(0), Some(0))), 10.0);
        assert_eq!(detail("by_sector", at(None, Some(1), None)), 153.0);
        assert_eq!(detail("by_sector", at(None, Some(0), None)), 20.0);
        assert_eq!(detail("by_entity", at(None, Some(0), Some(0))), 204.0);
        assert_eq!(detail("by_entity", at(None, Some(0), Some(1))), 30.0);
        assert_eq!(detail("by_level", at(None, None, None)), 255.0);
        // No light is named lamp: its fallback.
        assert_eq!(detail("by_light", at(None, None, None)), 50.0);
    }

    #[test]
    fn back_light_is_an_engine_input_bump_programs_take_and_their_shaders_never_see() {
        use materials::Place;
        let mut app = test_app("two_rooms.mmp");
        let mut library = app.assets.materials().clone();
        let bumpy = "texture0 default.png\ntexture1 default.png";
        with_material(&mut library, "curb", &format!("shader basic_bumpy\n{bumpy}\nshort 4\nback_light 30"));
        with_material(&mut library, "by_face", &format!("shader random_sections\n{bumpy}\nback_light face:corner|0"));
        with_material(&mut library, "bricks", &format!("shader lit\n{bumpy}\nback_light 10"));
        app.apply_materials(library.clone()).unwrap();
        let id = |app: &App, name: &str| app.materials.names().iter().position(|n| n == name).unwrap() as u32;
        let key = app.world.meta_keys.intern("corner");
        app.world.faces[2].set(key, vec![90.0]);
        let table = app.materials.table(&app.shaders, &app.frame_settings(), &app.world);
        // As the sine of its angle, and the shader's own inputs as they were.
        let curb = &table.get(id(&app, "curb"), false, false).surface;
        assert!((curb.back_light - 0.5).abs() < 1e-6);
        assert_eq!(curb.params.values[0], 4.0);
        let bricks = &table.get(id(&app, "bricks"), false, false).surface;
        assert!((bricks.back_light - 10f32.to_radians().sin()).abs() < 1e-6);
        // From a face's meta value, as any input can be.
        let r = table.get(id(&app, "by_face"), false, false);
        let face = |f| table.surface(r, Place { face: Some(f), ..Place::default() }, &app.world).back_light;
        assert_eq!((face(2), face(3)), (1.0, 0.0));
        // The creator offers it after the shader's own inputs, and only where it's taken.
        let draft = app.material_draft(id(&app, "curb"));
        assert_eq!(draft.label(material_edit::Row::Source(2)), "Engine: back_light (number, degrees)");
        assert!(!draft.rows().contains(&material_edit::Row::Source(3)));
        let plain = app.materials.names().iter().position(|n| n == "default").unwrap() as u32;
        assert!(!app.material_draft(plain).rows().contains(&material_edit::Row::Source(1)));
        // A program that doesn't bump doesn't take it; an angle past 90 degrees is refused.
        let mut flat = library.clone();
        with_material(&mut flat, "flat", "shader textured\ntexture0 default.png\nback_light 10");
        let error = app.apply_materials(flat).unwrap_err();
        assert!(error.contains("doesn't take 'back_light' (only lit, basic_bumpy, random_sections, reflective_bumpy)"), "{error}");
        with_material(&mut library, "bent", &format!("shader basic_bumpy\n{bumpy}\nback_light 95"));
        assert!(app.apply_materials(library).unwrap_err().contains("0 to 90 degrees"));
    }

    #[test]
    fn a_named_lights_brightness_reaches_a_material() {
        let mut app = test_app("two_rooms.mmp");
        let mut library = app.assets.materials().clone();
        with_material(&mut library, "lamp_glow", "shader textured\ntexture0 default.png\ndetail light:lamp|50");
        app.apply_materials(library).unwrap();
        let id = app.materials.names().iter().position(|n| n == "lamp_glow").unwrap() as u32;
        // A light named lamp, as the level's first, at half strength.
        app.world.light_names = vec![Some("lamp".into())];
        app.lights.0.insert(0, moose_assets::Light::point(0, Vec3::ZERO, Vec3::new(0.5, 0.25, 0.0), 5.0));
        let value = |app: &App| {
            let table = app.materials.table(&app.shaders, &app.frame_settings(), &app.world);
            table.get(id, false, false).surface.params.values[0]
        };
        app.settings.level_lights = true;
        assert_eq!(value(&app), 0.5);
        // Off, it's dark.
        app.settings.level_lights = false;
        assert_eq!(value(&app), 0.0);
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
    fn a_cleaves_ends_snap_to_vertices_with_ctrl() {
        use editor::{Field, Selection, ViewMode};
        let mut app = test_app("two_rooms.mmp");
        app.editor.on = true;
        // room_a (x from -4 to 4, z from 0 to 8) in the top view, the finest grid.
        app.editor.view = ViewMode::Ortho(wire::Axis::Top);
        app.editor.ortho_center = Vec3::new(0.0, 0.0, 4.0);
        app.editor.step = 0;
        let (w, h) = (app.width as usize, app.height as usize);
        let o = app.editor.ortho(w, h).unwrap();
        // A few pixels off two opposite corners: on the grid, a little off them; with Ctrl,
        // on them.
        let off = glam::Vec2::new(2.0, -2.0);
        let (a, b) = (Vec3::new(-4.0, 0.0, 8.0), Vec3::new(4.0, 0.0, 0.0));
        let near = |p: Vec3| o.to_screen(p) + off;
        let grid = app.editor.cut_snap(&o, near(a), false);
        assert!(o.to_screen(grid).distance(o.to_screen(a)) > 0.5, "{grid}");
        let (sa, sb) = (app.editor.cut_snap(&o, near(a), true), app.editor.cut_snap(&o, near(b), true));
        assert!(o.to_screen(sa).distance(o.to_screen(a)) < 1e-3 && o.to_screen(sb).distance(o.to_screen(b)) < 1e-3);
        // Cleaved from corner to corner: through the room's own corners, so no new vertex.
        let (sectors, vertices) = (app.editor.doc.sectors.len(), app.editor.doc.vertices.len());
        app.editor.selection = Some(Selection::Sector(0));
        app.edit(Field::Cleave, 1.0);
        assert!(!app.editor.cut_point(sa));
        assert!(app.editor.cut_point(sb));
        app.edit(Field::Cut, 1.0);
        assert_eq!(app.editor.doc.sectors.len(), sectors + 1);
        assert_eq!(app.editor.doc.vertices.len(), vertices);
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
        assert_eq!(label(&app).as_deref(), Some("walk (template)"));
        let meshes = app.assets.meshes().len();
        app.edit(Field::Animation, 1.0);
        assert_eq!(label(&app).as_deref(), Some("idle"));
        app.edit(Field::Animation, 1.0);
        assert_eq!(label(&app).as_deref(), Some("none"));
        let e = &app.world.entities[walker(&app)];
        assert_eq!(e.mesh, e.model, "not animated: its model as it is");
        app.edit(Field::Animation, 1.0);
        assert_eq!(label(&app).as_deref(), Some("walk"), "its own, over its template's");
        // One new level mesh per rebuild, and no more copies.
        assert_eq!(app.assets.meshes().len(), meshes + 3);
        // Its shadow, blurred by its template; its own option only where it differs.
        let shadow = |app: &App| app.editor.panel().into_iter().find(|r| r.label == "Shadow").map(|r| r.value);
        assert_eq!(shadow(&app).as_deref(), Some("blurred (template)"));
        app.edit(Field::Shadow, 1.0);
        assert_eq!(shadow(&app).as_deref(), Some("hard"));
        assert_eq!(app.world.entities[walker(&app)].shadow, moose_assets::ShadowKind::Hard);
        app.edit(Field::Shadow, 1.0);
        app.edit(Field::Shadow, 1.0);
        assert_eq!(shadow(&app).as_deref(), Some("blurred (template)"));
        let row = app.editor.doc.entities.iter().position(|e| e.name == "walker").unwrap();
        assert!(!app.editor.doc.entities[row].options.iter().any(|o| o.starts_with("shadow=")));
    }

    #[test]
    fn hotkeys_bind_one_key_to_one_setting_and_not_the_games_keys() {
        let mut app = test_app("walker_rooms.mmp");
        assert!(app.bind(Setting::Bump, Key::W).contains("taken"));
        assert_eq!(app.binding(Setting::Bump), None);
        app.bind(Setting::Bump, Key::H);
        assert_eq!(app.binding(Setting::Bump), Some(Key::H));
        // The key moves to another setting; the setting takes another key.
        app.bind(Setting::ShadowMesh, Key::H);
        assert_eq!((app.binding(Setting::Bump), app.binding(Setting::ShadowMesh)), (None, Some(Key::H)));
        app.bind(Setting::ShadowMesh, Key::F7);
        assert_eq!(app.bindings, vec![(Key::F7, Setting::ShadowMesh)]);
        app.unbind(Setting::ShadowMesh);
        assert!(app.bindings.is_empty());
        // Every setting and key is found again by its saved name.
        for setting in all_settings() {
            assert!(all_settings().iter().any(|s| format!("{s:?}") == format!("{setting:?}")));
        }
        assert_eq!(Key::named("f7"), Some(Key::F7));
        assert_eq!(Key::named(&Key::Key5.name()), Some(Key::Key5));
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
        // Flipped across, u mirrors about the middle and v stays; flipped down, the other
        // way round; each twice, as it was.
        let numbers = |app: &App| -> Vec<[f32; 2]> {
            uvs(app).iter().map(|r| [r[0].parse().unwrap(), r[1].parse().unwrap()]).collect()
        };
        let start = numbers(&app);
        let middle = start.iter().fold([0.0, 0.0], |m, t| [m[0] + t[0], m[1] + t[1]]).map(|c| c / start.len() as f32);
        for (field, k) in [(Field::TextureFlipAcross, 0), (Field::TextureFlipDown, 1)] {
            app.edit(field, 1.0);
            for (a, b) in start.iter().zip(numbers(&app)) {
                assert!((b[k] - (2.0 * middle[k] - a[k])).abs() < 1e-3, "{a:?} became {b:?}");
                assert!((b[1 - k] - a[1 - k]).abs() < 1e-3, "{a:?} became {b:?}");
            }
            app.edit(field, 1.0);
            for (a, b) in start.iter().zip(numbers(&app)) {
                assert!((a[0] - b[0]).abs() < 1e-3 && (a[1] - b[1]).abs() < 1e-3, "{a:?} came back {b:?}");
            }
        }
        assert!(app.editor.panel().iter().any(|r| r.label == "Texture flip across"));
        assert!(app.editor.panel().iter().any(|r| r.label == "Texture flip down"));
    }

    #[test]
    fn texture_numbers_read_back_and_take_typed_values_and_fine_steps() {
        use editor::{Field, Selection};
        let mut app = test_app("shiny_rooms.mmp");
        let wall = 2;
        app.editor.selection = Some(Selection::Surface(wall));
        // Mapped flat (as the test levels are): no offset, 2 m a repeat, square, unturned.
        let m = app.editor.texture_mapping(wall).unwrap();
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        assert!(close(m.offset.x, 0.0) && close(m.offset.y, 0.0), "{m:?}");
        assert!(close(m.size.x, 2.0) && close(m.size.y, 2.0) && close(m.turn, 0.0) && close(m.shear, 0.0), "{m:?}");
        let row = |app: &App, label: &str| app.editor.panel().into_iter().find(|r| r.label == label).unwrap().value;
        assert_eq!(row(&app, "Texture size across"), "2.0000 m");
        // Typed: sizes and the turn about the middle, offsets as they are.
        let middle = |app: &App| {
            let m = app.editor.texture_mapping(wall).unwrap();
            let d = &app.editor.doc;
            let points = d.surface_points(wall);
            let n = d.surface_normal(wall);
            let q = points.iter().map(|&p| editor_flat(p, n)).sum::<glam::Vec2>() / points.len() as f32;
            m.at(q)
        };
        let start = middle(&app);
        app.type_value(Field::TextureSizeU, "1.5 m");
        app.type_value(Field::TextureTurn, "30°");
        let m = app.editor.texture_mapping(wall).unwrap();
        assert!(close(m.size.x, 1.5) && close(m.size.y, 2.0) && close(m.turn, 30.0), "{m:?}");
        assert!(middle(&app).distance(start) < 1e-3);
        app.type_value(Field::TextureU, "0.3");
        assert!(close(app.editor.texture_mapping(wall).unwrap().offset.x, 0.3));
        assert_eq!(row(&app, "Texture turn"), "30.00°");
        // Not a number, or a zero size: nothing changes.
        let before = app.editor.doc.clone();
        app.type_value(Field::TextureSizeV, "big");
        app.type_value(Field::TextureSizeV, "0");
        assert_eq!(app.editor.doc, before);
        // Fine steps (Alt): a 128th of a repeat, a degree.
        app.editor.fine = true;
        app.edit(Field::TextureU, 1.0);
        let m = app.editor.texture_mapping(wall).unwrap();
        assert!(close(m.offset.x, 0.3 + 1.0 / 128.0), "{m:?}");
        app.edit(Field::TextureTurn, 1.0);
        let m = app.editor.texture_mapping(wall).unwrap();
        assert!(close(m.turn, 31.0), "{m:?}");
        // Flipped (a negative size), read and written back as it is: nothing moves.
        app.editor.fine = false;
        app.edit(Field::TextureFlipDown, 1.0);
        let uvs = app.editor.doc.clone();
        let m = app.editor.texture_mapping(wall).unwrap();
        app.editor.set_texture_mapping(wall, m, false);
        let uv_at = |d: &moose_assets::LevelDoc| -> Vec<[f32; 2]> {
            d.surfaces[wall].corners.iter().map(|(_, r)| {
                let t = &d.attributes[1].values[r[1]];
                [t[0].parse().unwrap(), t[1].parse().unwrap()]
            }).collect()
        };
        for (x, y) in uv_at(&uvs).iter().zip(uv_at(&app.editor.doc)) {
            assert!(close(x[0], y[0]) && close(x[1], y[1]), "{x:?} became {y:?}");
        }
    }

    /// A point's flat coordinates (meters) on a surface facing `normal`, as the editor's.
    fn editor_flat(p: Vec3, normal: Vec3) -> glam::Vec2 {
        let a = normal.abs();
        let (u, v) = if a.y >= a.x && a.y >= a.z { (p.x, p.z) } else if a.x >= a.z { (p.z, -p.y) } else { (p.x, -p.y) };
        glam::Vec2::new(u, v)
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
            // The level list, the materials and the material creator are made by the app.
            let made = matches!(page, Page::Levels | Page::Materials | Page::Material);
            assert!(made || !page.items().is_empty());
            // The material creator opens from the materials.
            if !matches!(page, Page::Main | Page::Material) {
                assert!(Page::Main.items().contains(&Item::Open(page)), "{page:?}");
            }
        }
    }
}
