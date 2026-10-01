use glam::{Affine3A, Quat, Vec3};
use moose_assets::{
    Aabb, Assets, DirectionalLight, EntityKind, Level, Light, MeshId, Occluder, ShadowKind, Plane, Portal,
    Sector,
};

/// Distance tolerance for containment and crossing tests, in meters.
const EPSILON: f32 = 1e-4;
/// How far past an opening's edge, in meters, light still counts as getting through.
const ON_OPENING: f32 = 1e-4;
/// Limit on openings a light is followed through.
const MAX_LIGHT_DEPTH: u16 = 16;
/// Upper bound on portal crossings in one trace; guards against degenerate loops.
const MAX_CROSSINGS: usize = 64;
/// Moves never end closer than this to a portal's plane while inside its outline, in
/// meters. It keeps every portal a camera looks through well clear of float error.
pub const PORTAL_CLEARANCE: f32 = 1e-3;

/// The running state of a loaded level, shared by every player and camera.
pub struct World {
    pub name: String,
    /// All static level geometry; each sector's polygons are a contiguous range of it.
    pub geometry: MeshId,
    pub sectors: Vec<Sector>,
    pub portals: Vec<Portal>,
    pub entities: Vec<Entity>,
    pub spawn_points: Vec<SpawnPoint>,
    /// Light that reaches everything (linear RGB; 1 is a surface's full color).
    pub ambient: Vec3,
    /// Lights from far away, entering through sky surfaces.
    pub directional: Vec<DirectionalLight>,
    /// The level's lights. Set them with [`World::set_lights`], which finds where each
    /// reaches.
    lights: Vec<Light>,
    /// Per sector, the lights that can reach into it (indices into `lights`); see
    /// [`World::set_lights`].
    sector_lights: Vec<Vec<u32>>,
    /// Per sector, every polygon bounding it (solid or portal), for containment and tracing.
    boundaries: Vec<Vec<Boundary>>,
    /// Posed model copies no entity uses (from a world this one replaced), to reuse.
    spare_copies: Vec<MeshId>,
}

/// A placed prop or actor.
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub name: String,
    /// `Prop` or `Actor`; decides the raster path when drawn.
    pub kind: EntityKind,
    /// What it draws: its model, or its own posed copy of it if it's animated (see
    /// [`World::animate`]).
    pub mesh: MeshId,
    /// The model it was made from.
    pub model: MeshId,
    /// The sector containing the entity's origin.
    pub sector: u32,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: f32,
    /// World-space box around the transformed mesh. Derived: call
    /// [`World::place_entity`] after changing the transform.
    pub bounds: Aabb,
    /// Every sector `bounds` overlaps, starting with `sector`. An entity poking through a
    /// portal lists both sides, so it can be tested from either. Derived like `bounds`.
    pub sectors: Vec<u32>,
    /// What it casts shadows with (see [`Occluder`]).
    pub occluder: Occluder,
    /// It never moves: static lights' shadows from it (and on it) can be worked out once.
    pub is_static: bool,
    /// How its shadows' edges are drawn.
    pub shadow: ShadowKind,
    /// The animation it plays (by name), if its model has a skeleton.
    pub animation: Option<String>,
}


impl Entity {
    /// Model-to-world transform.
    pub fn transform(&self) -> Affine3A {
        Affine3A::from_scale_rotation_translation(
            Vec3::splat(self.scale),
            self.rotation,
            self.position,
        )
    }
}

/// A place a player can start.
#[derive(Clone, Debug, PartialEq)]
pub struct SpawnPoint {
    pub name: String,
    pub sector: u32,
    pub position: Vec3,
    pub rotation: Quat,
}

/// Result of moving a point through the world.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trace {
    /// Where the move ended: the target, or just inside the first solid surface hit.
    pub position: Vec3,
    /// The sector containing `position`.
    pub sector: u32,
    /// True if a solid surface (or a portal that isn't passable) stopped the move.
    pub blocked: bool,
}

struct Boundary {
    plane: Plane,
    points: Vec<Vec3>,
    /// The portal this polygon is, if it is one.
    portal: Option<u32>,
    /// Directional lights enter the sector through it.
    sky: bool,
}

