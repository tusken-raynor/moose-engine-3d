//! Ambient light probes (see `moose_assets::AmbientProbes`): a grid over the level, each
//! probe a tiny cube map of what can be seen from its point, lit as the level is, and
//! reduced to the six colors of an ambient cube. Read at sample points in place of the
//! level's ambient light, they light a surface from what is around it: brighter toward lit
//! walls and floors, darker toward dark ones, in their colors.
//!
//! Light bounced off surfaces is far dimmer than a level's ambient light, which stands for
//! all of it: cheese_1's bounce is a few thousandths where its ambient light is 0.1. So the
//! probes keep how the light differs from place to place and way to way, and are scaled so
//! that the middle probe is as bright as the level's ambient light.
//!
//! Probes are drawn a frame at a time, never all at once (see [`Updates`]): every thread
//! draws one, with a renderer and view of its own, in batches, as many as fit in the
//! frame's time. A level loaded or rebuilt (the editor) keeps the probes it had, or the
//! plain ambient light, until each is drawn, nearest the eye first. Then, as the game goes
//! on, probes are drawn again as what lights them changes: those where a light that moved
//! or changed shines (most where a spot light's beam lands, and any that saw where it
//! reaches), and around what moved (its shadow moves with it), first; then, relit, the
//! stalest anywhere with the time left, or drawn whole, every probe now and then, at the
//! probe rate near the eye and less often farther. Probes outside the level take their
//! neighbors' light whenever that changes.

use std::f32::consts::PI;
use std::sync::Arc;
use std::time::{Duration, Instant};

use glam::Vec3;
use moose_assets::{Ambient, AmbientProbes};
use moose_raster::{RasterConfig, Renderer};
use moose_scene::Camera;
use moose_view::ViewGeometry;
use rayon::prelude::*;

use crate::materials::ShaderIds;
use crate::{App, Scene, SceneDraw, draw_scene};

/// Sizes of each face of a probe's cube map to choose from, in pixels each way. Coarser,
/// small bright things (a lamp's pool) fall between pixels or on them as the probe's point
/// moves, and neighboring probes disagree: blotches between them. Only the six colors are
/// kept, whatever the size. (64 would be next, but the rasterizer trips on one of
/// cheese_1's probe views at that size: a lattice strip out of range in `row_points`,
/// after other views were drawn.)
pub const PROBE_SIZES: [f32; 3] = [8.0, 16.0, 32.0];

/// Sample spacing for drawing probes, in pixels: every few pixels of a face lit where it is
/// (the screen's spacing, up to 32 pixels, would light a whole face from its corners).
const PROBE_SPACING: u32 = 4;
const PROBE_MIN_STEP: u32 = 2;

/// Probe spacings to choose from, in meters (0: no probes).
pub const PROBE_SPACINGS: [f32; 4] = [0.0, 4.0, 2.0, 1.0];

/// How often the probes near the eye are drawn again, a second, to choose from (0: never).
/// Those farther are drawn `FAR_SLOWER` times less often.
pub const PROBE_RATES: [f32; 6] = [0.0, 1.0, 2.0, 4.0, 8.0, 16.0];

/// How long a near probe drawn again fades from its old light to its new, to choose from,
/// in seconds (0: at once). No longer than the time until it is drawn again.
pub const PROBE_FADES: [f32; 4] = [0.0, 0.05, 0.1, 0.25];

/// Probes within this many meters of the eye are near: drawn at the face size chosen, and
/// faded from their old light to their new. Farther, they count for less (a probe `NEAR`
/// away half as much, see `eye_weight`), are drawn at `FAR_SIZE` at most, and take their
/// new light at once. Past `FAR`, they aren't drawn again.
const NEAR: f32 = 8.0;
const FAR: f32 = 24.0;
const FAR_SIZE: u32 = 8;

/// Far from the eye, with `Settings::sparse_far_probes`, the probes between every other
/// one along each axis show the blend of those around them (see `compose`): none of it
/// within `SPARSE_FROM` meters of the eye, all of it past `SPARSE_TO` (past which they
/// aren't drawn again), eased between by distance.
const SPARSE_FROM: f32 = 8.0;
const SPARSE_TO: f32 = 16.0;

/// A probe drawn near stays near until this many meters past `NEAR`, so one about that far
/// from the eye doesn't change size back and forth as the eye moves.
const NEAR_SLACK: f32 = 2.0;

/// A probe drawn at another size than last time fades to its new light over at least this
/// many seconds: the sizes see small bright things differently, and it would jump.
const RESIZE_FADE: f32 = 0.5;

/// How much a change marks the probes it reaches for drawing again: those marked are drawn
/// first, the most marked first, wherever they are (see `update_probes`). A point light
/// marks those it lights, by how much light reaches them (none at its range); a spot light,
/// those around where its light lands, traced along rays over its cone (see `BEAM_RING`),
/// by how much light reaches there, out to `POOL` meters (or as wide as the cone is there);
/// an entity that moved, those around it, to none `MOVED` meters past its bounds; a
/// directional light (the sun), all of them a little.
const LIGHT_MARK: f32 = 1.0;
const POOL: f32 = 3.0;
const MOVED_MARK: f32 = 1.0;
const MOVED: f32 = 3.0;
const SUN_MARK: f32 = 0.25;
/// The least a light that changed marks a probe that saw a sector it reaches (see
/// `Updates::mark_seeing`): enough to be drawn again.
const SEEING_MARK: f32 = 0.02;
/// No probe is marked more than this.
const MOST_MARKED: f32 = 8.0;
/// Marks below this are let go: a light's farthest reach.
const LEAST_MARK: f32 = 0.01;

/// Most probes a frame considers drawing again besides those marked: more than it could.
const MOST_DUE: usize = 4096;

/// Rays traced over a spot light's cone to find where its light lands: its axis, and rings
/// around it at half its outer angle and at the outer angle, `BEAM_RING` each.
const BEAM_RING: usize = 6;

/// How much a probe at distance `d` from the eye counts, from 1 at the eye.
fn eye_weight(d: f32) -> f32 {
    1.0 / (1.0 + (d / NEAR) * (d / NEAR))
}

/// Time a frame spends drawing probes before it is drawn, at least: keeping them up to
/// date, and while a bake is under way (the batch under way when it runs out is finished).
/// Then, with a frame rate cap, the time left before the frame is due (see `idle_probes`).
const BUDGET: Duration = Duration::from_micros(1500);
const BAKE_BUDGET: Duration = Duration::from_millis(6);


/// The display's gamma: framebuffer pixels hold `linear^(1 / GAMMA)`.
const GAMMA: f32 = 2.2;

/// Per face of a probe's cube map (in `moose_assets::CUBE_FACES`' order), `size` pixels
/// each way, per pixel: the way it looks, and the solid angle it covers.
fn pixel_rays(size: u32) -> [Vec<(Vec3, f32)>; 6] {
    let n = size as f32;
    std::array::from_fn(|face| {
        let rotation = Camera::cube_face(Vec3::ZERO, 0, face, size).rotation();
        (0..size * size)
            .map(|i| {
                // The pixel's center on the view plane at depth 1 (90 degrees: -1 to 1 each
                // way, y up), and the solid angle a pixel there covers.
                let x = ((i % size) as f32 + 0.5) / n * 2.0 - 1.0;
                let y = 1.0 - ((i / size) as f32 + 0.5) / n * 2.0;
                let r2 = 1.0 + x * x + y * y;
                ((rotation * Vec3::new(x, y, -1.0)).normalize(), (2.0 / n) * (2.0 / n) / (r2 * r2.sqrt()))
            })
            .collect()
    })
}

/// A pixel's light, linear.
fn linear(pixel: u32) -> Vec3 {
    let c = |shift: u32| ((pixel >> shift & 255) as f32 / 255.0).powf(GAMMA);
    Vec3::new(c(16), c(8), c(0))
}

/// The luminance of linear RGB.
fn luma(c: Vec3) -> f32 {
    c.dot(Vec3::new(0.2126, 0.7152, 0.0722))
}

/// An ambient cube's mean brightness.
fn brightness(a: &Ambient) -> f32 {
    a.faces.iter().map(|&f| luma(f)).sum::<f32>() / 6.0
}

