use std::ops::Range;

use glam::{Quat, Vec3};

use crate::geom::{Aabb, Plane};
use crate::store::MeshId;

/// A loaded `.mmp` level: world geometry split into convex sectors joined by portals.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub name: String,
    /// All solid surfaces as one mesh. Each sector's polygons are a contiguous range.
    pub geometry: MeshId,
    pub sectors: Vec<Sector>,
    /// Ordered by sector; each sector's portals are a contiguous range.
    pub portals: Vec<Portal>,
    pub spawns: Vec<EntitySpawn>,
    /// The terrains (`EntityKind::Terrain` spawns), carved by the sectors.
    pub terrain: Vec<Terrain>,
    /// What the level's surfaces are drawn with: the geometry's polygons name these by
    /// index (`Polygon::material`).
    pub bindings: Vec<crate::material::Binding>,
    /// Light that reaches everything, in linear RGB (1 is a surface's full color).
    pub ambient: Vec3,
    pub lights: Vec<Light>,
    /// Per light (its `lights`, then its `directional` lights), its name (`name=`), which
    /// materials read it by (`light:NAME`) and exclusions name it by.
    pub light_names: Vec<Option<String>>,
    /// Lights from far away (a sun), entering sectors through their sky surfaces.
    pub directional: Vec<DirectionalLight>,
    /// The keys of every meta value in the level (`$KEY=VALUE`; see [`crate::meta`]), the
    /// level's own (on its `name` line), and per `geometry` polygon, its surface's. Sectors'
    /// and entities' are theirs ([`Sector::meta`], [`EntitySpawn::meta`]).
    pub meta_keys: crate::meta::MetaKeys,
    pub meta: crate::meta::MetaValues,
    pub faces: Vec<crate::meta::MetaValues>,
    /// Per `geometry` polygon, the lights it isn't lit by: its surface's and its sector's.
    pub face_excluded_lights: Vec<LightMask>,
}

/// Light from so far away that it arrives along one direction everywhere, like sunlight. It
/// enters the level only through sky surfaces (`PolyFlags::SKY`), and from there through
/// open portals like any light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DirectionalLight {
    /// The way the light travels (unit length): the sun's direction reversed.
    pub direction: Vec3,
    /// Linear RGB, like a light's.
    pub color: Vec3,
    /// The source's angular diameter in degrees (the sun is about 0.53): its shadows
    /// soften over the part of it an occluder covers. 0 casts hard shadows.
    pub angle: f32,
    /// Whether it casts shadows.
    pub shadows: bool,
    /// Its bit in a [`LightMask`] (see [`Light::id`]).
    pub id: u8,
}

/// Lights a thing isn't lit by (`exclude_lights=` on a surface, sector or entity): bit k
/// for the level's light k (its `lights`, then its `directional` lights; only the first
/// [`FLASHLIGHT_ID`] can be), and bit [`FLASHLIGHT_ID`] for the player's flashlight.
pub type LightMask = u64;
/// The flashlight's bit in a [`LightMask`].
pub const FLASHLIGHT_ID: u8 = 63;
/// The [`Light::id`] of a light no mask can exclude.
pub const NO_LIGHT_ID: u8 = u8::MAX;

