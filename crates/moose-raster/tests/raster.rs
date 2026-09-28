use std::f32::consts::{PI, TAU};

use glam::Vec3;
use moose_assets::Assets;
use moose_raster::shaders::VertexColor;
use moose_raster::{
    LayoutError, MaterialId, Params, RasterConfig, RasterPath, Renderer, Surface, Target,
};
use moose_scene::{Camera, PORTAL_CLEARANCE, Viewport, World};
use moose_view::{EdgeLine, PolygonKind, ViewGeometry, pixel_edge};

const VP: Viewport = Viewport {
    x: 0,
    y: 0,
    width: 320,
    height: 180,
};

fn world() -> (World, Assets) {
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let level = assets.load_level("two_rooms.mmp").unwrap();
    (World::new(level, &assets), assets)
}

fn camera(world: &World, p: Vec3, yaw: f32, pitch: f32, roll: f32, vp: Viewport) -> Camera {
    let mut c = Camera::at_spawn(&world.spawn_points[0], vp);
    c.position = p;
    c.sector = world.find_sector(p).expect("camera outside the level");
    (c.yaw, c.pitch, c.roll) = (yaw, pitch, roll);
    c
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next()
    }
}

/// Random camera placements like the view module's sweep: sector interiors kept 0.3 m from
/// walls, and doorway positions within millimetres of a portal plane.
fn random_views(world: &World, count: usize, seed: u64, vp: Viewport) -> Vec<Camera> {
    let mut rng = Lcg(seed);
    let regions = [
        (Vec3::new(-3.7, 0.3, 0.3), Vec3::new(3.7, 3.7, 7.7)),
        (Vec3::new(-0.7, 0.3, -11.7), Vec3::new(0.7, 2.7, -0.3)),
        (Vec3::new(-3.7, 0.3, -19.7), Vec3::new(3.7, 3.7, -12.3)),
    ];
    (0..count)
        .map(|round| {
            let position = if round % 4 == 3 {
                let plane_z = if rng.next() < 0.5 { 0.0 } else { -12.0 };
                let offsets = [
                    PORTAL_CLEARANCE,
                    -PORTAL_CLEARANCE,
                    0.004,
                    -0.004,
                    0.03,
                    -0.03,
                ];
                let dz = offsets[(rng.next() * offsets.len() as f32) as usize % offsets.len()];
                Vec3::new(rng.range(-0.7, 0.7), rng.range(0.3, 2.7), plane_z + dz)
            } else {
                let (lo, hi) = regions[round % 3];
                Vec3::new(
                    rng.range(lo.x, hi.x),
                    rng.range(lo.y, hi.y),
                    rng.range(lo.z, hi.z),
                )
            };
            let (yaw, pitch, roll) = (
                rng.range(0.0, TAU),
                rng.range(-1.3, 1.3),
                rng.range(-PI, PI),
            );
            camera(world, position, yaw, pitch, roll, vp)
        })
        .collect()
}

fn render(
    renderer: &mut Renderer,
    geometry: &ViewGeometry,
    assets: &Assets,
    vp: Viewport,
    path: Option<RasterPath>,
) -> Vec<u32> {
    let shader = MaterialId(0);
    let mut pixels = vec![0xDEAD_BEEF; (vp.width * vp.height) as usize];
    let mut target = Target {
        pixels: &mut pixels,
        width: vp.width,
        height: vp.height,
    };
    renderer
        .render(&mut target, vp, geometry, assets, |p| Surface {
            path_override: if p.kind == PolygonKind::World {
                None
            } else {
                path
            },
            ..Surface::new(shader)
        })
        .unwrap();
    pixels
}

fn renderer(config: RasterConfig) -> Renderer {
    let mut r = Renderer::new(config);
    assert_eq!(r.register_material::<VertexColor>(), MaterialId(0));
    r
}

/// An edge crossing a row: the edge's index and its x there.
type Crossing = Option<(usize, f32)>;

/// Per pixel of the reference: the nearest surface's color, w and world position, and the
/// runner-up's w.
#[derive(Clone, Copy, Default)]
struct RefPixel {
    color: u32,
    w: f32,
    pos: Vec3,
    polygon: u32,
    second_w: f32,
    second_polygon: u32,
}

/// Brute-force reference: every polygon rasterized per pixel with the same coverage rule
/// (left/right chains, pixel_edge, carried lines), keeping the nearest two surfaces per
/// pixel by w, with exact per-pixel perspective-correct color.
fn reference(geometry: &ViewGeometry, assets: &Assets, vp: Viewport) -> Vec<RefPixel> {
    reference_with(geometry, assets, vp, |_, _, _| None)
}

/// The pixel whose color pixel `x` of a polygon's row shows: itself, or for polygons seen
/// in a mirror, which are shaded at half rate, the even pixel of its pair, counted from the
/// viewport's left edge `vx`, but not before the row's first pixel `x0`.
fn shown_pixel(p: &moose_view::ViewPolygon, x: i32, vx: i32, x0: i32) -> i32 {
    if p.mirror.is_none() {
        x
    } else {
        (x - (x - vx).rem_euclid(2)).max(x0)
    }
}

/// The perspective-correct interpolation parameter at pixel `x`'s center.
fn alpha_at(x: i32, xs: (f32, f32), (wl, wr): (f32, f32)) -> f32 {
    let s = ((x as f32 + 0.5 - xs.0) / (xs.1 - xs.0)).clamp(0.0, 1.0);
    s * wr / ((1.0 - s) * wl + s * wr)
}