/// A thread's own renderer and view for drawing probes, its pixels, and the frame it last
/// gathered what blocks the lights in (see `ViewConfig::same_world`).
pub(crate) struct Drawer {
    renderer: Renderer,
    geometry: ViewGeometry,
    pixels: Vec<u32>,
    prepared: u64,
}

/// Draws a probe at `p` (in `sector`) with `drawer` and reduces it to an ambient cube, and
/// gives the sectors its views saw (see `Updates::seen`): the
/// level drawn six times from it, a 90 degree square view down each axis (`size` pixels
/// each way, `rays` theirs), lit as it is (its lights and its own ambient light, not the
/// probes'), of the things that move only those flagged (see `ViewConfig::moving_entities`),
/// and each axis way's color the
/// light from the half of all ways around it, by the cosine, over pi (so an ambient cube
/// lights a surface facing that way as what it sees would). What blocks the lights is
/// gathered once a frame (`frame`) per drawer: the world stands still while they draw.
fn draw_probe(
    scene: &Scene,
    drawer: &mut Drawer,
    frame: u64,
    (p, sector): (Vec3, u32),
    size: u32,
    rays: &[Vec<(Vec3, f32)>; 6],
) -> Result<(Ambient, Vec<u32>), String> {
    let mut faces = [Vec3::ZERO; 6];
    let mut seen: Vec<u32> = Vec::new();
    for (face, rays) in rays.iter().enumerate() {
        let camera = Camera::cube_face(p, sector, face, size);
        drawer.geometry.config.same_world = drawer.prepared == frame;
        drawer.prepared = frame;
        let pixels = &mut drawer.pixels[..(size * size) as usize];
        draw_scene(scene, &mut drawer.renderer, &mut drawer.geometry, &camera, pixels, size, size)?;
        seen.extend(drawer.geometry.visits.iter().map(|v| v.sector));
        for (&pixel, &(ray, solid)) in pixels.iter().zip(rays) {
            let light = linear(pixel) * solid;
            for (sum, way) in faces.iter_mut().zip(Ambient::DIRECTIONS) {
                *sum += light * ray.dot(way).max(0.0);
            }
        }
    }
    seen.sort_unstable();
    seen.dedup();
    Ok((Ambient { faces: faces.map(|f| f / PI) }, seen))
}

/// What a probe sees through one pixel of its views, of what never moves (see
/// `fill_probe`): how far along the pixel's ray, the surface's normal (in 127ths), its
/// colors (gamma-encoded, as drawn in an ambient light of 1: what it gives back of the
/// light; a sky, what it gives), what it is (`NOTHING`, `SURFACE` or `GIVES`), the sector it
/// is in, and its entity (`NO_ENTITY`, a level surface).
#[derive(Clone, Copy, Default)]
struct Seen {
    t: f32,
    normal: [i8; 3],
    what: u8,
    color: [u8; 3],
    sector: u16,
    entity: u16,
}

const NOTHING: u8 = 0;
const SURFACE: u8 = 1;
/// A surface drawn unlit (a sky): it gives its colors, whatever light falls on it.
const GIVES: u8 = 2;
const NO_ENTITY: u16 = u16::MAX;

/// Probes relit a batch at a time: this many to a thread.
const RELIGHT_BATCH: usize = 8;

/// A moving entity drawn in probes (`probes`), where it is now: what its polygons (facing
/// out) and colors are, for each probe's rays to meet it (see `Relight`).
struct Mover {
    entity: u32,
    sector: u32,
    center: Vec3,
    radius: f32,
    /// Each polygon's normal and corners.
    polygons: Vec<(Vec3, Vec<Vec3>)>,
    color: [u8; 3],
}

/// What relighting a probe reads (see `relight_probe`): the world and its lights now, what
/// blocks them (`view`, prepared), the level's own ambient light, and the moving things
/// drawn in probes.
struct Relight<'a> {
    world: &'a moose_scene::World,
    view: &'a ViewGeometry,
    ambient: Ambient,
    movers: &'a [Mover],
    decode: [f32; 256],
}

impl Relight<'_> {
    /// The light falling on a surface at `p` facing `normal` (in `sector`, on `entity` if
    /// one's), as a sample point would have it (see `shaders::diffuse`), with each light's
    /// shadow as carving would have it there (see `ViewGeometry::light_reaching`).
    fn light(&self, p: Vec3, normal: Vec3, sector: u32, entity: Option<u32>) -> Vec3 {
        let mut light = self.ambient.at(normal);
        let lights = self.world.lights();
        for &i in self.world.sector_lights(sector) {
            let l = &lights[i as usize];
            let d = l.position - p;
            let d2 = d.length_squared().max(1e-8);
            let t = (1.0 - d2 / (l.range * l.range)).max(0.0);
            let inv = d2.sqrt().recip();
            let cos = (normal.dot(d) * inv).max(0.0);
            let (scale, offset) = l.cone();
            let c = (offset - d.dot(l.direction) * inv * scale).clamp(0.0, 1.0);
            let mut k = t * t * cos * c * c * (3.0 - 2.0 * c);
            if k <= 0.0 {
                continue;
            }
            if let Some(slot) = l.shadow {
                k *= self.view.light_reaching(slot, p, sector, entity);
            }
            light += l.color * k;
        }
        light
    }

    /// A color's bytes, linear.
    fn linear(&self, c: [u8; 3]) -> Vec3 {
        Vec3::new(self.decode[c[0] as usize], self.decode[c[1] as usize], self.decode[c[2] as usize])
    }
}

/// Fills probe `p`'s (in `sector`) buffer of what it sees (see [`Seen`]) with `drawer`:
/// each of its six views drawn twice, `size` pixels each way (`rays` theirs), of the level
/// and what doesn't move (`ids`: which polygon covers each pixel, for where it is and which
/// way it faces; `colors`: its colors); and gives the sectors its views saw.
fn fill_probe(
    (ids, colors): (&Scene, &Scene),
    drawer: &mut Drawer,
    frame: u64,
    (p, sector): (Vec3, u32),
    size: u32,
    rays: &[Vec<(Vec3, f32)>; 6],
) -> Result<(Box<[Seen]>, Vec<u32>), String> {
    let n = (size * size) as usize;
    let mut buffer = vec![Seen::default(); 6 * n].into_boxed_slice();
    let mut seen: Vec<u32> = Vec::new();
    let (world, assets) = (ids.world, ids.assets);
    for (face, rays) in rays.iter().enumerate() {
        let camera = Camera::cube_face(p, sector, face, size);
        let pixels = &mut drawer.pixels[..n];
        drawer.geometry.config.same_world = drawer.prepared == frame;
        drawer.prepared = frame;
        draw_scene(ids, &mut drawer.renderer, &mut drawer.geometry, &camera, pixels, size, size)?;
        seen.extend(drawer.geometry.visits.iter().map(|v| v.sector));
        let g = &drawer.geometry;
        for ((s, &id), &(ray, _)) in buffer[face * n..(face + 1) * n].iter_mut().zip(pixels.iter()).zip(rays) {
            // Bit 23, drawn unlit (see `draw_scene`).
            let gives = id >> 23 & 1 != 0;
            let Some(vp) = ((id & 0x7F_FFFF) as usize).checked_sub(1).and_then(|i| g.polygons.get(i)) else {
                continue;
            };
            let mesh = assets.mesh(vp.mesh);
            let polygon = &mesh.polygons[vp.source_polygon() as usize];
            let normal = (g.objects[vp.object as usize].rotation * polygon.plane.normal).normalize_or_zero();
            let on = g.world_positions[vp.first_vertex as usize];
            let toward = normal.dot(ray);
            if toward > -1e-6 {
                continue;
            }
            let t = normal.dot(on - p) / toward;
            let hit = p + ray * t;
            let (sector, entity) = match vp.source {
                moose_view::PolygonSource::World { sector, .. } | moose_view::PolygonSource::Terrain { sector, .. } => (sector, NO_ENTITY),
                moose_view::PolygonSource::Entity { entity, .. } => {
                    (world.find_sector(hit).unwrap_or(world.entities[entity as usize].sector), entity as u16)
                }
            };
            *s = Seen {
                t,
                normal: normal.to_array().map(|v| (v * 127.0).round() as i8),
                what: if gives { GIVES } else { SURFACE },
                color: [0; 3],
                sector: sector as u16,
                entity,
            };
        }
        drawer.geometry.config.same_world = true;
        draw_scene(colors, &mut drawer.renderer, &mut drawer.geometry, &camera, pixels, size, size)?;
        for (s, &c) in buffer[face * n..(face + 1) * n].iter_mut().zip(pixels.iter()) {
            s.color = [(c >> 16) as u8, (c >> 8) as u8, c as u8];
        }
    }
    seen.sort_unstable();
    seen.dedup();
    Ok((buffer, seen))
}