/// A light: it lights surfaces facing it within `range` of it, fading smoothly to nothing
/// there, and within its cone. A point light's cone is whole: it shines every way. A spot
/// light's shines along `direction`, at full strength within the inner half-angle and fading
/// smoothly to nothing at the outer one. A directional light ([`Light::directional`]) is a
/// point light so far away that its light arrives along one direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Light {
    /// The sector containing it; [`NO_SECTOR`] for a directional light, which enters through
    /// sky surfaces instead.
    pub sector: u32,
    pub position: Vec3,
    /// Linear RGB at full strength (up close, facing it); 1 is a surface's full color, and
    /// more overbrightens.
    pub color: Vec3,
    /// Where its light ends, in meters.
    pub range: f32,
    /// Where a spot light shines (unit length).
    pub direction: Vec3,
    /// Cosines of the cone's inner and outer half-angles; -1 for a point light (whole).
    pub cos_inner: f32,
    pub cos_outer: f32,
    /// Whether it casts shadows, and its shadow slot if so (below 32): the bit polygons
    /// in its shadow set in their shadow mask (a runtime choice; levels don't set it).
    pub shadow: Option<u8>,
    /// The radius of the light's source, in meters: the shadows it casts soften over the
    /// part of it an occluder covers. 0 casts hard shadows. For a directional light, the
    /// sine of the source's angular radius (its radius per meter of distance).
    pub radius: f32,
    /// Whether it casts shadows (when the renderer gives it a shadow slot).
    pub shadows: bool,
    /// Whether it is a directional light: `direction` is the way its light travels.
    pub directional: bool,
    /// It never moves or changes (the level's lights): shadows it casts from static
    /// occluders on static surfaces can be worked out once and kept.
    pub is_static: bool,
    /// How it moves, if it does (a level's moving light; see [`Light::at_time`]).
    pub motion: Option<Oscillation>,
    /// A spot light whose cone is drawn pixel by pixel with its shadow (a beam), not
    /// lit at sample points: the view cuts polygons by a square pyramid around its cone,
    /// and the renderer's shadow buffer fades the light across the cone in the part inside
    /// (see [`Light::sample_cone`]). It needs a shadow slot.
    pub beam: bool,
    /// A spot light whose cone is lit at sample points as they are, with no closer ones
    /// where it fades (the renderer's penumbra rule): for a soft cone, which they follow
    /// anyway, at no cost over a point light.
    pub coarse: bool,
    /// Its bit in a [`LightMask`]: its place among the level's lights (the flashlight's,
    /// [`FLASHLIGHT_ID`]), or [`NO_LIGHT_ID`] for one nothing can exclude.
    pub id: u8,
}

/// A light swinging back and forth through its position: `offset` either way, smoothly (a
/// sine), once every `period` seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oscillation {
    pub offset: Vec3,
    pub period: f32,
}

/// `Light::sector` of a directional light.
pub const NO_SECTOR: u32 = u32::MAX;

/// How far away a directional light is placed, in meters, so the standard lighting (for
/// point lights) lights with it: far enough that its light arrives along one direction
/// anywhere in a level, and its range far beyond that, so it doesn't fade.
const DIRECTIONAL_DISTANCE: f32 = 1.0e4;
const DIRECTIONAL_RANGE: f32 = 1.0e6;

impl From<&DirectionalLight> for Light {
    fn from(d: &DirectionalLight) -> Light {
        let mut light = Light::directional(d.direction, d.color, d.angle);
        light.shadows = d.shadows;
        light.is_static = true;
        light.id = d.id;
        light
    }
}

impl Light {
    /// Whether `mask` excludes it.
    pub fn excluded_by(&self, mask: LightMask) -> bool {
        self.id < 64 && mask >> self.id & 1 != 0
    }

    /// Where a moving light is `time` seconds in (a light that doesn't move stays put).
    pub fn at_time(&self, time: f32) -> Vec3 {
        match self.motion {
            Some(m) => {
                let phase = std::f32::consts::TAU * (time / m.period).fract();
                self.position + m.offset * phase.sin()
            }
            None => self.position,
        }
    }

    /// A directional light: light traveling along `direction` (any length), from a source
    /// `angle` degrees across (its shadows soften over the part of it an occluder covers).
    pub fn directional(direction: Vec3, color: Vec3, angle: f32) -> Light {
        let direction = direction.normalize();
        Light {
            sector: NO_SECTOR,
            position: -direction * DIRECTIONAL_DISTANCE,
            color,
            range: DIRECTIONAL_RANGE,
            direction,
            cos_inner: -1.0,
            cos_outer: -1.0,
            shadow: None,
            radius: (angle.to_radians() / 2.0).sin(),
            shadows: true,
            directional: true,
            is_static: false,
            motion: None,
            beam: false,
            coarse: false,
            id: NO_LIGHT_ID,
        }
    }

    /// A point light.
    pub fn point(sector: u32, position: Vec3, color: Vec3, range: f32) -> Light {
        Light {
            sector,
            position,
            color,
            range,
            direction: Vec3::NEG_Y,
            cos_inner: -1.0,
            cos_outer: -1.0,
            shadow: None,
            radius: 0.0,
            shadows: true,
            directional: false,
            is_static: false,
            motion: None,
            beam: false,
            coarse: false,
            id: NO_LIGHT_ID,
        }
    }