/// Covered pixels of a polygon on a row, with its exact w, color and world position at
/// each pixel center.
fn polygon_row(
    geometry: &ViewGeometry,
    assets: &Assets,
    p: &moose_view::ViewPolygon,
    vx: i32,
    row: i32,
    mut each: impl FnMut(usize, f32, u32, Vec3),
) {
    let verts = &geometry.vertices[p.vertices()];
    let positions = &geometry.world_positions[p.vertices()];
    let lines = &geometry.edge_lines[p.vertices()];
    let mut attrs = Vec::new();
    let stride = geometry.vertex_attributes(p, assets.mesh(p.mesh), &mut attrs);
    let n = verts.len();
    let (mut left, mut right): (Crossing, Crossing) = (None, None);
    for i in 0..n {
        let (a, c) = (verts[i], verts[(i + 1) % n]);
        if row < pixel_edge(a.y.min(c.y)) || row >= pixel_edge(a.y.max(c.y)) {
            continue;
        }
        let x = lines[i]
            .unwrap_or(EdgeLine::between((a.x, a.y), (c.x, c.y)))
            .x_at_row(row);
        if c.y > a.y {
            if left.is_none_or(|(_, l)| x > l) {
                left = Some((i, x));
            }
        } else if right.is_none_or(|(_, r)| x < r) {
            right = Some((i, x));
        }
    }
    let (Some((li, xl)), Some((ri, xr))) = (left, right) else {
        return;
    };
    let cross = |i: usize| {
        let (a, c) = (verts[i], verts[(i + 1) % n]);
        let t = ((row as f32 + 0.5 - a.y) / (c.y - a.y)).clamp(0.0, 1.0);
        let alpha = t * c.w / ((1.0 - t) * a.w + t * c.w);
        let ca = &attrs[i * stride..i * stride + 3];
        let cc = &attrs[((i + 1) % n) * stride..((i + 1) % n) * stride + 3];
        let col: [f32; 3] = std::array::from_fn(|k| ca[k] + (cc[k] - ca[k]) * alpha);
        let pos = positions[i].lerp(positions[(i + 1) % n], alpha);
        (a.w + (c.w - a.w) * t, col, pos)
    };
    let ((wl, cl, pl), (wr, cr, pr)) = (cross(li), cross(ri));
    for x in pixel_edge(xl)..pixel_edge(xr) {
        let s = ((x as f32 + 0.5 - xl) / (xr - xl)).clamp(0.0, 1.0);
        let alpha = s * wr / ((1.0 - s) * wl + s * wr);
        let shown = shown_pixel(p, x, vx, pixel_edge(xl));
        let color_alpha = alpha_at(shown, (xl, xr), (wl, wr));
        let ch = |k: usize| (cl[k] + (cr[k] - cl[k]) * color_alpha).clamp(0.0, 255.0) as u32;
        each(
            x as usize,
            wl + (wr - wl) * s,
            ch(0) << 16 | ch(1) << 8 | ch(2),
            pl.lerp(pr, alpha),
        );
    }
}