/// Relights probe `p` from its buffer of what it sees (see [`Seen`]): each pixel's surface
/// lit as it is now (see `Relight::light`), unless a moving thing drawn in probes is nearer
/// along its ray, lit there; and each axis way's color the light from the half of all ways
/// around it, by the cosine, over pi, as drawing a probe has it (see `draw_probe`).
fn relight_probe(buffer: &[Seen], p: Vec3, rays: &[Vec<(Vec3, f32)>; 6], r: &Relight) -> Ambient {
    let n = rays[0].len();
    let mut faces = [Vec3::ZERO; 6];
    for (face, rays) in rays.iter().enumerate() {
        for (s, &(ray, solid)) in buffer[face * n..(face + 1) * n].iter().zip(rays) {
            let mut seen = *s;
            let mut nearest = if s.what == NOTHING { f32::INFINITY } else { s.t };
            // A moving thing in front of it.
            for m in r.movers {
                let to = m.center - p;
                let along = to.dot(ray);
                if along + m.radius < 0.0 || to.length_squared() - along * along > m.radius * m.radius || along - m.radius > nearest {
                    continue;
                }
                for (normal, corners) in &m.polygons {
                    let toward = normal.dot(ray);
                    if toward > -1e-6 {
                        continue;
                    }
                    let t = normal.dot(corners[0] - p) / toward;
                    if t <= 0.0 || t >= nearest {
                        continue;
                    }
                    let q = p + ray * t;
                    let k = corners.len();
                    let inside = (0..k).all(|i| (corners[(i + 1) % k] - corners[i]).cross(q - corners[i]).dot(*normal) >= -1e-5);
                    if inside {
                        nearest = t;
                        seen = Seen {
                            t,
                            normal: normal.to_array().map(|v| (v * 127.0).round() as i8),
                            what: SURFACE,
                            color: m.color,
                            sector: m.sector as u16,
                            entity: m.entity as u16,
                        };
                    }
                }
            }
            let light = match seen.what {
                SURFACE => {
                    let normal = Vec3::from_array(seen.normal.map(|v| v as f32 / 127.0)).normalize_or_zero();
                    let entity = (seen.entity != NO_ENTITY).then_some(seen.entity as u32);
                    r.linear(seen.color) * r.light(p + ray * seen.t, normal, seen.sector as u32, entity)
                }
                GIVES => r.linear(seen.color),
                _ => Vec3::ZERO,
            } * solid;
            for (sum, way) in faces.iter_mut().zip(Ambient::DIRECTIONS) {
                *sum += light * ray.dot(way).max(0.0);
            }
        }
    }
    Ambient { faces: faces.map(|f| f / PI) }
}

/// The moving entities drawn in probes (`probes`), where they are now (see [`Mover`]):
/// their colors the average of their material's first texture (its smallest mip), or a
/// mid grey.
fn movers(scene: &Scene) -> Vec<Mover> {
    let (world, assets) = (scene.world, scene.assets);
    let mut out = Vec::new();
    for (i, e) in world.entities.iter().enumerate() {
        if e.is_static || !e.in_probes || !e.kind.is_drawn() {
            continue;
        }
        let mesh = assets.mesh(e.mesh);
        let transform = e.transform();
        let polygons = mesh
            .polygons
            .iter()
            .filter(|p| !p.flags.proxy())
            .map(|p| ((e.rotation * p.plane.normal).normalize_or_zero(), mesh.polygon_points(p).map(|q| transform.transform_point3(q)).collect()))
            .collect();
        let texture = e.binding.as_ref().and_then(|b| {
            let r = scene.table.get(b.material, false, false);
            let place = crate::materials::Place { face: None, sector: Some(e.sector), entity: Some(i as u32) };
            scene.table.surface(r, place, world).textures[0]
        });
        let color = texture
            .and_then(|t| assets.texture(t).levels.last().map(|l| l.texel(0, 0)))
            .map_or([150; 3], |c| [(c >> 16) as u8, (c >> 8) as u8, c as u8]);
        let (center, radius) = ((e.bounds.min + e.bounds.max) * 0.5, (e.bounds.max - e.bounds.min).length() * 0.5);
        out.push(Mover { entity: i as u32, sector: e.sector, center, radius, polygons, color });
    }
    out
}

/// What draws the probes, a frame at a time (see the module): when each was last drawn and
/// the light it drew, those a bake still has to draw, and the near ones blending from their
/// old light to their new.
pub(crate) struct Updates {
    clock: Instant,
    /// When the bake under way began, until it is done.
    started: Option<Instant>,
    /// What the probes' light is multiplied by: what makes the middle one as bright as the
    /// ambient light (see the module). Worked out from the first probes a bake draws, and
    /// again from all of them once it is done; until then, the last bake's.
    gain: Option<f32>,
    /// Per probe: where it is, the sector it is in (none outside every sector: never
    /// drawn), when it was last drawn (seconds on `clock`; never, -infinity), and the
    /// light it drew (before `gain`).
    positions: Vec<Vec3>,
    sectors: Vec<Option<u32>>,
    drawn: Vec<f32>,
    raw: Vec<Ambient>,
    /// Per probe, the size its faces were last drawn at (0: never drawn).
    sizes: Vec<u32>,
    /// Per probe, the sectors its views saw when it was last drawn; and per sector, the
    /// probes that saw it: those a flagged entity moving there must be drawn again for.
    seen: Vec<Vec<u32>>,
    seen_by: Vec<Vec<usize>>,
    /// Probes the bake under way still has to draw, the nearest the eye last.
    pending: Vec<usize>,
    /// Per probe, how much changes since it was drawn mark it for drawing again (see
    /// `LIGHT_MARK`); and the lights, and the moving entities' places, last frame, to see
    /// what changed.
    marked: Vec<f32>,
    lights: Vec<moose_assets::Light>,
    /// Per light last frame, the sectors it reached.
    light_sectors: Vec<Vec<u32>>,
    places: Vec<Option<(Vec3, glam::Quat, f32, Vec<u32>)>>,
    /// Probes blending: which, from what light to what, from when, over how long.
    blending: Vec<(usize, Ambient, Ambient, f32, f32)>,
    /// Per probe, its own light, as drawn (faded in): what the grid shows of it, but far
    /// from the eye between every other one (see `compose`).
    own: Vec<Ambient>,
    /// The probes marked (see `marked`), wherever they are.
    marked_list: Vec<usize>,
    /// The probes outside the level, and those they take their light from (see
    /// `outside_fills`).
    fills: Vec<(usize, Vec<usize>)>,
    /// The probes' cube map faces' size, and per pixel, its ray (see `pixel_rays`); and the
    /// same for far probes (see `FAR_SIZE`).
    size: u32,
    rays: [Vec<(Vec3, f32)>; 6],
    far_size: u32,
    far_rays: [Vec<(Vec3, f32)>; 6],
    /// One per thread.
    drawers: Vec<Drawer>,
    frame: u64,
    /// How long a batch takes, of late (seconds): whether one fits in the time left; and
    /// a batch relit only (see `relight`).
    batch_time: f32,
    relight_time: f32,
    /// Probes are relit from buffers of what they see (`buffers`, per probe, filled the
    /// first time each is drawn; see `fill_probe`), not drawn again whole; `view` is for
    /// what blocks the lights (see `ViewGeometry::light_reaching`).
    relight: bool,
    buffers: Vec<Option<Box<[Seen]>>>,
    view: ViewGeometry,
}