    /// A spot light shining along `direction` (any length), full within `inner` degrees of
    /// it and gone past `outer`.
    pub fn spot(
        sector: u32,
        position: Vec3,
        color: Vec3,
        range: f32,
        direction: Vec3,
        inner: f32,
        outer: f32,
    ) -> Light {
        Light {
            sector,
            position,
            color,
            range,
            direction: direction.normalize(),
            cos_inner: inner.to_radians().cos(),
            cos_outer: outer.to_radians().cos(),
            shadow: None,
            radius: 0.0,
            shadows: true,
            directional: false,
            is_static: false,
            motion: None,
            beam: false,
            coarse: false,
            id: NO_LIGHT_ID,
        }
    }

    /// Whether its cone is whole (a point light).
    pub fn is_point(&self) -> bool {
        self.cos_outer <= -1.0
    }

    /// Whether its cone reaches any of the sphere around `center` (conservatively: when a
    /// part of the sphere lies within the outer half-angle, widened by the sphere's own
    /// angular radius seen from the light). Always, for a point light.
    pub fn cone_reaches(&self, center: Vec3, radius: f32) -> bool {
        if self.is_point() {
            return true;
        }
        let to = center - self.position;
        let d = to.length();
        if d <= radius {
            return true;
        }
        let angle = (to.dot(self.direction) / d).clamp(-1.0, 1.0).acos();
        angle <= self.cos_outer.clamp(-1.0, 1.0).acos() + (radius / d).asin()
    }

    /// Its cone as lighting at sample points uses it (see [`Light::cone`]): whole for a
    /// beam, whose cone is drawn with its shadow instead.
    pub fn sample_cone(&self) -> (f32, f32) {
        if self.beam { (0.0, 1.0) } else { self.cone() }
    }

    /// Its cone as `(scale, offset)`: `clamp(cos * scale + offset, 0, 1)` goes from 0 at
    /// the outer half-angle to 1 at the inner, where `cos` is the cosine of the angle
    /// between `direction` and the way to the lit point. `(0, 1)` for a point light: 1
    /// everywhere.
    pub fn cone(&self) -> (f32, f32) {
        if self.is_point() {
            (0.0, 1.0)
        } else {
            let scale = 1.0 / (self.cos_inner - self.cos_outer).max(1e-4);
            (scale, -self.cos_outer * scale)
        }
    }
}

/// A convex region of the level.
#[derive(Clone, Debug, PartialEq)]
pub struct Sector {
    pub name: String,
    /// Range of `Level::geometry` polygons that bound this sector.
    pub polygons: Range<u32>,
    /// Range of `Level::portals` leading out of this sector.
    pub portals: Range<u32>,
    pub bounds: Aabb,
    /// Mean of the sector's vertices; always inside a convex sector.
    pub center: Vec3,
    /// Its meta values (`$KEY=VALUE` on its row).
    pub meta: crate::meta::MetaValues,
    /// The lights its surfaces aren't lit by (`exclude_lights=`; also in each of its faces'
    /// [`Level::face_excluded_lights`]).
    pub excluded_lights: LightMask,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortalFlags(pub u32);

impl PortalFlags {
    pub const RENDER_THROUGH: u32 = 0x1;
    pub const PASSABLE: u32 = 0x2;
    pub const ALL: u32 = Self::RENDER_THROUGH | Self::PASSABLE;

    pub fn render_through(self) -> bool {
        self.0 & Self::RENDER_THROUGH != 0
    }

    pub fn passable(self) -> bool {
        self.0 & Self::PASSABLE != 0
    }
}

/// One side of an opening between two sectors. The polygon is convex and faces
/// into `sector`; its mirror lists the same points in reverse and faces into `target`.
#[derive(Clone, Debug, PartialEq)]
pub struct Portal {
    pub sector: u32,
    pub target: u32,
    /// Index of the portal on the other side, in `Level::portals`.
    pub mirror: u32,
    /// Indices into the geometry mesh's `positions`, shared with the surrounding walls.
    pub positions: Vec<u32>,
    pub plane: Plane,
    pub flags: PortalFlags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntityKind {
    Spawn,
    Prop,
    Actor,
    /// A static mesh too big and too open for a prop, such as a landscape: carved at load
    /// into pieces, each inside one sector (see [`Terrain`]). Never drawn whole.
    Terrain,
    /// A view blocker: its model's polygons, never drawn, hide from the eye whatever lies
    /// wholly behind one of them (props, actors, terrain pieces). Placed inside solid
    /// things, such as a hill, where portals can't help.
    Blocker,
}

impl EntityKind {
    pub const ALL: [EntityKind; 5] =
        [EntityKind::Spawn, EntityKind::Prop, EntityKind::Actor, EntityKind::Terrain, EntityKind::Blocker];

