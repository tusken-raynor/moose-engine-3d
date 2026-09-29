use glam::{Affine3A, Quat, Vec3};
use moose_assets::{Aabb, Assets, EntityKind, Level, MeshId, Plane, Light, Portal, Sector};

/// Distance tolerance for containment and crossing tests, in meters.
const EPSILON: f32 = 1e-4;
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
    /// The level's lights. Set them with [`World::set_lights`], which finds where each
    /// reaches.
    lights: Vec<Light>,
    /// Per sector, the lights that can reach into it (indices into `lights`); see
    /// [`World::set_lights`].
    sector_lights: Vec<Vec<u32>>,
    /// Per sector, every polygon bounding it (solid or portal), for containment and tracing.
    boundaries: Vec<Vec<Boundary>>,
}

/// A placed prop or actor.
#[derive(Clone, Debug, PartialEq)]
pub struct Entity {
    pub name: String,
    /// `Prop` or `Actor`; decides the raster path when drawn.
    pub kind: EntityKind,
    pub mesh: MeshId,
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
    /// The shape it casts shadows with, if any.
    pub occluder: Occluder,
}

/// The shape an entity blocks light with, for lights that cast shadows: something simple,
/// standing in for its model.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Occluder {
    /// It casts no shadows.
    #[default]
    None,
    /// Its own model, which must be convex (a box, say): its faces toward the light.
    Mesh,
    /// A sphere, in model coordinates: seen from a light its outline is always a circle, so
    /// it casts shadows as a disk facing the light.
    Sphere { center: Vec3, radius: f32 },
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
                    sector: spawn.sector,
                    position: spawn.position,
                    rotation: spawn.rotation,
                    scale: spawn.scale,
                    bounds: Aabb::from_points([]),
                    sectors: Vec::new(),
                    occluder: Occluder::None,
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
            lights: Vec::new(),
            sector_lights: Vec::new(),
            boundaries,
        };
        for i in 0..world.entities.len() {
            world.place_entity(i, assets);
        }
        world.set_lights(level.lights, level.ambient);
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

    /// Replaces the lights and the ambient light, and finds the sectors each light reaches:
    /// its own, then through every open portal (one that can be seen through) that its
    /// range (and a spot light's cone) reaches, and on from there. Light never passes through walls. It does pass
    /// through the whole of an opening, not only the part the light can see through
    /// earlier openings.
    pub fn set_lights(&mut self, lights: Vec<Light>, ambient: Vec3) {
        self.ambient = ambient;
        self.sector_lights = vec![Vec::new(); self.sectors.len()];
        let mut reached = vec![false; self.sectors.len()];
        let mut stack = Vec::new();
        for (i, light) in lights.iter().enumerate() {
            reached.fill(false);
            reached[light.sector as usize] = true;
            stack.push(light.sector);
            while let Some(sector) = stack.pop() {
                self.sector_lights[sector as usize].push(i as u32);
                for b in &self.boundaries[sector as usize] {
                    let Some(portal) = b.portal.map(|p| &self.portals[p as usize]) else {
                        continue;
                    };
                    let target = portal.target as usize;
                    if reached[target] || !portal.flags.render_through() {
                        continue;
                    }
                    let nearest = closest_point_on_polygon(&b.points, b.plane.normal, light.position);
                    // A spot light's cone must reach the opening too.
                    let center = b.points.iter().copied().sum::<Vec3>() / b.points.len() as f32;
                    let radius = b.points.iter().map(|q| q.distance(center)).fold(0.0, f32::max);
                    if nearest.distance(light.position) < light.range
                        && light.cone_reaches(center, radius)
                    {
                        reached[target] = true;
                        stack.push(portal.target);
                    }
                }
            }
        }
        for list in &mut self.sector_lights {
            list.sort_unstable();
        }
        self.lights = lights;
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