impl Updates {
    /// Moves the blends on to `now`, in the probes' own light; gives those changed.
    fn blend(&mut self, now: f32) -> Vec<usize> {
        let mut changed = Vec::with_capacity(self.blending.len());
        let own = &mut self.own;
        self.blending.retain(|&(n, from, to, start, length)| {
            let t = ((now - start) / length).clamp(0.0, 1.0);
            own[n] = Ambient { faces: std::array::from_fn(|k| from.faces[k].lerp(to.faces[k], t)) };
            changed.push(n);
            t < 1.0
        });
        changed
    }

    /// Probe `n` was drawn, and its views saw sectors `seen`.
    fn saw(&mut self, n: usize, seen: Vec<u32>) {
        for &s in &self.seen[n] {
            if let Some(by) = self.seen_by.get_mut(s as usize) {
                by.retain(|&m| m != n);
            }
        }
        for &s in &seen {
            if let Some(by) = self.seen_by.get_mut(s as usize) {
                by.push(n);
            }
        }
        self.seen[n] = seen;
    }

    /// Marks probe `n` `mark` more (see `LIGHT_MARK`).
    fn add_mark(&mut self, n: usize, mark: f32) {
        if self.marked[n] <= LEAST_MARK && self.marked[n] + mark > LEAST_MARK {
            self.marked_list.push(n);
        }
        self.marked[n] = (self.marked[n] + mark).min(MOST_MARKED);
    }

    /// Whether probe `n` has been drawn.
    fn has_drawn(&self, n: usize) -> bool {
        self.drawn[n] > f32::NEG_INFINITY
    }

    /// Marks the probes within `radius` of `center` (see `LIGHT_MARK`): `mark` at the center,
    /// easing to none at `radius`, as far as `reaches` says each is reached.
    fn mark(&mut self, grid: &AmbientProbes, center: Vec3, radius: f32, mark: f32, reaches: impl Fn(Vec3) -> bool) {
        let lo = ((center - radius - grid.origin) / grid.spacing).floor().max(Vec3::ZERO);
        let hi = ((center + radius - grid.origin) / grid.spacing).ceil();
        for k in lo.z as usize..=(hi.z as usize).min(grid.dims[2] - 1) {
            for j in lo.y as usize..=(hi.y as usize).min(grid.dims[1] - 1) {
                for i in lo.x as usize..=(hi.x as usize).min(grid.dims[0] - 1) {
                    let n = grid.index([i, j, k]);
                    let d = self.positions[n].distance(center);
                    if self.sectors[n].is_some() && d < radius && reaches(self.positions[n]) {
                        let t = 1.0 - d / radius;
                        self.add_mark(n, mark * t * t);
                    }
                }
            }
        }
    }

    /// Marks the probes `light` reaches (see `LIGHT_MARK`), and those around where a spot
    /// light's beam lands (traced along its axis in `world`); gives where its light lands
    /// (a point light, where it is).
    fn mark_light(&mut self, grid: &AmbientProbes, world: &moose_scene::World, light: &moose_assets::Light) -> Vec<Vec3> {
        if light.directional {
            for n in 0..self.marked.len() {
                self.add_mark(n, SUN_MARK);
            }
            return Vec::new();
        }
        if light.is_point() || (light.sector as usize) >= world.sectors.len() {
            self.mark(grid, light.position, light.range, LIGHT_MARK, |_| true);
            return vec![light.position];
        }
        // How much of the light reaches a point (its falloff, see `shaders::diffuse`).
        let reach = |p: Vec3| {
            let t = (1.0 - p.distance_squared(light.position) / (light.range * light.range)).max(0.0);
            t * t
        };
        // Where its light lands: rays over its cone, each to the first surface.
        let axis = light.direction.normalize_or_zero();
        let helper = if axis.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let u = axis.cross(helper).normalize();
        let v = u.cross(axis);
        let outer = light.cos_outer.clamp(-1.0, 1.0).acos().min(1.4);
        let mut rays = vec![axis];
        for angle in [outer * 0.5, outer] {
            for k in 0..BEAM_RING {
                let a = k as f32 * std::f32::consts::TAU / BEAM_RING as f32;
                rays.push(axis * angle.cos() + (u * a.cos() + v * a.sin()) * angle.sin());
            }
        }
        let spread = (outer * 0.5).tan();
        let mut lands = Vec::new();
        for ray in rays {
            let hit = world.trace(light.sector, light.position, light.position + ray * light.range).position;
            let lit = reach(hit);
            if lit > LEAST_MARK {
                // As wide as the cone is there, at least `POOL`.
                let wide = POOL.max(hit.distance(light.position) * spread);
                self.mark(grid, hit, wide, LIGHT_MARK * lit, |_| true);
                lands.push(hit);
            }
        }
        lands
    }

    /// Marks every probe whose views saw one of `sectors` (those a light reaches), however
    /// far: it may see the light land (`lands`), and keep it there until drawn again. By
    /// how near it is to where it lands (`POOL` and nearer, `LIGHT_MARK`), and never so
    /// little that it isn't drawn again.
    fn mark_seeing(&mut self, sectors: &[u32], lands: &[Vec3]) {
        let mut probes: Vec<usize> = sectors.iter().filter_map(|&s| self.seen_by.get(s as usize)).flatten().copied().collect();
        probes.sort_unstable();
        probes.dedup();
        for n in probes {
            let d = lands.iter().map(|&q| q.distance(self.positions[n])).fold(f32::INFINITY, f32::min);
            let near = (POOL / d.max(POOL)).powi(2);
            self.add_mark(n, (LIGHT_MARK * near).max(SEEING_MARK));
        }
    }

    /// Marks the probes reached by what changed since last frame in `world`: its lights
    /// (where each shone, and where it shines now), and the moving entities that moved.
    fn mark_changes(&mut self, grid: &AmbientProbes, world: &moose_scene::World) {
        let lights = world.lights();
        let changed: Vec<usize> =
            (0..lights.len().max(self.lights.len())).filter(|&i| lights.get(i) != self.lights.get(i)).collect();
        if !changed.is_empty() {
            let reaches = light_sectors(world);
            for &i in &changed {
                let (now, was) = (lights.get(i).copied(), self.lights.get(i).copied());
                let mut lands = Vec::new();
                for light in [now, was].into_iter().flatten() {
                    lands.extend(self.mark_light(grid, world, &light));
                }
                let mut sectors = reaches.get(i).cloned().unwrap_or_default();
                sectors.extend(self.light_sectors.get(i).into_iter().flatten());
                sectors.sort_unstable();
                sectors.dedup();
                if !(now.is_some_and(|l| l.directional) || was.is_some_and(|l| l.directional)) {
                    self.mark_seeing(&sectors, &lands);
                }
            }
            self.light_sectors = reaches;
        }
        self.lights = lights.to_vec();
        self.places.resize(world.entities.len(), None);
        for (i, e) in world.entities.iter().enumerate() {
            if e.is_static || !e.kind.is_drawn() {
                continue;
            }
            let moved = self.places[i].as_ref().is_none_or(|(p, r, s, _)| (*p, *r, *s) != (e.position, e.rotation, e.scale));
            if !moved {
                continue;
            }
            let (center, radius) = ((e.bounds.min + e.bounds.max) * 0.5, (e.bounds.max - e.bounds.min).length() * 0.5);
            let was = self.places[i].replace((e.position, e.rotation, e.scale, e.sectors.clone()));
            let Some((.., was_in)) = was else { continue };
            // Its shadow, around it.
            self.mark(grid, center, radius + MOVED, MOVED_MARK, |_| true);
            // Drawn in probes: those that see where it was or is, by how much of their view
            // it fills.
            if e.in_probes {
                let mut probes: Vec<usize> = was_in
                    .iter()
                    .chain(&e.sectors)
                    .filter_map(|&s| self.seen_by.get(s as usize))
                    .flatten()
                    .copied()
                    .collect();
                probes.sort_unstable();
                probes.dedup();
                for n in probes {
                    let fills = (radius / self.positions[n].distance(center).max(radius)).min(1.0);
                    self.add_mark(n, MOVED_MARK * fills);
                }
            }
        }
    }
}

