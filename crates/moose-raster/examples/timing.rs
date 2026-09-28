//! Rough per-frame timings over random views of a test level.
//! Arguments: [WIDTH HEIGHT [LEVEL [BOUNCES [F0]]]] (default 1280 720 two_rooms.mmp 1 0.15);
//! RAYON_NUM_THREADS=1 for one thread; FRAMES=N for a longer run (default 500). Shiny
//! surfaces get the Fresnel shader: textured (metal_tile.png, 5 m fade) on levels with uvs,
//! read with sampler FILTER=bilinear_mipmap_linear (the default; any of the twelve names in
//! `moose_raster::shaders::filter`, METHOD_mipmap_MIP). WATER=1
//! makes the textured shiny floors water, its ripples held still 5 s in (redrawing them as
//! they move costs about 0.13 ms 20 times a second, outside the frame).
//!
//! MIN_STEP and LIGHT_SPACING set `RasterConfig`'s spacing limits (in pixels), and
//! PENUMBRA_THRESHOLD its penumbra rule (0 turns it off).
//! The level's lights are on; LIGHTS=0 turns them off (surfaces show their full color).
//!
//! cargo run --release -p moose-raster --example timing
use std::f32::consts::{PI, TAU};
use std::time::Instant;

use glam::Vec3;
use moose_assets::Assets;
use moose_raster::shaders::{TexturedFresnel, VertexColor, VertexColorFresnel, Water, filter};
use moose_raster::{
    Params, RasterConfig, Renderer, Surface, Target, register_per_filter,
};
use moose_scene::{Camera, Viewport, World};
use moose_view::{PolygonSource, ViewGeometry};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let size = |i: usize, default| {
        args.get(i)
            .map_or(default, |a| a.parse::<u32>().expect("WIDTH HEIGHT"))
    };
    let (width, height) = (size(0, 1280), size(1, 720));
    let file = args.get(2).map_or("two_rooms.mmp", String::as_str);
    let bounces: u8 = args.get(3).map_or(1, |a| a.parse().expect("BOUNCES"));
    let f0: f32 = args.get(4).map_or(0.15, |a| a.parse().expect("F0"));
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let level = assets.load_level(file).unwrap();
    let mut world = World::new(level, &assets);
    if std::env::var("LIGHTS").is_ok_and(|l| l == "0") {
        world.set_lights(Vec::new(), Vec3::ONE);
    }
    let vp = Viewport {
        x: 0,
        y: 0,
        width,
        height,
    };
    let setting = |name: &str, default: u32| {
        std::env::var(name).map_or(default, |v| v.parse().expect("a whole number"))
    };
    let defaults = RasterConfig::default();
    let mut renderer = Renderer::new(RasterConfig {
        min_step: setting("MIN_STEP", defaults.min_step),
        light_spacing: setting("LIGHT_SPACING", defaults.light_spacing),
        penumbra_threshold: std::env::var("PENUMBRA_THRESHOLD")
            .map_or(defaults.penumbra_threshold, |v| v.parse().expect("a number")),
        ..defaults
    });
    let shader = renderer.register_material::<VertexColor>();
    let fresnel = renderer.register_material::<VertexColorFresnel>();
    let sampler = std::env::var("FILTER").map_or(filter::BILINEAR_MIPMAP_LINEAR, |name| {
        filter::named(&name).expect("FILTER names a sampler")
    });
    let index = filter::ALL.iter().position(|&f| f == sampler).unwrap();
    let textured_fresnel = register_per_filter!(renderer, TexturedFresnel)[index];
    let water = register_per_filter!(renderer, Water)[index];
    let use_water = std::env::var("WATER").is_ok_and(|w| w == "1");
    let has_uvs = assets
        .mesh(world.geometry)
        .attribs
        .iter()
        .any(|a| a.name == "uv");
    let texture = has_uvs.then(|| assets.load_texture("metal_tile.png").unwrap());
    let water_textures = texture.filter(|_| use_water).map(|t| {
        let mut ripples = moose_assets::Ripples::new(1);
        ripples.advance_to(5.0);
        let rippled = ripples.texture("water", assets.texture(t).base());
        [
            assets.add_texture(rippled),
            assets.add_texture(ripples.heights("water heights")),
        ]
    });
    let geometry = assets.mesh(world.geometry);
    let mut out = ViewGeometry::new();
    out.config.max_reflections = bounces;
    let mut pixels = vec![0u32; (width * height) as usize];
    let mut seed = 1u64;
    let mut rnd = |lo: f32, hi: f32| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        lo + (hi - lo) * ((seed >> 40) as f32 / (1u64 << 24) as f32)
    };
    let frames: usize = std::env::var("FRAMES").map_or(500, |f| f.parse().unwrap());
    let (mut view_s, mut raster_s) = (0.0f64, 0.0f64);
    let mut parts = [0.0f64; 4]; // prepare, setup, rows, rows busy per thread
    for _ in 0..frames {
        let mut c = Camera::at_spawn(&world.spawn_points[0], vp);
        c.position = Vec3::new(rnd(-3.5, 3.5), rnd(0.5, 3.5), rnd(0.5, 7.5));
        c.sector = world.find_sector(c.position).unwrap();
        (c.yaw, c.pitch, c.roll) = (rnd(0.0, TAU), rnd(-1.0, 1.0), rnd(-PI / 8.0, PI / 8.0));
        let t0 = Instant::now();
        out.build(&world, &assets, &c.view());
        let t1 = Instant::now();
        let mut target = Target {
            pixels: &mut pixels,
            width,
            height,
        };
        let stats = renderer
            .render(&mut target, vp, &out, &assets, |p| match p.source {
                PolygonSource::World { polygon, .. } if p.reflection.is_some() => {
                    let n = geometry.polygons[polygon as usize].plane.normal;
                    let water_textures = water_textures.filter(|_| n.y > 0.9);
                    let mut s = match water_textures {
                        Some([water_texture, heights]) => Surface {
                            textures: [Some(water_texture), Some(heights)],
                            ..Surface::new(water)
                        },
                        None => Surface {
                            textures: [texture, None],
                            ..Surface::new(if texture.is_some() {
                                textured_fresnel
                            } else {
                                fresnel
                            })
                        },
                    };
                    // Water shifts by its texels' size: 2 m tiles of 128 texels.
                    s.params = Params::new(&[f0, 5.0, 2.0 / moose_assets::RIPPLE_SIZE as f32]);
                    s
                }
                _ => Surface::new(shader),
            })
            .unwrap();
        let t2 = Instant::now();
        view_s += (t1 - t0).as_secs_f64();
        raster_s += (t2 - t1).as_secs_f64();
        parts[0] += stats.prepare.as_secs_f64();
        parts[1] += stats.setup.as_secs_f64();
        parts[2] += stats.rows.as_secs_f64();
        parts[3] += stats.rows_busy.as_secs_f64() / stats.threads as f64;
    }
    let ms = |s: f64| s * 1000.0 / frames as f64;
    println!(
        "{file} ({bounces} bounces) {width}x{height}, {frames} frames on {} threads: view {:.3} ms/frame, raster {:.3} ms/frame",
        rayon::current_num_threads(),
        view_s * 1000.0 / frames as f64,
        raster_s * 1000.0 / frames as f64
    );
    println!(
        "  prepare {:.3} ms, setup {:.3} ms, rows {:.3} ms (threads busy {:.0}% of it)",
        ms(parts[0]),
        ms(parts[1]),
        ms(parts[2]),
        100.0 * parts[3] / parts[2]
    );
}