    /// Its name in level files.
    pub fn name(self) -> &'static str {
        match self {
            EntityKind::Spawn => "spawn",
            EntityKind::Prop => "prop",
            EntityKind::Actor => "actor",
            EntityKind::Terrain => "terrain",
            EntityKind::Blocker => "blocker",
        }
    }

    pub fn named(name: &str) -> Option<EntityKind> {
        EntityKind::ALL.into_iter().find(|k| k.name() == name)
    }

    /// Whether it is drawn as itself: a prop or an actor.
    pub fn is_drawn(self) -> bool {
        matches!(self, EntityKind::Prop | EntityKind::Actor)
    }
}

/// A terrain entity's model carved by the level's sectors: every polygon of it clipped to
/// each sector it crosses, so each piece lies inside one sector, and what lies outside
/// every sector dropped. Pieces are in world space, grouped by sector, and share their
/// corners exactly where they meet (cut points are computed the same way on both sides).
#[derive(Clone, Debug, PartialEq)]
pub struct Terrain {
    /// The entity (index in `Level::spawns`).
    pub spawn: u32,
    pub mesh: MeshId,
    /// Per sector, its range of the mesh's polygons.
    pub sectors: Vec<Range<u32>>,
}

/// Placement data for an entity, as read from the level. Creating live
/// entities from these is the scene's job.
#[derive(Clone, Debug, PartialEq)]
pub struct EntitySpawn {
    pub name: String,
    pub kind: EntityKind,
    pub sector: u32,
    /// The model to draw; `None` for spawn points.
    pub mesh: Option<MeshId>,
    pub position: Vec3,
    /// Pitch/yaw/roll applied roll, then pitch, then yaw (R = Ry * Rx * Rz).
    pub rotation: Quat,
    pub scale: f32,
    /// A prop that never moves: lighting from static lights can be worked out for it once.
    pub is_static: bool,
    /// What it casts shadows with.
    pub occluder: Occluder,
    /// How its shadows' edges are drawn.
    pub shadow: ShadowKind,
    /// The animation it plays (by name), if its model has a skeleton.
    pub animation: Option<String>,
    /// What its model is drawn with, if it names a material (`material=`); otherwise its
    /// vertex colors.
    pub binding: Option<crate::material::Binding>,
    /// Its meta values (`$KEY=VALUE`), over its template's.
    pub meta: crate::meta::MetaValues,
    /// The lights its model isn't lit by (`exclude_lights=`; its own, else its template's).
    pub excluded_lights: LightMask,
}

/// How an entity's shadows' edges are drawn, from a light with a size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShadowKind {
    /// Soft edges carved into the surfaces they fall on: exact, the best looking for most
    /// shapes, and cached for static props.
    #[default]
    Soft,
    /// Carved hard, then blurred on screen by how wide their soft edge would be: for
    /// figures that cast shadows with simple proxies, whose blur hides how simple they
    /// are. Worked out every frame.
    Blurred,
    /// Hard edges, from the light's center: the cheapest.
    Hard,
}

/// The shape an entity blocks light with, for lights that cast shadows: its model unless
/// the artist gives it something simpler. Any shape works; complex ones only cost more.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Occluder {
    /// It casts no shadows.
    None,
    /// Its own model.
    #[default]
    Mesh,
    /// One of its model's levels of detail (its own model until models have them).
    Lod(u8),
    /// A separate proxy model, placed like the entity.
    Model(MeshId),
    /// A polygon that always faces the light: `sides` points on the outline, seen from the
    /// light, of a ball of `radius` around `center` (in model space). A round shape's
    /// shadow with no silhouette to find.
    Facing { sides: u8, radius: f32, center: Vec3 },
}