impl App {
    /// Starts baking the level's ambient light probes, `settings.probe_spacing` apart (none
    /// at 0), `settings.probe_size` pixels a face each way: the grid made, and each grid
    /// point inside a sector to be drawn, nearest the eye first, a frame at a time (see
    /// `update_probes`; `finish_probes` draws them all now). Until a probe is drawn it keeps
    /// the light it had (the same grid baked before) or the plain ambient light.
    pub(crate) fn bake_probes(&mut self) -> Result<(), String> {
        let (old, old_updates) = (self.world.probes.take(), self.probe_updates.take());
        let spacing = self.settings.probe_spacing;
        if spacing <= 0.0 {
            return Ok(());
        }
        let positions = &self.assets.mesh(self.world.geometry).positions;
        let (min, max) = positions
            .iter()
            .fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &p| (lo.min(p), hi.max(p)));
        let mut grid = AmbientProbes::covering(min, max, spacing);
        let same = old.as_ref().is_some_and(|o| (o.origin, o.spacing, o.dims) == (grid.origin, grid.spacing, grid.dims));
        let gain = old_updates.as_ref().filter(|_| same).and_then(|u| u.gain);
        match old.filter(|_| same) {
            Some(old) => grid.probes.clone_from(&old.probes),
            None => grid.probes.fill(Ambient::uniform(self.world.ambient)),
        }
        grid.ambient = self.world.ambient;
        let positions: Vec<Vec3> = grid.cells().map(|cell| grid.position(cell)).collect();
        let sectors: Vec<Option<u32>> = positions.iter().map(|&p| self.world.find_sector(p)).collect();
        let eye = self.camera.position;
        let mut pending: Vec<usize> = (0..positions.len()).filter(|&i| sectors[i].is_some()).collect();
        pending.sort_by(|&a, &b| positions[b].distance(eye).total_cmp(&positions[a].distance(eye)));
        let size = self.settings.probe_size;
        // A renderer and view for each thread, drawing as the screen's does, but lit
        // densely, and with none of its overlays.
        let config = RasterConfig {
            light_spacing: PROBE_SPACING,
            min_step: PROBE_MIN_STEP,
            show_samples: false,
            smooth_edges: false,
            ..self.renderer.config
        };
        // Those of the last bake kept (their renderers' materials are the same), their
        // static lights' shadows baked again for the world as it is now.
        let mut drawers: Vec<Drawer> = old_updates.map(|u| u.drawers).unwrap_or_default();
        drawers.resize_with(rayon::current_num_threads(), || {
            let mut renderer = Renderer::new(config);
            ShaderIds::register(&mut renderer);
            Drawer { renderer, geometry: ViewGeometry::new(), pixels: Vec::new(), prepared: 0 }
        });
        for d in &mut drawers {
            d.renderer.config = config;
            d.geometry.config = self.geometry.config;
            d.geometry.config.moving_entities = if self.settings.probe_relight {
                // Relit, what moves is met along each ray as it is now.
                moose_view::MovingEntities::None
            } else {
                moose_view::MovingEntities::Flagged
            };
            d.geometry.config.max_reflections = 0;
            d.geometry.clear_shadows();
            d.geometry.bake_shadows(&self.world, &self.assets);
            d.pixels.resize((size * size) as usize, 0);
            d.prepared = 0;
        }
        let fills = outside_fills(&grid, &sectors.iter().map(Option::is_some).collect::<Vec<_>>());
        self.probe_updates = Some(Updates {
            clock: Instant::now(),
            started: Some(Instant::now()),
            gain,
            drawn: vec![f32::NEG_INFINITY; positions.len()],
            raw: vec![Ambient::default(); positions.len()],
            sizes: vec![0; positions.len()],
            seen: vec![Vec::new(); positions.len()],
            seen_by: vec![Vec::new(); self.world.sectors.len()],
            positions,
            sectors,
            marked: Vec::new(),
            lights: self.world.lights().to_vec(),
            light_sectors: light_sectors(&self.world),
            places: Vec::new(),
            pending,
            blending: Vec::new(),
            own: grid.probes.clone(),
            marked_list: Vec::new(),
            fills,
            size,
            rays: pixel_rays(size),
            far_size: size.min(FAR_SIZE),
            far_rays: pixel_rays(size.min(FAR_SIZE)),
            drawers,
            frame: 0,
            batch_time: 0.0,
            relight_time: 0.0,
            relight: self.settings.probe_relight,
            buffers: Vec::new(),
            view: ViewGeometry::new(),
        });
        self.world.probes = Some(Arc::new(grid));
        Ok(())
    }

    /// Draws every probe a bake still has to, now (for a screenshot).
    pub(crate) fn finish_probes(&mut self) -> Result<(), String> {
        while self.probe_updates.as_ref().is_some_and(|u| !u.pending.is_empty()) {
            self.draw_probes(Duration::MAX, false)?;
        }
        Ok(())
    }

    /// Draws more of the probes due (see `update_probes`) after a frame is drawn, until
    /// `until`: the time left before it is due to be shown. Only batches that fit by how long
    /// one takes of late: a frame is never late for them.
    pub(crate) fn idle_probes(&mut self, until: Instant) -> Result<(), String> {
        let started = Instant::now();
        let Some(left) = until.checked_duration_since(started) else {
            return Ok(());
        };
        let before = self.probe_ms;
        let result = self.draw_probes(left, true);
        self.probe_ms = before + started.elapsed().as_secs_f64() * 1000.0;
        result
    }

    /// Draws the probes due this frame, before it is drawn (see [`Updates`]): while a bake
    /// is under way, those it still has to draw, nearest the eye first, in [`BAKE_BUDGET`];
    /// then, with `settings.probe_rate` (none at 0), those what changed marks and those not
    /// drawn for a while (see `LIGHT_MARK`; relit, the stalest, as many as fit), in
    /// [`BUDGET`]. Near ones fade from their old light to their new.
    pub(crate) fn update_probes(&mut self) -> Result<(), String> {
        let baking = self.probe_updates.as_ref().is_some_and(|u| !u.pending.is_empty());
        self.draw_probes(if baking { BAKE_BUDGET } else { BUDGET }, false)
    }

    /// Draws the probes due (see `update_probes`) for up to `budget`: a batch at a time,
    /// one probe a thread (relit, eight). Only batches that fit, by how long one takes of
    /// late; but the first whatever it takes, unless `fit`.
    fn draw_probes(&mut self, budget: Duration, fit: bool) -> Result<(), String> {
        if self.geometry.config.basic_ambient {
            return Ok(());
        }
        let (Some(mut grid), Some(mut u)) = (self.world.probes.take(), self.probe_updates.take()) else {
            return Ok(());
        };
        // Only this one holds them now: changed in place.
        self.geometry.probes = None;
        let (result, mut changed) = match self.draw_due(&mut grid, &mut u, budget, fit) {
            Ok(drawn) => (Ok(()), drawn),
            Err(e) => (Err(e), Vec::new()),
        };
        let now = u.clock.elapsed().as_secs_f32();
        changed.extend(u.blend(now));
        // What the grid shows: each changed probe's own light, and far away, those between
        // every other one blended from them; and around the eye, where that blend eases in,
        // all of them, as the eye moves.
        let sparse = self.settings.sparse_far_probes && self.settings.probe_rate > 0.0 && u.pending.is_empty();
        let eye = self.camera.position;
        if !changed.is_empty() || sparse {
            let probes = Arc::make_mut(&mut grid);
            // Per probe, whether what the grid shows of it changed (or may have): each once.
            let mut touched = vec![false; probes.probes.len()];
            let mut compose_list = Vec::with_capacity(changed.len());
            let mut touch = |n: usize, list: &mut Vec<usize>| {
                if !std::mem::replace(&mut touched[n], true) {
                    list.push(n);
                }
            };
            for &n in &changed {
                touch(n, &mut compose_list);
                if sparse && on_coarse(cell_of(probes, n)) {
                    for m in neighbors(probes, n) {
                        touch(m, &mut compose_list);
                    }
                }
            }
            if sparse {
                let (from, to) = (SPARSE_FROM - probes.spacing, SPARSE_TO + probes.spacing);
                let lo = ((eye - to - probes.origin) / probes.spacing).floor().max(Vec3::ZERO);
                let hi = ((eye + to - probes.origin) / probes.spacing).ceil();
                for k in lo.z as usize..=(hi.z as usize).min(probes.dims[2] - 1) {
                    for j in lo.y as usize..=(hi.y as usize).min(probes.dims[1] - 1) {
                        for i in lo.x as usize..=(hi.x as usize).min(probes.dims[0] - 1) {
                            let n = probes.index([i, j, k]);
                            let d = u.positions[n].distance(eye);
                            if d >= from && d <= to && !on_coarse([i, j, k]) {
                                touch(n, &mut compose_list);
                            }
                        }
                    }
                }
            }
            for &n in &compose_list {
                compose(probes, &u, n, eye, sparse);
            }
            // The probes outside the level beside them follow.
            refill(probes, &u.fills, Some(&mut touched));
        }
        self.world.probes = Some(grid);
        self.probe_updates = Some(u);
        result
    }

    /// Draws the probes due (see `draw_probes`); gives those drawn.
    fn draw_due(&mut self, grid: &mut Arc<AmbientProbes>, u: &mut Updates, budget: Duration, fit: bool) -> Result<Vec<usize>, String> {
        u.frame += 1;
        let now = u.clock.elapsed().as_secs_f32();
        let rate = self.settings.probe_rate;
        let eye = self.camera.position;
        // Those due, the first first: a bake's; or those changes marked, the most marked
        // first, wherever they are; then those not drawn for a while, by how long, and by
        // their distance from the eye.
        let baking = !u.pending.is_empty();
        if u.marked.len() != u.positions.len() {
            u.marked = vec![0.0; u.positions.len()];
        }
        // While a bake is under way, those it drew already that changes marked since go
        // first: drawn under a light since turned off, they would keep it until drawn again.
        let mut redrawn = 0;
        let due: Vec<usize> = if baking {
            let mut due = Vec::new();
            if rate > 0.0 {
                u.mark_changes(grid, &self.world);
                u.marked_list.retain(|&n| u.marked[n] > LEAST_MARK);
                let mut marked: Vec<(f32, usize)> =
                    u.marked_list.iter().filter(|&&n| u.has_drawn(n)).map(|&n| (u.marked[n], n)).collect();
                marked.sort_by(|a, b| b.0.total_cmp(&a.0));
                due.extend(marked.into_iter().map(|(_, n)| n));
                redrawn = due.len();
            }
            due.extend(u.pending.iter().rev().copied());
            due
        } else if rate > 0.0 {
            u.mark_changes(grid, &self.world);
            // Far away, every other one along each axis, if so (see `compose`).
            let sparse = |n: usize, d: f32| self.settings.sparse_far_probes && d > SPARSE_TO && !on_coarse(cell_of(grid, n));
            // Those marked, wherever they are.
            u.marked_list.retain(|&n| u.marked[n] > LEAST_MARK);
            let mut marked: Vec<(f32, usize)> = u
                .marked_list
                .iter()
                .filter(|&&n| u.sectors[n].is_some() && !sparse(n, u.positions[n].distance(eye)))
                .map(|&n| (u.marked[n], n))
                .collect();
            // Then the rest by how stale they are. Relit, wherever they are, and all of them,
            // as many as the time allows: relighting is cheap, and a change no mark caught
            // (light seen from afar, bounced on) is gone in a few frames. Drawn whole, only
            // those within `FAR` not drawn for a while, by the rate.
            let mut scored: Vec<(f32, usize)> = Vec::new();
            let reach = if u.relight { i64::MAX / 4 } else { (FAR / grid.spacing).ceil() as i64 };
            let at = ((eye - grid.origin) / grid.spacing).round().as_i64vec3();
            for k in (at.z - reach).max(0)..(at.z + reach + 1).min(grid.dims[2] as i64) {
                for j in (at.y - reach).max(0)..(at.y + reach + 1).min(grid.dims[1] as i64) {
                    for i in (at.x - reach).max(0)..(at.x + reach + 1).min(grid.dims[0] as i64) {
                        let n = grid.index([i as usize, j as usize, k as usize]);
                        let d = u.positions[n].distance(eye);
                        if u.sectors[n].is_none() || (!u.relight && d > FAR) || sparse(n, d) || u.marked[n] > LEAST_MARK {
                            continue;
                        }
                        let score = (now - u.drawn[n]) * rate * eye_weight(d);
                        if u.relight || score >= 1.0 {
                            scored.push((score, n));
                        }
                    }
                }
            }
            marked.sort_by(|a, b| b.0.total_cmp(&a.0));
            // No more than a frame could ever take, the stalest.
            if scored.len() > MOST_DUE {
                scored.select_nth_unstable_by(MOST_DUE, |a, b| b.0.total_cmp(&a.0));
                scored.truncate(MOST_DUE);
            }
            scored.sort_by(|a, b| b.0.total_cmp(&a.0));
            marked.into_iter().chain(scored).map(|(_, n)| n).collect()
        } else {
            Vec::new()
        };
        if due.is_empty() {
            return Ok(Vec::new());
        }
        // Without updates, what a bake draws stays: drawn without the flashlight, or
        // wherever the player happened to point it would stay lit.
        if rate <= 0.0 {
            self.apply_lights(false);
        }
        let table = self.materials.table(&self.shaders, &self.frame_settings(), &self.world);
        let scene = Scene {
            world: &self.world,
            assets: &self.assets,
            shaders: &self.shaders,
            table: &table,
            cube_maps: &self.cube_maps,
            level_filters: &self.level_filters,
            entity_filters: &self.entity_filters,
            per_pixel: self.settings.per_pixel_crates,
            time: self.time,
            plain_copies: true,
            draw: crate::SceneDraw::Lit,
        };
        let (ids, colors) = (Scene { draw: SceneDraw::Ids, ..scene }, Scene { draw: SceneDraw::Colors, ..scene });
        // Relit: what blocks the lights now, the moving things drawn in probes, how each
        // color decodes.
        let relit_movers = if u.relight {
            if u.buffers.len() != u.positions.len() {
                u.buffers = vec![None; u.positions.len()];
            }
            u.view.config = self.geometry.config;
            u.view.prepare_lights(&self.world, &self.assets);
            movers(&scene)
        } else {
            Vec::new()
        };
        let decode = std::array::from_fn(|i| (i as f32 / 255.0).powf(GAMMA));
        let ambient = Ambient::shaped(self.world.ambient, &self.world.ambient_shape);
        let started = Instant::now();
        let frame = u.frame;
        let mut result = Ok(());
        let mut done = 0;
        // Per probe drawn: whether it is near, and the size it was drawn at before.
        let mut drawn_near: Vec<(bool, u32)> = Vec::with_capacity(due.len());
        loop {
            // Relit, a batch of those whose buffers are filled, or of those to fill.
            let threads = u.drawers.len();
            let relit_only = u.relight && due[done..].iter().take(threads).all(|&n| u.buffers[n].is_some());
            let (count, took) = if relit_only { (threads * RELIGHT_BATCH, u.relight_time) } else { (threads, u.batch_time) };
            let spent = started.elapsed();
            let next = spent + Duration::from_secs_f32(took);
            // The first batch whatever it takes (unless `fit`), then those that fit.
            let room = next <= budget || (!fit && done == 0);
            if done >= due.len() || !room {
                break;
            }
            let batch_started = Instant::now();
            if relit_only {
                // Up to the first whose buffer isn't filled yet.
                let filled = due[done..].iter().take(count).take_while(|&&n| u.buffers[n].is_some()).count();
                let batch = &due[done..done + filled];
                let relight = Relight { world: &self.world, view: &u.view, ambient, movers: &relit_movers, decode };
                let (buffers, positions, rays) = (&u.buffers, &u.positions, &u.rays);
                let lit: Vec<Ambient> = batch
                    .par_iter()
                    .map(|&n| relight_probe(buffers[n].as_deref().unwrap_or_default(), positions[n], rays, &relight))
                    .collect();
                for (&n, a) in batch.iter().zip(lit) {
                    drawn_near.push((true, u.sizes[n]));
                    (u.raw[n], u.drawn[n], u.marked[n], u.sizes[n]) = (a, now, 0.0, u.size);
                }
                done += batch.len();
                if batch.len() == count {
                    let took = batch_started.elapsed().as_secs_f32();
                    u.relight_time = if u.relight_time > 0.0 { u.relight_time * 0.8 + took * 0.2 } else { took };
                }
                continue;
            }
            let batch = &due[done..(done + threads).min(due.len())];
            // Near or far (with some slack for those drawn near before); far ones smaller,
            // but not for a bake, whose probes may stay.
            let nears: Vec<bool> = batch
                .iter()
                .map(|&n| {
                    let d = u.positions[n].distance(eye);
                    baking || d <= NEAR || (u.sizes[n] == u.size && d <= NEAR + NEAR_SLACK)
                })
                .collect();
            let (positions, sectors) = (&u.positions, &u.sectors);
            let (near, far) = ((u.size, &u.rays), (u.far_size, &u.far_rays));
            let relight = u.relight.then(|| Relight { world: &self.world, view: &u.view, ambient, movers: &relit_movers, decode });
            let buffers = &u.buffers;
            // Each probe drawn whole; or relit, its buffer filled first if it isn't.
            type Drawn = Result<(Ambient, Option<(Box<[Seen]>, Vec<u32>)>), String>;
            let drawn: Vec<Drawn> = u.drawers[..batch.len()]
                .par_iter_mut()
                .zip(batch.par_iter().zip(nears.par_iter()))
                .map(|(drawer, (&n, &is_near))| {
                    let at = (positions[n], sectors[n].unwrap_or(0));
                    match &relight {
                        Some(relight) => match &buffers[n] {
                            Some(buffer) => Ok((relight_probe(buffer, positions[n], near.1, relight), None)),
                            None => {
                                let (buffer, seen) = fill_probe((&ids, &colors), drawer, frame, at, near.0, near.1)?;
                                Ok((relight_probe(&buffer, positions[n], near.1, relight), Some((buffer, seen))))
                            }
                        },
                        None => {
                            let (size, rays) = if is_near { near } else { far };
                            let (a, seen) = draw_probe(&scene, drawer, frame, at, size, rays)?;
                            Ok((a, Some((Box::default(), seen))))
                        }
                    }
                })
                .collect();
            for ((&n, a), is_near) in batch.iter().zip(drawn).zip(nears) {
                match a {
                    Ok((a, filled)) => {
                        let is_near = is_near || u.relight;
                        let size = if is_near { u.size } else { u.far_size };
                        if let Some((buffer, seen)) = filled {
                            u.saw(n, seen);
                            if u.relight {
                                u.buffers[n] = Some(buffer);
                            }
                        }
                        drawn_near.push((is_near, u.sizes[n]));
                        (u.raw[n], u.drawn[n], u.marked[n], u.sizes[n]) = (a, now, 0.0, size);
                    }
                    Err(e) => result = Err(e),
                }
            }
            done += batch.len();
            // How long a batch takes: a full one's, a mean of late.
            if batch.len() == u.drawers.len() {
                let took = batch_started.elapsed().as_secs_f32();
                u.batch_time = if u.batch_time > 0.0 { u.batch_time * 0.8 + took * 0.2 } else { took };
            }
            if result.is_err() {
                break;
            }
        }
        if rate <= 0.0 {
            self.apply_lights(true);
        }
        let probes = Arc::make_mut(grid);
        if baking {
            u.pending.truncate(u.pending.len() - done.saturating_sub(redrawn));
            // The gain: from the first probes drawn, and once all are, from all of them;
            // those drawn before, again with it.
            let finished = u.pending.is_empty();
            if u.gain.is_none() || finished {
                let mut lights: Vec<f32> = (0..u.raw.len()).filter(|&n| u.has_drawn(n)).map(|n| brightness(&u.raw[n])).collect();
                lights.sort_by(f32::total_cmp);
                let middle = lights.get(lights.len() / 2).copied().unwrap_or(0.0);
                let gain = if middle > 0.0 { luma(self.world.ambient) / middle } else { 1.0 };
                if u.gain.is_some_and(|g| g != gain) {
                    for n in 0..u.raw.len() {
                        if u.has_drawn(n) {
                            u.own[n] = Ambient { faces: u.raw[n].faces.map(|f| f * gain) };
                            probes.probes[n] = u.own[n];
                        }
                    }
                }
                u.gain = Some(gain);
            }
        }
        let gain = u.gain.unwrap_or(1.0);
        // Those drawn stop blending to their old light (in one pass: there can be many).
        let mut redrawn = vec![false; u.positions.len()];
        for &n in &due[..done] {
            redrawn[n] = true;
        }
        u.blending.retain(|b| !redrawn[b.0]);
        for (&n, &(near, size_before)) in due.iter().zip(&drawn_near) {
            let new = Ambient { faces: u.raw[n].faces.map(|f| f * gain) };
            // Near ones fade; any drawn at another size than before, slowly.
            let resized = size_before != 0 && size_before != u.sizes[n];
            let length = match (resized, near) {
                (true, _) => self.settings.probe_fade.max(RESIZE_FADE),
                (false, true) => self.settings.probe_fade,
                (false, false) => 0.0,
            };
            if length > 0.0 && budget != Duration::MAX {
                // From the light it has now (part way through a blend, maybe) to the new.
                u.blending.push((n, u.own[n], new, now, length));
            } else {
                u.own[n] = new;
            }
        }
        if baking && u.pending.is_empty() {
            for (n, own) in u.own.iter().enumerate() {
                if u.sectors[n].is_some() {
                    probes.probes[n] = *own;
                }
            }
            refill(probes, &u.fills, None);
            if let Some(started) = u.started.take() {
                println!(
                    "baked {} ambient probes {} m apart ({}x{}x{}, {}x{} a face) in {:.1} ms, scaled by {gain:.1}",
                    u.sectors.iter().filter(|s| s.is_some()).count(),
                    probes.spacing,
                    probes.dims[0],
                    probes.dims[1],
                    probes.dims[2],
                    u.size,
                    u.size,
                    started.elapsed().as_secs_f64() * 1000.0,
                );
            }
        }
        result.map(|()| due[..done].to_vec())
    }
}