/// The reference with some polygons translucent (`opacity` returns their alpha at a
/// world position, given the world position of the opaque surface behind it; whether it
/// returns one at all must depend only on the polygon): opaque
/// polygons resolved by nearest w, then translucent ones blended back to front (by their
/// nearest vertex w, as the renderer sorts) where nearer than the opaque surface.
fn reference_with(
    geometry: &ViewGeometry,
    assets: &Assets,
    vp: Viewport,
    opacity: impl Fn(&moose_view::ViewPolygon, Vec3, Vec3) -> Option<f32>,
) -> Vec<RefPixel> {
    let (w_px, h_px) = (vp.width as usize, vp.height as usize);
    let mut px = vec![RefPixel::default(); w_px * h_px];
    for (index, p) in geometry.polygons.iter().enumerate() {
        if opacity(p, Vec3::ZERO, Vec3::ZERO).is_some() {
            continue;
        }
        let index = index as u32 + 1; // 0 = nothing
        let verts = &geometry.vertices[p.vertices()];
        let lines = &geometry.edge_lines[p.vertices()];
        let mut attrs = Vec::new();
        let stride = geometry.vertex_attributes(p, assets.mesh(p.mesh), &mut attrs);
        let positions = &geometry.world_positions[p.vertices()];
        let n = verts.len();
        for row in 0..h_px as i32 {
            let (mut left, mut right): (Crossing, Crossing) = (None, None);
            for i in 0..n {
                let (a, c) = (verts[i], verts[(i + 1) % n]);
                if row < pixel_edge(a.y.min(c.y)) || row >= pixel_edge(a.y.max(c.y)) {
                    continue;
                }
                let x = lines[i]
                    .unwrap_or(EdgeLine::between((a.x, a.y), (c.x, c.y)))
                    .x_at_row(row);
                if c.y > a.y {
                    if left.is_none_or(|(_, l)| x > l) {
                        left = Some((i, x));
                    }
                } else if right.is_none_or(|(_, r)| x < r) {
                    right = Some((i, x));
                }
            }
            let (Some((li, xl)), Some((ri, xr))) = (left, right) else {
                continue;
            };
            // w and color at each crossing, perspective-correct along the edge.
            let cross = |i: usize| {
                let (a, c) = (verts[i], verts[(i + 1) % n]);
                let t = ((row as f32 + 0.5 - a.y) / (c.y - a.y)).clamp(0.0, 1.0);
                let alpha = t * c.w / ((1.0 - t) * a.w + t * c.w);
                let ca = &attrs[i * stride..i * stride + 3];
                let cc = &attrs[((i + 1) % n) * stride..((i + 1) % n) * stride + 3];
                let col: [f32; 3] = std::array::from_fn(|k| ca[k] + (cc[k] - ca[k]) * alpha);
                let pos = positions[i].lerp(positions[(i + 1) % n], alpha);
                (a.w + (c.w - a.w) * t, col, pos)
            };
            let ((wl, cl, pl), (wr, cr, pr)) = (cross(li), cross(ri));
            for x in pixel_edge(xl)..pixel_edge(xr) {
                let s = ((x as f32 + 0.5 - xl) / (xr - xl)).clamp(0.0, 1.0);
                let w = wl + (wr - wl) * s;
                let i = row as usize * w_px + x as usize;
                let alpha = s * wr / ((1.0 - s) * wl + s * wr);
                let shown = shown_pixel(p, x, vp.x as i32, pixel_edge(xl));
                let color_alpha = alpha_at(shown, (xl, xr), (wl, wr));
                let ch =
                    |k: usize| (cl[k] + (cr[k] - cl[k]) * color_alpha).clamp(0.0, 255.0) as u32;
                let c = ch(0) << 16 | ch(1) << 8 | ch(2);
                let p = &mut px[i];
                if w > p.w {
                    (p.second_polygon, p.second_w) = (p.polygon, p.w);
                    (p.color, p.w, p.polygon) = (c, w, index);
                    p.pos = pl.lerp(pr, alpha);
                } else if w > p.second_w {
                    (p.second_polygon, p.second_w) = (index, w);
                }
            }
        }
    }
    let mut translucent: Vec<(f32, &moose_view::ViewPolygon)> = geometry
        .polygons
        .iter()
        .filter(|p| opacity(p, Vec3::ZERO, Vec3::ZERO).is_some())
        .map(|p| {
            let max_w = geometry.vertices[p.vertices()]
                .iter()
                .map(|v| v.w)
                .fold(0.0, f32::max);
            (max_w, p)
        })
        .collect();
    translucent.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, p) in translucent {
        for row in 0..h_px as i32 {
            polygon_row(geometry, assets, p, vp.x as i32, row, |x, w, c, pos| {
                let pixel = &mut px[row as usize * w_px + x];
                if w > pixel.w {
                    let a = opacity(p, pos, pixel.pos).unwrap();
                    let alpha = (a.clamp(0.0, 1.0) * 255.0).round() as u32;
                    pixel.color = moose_raster::shader::blend(alpha << 24 | c, pixel.color);
                }
            });
        }
    }
    px
}

fn channel_diff(a: u32, b: u32) -> u32 {
    (0..3)
        .map(|k| ((a >> (8 * k)) & 255).abs_diff((b >> (8 * k)) & 255))
        .max()
        .unwrap()
}

/// Test-only shader that writes the polygon's number (from its uniforms) instead of a
/// color, so visibility can be compared exactly.
mod id_shader {
    use moose_raster::shader::{Material, PixelContext, SampleContext, U32s, VertexContext};
    moose_raster::material_io! { vertex {} sampled {} fixed32 {} fixed16 {} float {} }
    pub struct IdShader;
    impl Material for IdShader {
        moose_raster::material_types!();
        fn shade_vertex(_: &Vertex, _: &VertexContext) -> Sampled {
            Sampled {}
        }
        fn shade_sample(_: &SampledLanes, _: &SampleContext) -> Interp {
            Interp {}
        }
        fn shade_pixel(_: &Fixed32, _: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
            U32s::splat(ctx.params.values[0] as u32)
        }
    }
}

/// Renders each pixel's winning polygon number (1-based, 0 = background).
fn render_ids(
    r: &mut Renderer,
    id: MaterialId,
    geometry: &ViewGeometry,
    assets: &Assets,
    path: Option<RasterPath>,
) -> Vec<u32> {
    let mut pixels = vec![0xDEAD_BEEF; (VP.width * VP.height) as usize];
    let mut target = Target {
        pixels: &mut pixels,
        width: VP.width,
        height: VP.height,
    };
    let base = geometry.polygons.as_ptr();
    r.render(&mut target, VP, geometry, assets, |p| {
        let index = unsafe { (p as *const moose_view::ViewPolygon).offset_from(base) } as f32 + 1.0;
        Surface {
            material: id,
            params: Params::new(&[index]),
            textures: [None; moose_raster::MAX_TEXTURES],
            path_override: if p.kind == PolygonKind::World {
                None
            } else {
                path
            },
        }
    })
    .unwrap();
    pixels
}