impl World {
    /// Takes over a loaded level. Prop and actor spawns become entities; spawn points
    /// become starting places for cameras.
    pub fn new(level: Level, assets: &Assets) -> World {
        let geometry = assets.mesh(level.geometry);
        let boundaries = level
            .sectors
            .iter()
            .map(|s| {
                let solids = s.polygons.clone().map(|i| {
                    let polygon = &geometry.polygons[i as usize];
                    Boundary {
                        plane: polygon.plane,
                        points: geometry.polygon_points(polygon).collect(),
                        portal: None,
                        sky: polygon.flags.sky(),
                    }
                });
                let portals = s.portals.clone().map(|i| {
                    let portal = &level.portals[i as usize];
                    Boundary {
                        plane: portal.plane,
                        points: portal
                            .positions
                            .iter()
                            .map(|&p| geometry.positions[p as usize])
                            .collect(),
                        portal: Some(i),
                        sky: false,
                    }
                });
                solids.chain(portals).collect()
            })
            .collect();

        let mut entities = Vec::new();
        let mut spawn_points = Vec::new();
        for spawn in level.spawns {
            match (spawn.kind, spawn.mesh) {
                (EntityKind::Spawn, _) => spawn_points.push(SpawnPoint {
                    name: spawn.name,
                    sector: spawn.sector,
                    position: spawn.position,
                    rotation: spawn.rotation,
                }),
                (kind, Some(mesh)) => entities.push(Entity {
                    name: spawn.name,
                    kind,
                    mesh,
                    model: mesh,
                    sector: spawn.sector,
                    position: spawn.position,
                    rotation: spawn.rotation,
                    scale: spawn.scale,
                    bounds: Aabb::from_points([]),
                    sectors: Vec::new(),
                    occluder: spawn.occluder,
                    is_static: spawn.is_static,
                    shadow: spawn.shadow,
                    animation: spawn.animation,
                }),
                (_, None) => unreachable!("the level loader gives every prop and actor a mesh"),
            }
        }

        let mut world = World {
            name: level.name,
            geometry: level.geometry,
            sectors: level.sectors,
            portals: level.portals,
            entities,
            spawn_points,
            ambient: level.ambient,
            directional: level.directional,
            lights: Vec::new(),
            sector_lights: Vec::new(),
            boundaries,
            spare_copies: Vec::new(),
        };
        for i in 0..world.entities.len() {
            world.place_entity(i, assets);
        }
        // Directional lights light like the rest (see `Light::directional`).
        let mut lights = level.lights;
        lights.extend(world.directional.iter().map(Light::from));
        world.set_lights(lights, level.ambient);
        world
    }

    /// The level's lights.
    pub fn lights(&self) -> &[Light] {
        &self.lights
    }

    /// The lights that can reach into `sector` (indices into [`World::lights`]).
    pub fn sector_lights(&self, sector: u32) -> &[u32] {
        &self.sector_lights[sector as usize]
    }