/// Per light in `world`, the sectors it reaches.
fn light_sectors(world: &moose_scene::World) -> Vec<Vec<u32>> {
    let mut reaches = vec![Vec::new(); world.lights().len()];
    for s in 0..world.sectors.len() as u32 {
        for &i in world.sector_lights(s) {
            reaches[i as usize].push(s);
        }
    }
    reaches
}

/// Probe `n`'s grid coordinates.
fn cell_of(grid: &AmbientProbes, n: usize) -> [usize; 3] {
    let [nx, ny, _] = grid.dims;
    [n % nx, n / nx % ny, n / (nx * ny)]
}

/// Whether a probe is one of every other along each axis: those drawn far from the eye
/// (see `compose`).
fn on_coarse([i, j, k]: [usize; 3]) -> bool {
    i % 2 == 0 && j % 2 == 0 && k % 2 == 0
}

/// The probes around probe `n` (the 26 next to it).
fn neighbors(grid: &AmbientProbes, n: usize) -> impl Iterator<Item = usize> + '_ {
    let cell = cell_of(grid, n);
    (0..27).filter(|&d| d != 13).filter_map(move |d| {
        let at: [i64; 3] = std::array::from_fn(|a| cell[a] as i64 + [d % 3, d / 3 % 3, d / 9][a] as i64 - 1);
        (0..3).all(|a| at[a] >= 0 && at[a] < grid.dims[a] as i64).then(|| grid.index(at.map(|v| v as usize)))
    })
}