#[test]
fn visibility_matches_the_reference_exactly() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = Renderer::new(RasterConfig::default());
    let id = r.register_material::<id_shader::IdShader>();
    let (mut pixels, mut ties) = (0usize, 0usize);
    let mut worst_gap = 0.0f32;
    for (i, cam) in random_views(&world, 300, 11, VP).iter().enumerate() {
        out.build(&world, &assets, &cam.view());
        let want = reference(&out, &assets, VP);
        for path in [Some(RasterPath::Span), Some(RasterPath::PerPixel)] {
            let got = render_ids(&mut r, id, &out, &assets, path);
            for (j, (&g, w)) in got.iter().zip(&want).enumerate() {
                pixels += 1;
                if g == w.polygon {
                    continue;
                }
                // Only acceptable in a near-tie: the runner-up, at practically the same depth.
                let gap = (w.w - w.second_w) / w.w;
                assert!(
                    g == w.second_polygon && gap < 1e-4,
                    "view {i} {path:?} pixel ({}, {}): got polygon {g}, nearest {} (w {}), runner-up {} (w {})",
                    j % VP.width as usize,
                    j / VP.width as usize,
                    w.polygon,
                    w.w,
                    w.second_polygon,
                    w.second_w
                );
                ties += 1;
                worst_gap = worst_gap.max(gap);
            }
        }
    }
    println!(
        "{pixels} pixels: {ties} near-ties resolved to the runner-up (largest relative w gap {worst_gap:e})"
    );
}

#[test]
fn colors_match_the_reference_closely() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = renderer(RasterConfig::default());
    let (mut pixels, mut off, mut worst) = (0usize, 0usize, 0u32);
    for (i, cam) in random_views(&world, 300, 11, VP).iter().enumerate() {
        out.build(&world, &assets, &cam.view());
        let got = render(&mut r, &out, &assets, VP, None);
        let want = reference(&out, &assets, VP);
        assert!(
            got.iter().all(|&c| c != 0xDEAD_BEEF),
            "view {i}: a pixel was never written"
        );
        for (&g, w) in got.iter().zip(&want) {
            pixels += 1;
            let d = channel_diff(g, w.color);
            worst = worst.max(d);
            if d > 2 {
                off += 1;
            }
        }
    }
    // Every-N perspective sampling (N >= 4, as the spec sets) and 8.8 stepping make small
    // differences; larger ones appear only on surfaces seen edge-on from very close.
    println!("{pixels} pixels: {off} off by more than 2 levels, worst {worst}");
    assert!(
        off * 1000 < pixels,
        "{off} of {pixels} pixels off by more than 2 levels"
    );
}

#[test]
fn per_pixel_path_matches_span_path() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = renderer(RasterConfig::default());
    let (mut pixels, mut differ) = (0usize, 0usize);
    for cam in random_views(&world, 300, 23, VP) {
        out.build(&world, &assets, &cam.view());
        let span = render(&mut r, &out, &assets, VP, Some(RasterPath::Span));
        let per_pixel = render(&mut r, &out, &assets, VP, Some(RasterPath::PerPixel));
        pixels += span.len();
        differ += span
            .iter()
            .zip(&per_pixel)
            .filter(|(a, b)| channel_diff(**a, **b) > 2)
            .count();
    }
    assert!(
        differ * 100_000 < pixels,
        "{differ} of {pixels} pixels differ between the paths"
    );
}

#[test]
fn threads_and_bands_do_not_change_the_image() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let cams = random_views(&world, 20, 5, VP);
    let one = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let many = rayon::ThreadPoolBuilder::new()
        .num_threads(8)
        .build()
        .unwrap();
    for cam in &cams {
        out.build(&world, &assets, &cam.view());
        let base = one.install(|| {
            render(
                &mut renderer(RasterConfig::default()),
                &out,
                &assets,
                VP,
                None,
            )
        });
        for band_rows in [1, 7, 16, 64, 1000] {
            // Setup split across threads (forced, for these small frames) or not.
            for parallel_setup in [0, usize::MAX] {
                let config = RasterConfig {
                    band_rows,
                    parallel_setup,
                    ..RasterConfig::default()
                };
                let img = many.install(|| render(&mut renderer(config), &out, &assets, VP, None));
                assert!(
                    img == base,
                    "band_rows {band_rows}, parallel_setup {parallel_setup} changed the image"
                );
            }
        }
    }
}

#[test]
fn splitscreen_viewports_draw_only_their_own_pixels() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = renderer(RasterConfig::default());
    let (w, h) = (320, 360);
    let mut pixels = vec![0xDEAD_BEEF; w * h];
    for (vp, z) in [
        (
            Viewport {
                x: 0,
                y: 0,
                width: 320,
                height: 180,
            },
            6.0,
        ),
        (
            Viewport {
                x: 0,
                y: 180,
                width: 320,
                height: 180,
            },
            -16.0,
        ),
    ] {
        let cam = camera(&world, Vec3::new(0.0, 1.7, z), 0.0, 0.0, 0.0, vp);
        out.build(&world, &assets, &cam.view());
        let mut target = Target {
            pixels: &mut pixels,
            width: w as u32,
            height: h as u32,
        };
        r.render(&mut target, vp, &out, &assets, |_| {
            Surface::new(MaterialId(0))
        })
        .unwrap();
    }
    assert!(pixels.iter().all(|&c| c != 0xDEAD_BEEF));
    // Top half looks down room_a toward the warm hallway; bottom half is inside cool room_b.
    let avg = |rows: std::ops::Range<usize>| {
        let px: Vec<u32> = rows
            .flat_map(|y| pixels[y * w..(y + 1) * w].to_vec())
            .collect();
        let sum = |k: u32| px.iter().map(|c| (c >> k) & 255).sum::<u32>() / px.len() as u32;
        (sum(16), sum(0)) // red, blue
    };
    let ((top_r, top_b), (bot_r, bot_b)) = (avg(0..180), avg(180..360));
    assert!(
        top_r > top_b && bot_b > bot_r,
        "top ({top_r}, {top_b}) should be warm, bottom ({bot_r}, {bot_b}) cool"
    );
}

