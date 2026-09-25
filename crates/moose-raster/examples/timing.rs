//! Rough per-frame timings over random views of a test level.
//! Arguments: [WIDTH HEIGHT [LEVEL [BOUNCES [F0]]]] (default 1280 720 two_rooms.mmp 1 0.15);
//! RAYON_NUM_THREADS=1 for one thread. Shiny surfaces get the Fresnel shader.
//!
//! cargo run --release -p moose-raster --example timing
use std::f32::consts::{PI, TAU};
use std::time::Instant;

use glam::Vec3;
use moose_assets::Assets;
use moose_raster::shaders::{VertexColor, VertexColorFresnel};
use moose_raster::{RasterConfig, Renderer, Surface, Target};
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
    let world = World::new(level, &assets);
    let vp = Viewport {
        x: 0,
        y: 0,
        width,
        height,
    };
    let mut renderer = Renderer::new(RasterConfig::default());
    let shader = renderer.register_shader::<VertexColor>();
    let fresnel = renderer.register_shader::<VertexColorFresnel>();
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
    let frames = 500;
    let (mut view_s, mut raster_s) = (0.0f64, 0.0f64);
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
        let mirrors = &out.mirrors;
        renderer
            .render(&mut target, vp, &out, &assets, |p| match p.source {
                PolygonSource::World { polygon, .. } if p.reflection.is_some() => {
                    let eye = p.mirror.map_or(c.position, |m| mirrors[m as usize].eye);
                    let n = geometry.polygons[polygon as usize].plane.normal;
                    let mut s = Surface::new(fresnel);
                    s.uniforms.values[..7]
                        .copy_from_slice(&[eye.x, eye.y, eye.z, n.x, n.y, n.z, f0]);
                    s
                }
                _ => Surface::new(shader),
            })
            .unwrap();
        let t2 = Instant::now();
        view_s += (t1 - t0).as_secs_f64();
        raster_s += (t2 - t1).as_secs_f64();
    }
    println!(
        "{file} ({bounces} bounces) {width}x{height}, {frames} frames on {} threads: view {:.3} ms/frame, raster {:.3} ms/frame",
        rayon::current_num_threads(),
        view_s * 1000.0 / frames as f64,
        raster_s * 1000.0 / frames as f64
    );
}