/// What the grid shows of probe `n`: its own light (see `Updates::own`); but with `sparse`,
/// for one between every other probe along each axis, its own eased by its distance from
/// `eye` into the blend of those around it inside the level (all of it past `SPARSE_TO`,
/// where it isn't drawn again; see `SPARSE_FROM`): far away, only every other one is drawn,
/// so the lights there are drawn again eight times as often, at what fewer probes show,
/// with no seam where that begins.
fn compose(grid: &mut AmbientProbes, u: &Updates, n: usize, eye: Vec3, sparse: bool) {
    // Outside the level, their neighbors' (see `refill`).
    if u.sectors[n].is_none() {
        return;
    }
    let cell = cell_of(grid, n);
    let t = ((u.positions[n].distance(eye) - SPARSE_FROM) / (SPARSE_TO - SPARSE_FROM)).clamp(0.0, 1.0);
    if !sparse || on_coarse(cell) || t <= 0.0 {
        grid.probes[n] = u.own[n];
        return;
    }
    // Per axis, the ones every other it is between (or on), and how much each counts.
    let around: [Vec<(usize, f32)>; 3] = std::array::from_fn(|a| {
        let (i, last) = (cell[a], grid.dims[a] - 1);
        match i % 2 {
            0 => vec![(i, 1.0)],
            _ if i < last => vec![(i - 1, 0.5), (i + 1, 0.5)],
            _ => vec![(i - 1, 1.0)],
        }
    });
    let (mut sum, mut total) = (Ambient::default(), 0.0);
    for &(x, wx) in &around[0] {
        for &(y, wy) in &around[1] {
            for &(z, wz) in &around[2] {
                let m = grid.index([x, y, z]);
                if u.sectors[m].is_some() {
                    let w = wx * wy * wz;
                    for (s, c) in sum.faces.iter_mut().zip(u.own[m].faces) {
                        *s += c * w;
                    }
                    total += w;
                }
            }
        }
    }
    if total <= 0.0 {
        grid.probes[n] = u.own[n];
        return;
    }
    let t = t * t * (3.0 - 2.0 * t);
    grid.probes[n] = Ambient { faces: std::array::from_fn(|k| u.own[n].faces[k].lerp(sum.faces[k] / total, t)) };
}