#[test]
fn layout_errors_name_the_missing_attribute() {
    mod uv_shader {
        use moose_raster::shader::{Material, PixelContext, SampleContext, U32s, VertexContext};
        moose_raster::material_io! { vertex { uv: 2 } sampled {} fixed32 {} fixed16 {} float {} }
        pub struct NeedsUv;
        impl Material for NeedsUv {
            moose_raster::material_types!();
            fn shade_vertex(_: &Vertex, _: &VertexContext) -> Sampled {
                Sampled {}
            }
            fn shade_sample(_: &SampledLanes, _: &SampleContext) -> Interp {
                Interp {}
            }
            fn shade_pixel(_: &Fixed32, _: &Fixed16, _: &Floats, _: &PixelContext) -> U32s {
                U32s::splat(0)
            }
        }
    }
    use uv_shader::NeedsUv;
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    out.build(
        &world,
        &assets,
        &camera(&world, Vec3::new(0.0, 1.7, 6.0), 0.0, 0.0, 0.0, VP).view(),
    );
    let mut r = Renderer::new(RasterConfig::default());
    let uv = r.register_material::<NeedsUv>();
    let mut pixels = vec![0; (VP.width * VP.height) as usize];
    let mut target = Target {
        pixels: &mut pixels,
        width: VP.width,
        height: VP.height,
    };
    let err = r
        .render(&mut target, VP, &out, &assets, |_| Surface::new(uv))
        .unwrap_err();
    assert_eq!(
        err,
        LayoutError::MissingAttribute {
            mesh: "Two Rooms".into(),
            name: "uv"
        }
    );
}

/// A copy of `geometry` keeping only the polygons `keep` accepts.
fn filtered(
    geometry: &ViewGeometry,
    keep: impl Fn(&moose_view::ViewPolygon) -> bool,
) -> ViewGeometry {
    let mut g = ViewGeometry::new();
    g.vertices = geometry.vertices.clone();
    g.edge_lines = geometry.edge_lines.clone();
    g.world_positions = geometry.world_positions.clone();
    g.weights = geometry.weights.clone();
    g.objects = geometry.objects.clone();
    (g.eye, g.focal) = (geometry.eye, geometry.focal);
    g.polygons = geometry
        .polygons
        .iter()
        .filter(|p| keep(p))
        .copied()
        .collect();
    g
}

fn is_entity(p: &moose_view::ViewPolygon) -> bool {
    matches!(p.source, moose_view::PolygonSource::Entity { .. })
}

/// Renders with crates translucent at `alpha` (shader 1), everything else opaque (shader 0).
fn render_translucent(
    r: &mut Renderer,
    geometry: &ViewGeometry,
    assets: &Assets,
    alpha: f32,
) -> Vec<u32> {
    let mut pixels = vec![0xDEAD_BEEF; (VP.width * VP.height) as usize];
    let mut target = Target {
        pixels: &mut pixels,
        width: VP.width,
        height: VP.height,
    };
    r.render(&mut target, VP, geometry, assets, |p| {
        if is_entity(p) {
            let mut s = Surface::new(MaterialId(1));
            s.params.values[0] = alpha;
            s
        } else {
            Surface::new(MaterialId(0))
        }
    })
    .unwrap();
    pixels
}

fn translucent_renderer() -> Renderer {
    let mut r = renderer(RasterConfig::default());
    assert_eq!(
        r.register_material::<moose_raster::shaders::VertexColorTranslucent>(),
        MaterialId(1)
    );
    r
}

#[test]
fn invisible_translucent_crates_change_nothing() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = translucent_renderer();
    for cam in random_views(&world, 100, 31, VP) {
        out.build(&world, &assets, &cam.view());
        let with = render_translucent(&mut r, &out, &assets, 0.0);
        let without = render(
            &mut r,
            &filtered(&out, |p| !is_entity(p)),
            &assets,
            VP,
            None,
        );
        assert!(with == without, "alpha 0 crates changed the image");
    }
}

#[test]
fn a_fully_opaque_translucent_crate_matches_the_opaque_one() {
    // With a single crate there is no translucent-on-translucent ordering to differ.
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = translucent_renderer();
    let mut compared = 0;
    for cam in random_views(&world, 200, 37, VP) {
        out.build(&world, &assets, &cam.view());
        let first = out.polygons.iter().find_map(|p| match p.source {
            moose_view::PolygonSource::Entity { entity, .. } => Some(entity),
            _ => None,
        });
        let Some(first) = first else { continue };
        let one = filtered(
            &out,
            |p| !matches!(p.source, moose_view::PolygonSource::Entity { entity, .. } if entity != first),
        );
        let translucent = render_translucent(&mut r, &one, &assets, 1.0);
        let opaque = render(&mut r, &one, &assets, VP, None);
        assert!(
            translucent == opaque,
            "alpha 1 crate differs from the opaque crate"
        );
        compared += 1;
    }
    assert!(compared > 20);
}