    /// Replaces the lights and the ambient light, and finds the sectors each light reaches.
    /// A light reaches its own sector (a directional light, every sector with a sky surface
    /// it shines in through), then goes on through the open portals (ones that can be seen
    /// through) it shines out through, within its range (and a spot light's cone). Each
    /// opening is clipped to the window it is seen through (what the openings before it
    /// let through, as the light sees them), and the light stops where that is empty. So
    /// light that falls through a doorway onto the floor beyond doesn't carry on through
    /// the next doorway. Light never passes through walls. Within a sector it reaches, all
    /// of it is lit (shadows are what keep parts of it dark).
    pub fn set_lights(&mut self, lights: Vec<Light>, ambient: Vec3) {
        self.ambient = ambient;
        self.sector_lights = vec![Vec::new(); self.sectors.len()];
        let mut listed = vec![false; self.sectors.len()];
        // Sectors to visit: the sector, the window into it (planes, each keeping the side
        // where `normal · p + d >= 0`; none for the light's own sector), and the depth.
        let mut stack: Vec<(u32, Window, u16)> = Vec::new();
        for (i, light) in lights.iter().enumerate() {
            listed.fill(false);
            if light.directional {
                // A sky surface faces into its sector, away from the light coming in: a
                // window into it.
                for (s, bounds) in self.boundaries.iter().enumerate() {
                    for b in bounds {
                        if b.sky && b.plane.normal.dot(light.direction) > 0.0 {
                            stack.push((s as u32, window(light, &b.points), 0));
                        }
                    }
                }
            } else {
                stack.push((light.sector, Vec::new(), 0));
            }
            while let Some((sector, seen_through, depth)) = stack.pop() {
                if !listed[sector as usize] {
                    listed[sector as usize] = true;
                    self.sector_lights[sector as usize].push(i as u32);
                }
                if depth >= MAX_LIGHT_DEPTH {
                    continue;
                }
                for b in &self.boundaries[sector as usize] {
                    let Some(portal) = b.portal.map(|p| &self.portals[p as usize]) else {
                        continue;
                    };
                    // Out through it: the portal faces into this sector.
                    let out = if light.directional {
                        b.plane.normal.dot(light.direction) < 0.0
                    } else {
                        b.plane.distance(light.position) > ON_OPENING
                    };
                    if !portal.flags.render_through() || !out {
                        continue;
                    }
                    // What of the opening the light sees through the window it came in by.
                    let mut points = b.points.clone();
                    for &(normal, d) in &seen_through {
                        points = clip_polygon(&points, normal, d);
                        if points.len() < 3 {
                            break;
                        }
                    }
                    if points.len() < 3 {
                        continue;
                    }
                    if !light.directional {
                        let nearest = closest_point_on_polygon(&points, b.plane.normal, light.position);
                        // A spot light's cone must reach the opening too.
                        let center = points.iter().copied().sum::<Vec3>() / points.len() as f32;
                        let radius = points.iter().map(|q| q.distance(center)).fold(0.0, f32::max);
                        if nearest.distance(light.position) >= light.range
                            || !light.cone_reaches(center, radius)
                        {
                            continue;
                        }
                    }
                    stack.push((portal.target, window(light, &points), depth + 1));
                }
            }
        }
        for list in &mut self.sector_lights {
            list.sort_unstable();
        }
        self.lights = lights;
    }

    /// Poses the animated entities `time` seconds into their animations: moves each one's
    /// own copy of its model (made the first time) to its skeleton's pose, and places it
    /// again. An entity whose model has no such animation stays as it is.
    pub fn animate(&mut self, assets: &mut Assets, time: f32) {
        let mut posed = Vec::new();
        for i in 0..self.entities.len() {
            let entity = &self.entities[i];
            let model = assets.mesh(entity.model);
            let (Some(name), Some(skin)) = (&entity.animation, &model.skin) else {
                continue;
            };
            let Some(animation) = skin.animation(name) else {
                continue;
            };
            let matrices = skin.matrices(animation, time);
            posed.resize(model.positions.len(), Vec3::ZERO);
            skin.pose_positions(&model.positions, &matrices, &mut posed);
            if entity.mesh == entity.model {
                let copy = model.clone();
                let mesh = match self.spare_copies.pop() {
                    Some(mesh) => {
                        *assets.mesh_mut(mesh) = copy;
                        mesh
                    }
                    None => assets.add_mesh(copy),
                };
                self.entities[i].mesh = mesh;
            }
            assets.mesh_mut(self.entities[i].mesh).set_positions(&posed);
            self.place_entity(i, assets);
        }
    }

    /// Hands the posed copies of models this world made (see [`animate`](Self::animate))
    /// to `to`, which replaces it, to reuse rather than add more.
    pub fn hand_over_copies(&self, to: &mut World) {
        to.spare_copies.extend(
            self.entities
                .iter()
                .filter(|e| e.mesh != e.model)
                .map(|e| e.mesh)
                .chain(self.spare_copies.iter().copied()),
        );
    }