/// Which probes outside the level (`inside` false) take their light from which, so blends
/// near walls don't take the dark from beyond them: each the mean of its neighbors along
/// the axes that have light, a layer at a time out from the level (three layers; farther
/// ones stay dark). In layer order: a probe's sources come before it.
fn outside_fills(grid: &AmbientProbes, inside: &[bool]) -> Vec<(usize, Vec<usize>)> {
    let cells: Vec<[usize; 3]> = grid.cells().collect();
    let mut lit = inside.to_vec();
    let mut fills = Vec::new();
    for _ in 0..3 {
        let first = fills.len();
        for &cell in &cells {
            let n = grid.index(cell);
            if lit[n] {
                continue;
            }
            let mut sources = Vec::new();
            for axis in 0..3 {
                for step in [-1i64, 1] {
                    let mut near = cell;
                    let at = near[axis] as i64 + step;
                    if at < 0 || at >= grid.dims[axis] as i64 {
                        continue;
                    }
                    near[axis] = at as usize;
                    let j = grid.index(near);
                    if lit[j] {
                        sources.push(j);
                    }
                }
            }
            if !sources.is_empty() {
                fills.push((n, sources));
            }
        }
        if fills.len() == first {
            break;
        }
        for &(n, _) in &fills[first..] {
            lit[n] = true;
        }
    }
    fills
}

/// Gives the probes outside the level their neighbors' light again (see `outside_fills`):
/// those whose sources changed (`changed`, per probe; those filled are marked changed in
/// turn, for the next layer), or with `changed` none, all of them. Their light follows the
/// level's as it changes: one filled once would keep whatever lit its neighbors then.
fn refill(grid: &mut AmbientProbes, fills: &[(usize, Vec<usize>)], mut changed: Option<&mut [bool]>) {
    for (n, sources) in fills {
        if let Some(changed) = changed.as_deref_mut() {
            if !sources.iter().any(|&j| changed[j]) {
                continue;
            }
            changed[*n] = true;
        }
        let mut sum = Ambient::default();
        for &j in sources {
            for (s, c) in sum.faces.iter_mut().zip(grid.probes[j].faces) {
                *s += c;
            }
        }
        grid.probes[*n] = Ambient { faces: sum.faces.map(|c| c / sources.len() as f32) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// two_rooms with probes 1 m apart, drawn again 4 times a second, taking their new
    /// light at once, and the flashlight locked on the wall between the rooms.
    fn app() -> App {
        let args = ["--level", "two_rooms.mmp", "--size", "64x36", "--no-sound", "--probes", "1", "--probe-rate", "4", "--probe-fade", "0"];
        let mut app = App::new(&crate::parse_options(args.map(String::from)).unwrap()).unwrap();
        let at = Vec3::new(2.5, 1.5, 3.0);
        let sector = app.world.find_sector(at).unwrap();
        app.settings.flashlight_lock = Some((sector, at, Vec3::NEG_Z));
        app
    }

    /// Sets the flashlight on or off.
    fn flashlight(app: &mut App, on: bool) {
        app.settings.flashlight = on;
        app.apply_lights(true);
    }

    /// Every probe's light, without the gain (which a bake works out from what it drew).
    fn light(app: &App) -> Vec<Ambient> {
        let gain = app.probe_updates.as_ref().unwrap().gain.unwrap();
        app.world.probes.as_ref().unwrap().probes.iter().map(|a| Ambient { faces: a.faces.map(|f| f / gain) }).collect()
    }

    /// The most two probe grids' light differs by, as a share of the brightest.
    fn differs(a: &[Ambient], b: &[Ambient]) -> f32 {
        let most = a.iter().chain(b).map(brightness).fold(0.0, f32::max);
        a.iter().zip(b).map(|(a, b)| (0..6).map(|k| luma((a.faces[k] - b.faces[k]).abs())).fold(0.0, f32::max)).fold(0.0, f32::max) / most
    }

    /// Probes baked without the flashlight.
    fn dark() -> Vec<Ambient> {
        let mut app = app();
        flashlight(&mut app, false);
        app.bake_probes().unwrap();
        app.finish_probes().unwrap();
        light(&app)
    }

    /// Updates until nothing more is due.
    fn settle(app: &mut App) {
        for _ in 0..50 {
            app.update_probes().unwrap();
        }
    }

    #[test]
    fn probes_outside_the_level_follow_their_neighbors() {
        // A row of five probes, the last two outside the level: they take the light of
        // the one before them, a layer at a time, and again whenever it changes, not only
        // when first filled.
        let mut grid = AmbientProbes::covering(Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0), 1.0);
        let inside = [true, true, true, false, false];
        let fills = outside_fills(&grid, &inside);
        assert_eq!(fills, vec![(3, vec![2]), (4, vec![3])]);
        let lit = Ambient::uniform(Vec3::splat(2.0));
        grid.probes[2] = lit;
        refill(&mut grid, &fills, None);
        assert_eq!(grid.probes[4], lit);
        // The light goes; only what changed is told.
        grid.probes[2] = Ambient::uniform(Vec3::splat(0.1));
        let mut changed = vec![false, false, true, false, false];
        refill(&mut grid, &fills, Some(&mut changed));
        assert_eq!(grid.probes[4], grid.probes[2]);
        // Unchanged, they stay.
        grid.probes[4] = lit;
        refill(&mut grid, &fills, Some(&mut vec![false; 5]));
        assert_eq!(grid.probes[4], lit);
    }

    #[test]
    fn a_light_turned_off_after_a_bake_leaves_none_behind() {
        // The flashlight on as a bake ends: every probe that saw its light must lose it once
        // it's off, though most are too far from where it lands to be marked by that (down
        // the hall, seeing the lit floor through the doorway).
        let dark = dark();
        let mut app = app();
        flashlight(&mut app, true);
        app.bake_probes().unwrap();
        app.finish_probes().unwrap();
        assert!(differs(&light(&app), &dark) > 0.05, "the flashlight lit no probes");
        flashlight(&mut app, false);
        settle(&mut app);
        let left = differs(&light(&app), &dark);
        assert!(left < 0.01, "{left} of the brightest probe's light left");
    }

    #[test]
    fn a_light_turned_on_and_off_during_a_bake_leaves_none_behind() {
        // The lights are as they were when the bake began; the probes it drew while the
        // flashlight was on in between must be drawn again.
        let dark = dark();
        let mut app = app();
        flashlight(&mut app, false);
        app.bake_probes().unwrap();
        app.update_probes().unwrap();
        flashlight(&mut app, true);
        for _ in 0..3 {
            app.update_probes().unwrap();
        }
        flashlight(&mut app, false);
        app.finish_probes().unwrap();
        settle(&mut app);
        let left = differs(&light(&app), &dark);
        assert!(left < 0.01, "{left} of the brightest probe's light left");
    }
}