#[test]
fn translucent_crates_match_the_reference() {
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = translucent_renderer();
    let (mut pixels, mut off, mut worst) = (0usize, 0usize, 0u32);
    for cam in random_views(&world, 300, 41, VP) {
        out.build(&world, &assets, &cam.view());
        let got = render_translucent(&mut r, &out, &assets, 0.5);
        let want = reference_with(&out, &assets, VP, |p, _, _| is_entity(p).then_some(0.5));
        for (&g, w) in got.iter().zip(&want) {
            pixels += 1;
            let d = channel_diff(g, w.color);
            worst = worst.max(d);
            if d > 2 {
                off += 1;
            }
        }
    }
    println!(
        "{pixels} pixels with half-transparent crates: {off} off by more than 2 levels, worst {worst}"
    );
    assert!(
        off * 1000 < pixels,
        "{off} of {pixels} pixels off by more than 2 levels"
    );
    // The largest differences come from the minimum sample interval (N >= 4 by default)
    // on surfaces seen edge-on from very close; blending adds the errors of two layers.
    assert!(worst <= 24, "worst difference {worst}");
}

#[test]
fn a_minimum_step_of_one_is_exact_on_steep_spans() {
    // With a sample on every pixel of steep spans, only 8.8 stepping and rounding remain.
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = translucent_renderer();
    r.config.min_step = 1;
    let mut worst = 0;
    for cam in random_views(&world, 300, 41, VP) {
        out.build(&world, &assets, &cam.view());
        let got = render_translucent(&mut r, &out, &assets, 0.5);
        let want = reference_with(&out, &assets, VP, |p, _, _| is_entity(p).then_some(0.5));
        worst = got
            .iter()
            .zip(&want)
            .map(|(&g, w)| channel_diff(g, w.color))
            .fold(worst, u32::max);
    }
    assert!(worst <= 2, "worst difference {worst} with min_step 1");
}

#[test]
fn fresnel_floors_match_the_reference() {
    // Shiny floors (seen directly) drawn over their reflections with the Fresnel shader,
    // and with the Fresnel disperse shader without and with a fade range, against the
    // reference computing the exact Fresnel term from each pixel's exact world position, and
    // the fade from the exact distance between the floor point and the real (unreflected)
    // point it reflects.
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let level = assets.load_level("shiny_rooms.mmp").unwrap();
    let world = World::new(level, &assets);
    let geometry = assets.mesh(world.geometry);
    let mut r = renderer(RasterConfig::default());
    let fresnel = r.register_material::<moose_raster::shaders::VertexColorFresnel>();
    let disperse = r.register_material::<moose_raster::shaders::VertexColorFresnelDisperse>();
    const F0: f32 = 0.15;
    // (shader, fade range): plain Fresnel, then dispersing without and with a fade.
    let cases = [(fresnel, 0.0), (disperse, 0.0), (disperse, 2.5)];
    // Shiny floors with their reflection drawn (one bounce deep, so all seen directly).
    let floor = |p: &moose_view::ViewPolygon| p.reflection.is_some();
    let normal = |p: &moose_view::ViewPolygon| match p.source {
        moose_view::PolygonSource::World { polygon, .. } => {
            geometry.polygons[polygon as usize].plane.normal
        }
        _ => unreachable!(),
    };
    let mut out = ViewGeometry::new();
    let (mut pixels, mut on_floor, mut faded) = (0usize, 0usize, 0usize);
    let (mut off, mut worst, mut worst_on_floor) = ([0usize; 3], [0u32; 3], [0u32; 3]);
    for cam in random_views(&world, 150, 43, VP) {
        let view = cam.view();
        out.build(&world, &assets, &view);
        let floors = reference_with(&out, &assets, VP, |p, _, _| floor(p).then_some(1.0));
        let plain = reference_with(&out, &assets, VP, |p, _, _| floor(p).then_some(0.0));
        let mut images = Vec::new();
        for (k, (shader, range)) in cases.into_iter().enumerate() {
            let mut got = vec![0xDEAD_BEEF; (VP.width * VP.height) as usize];
            let mut target = Target {
                pixels: &mut got,
                width: VP.width,
                height: VP.height,
            };
            r.render(&mut target, VP, &out, &assets, |p| {
                if floor(p) {
                    let mut s = Surface::new(shader);
                    s.params = Params::new(&[F0, range]);
                    s
                } else {
                    Surface::new(MaterialId(0))
                }
            })
            .unwrap();
            let want = reference_with(&out, &assets, VP, |p, pos, behind| {
                floor(p).then(|| {
                    let cos = normal(p)
                        .dot((view.position - pos).normalize())
                        .clamp(0.0, 1.0);
                    let falloff = moose_raster::shaders::vertex_color_fresnel::FALLOFF;
                    let fresnel = F0 + (1.0 - F0) * (1.0 - cos).powi(falloff);
                    let fade = if range > 0.0 {
                        (1.0 - (pos.distance(behind) / range).min(1.0)).powi(2)
                    } else {
                        1.0
                    };
                    1.0 - fresnel * fade
                })
            });
            for ((&g, w), (f, p)) in got.iter().zip(&want).zip(floors.iter().zip(&plain)) {
                let d = channel_diff(g, w.color);
                worst[k] = worst[k].max(d);
                if f.color != p.color {
                    worst_on_floor[k] = worst_on_floor[k].max(d);
                }
                if d > 2 {
                    off[k] += 1;
                }
            }
            images.push(got);
        }
        for (i, (f, p)) in floors.iter().zip(&plain).enumerate() {
            pixels += 1;
            if f.color != p.color {
                on_floor += 1;
                // Same Fresnel term; alpha is rounded in integers in one, floats in the other.
                assert!(
                    channel_diff(images[0][i], images[1][i]) <= 1,
                    "the disperse shader without a fade differs from plain Fresnel"
                );
                faded += (channel_diff(images[1][i], images[2][i]) > 4) as usize;
            }
        }
    }
    println!(
        "{pixels} pixels, about {on_floor} on shiny floors, {faded} visibly faded; off by more than 2 levels {off:?}, worst {worst:?} ({worst_on_floor:?} on floors) for Fresnel, disperse, disperse with a 2.5 m fade"
    );
    assert!(on_floor * 10 > pixels, "the views hardly saw the floor");
    assert!(faded * 4 > on_floor, "the fade hardly changed the floor");
    for k in 0..cases.len() {
        assert!(
            off[k] * 1000 < pixels,
            "{} of {pixels} pixels off by more than 2 levels",
            off[k]
        );
        // On the floors: the table's 1/255 steps in cos (up to FALLOFF/255 of alpha at
        // grazing angles) and cos stepped linearly between samples. Elsewhere, as in the
        // other color tests, the minimum sample interval on surfaces seen edge-on from close.
        assert!(
            worst_on_floor[k] <= 6,
            "worst difference {} on floors",
            worst_on_floor[k]
        );
        assert!(worst[k] <= 24, "worst difference {}", worst[k]);
    }
}