    /// Recomputes an entity's derived `bounds` and `sectors` from its transform.
    /// Call after moving, turning or scaling it.
    pub fn place_entity(&mut self, index: usize, assets: &Assets) {
        let entity = &self.entities[index];
        let local = assets.mesh(entity.mesh).bounds;
        let transform = entity.transform();
        let corners = (0..8).map(|i| {
            let pick = |bit: usize, lo: f32, hi: f32| if i & bit == 0 { lo } else { hi };
            transform.transform_point3(Vec3::new(
                pick(1, local.min.x, local.max.x),
                pick(2, local.min.y, local.max.y),
                pick(4, local.min.z, local.max.z),
            ))
        });
        let bounds = Aabb::from_points(corners);
        let (center, half) = (
            (bounds.min + bounds.max) / 2.0,
            (bounds.max - bounds.min) / 2.0,
        );
        // A sector is touched unless the box lies wholly behind one of its planes. Near
        // corners this can include a sector the box misses, which only costs a test.
        let touches = |s: usize| {
            self.boundaries[s]
                .iter()
                .all(|b| b.plane.distance(center) + b.plane.normal.abs().dot(half) >= -EPSILON)
        };
        let home = entity.sector;
        let mut sectors = vec![home];
        sectors
            .extend((0..self.sectors.len() as u32).filter(|&s| s != home && touches(s as usize)));
        let entity = &mut self.entities[index];
        entity.bounds = bounds;
        entity.sectors = sectors;
    }

    /// True if `p` is inside `sector` (surfaces count as inside).
    pub fn sector_contains(&self, sector: u32, p: Vec3) -> bool {
        self.boundaries[sector as usize]
            .iter()
            .all(|b| b.plane.distance(p) >= -EPSILON)
    }

    /// The first sector containing `p`, found by testing every sector. Prefer
    /// [`trace`](Self::trace) when moving from a known sector.
    pub fn find_sector(&self, p: Vec3) -> Option<u32> {
        (0..self.sectors.len() as u32).find(|&s| self.sector_contains(s, p))
    }

    /// Moves a point from `from` (inside `sector`) toward `to`, following it through
    /// passable portals. Solid surfaces stop it just inside the sector. This is sector
    /// tracking only: there is no collision radius or sliding.
    ///
    /// A move never ends within [`PORTAL_CLEARANCE`] of a portal's plane inside its
    /// outline. One heading into a passable portal is carried through to that distance on
    /// the far side (so even tiny steps get through); any other is held back to it.
    pub fn trace(&self, sector: u32, from: Vec3, to: Vec3) -> Trace {
        let mut trace = self.trace_exact(sector, from, to);
        let heading = to - from;
        for _ in 0..2 {
            let near_portal = self.boundaries[trace.sector as usize].iter().find_map(|b| {
                let d = b.plane.distance(trace.position);
                let portal = b.portal?;
                (d < PORTAL_CLEARANCE
                    && contains_on_plane(&b.points, b.plane.normal, trace.position))
                .then_some((portal, b.plane.normal, d))
            });
            let Some((portal, normal, d)) = near_portal else {
                break;
            };
            let portal = &self.portals[portal as usize];
            if portal.flags.passable() && heading.dot(normal) < 0.0 {
                trace.position -= normal * (PORTAL_CLEARANCE + d);
                trace.sector = portal.target;
            } else {
                trace.position += normal * (PORTAL_CLEARANCE - d);
                break;
            }
        }
        trace
    }

    fn trace_exact(&self, sector: u32, from: Vec3, to: Vec3) -> Trace {
        let (mut sector, mut start) = (sector, from);
        for _ in 0..MAX_CROSSINGS {
            let delta = to - start;
            // Earliest exit: the smallest t where the segment leaves through a boundary plane.
            let mut exit_t = f32::INFINITY;
            for b in &self.boundaries[sector as usize] {
                let (d0, d1) = (b.plane.distance(start), b.plane.distance(to));
                if d1 < -EPSILON && d0 > d1 {
                    exit_t = exit_t.min((d0 / (d0 - d1)).max(0.0));
                }
            }
            if exit_t == f32::INFINITY {
                return Trace {
                    position: to,
                    sector,
                    blocked: false,
                };
            }
            let hit = start + delta * exit_t;
            // The exit plane may hold several coplanar polygons (a doorway wall and its portal).
            // Cross only if a passable portal covers the hit point.
            let through = self.boundaries[sector as usize].iter().find_map(|b| {
                let portal = b.portal?;
                let crossing = b.plane.distance(hit).abs() <= EPSILON
                    && b.plane.normal.dot(delta) < 0.0
                    && contains_on_plane(&b.points, b.plane.normal, hit)
                    && self.portals[portal as usize].flags.passable();
                crossing.then_some(portal)
            });
            match through {
                Some(portal) => {
                    sector = self.portals[portal as usize].target;
                    start = hit;
                }
                None => {
                    let back = if delta.length() > 0.0 {
                        delta.normalize() * EPSILON
                    } else {
                        Vec3::ZERO
                    };
                    return Trace {
                        position: hit - back,
                        sector,
                        blocked: true,
                    };
                }
            }
        }
        Trace {
            position: start,
            sector,
            blocked: true,
        }
    }
}