#[test]
fn shaders_see_their_screen_pixel() {
    // A shader writing its own framebuffer coordinates: every drawn pixel must name
    // itself, in an offset viewport, through every path (world spans, props, per-pixel
    // actors, translucent blending at alpha 1).
    mod coords {
        use moose_raster::shader::{Material, PixelContext, SampleContext, U32s, VertexContext};
        moose_raster::material_io! { vertex {} sampled {} fixed32 {} fixed16 {} float {} }
        pub struct Coords;
        impl Material for Coords {
            moose_raster::material_types!();
            fn shade_vertex(_: &Vertex, _: &VertexContext) -> Sampled {
                Sampled {}
            }
            fn shade_sample(_: &SampledLanes, _: &SampleContext) -> Interp {
                Interp {}
            }
            fn shade_pixel(_: &Fixed32, _: &Fixed16, _: &Floats, ctx: &PixelContext) -> U32s {
                let x: U32s = moose_raster::wide::bytemuck::cast(ctx.at.x_lanes());
                x | U32s::splat((ctx.at.y as u32) << 12)
            }
        }
        pub struct CoordsTranslucent;
        impl Material for CoordsTranslucent {
            moose_raster::material_types!();
            const TRANSLUCENT: bool = true;
            fn shade_vertex(_: &Vertex, _: &VertexContext) -> Sampled {
                Sampled {}
            }
            fn shade_sample(_: &SampledLanes, _: &SampleContext) -> Interp {
                Interp {}
            }
            fn shade_pixel(a: &Fixed32, b: &Fixed16, c: &Floats, ctx: &PixelContext) -> U32s {
                Coords::shade_pixel(a, b, c, ctx) | U32s::splat(0xFF00_0000)
            }
        }
    }
    use coords::{Coords, CoordsTranslucent};
    let (world, assets) = world();
    let mut out = ViewGeometry::new();
    let mut r = Renderer::new(RasterConfig::default());
    let (opaque, translucent) = (
        r.register_material::<Coords>(),
        r.register_material::<CoordsTranslucent>(),
    );
    let (w, h) = (400u32, 300u32);
    let vp = Viewport {
        x: 37,
        y: 53,
        width: 320,
        height: 180,
    };
    let mut drawn = 0;
    for (i, cam) in random_views(&world, 40, 57, vp).into_iter().enumerate() {
        out.build(&world, &assets, &cam.view());
        let mut pixels = vec![0u32; (w * h) as usize];
        let mut target = Target {
            pixels: &mut pixels,
            width: w,
            height: h,
        };
        r.render(&mut target, vp, &out, &assets, |p| {
            let mut s = Surface::new(if is_entity(p) && i % 2 == 0 {
                translucent
            } else {
                opaque
            });
            if is_entity(p) && i % 3 == 0 {
                s.path_override = Some(RasterPath::PerPixel);
            }
            s
        })
        .unwrap();
        for y in vp.y..vp.y + vp.height {
            for x in vp.x..vp.x + vp.width {
                let got = pixels[(y * w + x) as usize];
                if got != RasterConfig::default().background {
                    assert_eq!(got & 0xFF_FFFF, x | y << 12, "view {i}: pixel ({x}, {y})");
                    drawn += 1;
                }
            }
        }
    }
    assert!(drawn > 40 * 320 * 180 * 9 / 10);
}

/// A polygon's exact (perspective-correct) attribute values at each covered pixel's center
/// on `row`, with the same coverage rule as the renderer.
fn polygon_row_attributes(
    geometry: &ViewGeometry,
    assets: &Assets,
    p: &moose_view::ViewPolygon,
    row: i32,
    mut each: impl FnMut(i32, &[f32]),
) {
    let verts = &geometry.vertices[p.vertices()];
    let lines = &geometry.edge_lines[p.vertices()];
    let mut attrs = Vec::new();
    let stride = geometry.vertex_attributes(p, assets.mesh(p.mesh), &mut attrs);
    let n = verts.len();
    let (mut left, mut right): (Crossing, Crossing) = (None, None);
    for i in 0..n {
        let (a, c) = (verts[i], verts[(i + 1) % n]);
        if row < pixel_edge(a.y.min(c.y)) || row >= pixel_edge(a.y.max(c.y)) {
            continue;
        }
        let x = lines[i]
            .unwrap_or(EdgeLine::between((a.x, a.y), (c.x, c.y)))
            .x_at_row(row);
        if c.y > a.y {
            if left.is_none_or(|(_, l)| x > l) {
                left = Some((i, x));
            }
        } else if right.is_none_or(|(_, r)| x < r) {
            right = Some((i, x));
        }
    }
    let (Some((li, xl)), Some((ri, xr))) = (left, right) else {
        return;
    };
    let cross = |i: usize| {
        let (a, c) = (verts[i], verts[(i + 1) % n]);
        let t = ((row as f32 + 0.5 - a.y) / (c.y - a.y)).clamp(0.0, 1.0);
        let alpha = t * c.w / ((1.0 - t) * a.w + t * c.w);
        let j = (i + 1) % n;
        let v: Vec<f32> = (0..stride)
            .map(|k| {
                attrs[i * stride + k] + (attrs[j * stride + k] - attrs[i * stride + k]) * alpha
            })
            .collect();
        (a.w + (c.w - a.w) * t, v)
    };
    let ((wl, vl), (wr, vr)) = (cross(li), cross(ri));
    let mut v = vec![0.0; stride];
    for x in pixel_edge(xl)..pixel_edge(xr) {
        let alpha = alpha_at(x, (xl, xr), (wl, wr));
        for k in 0..stride {
            v[k] = vl[k] + (vr[k] - vl[k]) * alpha;
        }
        each(x, &v);
    }
}

#[test]
fn textured_floors_show_the_texel_under_each_pixel() {
    // Shiny floors without their reflections (0 bounces) drawn with the plain textured
    // shader: every floor pixel shows the texel at its exact perspective-correct uv, or
    // (where the per-pixel interpolation between sample points lands near a texel edge) a
    // neighboring one.
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let level = assets.load_level("shiny_rooms.mmp").unwrap();
    let world = World::new(level, &assets);
    let texture = assets.load_texture("test_floor.png").unwrap();
    let mut r = renderer(RasterConfig::default());
    let textured = r.register_material::<moose_raster::shaders::TexturedNearest>();
    let mut out = ViewGeometry::new();
    out.config.max_reflections = 0;
    let floor = |p: &moose_view::ViewPolygon| p.flags.reflective();
    let tex = assets.texture(texture);
    let (mut exact, mut near, mut total) = (0usize, 0usize, 0usize);
    for cam in random_views(&world, 60, 61, VP) {
        out.build(&world, &assets, &cam.view());
        let mut got = vec![0u32; (VP.width * VP.height) as usize];
        let mut target = Target {
            pixels: &mut got,
            width: VP.width,
            height: VP.height,
        };
        r.render(&mut target, VP, &out, &assets, |p| {
            if floor(p) {
                Surface {
                    textures: [Some(texture), None],
                    ..Surface::new(textured)
                }
            } else {
                Surface::new(MaterialId(0))
            }
        })
        .unwrap();
        let owners = reference(&out, &assets, VP);
        for (index, p) in out.polygons.iter().enumerate() {
            if !floor(p) {
                continue;
            }
            for row in 0..VP.height as i32 {
                // Each covered pixel's exact uv (color first, then uv).
                let mut uvs: Vec<(i32, f32, f32)> = Vec::new();
                polygon_row_attributes(&out, &assets, p, row, |x, v| uvs.push((x, v[3], v[4])));
                for (k, &(x, u, v)) in uvs.iter().enumerate() {
                    let i = (row as u32 * VP.width + x as u32) as usize;
                    if owners[i].polygon != index as u32 + 1 {
                        continue; // hidden
                    }
                    total += 1;
                    let texel =
                        |tx: i32, ty: i32| tex.texel(tx as u32 & 63, ty as u32 & 63) & 0xFF_FFFF;
                    let (tx, ty) = ((u * 64.0).floor() as i32, (v * 64.0).floor() as i32);
                    if got[i] == texel(tx, ty) {
                        exact += 1;
                        continue;
                    }
                    // Otherwise a texel within the pixel's footprint (the uvs of its
                    // neighbors on the row) or one beyond: far away, one pixel spans
                    // several texels.
                    let around = &uvs[k.saturating_sub(1)..(k + 2).min(uvs.len())];
                    let range = |f: fn(&(i32, f32, f32)) -> f32| {
                        let lo = around.iter().map(f).fold(f32::INFINITY, f32::min);
                        let hi = around.iter().map(f).fold(f32::NEG_INFINITY, f32::max);
                        ((lo * 64.0).floor() as i32 - 1)..=((hi * 64.0).floor() as i32 + 1)
                    };
                    let (us, vs) = (range(|e| e.1), range(|e| e.2));
                    assert!(
                        vs.clone()
                            .any(|ty| us.clone().any(|tx| got[i] == texel(tx, ty))),
                        "pixel ({x}, {row}): {:06x} is not within its footprint, texels {us:?} x {vs:?}",
                        got[i]
                    );
                    near += 1;
                }
            }
        }
    }
    println!("{total} floor pixels: {exact} exact, {near} another texel in their footprint");
    assert!(total > 100_000, "the views hardly saw the floor");
    // Texel edges shift by a fraction of a pixel where uv is interpolated linearly between
    // sample points (and far away every pixel spans several texels): 95.7% exact when
    // written.
    assert!(exact * 100 > total * 94, "{exact} of {total} exact");
}