impl World {
    /// Pushes `p` (inside `sector`) out to at least `radius` from every solid surface of
    /// that sector, measured to the nearest point on each polygon (so a doorway's frame
    /// pushes back but its opening does not). A stand-in for real collision: move the
    /// camera to the result with a trace so portal crossings are tracked.
    pub fn keep_clear(&self, sector: u32, p: Vec3, radius: f32) -> Vec3 {
        let mut p = p;
        for _ in 0..2 {
            for b in self.boundaries[sector as usize]
                .iter()
                .filter(|b| b.portal.is_none())
            {
                let closest = closest_point_on_polygon(&b.points, b.plane.normal, p);
                let away = p - closest;
                let dist = away.length();
                if dist < radius {
                    let dir = if dist > 1e-6 {
                        away / dist
                    } else {
                        b.plane.normal
                    };
                    p += dir * (radius - dist);
                }
            }
        }
        p
    }
}

/// The point of a convex polygon nearest to `p`.
/// Planes bounding what a light sees through an opening, each `(normal, d)` keeping the
/// side where `normal · p + d >= 0`.
type Window = Vec<(Vec3, f32)>;

/// The window a light sees through a convex opening: planes through the light (for a
/// directional light, along its direction) and each of the opening's edges, each keeping
/// the side the opening is on.
fn window(light: &Light, points: &[Vec3]) -> Window {
    let n = points.len();
    let center = points.iter().copied().sum::<Vec3>() / n as f32;
    (0..n)
        .filter_map(|k| {
            let (a, b) = (points[k], points[(k + 1) % n]);
            let normal = if light.directional {
                (b - a).cross(light.direction)
            } else {
                (a - light.position).cross(b - light.position)
            };
            let len = normal.length();
            (len > 1e-9).then(|| {
                let normal = normal / len;
                let normal = if normal.dot(center - a) < 0.0 { -normal } else { normal };
                (normal, -normal.dot(a))
            })
        })
        .collect()
}

/// A convex polygon clipped to the side of a plane where `normal · p + d >= 0` (points
/// within [`ON_OPENING`] of it count as on it, so openings that meet at an edge still let
/// light through).
fn clip_polygon(points: &[Vec3], normal: Vec3, d: f32) -> Vec<Vec3> {
    let n = points.len();
    let mut out = Vec::with_capacity(n + 1);
    for k in 0..n {
        let (a, b) = (points[k], points[(k + 1) % n]);
        let (da, db) = (normal.dot(a) + d + ON_OPENING, normal.dot(b) + d + ON_OPENING);
        if da >= 0.0 {
            out.push(a);
        }
        if (da > 0.0 && db < 0.0) || (da < 0.0 && db > 0.0) {
            out.push(a + (b - a) * (da / (da - db)));
        }
    }
    out
}

fn closest_point_on_polygon(points: &[Vec3], normal: Vec3, p: Vec3) -> Vec3 {
    let on_plane = p - normal * normal.dot(p - points[0]);
    if contains_on_plane(points, normal, on_plane) {
        return on_plane;
    }
    (0..points.len())
        .map(|i| {
            let (a, b) = (points[i], points[(i + 1) % points.len()]);
            let t = ((p - a).dot(b - a) / (b - a).length_squared()).clamp(0.0, 1.0);
            a + (b - a) * t
        })
        .min_by(|x, y| x.distance_squared(p).total_cmp(&y.distance_squared(p)))
        .unwrap()
}

/// True if `p`, lying on the plane of the convex polygon `points`, is inside it or on its edge.
fn contains_on_plane(points: &[Vec3], normal: Vec3, p: Vec3) -> bool {
    (0..points.len()).all(|i| {
        let (a, b) = (points[i], points[(i + 1) % points.len()]);
        (b - a).cross(p - a).dot(normal) >= -EPSILON * (b - a).length()
    })
}
