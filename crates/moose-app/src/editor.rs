//! The level editor: a mode of the app (Tab) for picking what's in the level with the
//! mouse and changing it.
//!
//! The editor works on the level's own tables ([`LevelDoc`]): every edit changes them,
//! and the app then rebuilds the level from them, through the level loader, which checks
//! the edit (an edit it rejects is undone, with the loader's reason). Undo and redo are
//! snapshots of the tables; saving writes them to the level's file.
//!
//! Things are picked in the 3D view or in 2D views (top, front, side; see [`crate::wire`]):
//! entities and surfaces, or with vertex picking on (V) vertices. A surface's panel selects
//! its sector. Sector tools: extrude a surface into a new sector, push or pull it, join
//! or part sectors with openings, cleave a sector along a line drawn in a 2D view, merge
//! two sectors into one, add a room, delete a sector; vertices move on the grid.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use glam::{EulerRot, Quat, Vec2, Vec3};
use moose_assets::{Assets, EntityDoc, EntityKind, LevelDoc, LightDoc};
use moose_scene::World;
use moose_view::PolygonSource;

use crate::ui::Canvas;
use crate::wire::{self, Axis, Ortho, Projection};

/// What the editor shows: the 3D view, or a 2D view along an axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Perspective,
    Ortho(Axis),
}

impl ViewMode {
    /// The next one (F6): 3D, top, front, side.
    pub fn next(self) -> ViewMode {
        match self {
            ViewMode::Perspective => ViewMode::Ortho(Axis::Top),
            ViewMode::Ortho(Axis::Top) => ViewMode::Ortho(Axis::Front),
            ViewMode::Ortho(Axis::Front) => ViewMode::Ortho(Axis::Side),
            ViewMode::Ortho(Axis::Side) => ViewMode::Perspective,
        }
    }
}

/// What is selected, by its row in the level's tables (so it survives rebuilding the
/// level).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Surface(usize),
    Sector(usize),
    Vertex(usize),
    Entity(usize),
    Light(usize),
}

/// A change the panel or a key asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    X,
    Y,
    Z,
    Yaw,
    Pitch,
    Roll,
    Scale,
    Kind,
    Model,
    /// What a surface or entity is drawn with (cycles through the materials, and none).
    Material,
    /// The animation it plays (cycles through its model's, and none).
    Animation,
    Static,
    Occluder,
    /// How its shadows' edges are drawn: soft, blurred or hard.
    Shadow,
    Reflective,
    Sky,
    Duplicate,
    Delete,
    /// Selects a surface's sector.
    SelectSector,
    /// How far to extrude (not an edit).
    ExtrudeBy,
    Extrude,
    /// Pushes a surface out (or pulls it in) on the grid.
    Push,
    Adjoin,
    Unadjoin,
    /// Starts cutting the selected sector: two clicks in a 2D view (not an edit).
    Cleave,
    /// Cuts along the line clicked (see [`Editor::cut`]).
    Cut,
    NewRoom,
    /// A light's color channels, range, and source radius.
    Red,
    Green,
    Blue,
    Range,
    Radius,
    Shadows,
    /// A light's cone: on or off, and its half-angles.
    Spot,
    Inner,
    Outer,
    AddLight,
    AddProp,
    AddSpawn,
    /// The level's ambient light.
    AmbientRed,
    AmbientGreen,
    AmbientBlue,
    /// The sun (the first directional light): its size, and where it comes from.
    SunAngle,
    SunYaw,
    SunPitch,
    /// A surface's texture: shifted across, shifted down, scaled, sized across or down
    /// (meters a repeat), turned, flipped across (mirrored left to right) or down (top to
    /// bottom), or mapped flat from its corners' positions. Across, down, the sizes and the
    /// turn also take a typed number (see [`Field::typed`]).
    TextureU,
    TextureV,
    TextureScale,
    TextureSizeU,
    TextureSizeV,
    TextureTurn,
    TextureFlipAcross,
    TextureFlipDown,
    TextureProject,
    /// Puts what was copied (Ctrl+C) where the view is.
    Paste,
    /// Opens (or leaves) the mesh editor on an entity's model (M).
    EditModel,
    /// Mesh editor: steps through the model's polygons.
    PolygonPick,
    /// Starts stitching the selected surface: the next surface clicked is the one it
    /// continues the material and texture of (not an edit).
    Stitch,
    /// Starts merging the selected surface: the next surface clicked merges into it (not an
    /// edit).
    Merge,
    /// Starts merging the selected sector: the sector of the next surface clicked merges
    /// into it (not an edit).
    MergeSector,
    /// Starts picking the lights the selected surface, sector or entity isn't lit by: each
    /// light clicked is excluded, or lights it again (not an edit).
    ExcludeLights,
    /// Whether the player's flashlight, or the sun, lights the selected surface, sector or
    /// entity.
    FlashlightLights,
    SunLights,
    /// The surface clicked while stitching or merging (an edit): see [`Editor::picking`].
    Picked,
    /// The surface right-clicked while stitching (an edit): stitched mirrored, its texture
    /// reflected across the shared edge.
    PickedMirrored,
    /// The line dragged across the selected surface in a 2D view (an edit): cuts it in two.
    CutFace,
    /// Mesh editor: clicks pick proxies or drawn polygons (P).
    PickProxies,
    /// Mesh editor: a polygon drawn or a shadow proxy.
    Proxy,
    /// Mesh editor: a proxy box around a bone's polygons (the selected polygon's bone).
    ProxyBox,
    /// Mesh editor: removes a bone's proxies.
    RemoveProxies,
}

/// What Ctrl+C copied.
#[derive(Clone, Debug)]
pub enum Clip {
    Entity(EntityDoc),
    Light(LightDoc),
}

impl Field {
    /// Whether clicking its row types a number into it (rather than changing it).
    pub fn typed(self) -> bool {
        matches!(
            self,
            Field::TextureU | Field::TextureV | Field::TextureSizeU | Field::TextureSizeV | Field::TextureTurn
        )
    }

    /// Whether it changes the level (rather than what the editor is doing).
    pub fn is_edit(self) -> bool {
        !matches!(
            self,
            Field::SelectSector
                | Field::ExtrudeBy
                | Field::Cleave
                | Field::EditModel
                | Field::PolygonPick
                | Field::PickProxies
                | Field::Stitch
                | Field::Merge
                | Field::MergeSector
                | Field::ExcludeLights
        )
    }
}

/// A row of the properties panel: its label, its value, and what clicking it (or
/// scrolling over it) changes.
pub struct PanelRow {
    pub label: String,
    pub value: String,
    pub field: Option<Field>,
}

/// Grid steps for moving things (G steps through them), in meters.
pub const STEPS: [f32; 5] = [0.0625, 0.125, 0.25, 0.5, 1.0];
/// How far a turn goes, in degrees.
pub const TURN: f32 = 15.0;
/// How long a status message shows.
const STATUS_TIME: Duration = Duration::from_secs(4);
/// How near the pointer must be to a vertex or an edge to pick it, in pixels.
const PICK_RADIUS: f32 = 7.0;
/// A new room's size, in meters.
const ROOM: Vec3 = Vec3::new(4.0, 3.0, 4.0);
/// A new level's room, in meters (x, height, z).
const LEVEL_ROOM: Vec3 = Vec3::new(10.0, 5.0, 10.0);
/// What a new level's surfaces are drawn with.
pub const DEFAULT_MATERIAL: &str = "default";

type Snapshot = (LevelDoc, Option<Selection>);

/// What clicking a second surface does (see [`Editor::picking`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// The selected surface continues the clicked one's material and texture.
    Stitch,
    /// The clicked surface merges into the selected one.
    Merge,
    /// The clicked surface's sector merges into the selected sector (whose index the
    /// pick holds).
    MergeSector,
    /// The clicked light is excluded from (or lights again) the surface, sector or entity
    /// whose index the pick holds.
    Exclude(Excluder),
}

/// What has a list of the lights it isn't lit by (`exclude_lights=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Excluder {
    Surface,
    Sector,
    Entity,
}

/// A line being dragged across a surface in a 2D view, to cut it (see `Field::CutFace`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceCut {
    pub from: Vec3,
    pub to: Vec3,
    /// Where the press was, on screen.
    pub pressed: Vec2,
}

pub struct Editor {
    /// Editing (Tab), as opposed to playing.
    pub on: bool,
    /// The materials' names (`assets/materials`), for the Material fields.
    pub materials: Vec<String>,
    pub doc: LevelDoc,
    /// Where the level is saved.
    pub path: PathBuf,
    pub selection: Option<Selection>,
    /// What the pointer is over.
    pub hover: Option<Selection>,
    /// Clicks pick vertices (V), not things.
    pub vertices: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// The tables as last saved (or loaded), to tell if there are unsaved changes.
    saved: LevelDoc,
    status: Option<(String, Instant)>,
    /// The grid step, an index into [`STEPS`].
    pub step: usize,
    pub view: ViewMode,
    /// Wireframe over the 3D view (F5).
    pub wire: bool,
    /// The 2D views' center and scale (pixels per meter), shared by all three.
    pub ortho_center: Vec3,
    pub ortho_scale: f32,
    /// How far Extrude goes, in meters.
    pub extrude_by: f32,
    /// While cutting a sector: the points clicked so far (the cut runs through two).
    pub cutting: Option<Vec<Vec3>>,
    /// While a second surface is to be clicked: the selected surface, and what the click
    /// does with the one clicked (stitch from it, or merge it in).
    pub picking: Option<(usize, Pick)>,
    /// While a line is dragged across the selected surface in a 2D view: where it started
    /// and where it is now (snapped), and where the press was on screen.
    pub face_cut: Option<FaceCut>,
    /// A number being typed into a panel row (clicked), and the text so far.
    pub typing: Option<(Field, String)>,
    /// Fine steps (Alt held): scrolling texture rows moves them a little.
    pub fine: bool,
    /// Where new things go: the middle of the 2D view, or in front of the camera. Set by
    /// the app each frame.
    pub anchor: Vec3,
    /// The world point under the pointer in a 2D view, snapped where a cleave's end goes:
    /// to the grid, or with Ctrl to the nearest vertex (see [`Editor::cut_snap`]). For the
    /// cleave's clicks and its preview.
    pub pointer: Option<Vec3>,
    pub clipboard: Option<Clip>,
    /// Camera places (Ctrl+1 to 4 keep one, 1 to 4 go back to it): position, yaw, pitch.
    pub bookmarks: [Option<(Vec3, f32, f32)>; 4],
    /// What the level loader last refused an edit over, and when (drawn in red a while).
    problem: Option<(Selection, Instant)>,
    /// The animations of the level's models, by file name (see
    /// [`learn_animations`](Self::learn_animations)).
    animations: std::collections::HashMap<String, Vec<String>>,
    /// The mesh editor, while it's open.
    pub model: Option<crate::mesh_edit::ModelEdit>,
}

impl Editor {
    pub fn new(doc: LevelDoc, path: PathBuf) -> Editor {
        Editor {
            on: false,
            materials: Vec::new(),
            saved: doc.clone(),
            doc,
            path,
            selection: None,
            hover: None,
            vertices: false,
            undo: Vec::new(),
            redo: Vec::new(),
            status: None,
            step: 2,
            view: ViewMode::Perspective,
            wire: false,
            ortho_center: Vec3::ZERO,
            ortho_scale: 40.0,
            extrude_by: 2.0,
            cutting: None,
            picking: None,
            face_cut: None,
            typing: None,
            fine: false,
            anchor: Vec3::ZERO,
            pointer: None,
            clipboard: None,
            bookmarks: [None; 4],
            problem: None,
            animations: Default::default(),
            model: None,
        }
    }

    /// Notes the animations of the models `world`'s entities use, for the panel.
    pub fn learn_animations(&mut self, world: &World, assets: &Assets) {
        for e in &world.entities {
            if let (Some(name), Some(skin)) = (assets.mesh_name(e.model), &assets.mesh(e.model).skin) {
                let names = skin.animations.iter().map(|a| a.name.clone()).collect();
                self.animations.insert(name.to_string(), names);
            }
        }
    }

    /// Before an edit that failed is undone: finds what the loader's message (`why`, with
    /// a line of the tables' text) is about, to show it.
    pub fn find_problem(&mut self, why: &str) {
        // Messages read "path:line: what".
        let line = why
            .split(':')
            .find_map(|part| part.trim().parse::<usize>().ok());
        let Some((section, row)) = line.and_then(|l| self.doc.row_at_line(l)) else {
            return;
        };
        let selection = match section.as_str() {
            "surfaces" => Selection::Surface(row),
            "sectors" => Selection::Sector(row),
            "vertices" => Selection::Vertex(row),
            "entities" => Selection::Entity(row),
            "lights" => Selection::Light(row),
            _ => return,
        };
        self.problem = Some((selection, Instant::now()));
    }

    #[cfg(test)]
    pub fn problem_for_tests(&self) -> Option<Selection> {
        self.problem.map(|(selection, _)| selection)
    }

    /// Copies the selected entity or light (Ctrl+C).
    pub fn copy(&mut self) {
        self.clipboard = match self.selection {
            Some(Selection::Entity(i)) => Some(Clip::Entity(self.doc.entities[i].clone())),
            Some(Selection::Light(i)) => Some(Clip::Light(self.doc.lights[i].clone())),
            _ => {
                self.say("select an entity or a light to copy");
                return;
            }
        };
        self.say("copied: Ctrl+V puts it where the view is");
    }

    /// Whether there are changes to save: edits since the last save, or a new level not
    /// saved yet.
    pub fn dirty(&self) -> bool {
        self.doc != self.saved || !self.path.exists()
    }

    pub fn say(&mut self, message: impl Into<String>) {
        self.status = Some((message.into(), Instant::now()));
    }

    pub fn step(&self) -> f32 {
        STEPS[self.step]
    }

    /// The 2D view, if one is showing, for a `width` x `height` screen.
    pub fn ortho(&self, width: usize, height: usize) -> Option<Ortho> {
        match self.view {
            ViewMode::Ortho(axis) => Some(Ortho {
                axis,
                center: self.ortho_center,
                scale: self.ortho_scale,
                width: width as f32,
                height: height as f32,
            }),
            ViewMode::Perspective => None,
        }
    }

    // ---- History

    /// Before an edit: the tables and selection to put back if it fails, or to undo to.
    pub fn begin(&mut self) -> Snapshot {
        (self.doc.clone(), self.selection)
    }

    /// After an edit the level accepted.
    pub fn commit(&mut self, before: Snapshot, what: &str) {
        self.undo.push(before);
        self.redo.clear();
        self.tidy();
        self.say(what);
    }

    /// After an edit that failed: puts the tables back.
    pub fn revert(&mut self, before: Snapshot, why: &str) {
        (self.doc, self.selection) = before;
        self.tidy();
        self.say(why);
    }

    /// Whether `selection` is in the tables as they are.
    pub fn exists(&self, selection: Selection) -> bool {
        let d = &self.doc;
        match selection {
            Selection::Surface(i) => i < d.surfaces.len(),
            Selection::Sector(i) => i < d.sectors.len(),
            Selection::Vertex(i) => i < d.vertices.len(),
            Selection::Entity(i) => i < d.entities.len(),
            Selection::Light(i) => i < d.lights.len(),
        }
    }

    /// `selection` if it is in the tables as they are.
    fn current(&self, selection: Option<Selection>) -> Option<Selection> {
        selection.filter(|&s| self.exists(s))
    }

    /// After the tables change: drops what pointed at things they no longer have (what
    /// the pointer is over, the selection, a refused edit's outline). What the pointer is
    /// over is picked again next frame.
    pub fn tidy(&mut self) {
        self.hover = None;
        self.typing = None;
        self.selection = self.current(self.selection);
        self.problem = self.problem.filter(|&(s, _)| self.exists(s));
        self.picking = self.picking.filter(|&(i, kind)| match kind {
            Pick::MergeSector | Pick::Exclude(Excluder::Sector) => i < self.doc.sectors.len(),
            Pick::Exclude(Excluder::Entity) => i < self.doc.entities.len(),
            _ => i < self.doc.surfaces.len(),
        });
    }

    /// Steps back (or forward, `redo`) through the history. Returns whether there was a
    /// step (the level must then be rebuilt from the tables).
    pub fn travel(&mut self, redo: bool) -> bool {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        let Some(state) = from.pop() else {
            return false;
        };
        to.push((self.doc.clone(), self.selection));
        (self.doc, self.selection) = state;
        self.tidy();
        true
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.doc.clone();
    }

    // ---- Picking

    /// The selection a drawn polygon's source stands for.
    pub fn selection_of(&self, source: PolygonSource) -> Option<Selection> {
        match source {
            PolygonSource::World { polygon, .. } => {
                // The level's drawn polygons are its solid surfaces, in order.
                let surface = self
                    .doc
                    .surfaces
                    .iter()
                    .enumerate()
                    .filter(|(_, s)| s.adjoin.is_none())
                    .nth(polygon as usize)?
                    .0;
                Some(Selection::Surface(surface))
            }
            // A terrain's pieces stand for the terrain.
            PolygonSource::Entity { entity, .. } | PolygonSource::Terrain { entity, .. } => {
                // The world's entities are the rows that aren't spawn points, in order.
                let row = self
                    .doc
                    .entities
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| e.kind != EntityKind::Spawn)
                    .nth(entity as usize)?
                    .0;
                Some(Selection::Entity(row))
            }
        }
    }

    /// The world entity row `row` is, if it isn't a spawn point.
    pub fn world_entity(&self, row: usize) -> Option<usize> {
        (self.doc.entities[row].kind != EntityKind::Spawn).then(|| {
            self.doc.entities[..row].iter().filter(|e| e.kind != EntityKind::Spawn).count()
        })
    }

    /// The vertices surfaces use.
    fn used_vertices(&self) -> Vec<usize> {
        let mut used: Vec<usize> = self
            .doc
            .surfaces
            .iter()
            .flat_map(|s| s.corners.iter().map(|&(v, _)| v))
            .collect();
        used.sort_unstable();
        used.dedup();
        used
    }

    /// The vertex nearest screen point `at` through `projection`, within a few pixels.
    pub fn pick_vertex(&self, projection: &Projection, at: Vec2) -> Option<Selection> {
        self.used_vertices()
            .into_iter()
            .filter_map(|v| {
                let p = projection.point(self.doc.vertices[v])?;
                Some((p.distance(at), v))
            })
            .filter(|&(d, _)| d < PICK_RADIUS)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, v)| Selection::Vertex(v))
    }

    /// The light or spawn point whose marker is nearest screen point `at` through
    /// `projection`, within a few pixels.
    pub fn pick_marker(&self, projection: &Projection, at: Vec2) -> Option<Selection> {
        let lights = self.doc.lights.iter().enumerate().map(|(i, l)| (l.position, Selection::Light(i)));
        let spawns = self
            .doc
            .entities
            .iter()
            .enumerate()
            .filter(|(_, e)| e.kind == EntityKind::Spawn)
            .map(|(i, e)| (e.position, Selection::Entity(i)));
        lights
            .chain(spawns)
            .filter_map(|(p, selection)| Some((projection.point(p)?.distance(at), selection)))
            .filter(|&(d, _)| d < PICK_RADIUS + 2.0)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, selection)| selection)
    }

    /// What is under screen point `at` in a 2D view: an entity whose outline (its box on
    /// screen) holds it, the smallest; otherwise the surface (openings too) with an edge
    /// nearest it, within a few pixels.
    pub fn pick_2d(&self, ortho: &Ortho, world: &World, at: Vec2) -> Option<Selection> {
        let mut best: Option<(f32, Selection)> = None;
        // Not terrains, which cover everything.
        for (i, e) in world.entities.iter().enumerate() {
            if e.kind == EntityKind::Terrain {
                continue;
            }
            let corners = (0..8).map(|k| {
                let pick = |bit: usize, lo: f32, hi: f32| if k & bit == 0 { lo } else { hi };
                ortho.to_screen(Vec3::new(
                    pick(1, e.bounds.min.x, e.bounds.max.x),
                    pick(2, e.bounds.min.y, e.bounds.max.y),
                    pick(4, e.bounds.min.z, e.bounds.max.z),
                ))
            });
            let (lo, hi) = corners.fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), c| {
                (lo.min(c), hi.max(c))
            });
            if at.cmpge(lo).all() && at.cmple(hi).all() {
                let area = (hi - lo).x * (hi - lo).y;
                let source = PolygonSource::Entity { entity: i as u32, polygon: 0 };
                if best.is_none_or(|(a, _)| area < a)
                    && let Some(selection) = self.selection_of(source)
                {
                    best = Some((area, selection));
                }
            }
        }
        if let Some((_, selection)) = best {
            return Some(selection);
        }
        let mut nearest: Option<(f32, usize)> = None;
        for i in 0..self.doc.surfaces.len() {
            let points: Vec<Vec2> = self.doc.surface_points(i).into_iter().map(|p| ortho.to_screen(p)).collect();
            for k in 0..points.len() {
                let d = distance_to_segment(at, points[k], points[(k + 1) % points.len()]);
                if d < PICK_RADIUS && nearest.is_none_or(|(n, _)| d < n) {
                    nearest = Some((d, i));
                }
            }
        }
        nearest.map(|(_, i)| Selection::Surface(i))
    }

    // ---- The panel

    /// The properties panel's rows for the selection.
    pub fn panel(&self) -> Vec<PanelRow> {
        let mut rows = self.panel_rows();
        // A row being typed into shows the text so far.
        if let Some((field, text)) = &self.typing
            && let Some(row) = rows.iter_mut().find(|r| r.field == Some(*field))
        {
            row.value = format!("{text}_");
        }
        rows
    }

    fn panel_rows(&self) -> Vec<PanelRow> {
        if let Some(model) = &self.model {
            return model.panel();
        }
        let row = |label: &str, value: String, field: Option<Field>| PanelRow {
            label: label.to_string(),
            value,
            field,
        };
        let n = moose_assets::number;
        match self.current(self.selection) {
            None => {
                let ambient = self.doc.ambient.unwrap_or(Vec3::ONE);
                let mut rows = vec![
                    row("Level", self.doc.name.clone(), None),
                    row("New room", format!("{}x{}x{} m", ROOM.x, ROOM.y, ROOM.z), Some(Field::NewRoom)),
                    row("Add a light", String::new(), Some(Field::AddLight)),
                    row("Add a prop", String::new(), Some(Field::AddProp)),
                    row("Add a spawn point", String::new(), Some(Field::AddSpawn)),
                    row("Ambient red", n(ambient.x), Some(Field::AmbientRed)),
                    row("Ambient green", n(ambient.y), Some(Field::AmbientGreen)),
                    row("Ambient blue", n(ambient.z), Some(Field::AmbientBlue)),
                ];
                if let Some(sun) = self.doc.directional.first() {
                    let (yaw, pitch) = aim_of(-sun.direction);
                    rows.push(row("Sun size", format!("{}°", n(sun.angle)), Some(Field::SunAngle)));
                    rows.push(row("Sun from (yaw)", format!("{}°", n(yaw)), Some(Field::SunYaw)));
                    rows.push(row("Sun height", format!("{}°", n(pitch)), Some(Field::SunPitch)));
                }
                rows
            }
            Some(Selection::Light(i)) => {
                let l = &self.doc.lights[i];
                let option = |key: &str| light_option(l, key).map(str::to_string);
                let mut rows = vec![
                    row("Light", i.to_string(), None),
                    row("Name", option("name").unwrap_or("none".into()), None),
                    row("X", n(l.position.x), Some(Field::X)),
                    row("Y", n(l.position.y), Some(Field::Y)),
                    row("Z", n(l.position.z), Some(Field::Z)),
                    row("Red", n(l.color.x), Some(Field::Red)),
                    row("Green", n(l.color.y), Some(Field::Green)),
                    row("Blue", n(l.color.z), Some(Field::Blue)),
                    row("Range", format!("{} m", n(l.range)), Some(Field::Range)),
                    row("Source radius", format!("{} m", option("radius").unwrap_or("0".into())), Some(Field::Radius)),
                    row("Shadows", option("shadows").unwrap_or("on".into()), Some(Field::Shadows)),
                    row("Spot", if l.spot.is_some() { "on" } else { "off" }.into(), Some(Field::Spot)),
                ];
                if let Some((dir, inner, outer)) = l.spot {
                    let (yaw, pitch) = aim_of(dir);
                    rows.push(row("Aim (yaw)", format!("{}°", n(yaw)), Some(Field::Yaw)));
                    rows.push(row("Aim (pitch)", format!("{}°", n(pitch)), Some(Field::Pitch)));
                    rows.push(row("Inner", format!("{}°", n(inner)), Some(Field::Inner)));
                    rows.push(row("Outer", format!("{}°", n(outer)), Some(Field::Outer)));
                }
                if let Some(motion) = option("oscillate") {
                    rows.push(row("Swings", motion, None));
                }
                rows.push(row("Sector", self.doc.sectors[l.sector].name.clone(), None));
                rows.push(row("Duplicate", String::new(), Some(Field::Duplicate)));
                rows.push(row("Delete", String::new(), Some(Field::Delete)));
                rows
            }
            Some(Selection::Surface(i)) => {
                let s = &self.doc.surfaces[i];
                let mut rows = vec![
                    row("Surface", i.to_string(), None),
                    row("Sector", self.doc.sectors[s.sector].name.clone(), Some(Field::SelectSector)),
                    row("Corners", s.corners.len().to_string(), None),
                ];
                match s.adjoin {
                    Some(a) => {
                        let other = self.doc.surfaces[self.doc.adjoins[self.doc.adjoins[a].mirror].surface].sector;
                        rows.push(row("Opening to", self.doc.sectors[other].name.clone(), None));
                        rows.push(row("Wall it up", String::new(), Some(Field::Unadjoin)));
                    }
                    None => {
                        let on = |bit: u32| if s.flags & bit != 0 { "on" } else { "off" }.to_string();
                        let material = s.options.iter().find_map(|o| o.strip_prefix("material=")).unwrap_or("none");
                        rows.push(row("Material", material.into(), Some(Field::Material)));
                        let pick = |kind: Pick| match kind {
                            _ if self.picking != Some((i, kind)) => "",
                            Pick::Stitch => "click a face (right: mirrored)",
                            Pick::Merge => "click a face",
                            Pick::MergeSector | Pick::Exclude(_) => "",
                        };
                        rows.push(row("Stitch from a touching face", pick(Pick::Stitch).into(), Some(Field::Stitch)));
                        rows.push(row("Merge with a touching face", pick(Pick::Merge).into(), Some(Field::Merge)));
                        rows.push(row("Cut it", "drag in 2D (Ctrl: vertices)".into(), None));
                        rows.push(row("Reflective", on(0x1), Some(Field::Reflective)));
                        rows.push(row("Sky", on(0x2), Some(Field::Sky)));
                        rows.push(row("Extrude by", format!("{} m", n(self.extrude_by)), Some(Field::ExtrudeBy)));
                        rows.push(row("Extrude", String::new(), Some(Field::Extrude)));
                        rows.push(row("Push/pull", "scroll".into(), Some(Field::Push)));
                        rows.push(row("Open to a match", String::new(), Some(Field::Adjoin)));
                        self.exclusion_rows(Excluder::Surface, i, &mut rows);
                        if self.doc.attributes.iter().any(|a| a.name == "uv") {
                            // Its mapping as numbers (when its coordinates make one).
                            let m = self.texture_mapping(i);
                            let shown = |f: fn(&TextureMapping) -> String| m.as_ref().map_or("scroll".into(), f);
                            rows.push(row("Texture across", shown(|m| format!("{:.4}", m.offset.x)), Some(Field::TextureU)));
                            rows.push(row("Texture down", shown(|m| format!("{:.4}", m.offset.y)), Some(Field::TextureV)));
                            rows.push(row("Texture scale", "scroll".into(), Some(Field::TextureScale)));
                            rows.push(row("Texture size across", shown(|m| format!("{:.4} m", m.size.x)), Some(Field::TextureSizeU)));
                            rows.push(row("Texture size down", shown(|m| format!("{:.4} m", m.size.y)), Some(Field::TextureSizeV)));
                            rows.push(row("Texture turn", shown(|m| format!("{:.2}°", m.turn)), Some(Field::TextureTurn)));
                            rows.push(row("Texture flip across", String::new(), Some(Field::TextureFlipAcross)));
                            rows.push(row("Texture flip down", String::new(), Some(Field::TextureFlipDown)));
                            rows.push(row("Texture flat", String::new(), Some(Field::TextureProject)));
                        }
                    }
                }
                rows
            }
            Some(Selection::Sector(s)) => {
                let range = self.doc.sector_surfaces(s);
                let openings = self.doc.surfaces[range.clone()].iter().filter(|x| x.adjoin.is_some()).count();
                let mut rows = vec![
                    row("Sector", self.doc.sectors[s].name.clone(), None),
                    row("Surfaces", range.len().to_string(), None),
                    row("Openings", openings.to_string(), None),
                    row("Cleave", "in 2D (Ctrl: vertices)".into(), Some(Field::Cleave)),
                    row(
                        "Merge with a sector",
                        if self.picking == Some((s, Pick::MergeSector)) { "click a face of it" } else { "" }.into(),
                        Some(Field::MergeSector),
                    ),
                ];
                self.exclusion_rows(Excluder::Sector, s, &mut rows);
                rows.push(row("Delete", String::new(), Some(Field::Delete)));
                rows
            }
            Some(Selection::Vertex(v)) => {
                let p = self.doc.vertices[v];
                vec![
                    row("Vertex", v.to_string(), None),
                    row("X", n(p.x), Some(Field::X)),
                    row("Y", n(p.y), Some(Field::Y)),
                    row("Z", n(p.z), Some(Field::Z)),
                ]
            }
            Some(Selection::Entity(i)) => {
                let e = &self.doc.entities[i];
                let (yaw, pitch, roll) = e.rotation.to_euler(EulerRot::YXZ);
                // Props and actors switch between each other; other kinds stay as they are.
                let mut rows = vec![
                    row("Name", e.name.clone(), None),
                    row("Kind", e.kind.name().into(), e.kind.is_drawn().then_some(Field::Kind)),
                ];
                // Its options in effect (its template's, unless it sets its own), each
                // marked where it comes from the template.
                let options = self.doc.options_of(e);
                let from_template = |key: &str| {
                    let own = e.options.iter().any(|o| moose_assets::option_key(o) == key);
                    let set = options.iter().any(|o| moose_assets::option_key(o) == key);
                    if set && !own { " (template)" } else { "" }
                };
                let value = |key: &str| options.iter().find_map(|o| o.strip_prefix(key)?.strip_prefix('='));
                if let Some(named) = &e.model {
                    let model = self.doc.model_file(e).unwrap_or(named).to_string();
                    let shown = if self.doc.template_of(e).is_some() { format!("{named} (template)") } else { named.clone() };
                    rows.push(row("Model", shown, Some(Field::Model)));
                    if matches!(e.kind, EntityKind::Prop | EntityKind::Actor | EntityKind::Terrain) {
                        let shown = format!("{}{}", value("material").unwrap_or("none"), from_template("material"));
                        rows.push(row("Material", shown, Some(Field::Material)));
                    }
                    let playing = value("anim");
                    if playing.is_some() || self.animations.get(&model).is_some_and(|a| !a.is_empty()) {
                        let shown = format!("{}{}", playing.unwrap_or("none"), from_template("anim"));
                        rows.push(row("Animation", shown, Some(Field::Animation)));
                    }
                    if model.ends_with(".mmdl") {
                        rows.push(row("Mesh editor (M)", String::new(), Some(Field::EditModel)));
                    }
                }
                rows.extend([
                    row("X", n(e.position.x), Some(Field::X)),
                    row("Y", n(e.position.y), Some(Field::Y)),
                    row("Z", n(e.position.z), Some(Field::Z)),
                    row("Yaw", n(yaw.to_degrees()), Some(Field::Yaw)),
                    row("Pitch", n(pitch.to_degrees()), Some(Field::Pitch)),
                    row("Roll", n(roll.to_degrees()), Some(Field::Roll)),
                    row("Scale", n(e.scale), Some(Field::Scale)),
                ]);
                if e.kind == EntityKind::Prop {
                    let on = if is_static(&options) { "on" } else { "off" };
                    rows.push(row("Static", format!("{on}{}", from_template("static")), Some(Field::Static)));
                }
                if e.kind.is_drawn() {
                    let occluder = value("occluder").unwrap_or("mesh");
                    rows.push(row("Occluder", format!("{occluder}{}", from_template("occluder")), Some(Field::Occluder)));
                    let shadow = value("shadow").unwrap_or("soft");
                    rows.push(row("Shadow", format!("{shadow}{}", from_template("shadow")), Some(Field::Shadow)));
                }
                if e.kind.is_drawn() {
                    self.exclusion_rows(Excluder::Entity, i, &mut rows);
                }
                rows.push(row("Sector", self.doc.sectors[e.sector].name.clone(), None));
                rows.push(row("Duplicate", String::new(), Some(Field::Duplicate)));
                rows.push(row("Delete", String::new(), Some(Field::Delete)));
                rows
            }
        }
    }

    // ---- Light exclusion lists

    /// What `selection` is, as something with a list of the lights it isn't lit by: a
    /// solid surface, a sector, or a drawn entity.
    fn excluder(&self, selection: Selection) -> Option<(Excluder, usize)> {
        match selection {
            Selection::Surface(i) if self.doc.surfaces[i].adjoin.is_none() => Some((Excluder::Surface, i)),
            Selection::Sector(s) => Some((Excluder::Sector, s)),
            Selection::Entity(e) if self.doc.entities[e].kind.is_drawn() => Some((Excluder::Entity, e)),
            _ => None,
        }
    }

    /// The lights `thing` `i` isn't lit by, by name (an entity's own, else its template's),
    /// and whether they are its template's.
    pub fn excluded(&self, thing: Excluder, i: usize) -> (Vec<String>, bool) {
        let names = |options: &[String]| -> Option<Vec<String>> {
            let list = options.iter().find_map(|o| o.strip_prefix("exclude_lights="))?;
            Some(if list == "none" { Vec::new() } else { list.split(',').map(str::to_string).collect() })
        };
        match thing {
            Excluder::Surface => (names(&self.doc.surfaces[i].options).unwrap_or_default(), false),
            Excluder::Sector => (names(&self.doc.sectors[i].options).unwrap_or_default(), false),
            Excluder::Entity => {
                let e = &self.doc.entities[i];
                match names(&e.options) {
                    Some(own) => (own, false),
                    None => (self.doc.template_of(e).and_then(|t| names(&t.options)).unwrap_or_default(), true),
                }
            }
        }
    }

    /// Excludes light `name` from `thing` `i`, or lights it with it again if it was
    /// excluded. Returns whether it is excluded now. An entity keeps its own list only
    /// where it differs from its template's.
    fn toggle_excluded(&mut self, thing: Excluder, i: usize, name: &str) -> bool {
        let (mut names, _) = self.excluded(thing, i);
        let off = !names.iter().any(|n| n == name);
        if off {
            names.push(name.to_string());
        } else {
            names.retain(|n| n != name);
        }
        let template = match thing {
            Excluder::Entity => {
                let e = &self.doc.entities[i];
                let t = self.doc.template_of(e).and_then(|t| {
                    t.options.iter().find_map(|o| o.strip_prefix("exclude_lights=")).map(str::to_string)
                });
                Some(t)
            }
            _ => None,
        };
        let options = match thing {
            Excluder::Surface => &mut self.doc.surfaces[i].options,
            Excluder::Sector => &mut self.doc.sectors[i].options,
            Excluder::Entity => &mut self.doc.entities[i].options,
        };
        options.retain(|o| moose_assets::option_key(o) != "exclude_lights");
        let list = if names.is_empty() { "none".to_string() } else { names.join(",") };
        let same_as_template = match &template {
            Some(t) => t.as_deref().unwrap_or("none") == list,
            None => names.is_empty(),
        };
        if !same_as_template {
            options.push(format!("exclude_lights={list}"));
        }
        off
    }

    /// Light `i`'s name, given one (`light_N`, unused) if it has none.
    fn light_name(&mut self, i: usize) -> String {
        if let Some(name) = light_option(&self.doc.lights[i], "name") {
            return name.to_string();
        }
        let name = self.unused_light_name("light");
        set_light_option(&mut self.doc.lights[i], "name", Some(name.clone()));
        name
    }

    /// The sun's name (the first directional light's), given one (`sun`) if it has none;
    /// `None` without a sun.
    fn sun_name(&mut self) -> Option<String> {
        let named = |o: &String| o.strip_prefix("name=").map(str::to_string);
        let sun = self.doc.directional.first()?;
        if let Some(name) = sun.options.iter().find_map(named) {
            return Some(name);
        }
        let name = self.unused_light_name("sun");
        self.doc.directional[0].options.push(format!("name={name}"));
        Some(name)
    }

    /// `base`, or `base_N`, so no light has it.
    fn unused_light_name(&self, base: &str) -> String {
        let taken: Vec<&str> = self
            .doc
            .lights
            .iter()
            .flat_map(|l| &l.options)
            .chain(self.doc.directional.iter().flat_map(|d| &d.options))
            .filter_map(|o| o.strip_prefix("name="))
            .collect();
        std::iter::once(base.to_string())
            .chain((1..).map(|k| format!("{base}_{k}")))
            .find(|n| !taken.contains(&n.as_str()) && n != "flashlight" && n != "none")
            .unwrap()
    }

    /// Takes light `name` out of every exclusion list (it is going). A list left empty
    /// goes, except an entity's over a template's list, which stays as `none`.
    fn forget_light(&mut self, name: &str) {
        let forget = |options: &mut Vec<String>, keep_none: bool| {
            for o in options.iter_mut() {
                if let Some(list) = o.strip_prefix("exclude_lights=")
                    && list.split(',').any(|n| n == name)
                {
                    let kept: Vec<&str> = list.split(',').filter(|n| *n != name).collect();
                    *o = match kept.is_empty() {
                        true if keep_none => "exclude_lights=none".to_string(),
                        true => String::new(),
                        false => format!("exclude_lights={}", kept.join(",")),
                    };
                }
            }
            options.retain(|o| !o.is_empty());
        };
        let doc = &mut self.doc;
        for e in 0..doc.entities.len() {
            let over = doc
                .template_of(&doc.entities[e])
                .is_some_and(|t| t.options.iter().any(|o| moose_assets::option_key(o) == "exclude_lights"));
            forget(&mut doc.entities[e].options, over);
        }
        for options in doc
            .surfaces
            .iter_mut()
            .map(|s| &mut s.options)
            .chain(doc.sectors.iter_mut().map(|s| &mut s.options))
            .chain(doc.templates.iter_mut().map(|t| &mut t.options))
        {
            forget(options, false);
        }
    }

    /// The rows for `thing` `i`'s light exclusion list: the lights it isn't lit by
    /// (click: pick lights to toggle), and whether the flashlight and the sun light it.
    fn exclusion_rows(&self, thing: Excluder, i: usize, rows: &mut Vec<PanelRow>) {
        let (names, template) = self.excluded(thing, i);
        let sun = self.doc.directional.first().and_then(|d| d.options.iter().find_map(|o| o.strip_prefix("name=")));
        let listed: Vec<&str> =
            names.iter().map(String::as_str).filter(|n| *n != "flashlight" && Some(*n) != sun).collect();
        let shown = if self.picking == Some((i, Pick::Exclude(thing))) {
            "click lights (again: lit again)".to_string()
        } else {
            let list = if listed.is_empty() { "none".to_string() } else { listed.join(", ") };
            format!("{list}{}", if template { " (template)" } else { "" })
        };
        rows.push(PanelRow { label: "Lights kept off".into(), value: shown, field: Some(Field::ExcludeLights) });
        let lit = |name: Option<&str>| if name.is_some_and(|n| names.iter().any(|m| m == n)) { "off" } else { "on" };
        rows.push(PanelRow {
            label: "Lit by the flashlight".into(),
            value: lit(Some("flashlight")).into(),
            field: Some(Field::FlashlightLights),
        });
        if !self.doc.directional.is_empty() {
            rows.push(PanelRow { label: "Lit by the sun".into(), value: lit(sun).into(), field: Some(Field::SunLights) });
        }
    }

    // ---- Changes

    /// Carries out `field` when it isn't an edit (see [`Field::is_edit`]).
    pub fn command(&mut self, field: Field, dir: f32) {
        match (self.selection, field) {
            (Some(Selection::Surface(i)), Field::Stitch) => {
                self.picking = Some((i, Pick::Stitch));
                self.say("click the touching face to continue its material and texture; right-click mirrors it (Esc stops)");
            }
            (Some(Selection::Surface(i)), Field::Merge) => {
                self.picking = Some((i, Pick::Merge));
                self.say("click the touching face to merge into this one (Esc stops)");
            }
            (Some(Selection::Surface(i)), Field::SelectSector) => {
                self.selection = Some(Selection::Sector(self.doc.surfaces[i].sector));
            }
            (_, Field::ExtrudeBy) => {
                let step = self.step();
                self.extrude_by = snap((self.extrude_by + step * dir.signum()).max(step), step);
            }
            (Some(selection), Field::ExcludeLights) => {
                if let Some((thing, i)) = self.excluder(selection) {
                    self.picking = Some((i, Pick::Exclude(thing)));
                    self.say("click lights to keep them off this (again: lit by it again); Esc stops");
                }
            }
            (Some(Selection::Sector(s)), Field::MergeSector) => {
                self.picking = Some((s, Pick::MergeSector));
                self.say("click a face of the sector to merge into this one, or an opening onto it (Esc stops)");
            }
            (Some(Selection::Sector(_)), Field::Cleave) => {
                self.cutting = Some(Vec::new());
                self.say("cleave: click where the cut starts and ends, in a 2D view (Esc: stop)");
            }
            _ => {}
        }
    }

    /// Changes the selection's `field` (`dir` is the way: +1 or -1, or scroll lines), in
    /// the tables. `world` finds the sector a moved entity is in; `models` are the model
    /// files to cycle through. Returns what was done, or why it can't be.
    pub fn change(&mut self, field: Field, dir: f32, world: &World, models: &[String]) -> Result<String, String> {
        let step = self.step();
        let n = moose_assets::number;
        match (self.selection, field) {
            (None, Field::NewRoom) => {
                let min = (self.anchor - Vec3::new(ROOM.x / 2.0, 1.7, ROOM.z / 2.0)).map(|v| snap(v, step));
                let sector = self.doc.add_box(min, min + ROOM, "room")?;
                self.selection = Some(Selection::Sector(sector));
                Ok(format!("added {}: extrude a wall into it, or open matching walls", self.doc.sectors[sector].name))
            }
            (_, Field::Paste) => {
                let at = self.anchor.map(|v| snap(v, step));
                let sector = sector_of(world, at)?;
                match self.clipboard.clone() {
                    Some(Clip::Entity(mut e)) => {
                        e.name = self.unique_name(&e.name);
                        (e.position, e.sector) = (at, sector);
                        let name = e.name.clone();
                        self.doc.entities.push(e);
                        self.selection = Some(Selection::Entity(self.doc.entities.len() - 1));
                        Ok(format!("pasted {name}"))
                    }
                    Some(Clip::Light(mut l)) => {
                        (l.position, l.sector) = (at, sector);
                        self.doc.lights.push(l);
                        self.selection = Some(Selection::Light(self.doc.lights.len() - 1));
                        Ok("pasted a light".into())
                    }
                    None => Err("nothing copied (Ctrl+C)".into()),
                }
            }
            (Some(Selection::Surface(i)), Field::TextureSizeU | Field::TextureSizeV | Field::TextureTurn) => {
                // Sized or turned about the middle of its texture: by a step a notch (a fine
                // one with Alt).
                let mut m = self.texture_mapping(i).ok_or("this surface's texture coordinates aren't a flat mapping")?;
                match field {
                    Field::TextureTurn => m.turn += if self.fine { 1.0 } else { 15.0 } * dir,
                    _ => {
                        let k = if self.fine { 1.01f32 } else { 1.25 }.powf(dir);
                        if field == Field::TextureSizeU { m.size.x *= k } else { m.size.y *= k }
                    }
                }
                self.set_texture_mapping(i, m, true);
                Ok(format!("surface {i}'s texture {}", if field == Field::TextureTurn { "turned" } else { "sized" }))
            }
            (
                Some(Selection::Surface(i)),
                Field::TextureU
                | Field::TextureV
                | Field::TextureScale
                | Field::TextureFlipAcross
                | Field::TextureFlipDown
                | Field::TextureProject,
            ) => {
                let normal = self.doc.surface_normal(i);
                let uvs = self.corner_values(i, "uv");
                if uvs.is_empty() {
                    return Err("this level has no texture coordinates".into());
                }
                let center = uvs.iter().fold(Vec2::ZERO, |c, uv| c + Vec2::new(uv[0], uv[1])) / uvs.len() as f32;
                // An eighth of a repeat a notch, or with Alt a 128th; scaled by a quarter, or
                // a hundredth.
                let (shift, scale) = if self.fine { (1.0 / 128.0, 1.01f32) } else { (0.125, 1.25) };
                self.doc.set_corner_values(i, "uv", |_, p, uv| {
                    let t = Vec2::new(uv[0], uv[1]);
                    let t = match field {
                        Field::TextureU => t + Vec2::X * shift * dir,
                        Field::TextureV => t + Vec2::Y * shift * dir,
                        Field::TextureScale => center + (t - center) * scale.powf(-dir),
                        // Mirrored about the middle of the surface's texture.
                        Field::TextureFlipAcross => Vec2::new(2.0 * center.x - t.x, t.y),
                        Field::TextureFlipDown => Vec2::new(t.x, 2.0 * center.y - t.y),
                        _ => planar_uv(p, normal),
                    };
                    vec![t.x, t.y]
                });
                Ok(match field {
                    Field::TextureFlipAcross => format!("surface {i}'s texture flipped across"),
                    Field::TextureFlipDown => format!("surface {i}'s texture flipped down"),
                    _ => format!("surface {i}'s texture moved"),
                })
            }
            (Some(Selection::Surface(i)), Field::Reflective | Field::Sky) => {
                let bit = if field == Field::Reflective { 0x1 } else { 0x2 };
                let s = &mut self.doc.surfaces[i];
                s.flags ^= bit;
                // A surface can't be both.
                s.flags &= !(0x3 & !bit);
                Ok(format!("surface {i}: flags {:#x}", s.flags))
            }
            (Some(Selection::Surface(i)), Field::Extrude) => {
                let (sector, far) = self.doc.extrude(i, self.extrude_by)?;
                self.selection = Some(Selection::Surface(far));
                Ok(format!("extruded into {} (its far end is selected)", self.doc.sectors[sector].name))
            }
            (Some(Selection::Surface(i)), Field::Material) => {
                let name = cycle_material(&mut self.doc.surfaces[i].options, &self.materials, dir);
                Ok(format!("surface {i} drawn with {name}"))
            }
            (_, Field::Picked | Field::PickedMirrored) => {
                let (target, kind) = self.picking.take().ok_or("no surface to click")?;
                let mirrored = field == Field::PickedMirrored;
                if let Pick::Exclude(thing) = kind {
                    // Picking goes on, for the next light.
                    self.picking = Some((target, kind));
                    let Some(Selection::Light(light)) = self.hover else {
                        return Err("click a light (Esc stops)".into());
                    };
                    let name = self.light_name(light);
                    let off = self.toggle_excluded(thing, target, &name);
                    return Ok(format!("{name} {}", if off { "kept off it" } else { "lights it again" }));
                }
                let Some(Selection::Surface(source)) = self.hover else {
                    return Err("click a surface".into());
                };
                match kind {
                    Pick::Stitch => {
                        let textured = self.doc.stitch(target, source, mirrored)?;
                        self.selection = Some(Selection::Surface(target));
                        Ok(match textured {
                            true if mirrored => format!("surface {target} mirrors surface {source}'s material and texture"),
                            true => format!("surface {target} continues surface {source}'s material and texture"),
                            false => format!("surface {target} has surface {source}'s material (this level has no texture coordinates)"),
                        })
                    }
                    Pick::Merge | Pick::MergeSector | Pick::Exclude(_) if mirrored => {
                        self.picking = Some((target, kind));
                        Err("a merge has no mirrored kind: left-click the face".into())
                    }
                    Pick::Merge => {
                        let merged = self.doc.merge_surfaces(target, source)?;
                        self.selection = Some(Selection::Surface(merged));
                        Ok(format!("surface {source} merged into surface {merged}"))
                    }
                    Pick::Exclude(_) => unreachable!("handled above"),
                    Pick::MergeSector => {
                        // An opening of the sector itself stands for the sector beyond it.
                        let clicked = &self.doc.surfaces[source];
                        let other = match clicked.adjoin {
                            Some(a) if clicked.sector == target => {
                                self.doc.surfaces[self.doc.adjoins[self.doc.adjoins[a].mirror].surface].sector
                            }
                            _ => clicked.sector,
                        };
                        let name = self.doc.sectors[other].name.clone();
                        let merged = self.doc.merge_sectors(target, other)?;
                        self.selection = Some(Selection::Sector(merged));
                        Ok(format!("{name} merged into {}", self.doc.sectors[merged].name))
                    }
                }
            }
            (Some(Selection::Surface(i)), Field::CutFace) => {
                let cut = self.face_cut.take().ok_or("no cut drawn")?;
                let ViewMode::Ortho(axis) = self.view else {
                    return Err("cut a surface in a 2D view".into());
                };
                // The plane through the line, square to the view.
                let (right, up) = axis.basis();
                let normal = (cut.to - cut.from).cross(right.cross(up));
                if normal.length_squared() < 1e-8 {
                    return Err("the cut's ends are the same point".into());
                }
                let other = self.doc.cut_surface(i, normal, cut.from)?;
                Ok(format!("cut surface {i} in two: {i} and {other}"))
            }
            (Some(Selection::Surface(i)), Field::Push) => {
                self.doc.push_surface(i, -step * dir.signum());
                Ok(format!("surface {i} {} {} m", if dir > 0.0 { "pushed out" } else { "pulled in" }, n(step)))
            }
            (Some(Selection::Surface(i)), Field::Adjoin) => {
                let other = self.doc.adjoin(i)?;
                Ok(format!("opened to {}", self.doc.sectors[self.doc.surfaces[other].sector].name))
            }
            (Some(Selection::Surface(i)), Field::Unadjoin) => {
                self.doc.unadjoin(i)?;
                Ok("walled up".into())
            }
            (Some(Selection::Sector(s)), Field::Delete) => {
                let name = self.doc.sectors[s].name.clone();
                self.doc.delete_sector(s)?;
                self.selection = None;
                Ok(format!("deleted {name}"))
            }
            (Some(Selection::Sector(s)), Field::Cut) => {
                let points = self.cutting.take().unwrap_or_default();
                let ViewMode::Ortho(axis) = self.view else {
                    return Err("cut in a 2D view".into());
                };
                let [a, b] = points[..] else {
                    return Err("a cut runs through two points".into());
                };
                let (right, up) = axis.basis();
                let normal = (b - a).cross(right.cross(up));
                if normal.length_squared() < 1e-8 {
                    return Err("the cut's ends are the same point".into());
                }
                let new = self.doc.cleave(s, normal, a)?;
                Ok(format!("cleaved into {} and {}", self.doc.sectors[s].name, self.doc.sectors[new].name))
            }
            (Some(Selection::Vertex(v)), Field::X | Field::Y | Field::Z) => {
                let axis = match field {
                    Field::X => Vec3::X,
                    Field::Y => Vec3::Y,
                    _ => Vec3::Z,
                };
                let p = self.doc.vertices[v];
                let to = snap(p.dot(axis) + step * dir, step);
                self.doc.move_vertex(v, axis * (to - p.dot(axis)));
                let p = self.doc.vertices[v];
                Ok(format!("vertex {v} at {}, {}, {}", n(p.x), n(p.y), n(p.z)))
            }
            (None, Field::AddLight | Field::AddProp | Field::AddSpawn) => {
                let at = self.anchor.map(|v| snap(v, step));
                let sector = sector_of(world, at)?;
                match field {
                    Field::AddLight => {
                        self.doc.lights.push(LightDoc {
                            sector,
                            position: at,
                            color: Vec3::new(1.0, 0.9, 0.75),
                            range: 6.0,
                            spot: None,
                            options: vec!["radius=0.05".into()],
                        });
                        self.selection = Some(Selection::Light(self.doc.lights.len() - 1));
                        Ok("added a light".into())
                    }
                    _ => {
                        let spawn = field == Field::AddSpawn;
                        let model = (!spawn).then(|| {
                            models.iter().find(|m| m.as_str() == "crate.obj").or(models.first()).cloned()
                        });
                        let model = match model {
                            Some(None) => return Err("there are no models in assets/models".into()),
                            Some(Some(m)) => Some(m),
                            None => None,
                        };
                        let name = self.unique_name(if spawn { "spawn" } else { "prop" });
                        self.doc.entities.push(EntityDoc {
                            kind: if spawn { EntityKind::Spawn } else { EntityKind::Prop },
                            sector,
                            model,
                            position: at,
                            rotation: Quat::IDENTITY,
                            scale: 1.0,
                            name: name.clone(),
                            options: if spawn { Vec::new() } else { vec!["static".into()] },
                        });
                        self.selection = Some(Selection::Entity(self.doc.entities.len() - 1));
                        Ok(format!("added {name}"))
                    }
                }
            }
            (None, Field::AmbientRed | Field::AmbientGreen | Field::AmbientBlue) => {
                let a = self.doc.ambient.get_or_insert(Vec3::ONE);
                let c = match field {
                    Field::AmbientRed => &mut a.x,
                    Field::AmbientGreen => &mut a.y,
                    _ => &mut a.z,
                };
                *c = (*c + 0.01 * dir).max(0.0);
                Ok(format!("ambient {}, {}, {}", n(a.x), n(a.y), n(a.z)))
            }
            (None, Field::SunAngle | Field::SunYaw | Field::SunPitch) => {
                let Some(sun) = self.doc.directional.first_mut() else {
                    return Err("the level has no sun".into());
                };
                match field {
                    Field::SunAngle => sun.angle = (sun.angle + 0.25 * dir).clamp(0.0, 45.0),
                    _ => {
                        let (mut yaw, mut pitch) = aim_of(-sun.direction);
                        if field == Field::SunYaw {
                            yaw += TURN * dir;
                        } else {
                            pitch = (pitch + 5.0 * dir).clamp(5.0, 90.0);
                        }
                        sun.direction = -aim(yaw, pitch);
                    }
                }
                Ok("sun moved".into())
            }
            (Some(selection), Field::FlashlightLights | Field::SunLights) => {
                let (thing, i) = self.excluder(selection).ok_or("this has no lights to keep off")?;
                let name = match field {
                    Field::FlashlightLights => "flashlight".to_string(),
                    _ => self.sun_name().ok_or("this level has no sun")?,
                };
                let off = self.toggle_excluded(thing, i, &name);
                Ok(format!("{} {}", if field == Field::SunLights { "the sun" } else { "the flashlight" }, if off { "kept off it" } else { "lights it again" }))
            }
            (Some(Selection::Light(i)), Field::Duplicate) => {
                let mut copy = self.doc.lights[i].clone();
                // Names are each light's own.
                copy.options.retain(|o| moose_assets::option_key(o) != "name");
                copy.position.x += step;
                copy.sector = sector_of(world, copy.position)?;
                self.doc.lights.push(copy);
                self.selection = Some(Selection::Light(self.doc.lights.len() - 1));
                Ok("duplicated the light".into())
            }
            (Some(Selection::Light(i)), Field::Delete) => {
                // What kept it off forgets it.
                if let Some(name) = light_option(&self.doc.lights[i], "name").map(str::to_string) {
                    self.forget_light(&name);
                }
                self.doc.lights.remove(i);
                self.selection = None;
                Ok("deleted the light".into())
            }
            (Some(Selection::Light(i)), field) => {
                let l = &mut self.doc.lights[i];
                match field {
                    Field::X => l.position.x = snap(l.position.x + step * dir, step),
                    Field::Y => l.position.y = snap(l.position.y + step * dir, step),
                    Field::Z => l.position.z = snap(l.position.z + step * dir, step),
                    Field::Red => l.color.x = (l.color.x + 0.05 * dir).max(0.0),
                    Field::Green => l.color.y = (l.color.y + 0.05 * dir).max(0.0),
                    Field::Blue => l.color.z = (l.color.z + 0.05 * dir).max(0.0),
                    Field::Range => l.range = (l.range + 0.5 * dir).max(0.5),
                    Field::Radius => {
                        let r = light_option(l, "radius").and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
                        let r = (r + 0.01 * dir).max(0.0);
                        set_light_option(l, "radius", (r > 0.0).then(|| n(r)));
                    }
                    Field::Shadows => {
                        let off = light_option(l, "shadows") == Some("off");
                        set_light_option(l, "shadows", (!off).then(|| "off".to_string()));
                    }
                    Field::Spot => {
                        l.spot = match l.spot {
                            Some(_) => None,
                            None => Some((Vec3::NEG_Y, 25.0, 40.0)),
                        };
                    }
                    Field::Yaw | Field::Pitch | Field::Inner | Field::Outer => {
                        let Some((dir3, inner, outer)) = &mut l.spot else {
                            return Err("that light isn't a spot light".into());
                        };
                        match field {
                            Field::Inner => *inner = (*inner + dir).clamp(0.0, *outer),
                            Field::Outer => *outer = (*outer + dir).clamp(*inner, 180.0),
                            _ => {
                                let (mut yaw, mut pitch) = aim_of(*dir3);
                                if field == Field::Yaw {
                                    yaw += TURN * dir;
                                } else {
                                    pitch = (pitch + TURN * dir).clamp(-90.0, 90.0);
                                }
                                *dir3 = aim(yaw, pitch);
                            }
                        }
                    }
                    _ => return Err("that doesn't apply here".into()),
                }
                if matches!(field, Field::X | Field::Y | Field::Z) {
                    l.sector = sector_of(world, l.position)?;
                }
                Ok(format!("light {i} changed"))
            }
            (Some(Selection::Entity(i)), Field::Duplicate) => {
                let mut copy = self.doc.entities[i].clone();
                copy.name = self.unique_name(&copy.name);
                copy.position.x += step;
                copy.sector = sector_of(world, copy.position)?;
                let name = copy.name.clone();
                self.doc.entities.push(copy);
                self.selection = Some(Selection::Entity(self.doc.entities.len() - 1));
                Ok(format!("duplicated as {name}"))
            }
            (Some(Selection::Entity(i)), Field::Delete) => {
                let e = &self.doc.entities[i];
                let spawns = self.doc.entities.iter().filter(|e| e.kind == EntityKind::Spawn).count();
                if e.kind == EntityKind::Spawn && spawns == 1 {
                    return Err("a level needs a spawn point".into());
                }
                let name = e.name.clone();
                self.doc.entities.remove(i);
                self.selection = None;
                Ok(format!("deleted {name}"))
            }
            (Some(Selection::Entity(i)), field) => {
                // Its template's options, and the templates it could place instead of a model.
                let template: Vec<String> =
                    self.doc.template_of(&self.doc.entities[i]).map(|t| t.options.clone()).unwrap_or_default();
                let models: Vec<String> =
                    self.doc.templates.iter().map(|t| t.name.clone()).chain(models.iter().cloned()).collect();
                let model_files: Vec<(String, String)> =
                    self.doc.templates.iter().map(|t| (t.name.clone(), t.model.clone())).collect();
                let materials = self.materials.clone();
                let e = &mut self.doc.entities[i];
                let turn = |q: Quat, which: usize| {
                    let (mut yaw, mut pitch, mut roll) = q.to_euler(EulerRot::YXZ);
                    let t = (TURN * dir).to_radians();
                    match which {
                        0 => yaw += t,
                        1 => pitch += t,
                        _ => roll += t,
                    }
                    Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll)
                };
                match field {
                    Field::X => e.position.x = snap(e.position.x + step * dir, step),
                    Field::Y => e.position.y = snap(e.position.y + step * dir, step),
                    Field::Z => e.position.z = snap(e.position.z + step * dir, step),
                    Field::Yaw => e.rotation = turn(e.rotation, 0),
                    Field::Pitch => e.rotation = turn(e.rotation, 1),
                    Field::Roll => e.rotation = turn(e.rotation, 2),
                    Field::Scale => e.scale = (e.scale * 1.1f32.powf(dir)).clamp(0.01, 100.0),
                    Field::Kind => {
                        e.kind = match e.kind {
                            EntityKind::Prop => EntityKind::Actor,
                            _ => EntityKind::Prop,
                        };
                        if e.kind == EntityKind::Actor {
                            not_static(e, &template);
                        }
                    }
                    Field::Material => {
                        // Its own; none falls back to its template's.
                        cycle_material(&mut e.options, &materials, dir);
                    }
                    Field::Model => {
                        let now = models.iter().position(|m| Some(m) == e.model.as_ref());
                        let next = match now {
                            Some(k) => (k as isize + dir.signum() as isize).rem_euclid(models.len() as isize),
                            None => 0,
                        };
                        e.model = models.get(next as usize).cloned().or(e.model.take());
                        // The new model's animations may differ.
                        e.options.retain(|o| !o.starts_with("anim="));
                    }
                    Field::Animation => {
                        // none → each of the model's animations → none.
                        let file = e.model.as_ref().map(|m| {
                            model_files.iter().find(|(t, _)| t == m).map_or(m.clone(), |(_, f)| f.clone())
                        });
                        let names = file.and_then(|m| self.animations.get(&m)).cloned().unwrap_or_default();
                        let options = moose_assets::merged_options(&template, &e.options);
                        let now = options.iter().find_map(|o| o.strip_prefix("anim="));
                        let k = now.and_then(|a| names.iter().position(|n| n == a)).map_or(0, |k| k + 1);
                        let next = (k as isize + dir.signum() as isize).rem_euclid(names.len() as isize + 1) as usize;
                        e.options.retain(|o| !o.starts_with("anim="));
                        if next > 0 {
                            e.options.push(format!("anim={}", names[next - 1]));
                            // Animated models move, so they can't be static.
                            not_static(e, &template);
                        } else if template.iter().any(|o| o.starts_with("anim=") && o != "anim=none") {
                            // None, over its template's.
                            e.options.push("anim=none".into());
                        }
                    }
                    Field::Static => {
                        // Its own option only where it differs from its template.
                        let on = !is_static(&moose_assets::merged_options(&template, &e.options));
                        e.options.retain(|o| moose_assets::option_key(o) != "static");
                        if on != is_static(&template) {
                            e.options.push(if on { "static" } else { "static=off" }.into());
                        }
                    }
                    Field::Shadow => {
                        // soft → blurred → hard → soft; its own option only where it
                        // differs from its template.
                        const KINDS: [&str; 3] = ["soft", "blurred", "hard"];
                        let of = |options: &[String]| {
                            options.iter().find_map(|o| o.strip_prefix("shadow=")).unwrap_or("soft").to_string()
                        };
                        let now = of(&moose_assets::merged_options(&template, &e.options));
                        let k = KINDS.iter().position(|&s| s == now).unwrap_or(0) as isize;
                        let next = KINDS[(k + dir.signum() as isize).rem_euclid(3) as usize];
                        e.options.retain(|o| moose_assets::option_key(o) != "shadow");
                        if next != of(&template) {
                            e.options.push(format!("shadow={next}"));
                        }
                    }
                    Field::Occluder => {
                        // mesh (the default) → none → back.
                        let now = e.options.iter().position(|o| o.starts_with("occluder="));
                        match now {
                            Some(k) => {
                                e.options.remove(k);
                            }
                            None => e.options.push("occluder=none".into()),
                        }
                    }
                    _ => return Err("that doesn't apply here".into()),
                }
                if matches!(field, Field::X | Field::Y | Field::Z) {
                    e.sector = sector_of(world, e.position)?;
                }
                let (name, p) = (e.name.clone(), e.position);
                Ok(format!("{name} at {}, {}, {}", n(p.x), n(p.y), n(p.z)))
            }
            _ => Err("that doesn't apply here".into()),
        }
    }

    /// While cutting, a click in a 2D view at world point `p` (on the grid). Returns
    /// whether the cut now has both its points.
    /// Where a cut's end goes for the pointer at `at` in 2D view `ortho`: the nearest
    /// vertex's place within a few pixels with `to_vertex` (Ctrl), else on the grid.
    pub fn cut_snap(&self, ortho: &Ortho, at: Vec2, to_vertex: bool) -> Vec3 {
        let vertex = to_vertex
            .then(|| {
                self.used_vertices()
                    .into_iter()
                    .map(|v| (ortho.to_screen(self.doc.vertices[v]).distance(at), v))
                    .filter(|&(d, _)| d < 3.0 * PICK_RADIUS)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
            })
            .flatten();
        match vertex {
            Some((_, v)) => self.doc.vertices[v],
            None => ortho.to_world(at.x, at.y).map(|v| snap(v, self.step())),
        }
    }

    /// While cleaving, an end of the cut at `p` (snapped already: see `pointer`). Returns
    /// whether that is both.
    pub fn cut_point(&mut self, p: Vec3) -> bool {
        let Some(points) = &mut self.cutting else {
            return false;
        };
        points.push(p);
        points.len() == 2
    }

    /// Surface `i`'s texture mapping as numbers (see [`TextureMapping`]), fitted to its
    /// corners' texture coordinates; `None` without them, or if they don't make one (all
    /// in a line).
    pub fn texture_mapping(&self, i: usize) -> Option<TextureMapping> {
        let normal = self.doc.surface_normal(i);
        let uvs = self.corner_values(i, "uv");
        let points = self.doc.surface_points(i);
        if uvs.len() != points.len() || uvs.is_empty() {
            return None;
        }
        let samples: Vec<([f32; 3], [f32; 2])> = points
            .iter()
            .zip(&uvs)
            .map(|(&p, uv)| {
                let q = flat_coords(p, normal);
                ([1.0, q.x, q.y], [uv[0], uv.get(1).copied().unwrap_or(0.0)])
            })
            .collect();
        let fit = moose_assets::fit_affine(&samples)?;
        // uv = offset + A q: A's columns are what one meter along each flat axis does.
        let (x, y) = (Vec2::from(fit[1]), Vec2::from(fit[2]));
        let turn = x.y.atan2(x.x);
        let (sin, cos) = turn.sin_cos();
        let along = x.length();
        // A = R(turn) [[1 / size.x, shear], [0, 1 / size.y]].
        let (shear, down) = (cos * y.x + sin * y.y, -sin * y.x + cos * y.y);
        if along < 1e-9 || down.abs() < 1e-9 {
            return None;
        }
        Some(TextureMapping {
            offset: Vec2::from(fit[0]),
            size: Vec2::new(1.0 / along, 1.0 / down),
            turn: turn.to_degrees(),
            shear,
        })
    }

    /// Whether a surface is selected whose texture mapping can be read as numbers.
    pub fn texture_mapping_selected(&self) -> bool {
        matches!(self.current(self.selection), Some(Selection::Surface(i)) if self.texture_mapping(i).is_some())
    }

    /// Gives surface `i` texture mapping `m`; with `keep_middle`, moved so the texture at
    /// the middle of the surface stays where it was (sizing and turning about it).
    pub fn set_texture_mapping(&mut self, i: usize, mut m: TextureMapping, keep_middle: bool) {
        let normal = self.doc.surface_normal(i);
        let points = self.doc.surface_points(i);
        if keep_middle && let Some(before) = self.texture_mapping(i) {
            let middle = points.iter().map(|&p| flat_coords(p, normal)).sum::<Vec2>() / points.len() as f32;
            m.offset += before.at(middle) - m.at(middle);
        }
        self.doc.set_corner_values(i, "uv", |_, p, _| m.at(flat_coords(p, normal)).to_array().to_vec());
    }

    /// Sets field `field` of the selected surface to typed number `value`.
    pub fn set_typed(&mut self, field: Field, value: f32) -> Result<String, String> {
        let Some(Selection::Surface(i)) = self.selection else {
            return Err("select a surface".into());
        };
        let mut m = self.texture_mapping(i).ok_or("this surface's texture coordinates aren't a flat mapping")?;
        let keep_middle = match field {
            Field::TextureU => {
                m.offset.x = value;
                false
            }
            Field::TextureV => {
                m.offset.y = value;
                false
            }
            Field::TextureSizeU | Field::TextureSizeV if value == 0.0 => {
                return Err("a repeat needs some size".into());
            }
            Field::TextureSizeU => {
                m.size.x = value;
                true
            }
            Field::TextureSizeV => {
                m.size.y = value;
                true
            }
            Field::TextureTurn => {
                m.turn = value;
                true
            }
            _ => return Err("that doesn't take a number".into()),
        };
        self.set_texture_mapping(i, m, keep_middle);
        Ok(format!("surface {i}'s texture set"))
    }

    /// Surface `surface`'s corners' values of attribute `name` (none if the level has no
    /// such attribute).
    fn corner_values(&self, surface: usize, name: &str) -> Vec<Vec<f32>> {
        let Some(k) = self.doc.attributes.iter().position(|a| a.name == name) else {
            return Vec::new();
        };
        self.doc.surfaces[surface]
            .corners
            .iter()
            .filter_map(|(_, rows)| rows.get(k))
            .map(|&row| self.doc.attributes[k].values[row].iter().map(|t| t.parse().unwrap_or(0.0)).collect())
            .collect()
    }

    /// `name` with a number after it that no entity has.
    fn unique_name(&self, name: &str) -> String {
        let base = name.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('_');
        (2..)
            .map(|k| format!("{base}_{k}"))
            .find(|n| self.doc.entities.iter().all(|e| &e.name != n))
            .unwrap()
    }

    // ---- Drawing

    /// Draws a 2D view: the grid, the level's wireframe, and the editor's marks.
    pub fn draw_2d(&self, canvas: &mut Canvas, ortho: &Ortho, world: &World, assets: &Assets) {
        wire::draw_grid(canvas, ortho, self.step());
        let projection = Projection::Ortho(ortho);
        wire::draw_level(canvas, &projection, world, assets);
        match &self.model {
            Some(model) => model.draw(canvas, &projection, world, assets),
            None => self.draw_marks(canvas, &projection, world, assets),
        }
    }

    /// Draws the editor over the frame: in the 3D view its wireframe (F5) and marks, then
    /// the panel and the status line.
    pub fn draw(&self, canvas: &mut Canvas, camera: &moose_scene::View, world: &World, assets: &Assets) {
        if self.view == ViewMode::Perspective {
            let projection = Projection::Perspective(camera);
            if self.wire {
                wire::draw_level(canvas, &projection, world, assets);
            }
            match &self.model {
                Some(model) => model.draw(canvas, &projection, world, assets),
                None => self.draw_marks(canvas, &projection, world, assets),
            }
        }
        self.draw_panel(canvas);
    }

    /// The level's lights (a diamond, with a line where a spot light shines) and spawn
    /// points (a square, with a line where they face); the selected or pointed-at one in
    /// its color, the selected light's reach as a circle in a 2D view.
    fn draw_markers(&self, canvas: &mut Canvas, projection: &Projection) {
        let color_of = |selection: Selection, normal: u32| {
            if self.selection == Some(selection) {
                SELECTED
            } else if self.hover == Some(selection) {
                HOVER
            } else {
                normal
            }
        };
        for (i, l) in self.doc.lights.iter().enumerate() {
            let color = color_of(Selection::Light(i), LIGHT);
            if let Some(p) = projection.point(l.position) {
                for (a, b) in [((0.0, -6.0), (6.0, 0.0)), ((6.0, 0.0), (0.0, 6.0)), ((0.0, 6.0), (-6.0, 0.0)), ((-6.0, 0.0), (0.0, -6.0))] {
                    canvas.line(p.x + a.0, p.y + a.1, p.x + b.0, p.y + b.1, color);
                }
            }
            if let Some((dir, _, _)) = l.spot {
                projection.line(canvas, l.position, l.position + dir.normalize_or_zero() * 0.75, color);
            }
            if self.selection == Some(Selection::Light(i))
                && let Projection::Ortho(o) = projection
            {
                let (right, up) = o.axis.basis();
                let ring: Vec<Vec3> = (0..48)
                    .map(|k| {
                        let a = k as f32 * std::f32::consts::TAU / 48.0;
                        l.position + (right * a.cos() + up * a.sin()) * l.range
                    })
                    .collect();
                wire::outline(canvas, projection, &ring, RANGE);
            }
        }
        for (i, e) in self.doc.entities.iter().enumerate() {
            if e.kind != EntityKind::Spawn {
                continue;
            }
            let color = color_of(Selection::Entity(i), SPAWN);
            if let Some(p) = projection.point(e.position) {
                canvas.fill_centered(p.x, p.y, 7, color);
            }
            projection.line(canvas, e.position, e.position + e.rotation * Vec3::NEG_Z * 0.75, color);
        }
    }

    /// The selection and what the pointer is over, outlined; the vertices, with vertex
    /// picking on; and a cut being drawn.
    fn draw_marks(&self, canvas: &mut Canvas, projection: &Projection, world: &World, assets: &Assets) {
        self.draw_markers(canvas, projection);
        if self.vertices {
            for v in self.used_vertices() {
                if let Some(p) = projection.point(self.doc.vertices[v]) {
                    canvas.fill_centered(p.x, p.y, 3, VERTEX);
                }
            }
        }
        let problem = self
            .problem
            .filter(|(_, at)| at.elapsed() < STATUS_TIME)
            .map(|(selection, _)| selection);
        for (selection, color) in [(self.hover, HOVER), (self.selection, SELECTED), (problem, PROBLEM)] {
            // Something the tables no longer have (they changed since it was picked) isn't drawn.
            match self.current(selection) {
                Some(Selection::Surface(i)) => {
                    wire::outline(canvas, projection, &self.doc.surface_points(i), color);
                }
                Some(Selection::Sector(s)) => {
                    for i in self.doc.sector_surfaces(s) {
                        wire::outline(canvas, projection, &self.doc.surface_points(i), color);
                    }
                }
                Some(Selection::Vertex(v)) => {
                    if let Some(p) = projection.point(self.doc.vertices[v]) {
                        canvas.fill_centered(p.x, p.y, 7, color);
                    }
                }
                Some(Selection::Light(_)) => {}
                Some(Selection::Entity(row)) => {
                    let Some(e) = self.world_entity(row).and_then(|k| world.entities.get(k)) else {
                        continue;
                    };
                    let mesh = assets.mesh(e.mesh);
                    let transform = e.transform();
                    for polygon in &mesh.polygons {
                        let points: Vec<Vec3> =
                            mesh.polygon_points(polygon).map(|p| transform.transform_point3(p)).collect();
                        wire::outline(canvas, projection, &points, color);
                    }
                }
                None => {}
            }
        }
        if let Some(cut) = &self.face_cut {
            projection.line(canvas, cut.from, cut.to, CUT);
            for p in [cut.from, cut.to] {
                if let Some(s) = projection.point(p) {
                    canvas.fill_centered(s.x, s.y, 5, CUT);
                }
            }
        }
        if let Some(points) = &self.cutting {
            let mut line: Vec<Vec3> = points.clone();
            line.extend(self.pointer);
            for pair in line.windows(2) {
                projection.line(canvas, pair[0], pair[1], CUT);
            }
            for &p in points {
                if let Some(s) = projection.point(p) {
                    canvas.fill_centered(s.x, s.y, 5, CUT);
                }
            }
        }
    }

    fn draw_panel(&self, canvas: &mut Canvas) {
        let layout = self.layout(canvas.width, canvas.height);
        canvas.shade(layout.x, layout.y, layout.width, layout.height, 70);
        let view = match self.view {
            ViewMode::Perspective => "3D",
            ViewMode::Ortho(axis) => axis.name(),
        };
        let title = match &self.model {
            Some(model) => format!("MESH EDITOR  {view}{}", if model.dirty() { "  (unsaved)" } else { "" }),
            None => format!(
                "EDITOR  {view}{}{}",
                if self.vertices { "  vertices" } else { "" },
                if self.dirty() { "  (unsaved)" } else { "" }
            ),
        };
        canvas.text(layout.x + layout.pad, layout.y + layout.pad, &title, TITLE, layout.scale);
        let panel = self.panel();
        for (k, row) in panel.iter().enumerate() {
            let y = layout.row_y(k);
            let color = if row.field.is_some() { TEXT } else { DIM };
            canvas.text(layout.x + layout.pad, y, &row.label, color, layout.scale);
            let vx = layout.x + layout.width - layout.pad - Canvas::text_width(&row.value, layout.scale);
            canvas.text(vx, y, &row.value, color, layout.scale);
        }
        // The key hints at the bottom, below the rows (a line apart): those that fit.
        let hints = self.hints();
        let line = Canvas::line_height(layout.scale);
        let bottom = layout.y + layout.height - layout.pad;
        let mut y = (bottom.saturating_sub(hints.len() * line)).max(layout.row_y(panel.len()) + line);
        for hint in &hints {
            if y + line > bottom {
                break;
            }
            canvas.text(layout.x + layout.pad, y, hint, DIM, layout.scale);
            y += line;
        }
        if let Some((message, at)) = &self.status
            && at.elapsed() < STATUS_TIME
        {
            let y = canvas.height - line - layout.pad;
            let w = Canvas::text_width(message, layout.scale) + 2 * layout.pad;
            canvas.shade(0, y - layout.pad / 2, w, line + layout.pad, 90);
            canvas.text(layout.pad, y, message, TEXT, layout.scale);
        }
    }

    /// The key hints at the bottom of the panel.
    fn hints(&self) -> [String; 10] {
        if let Some(model) = &self.model {
            return model.hints();
        }
        [
            format!("Grid {} m (G)", moose_assets::number(self.step())),
            "F6: 3D/top/front/side. F5: wire".to_string(),
            "Click: select. V: pick vertices".to_string(),
            "Hold right: look (or pan in 2D)".to_string(),
            "Scroll/click a row (Alt: fine; numbers: type)".to_string(),
            "Arrows, PgUp/Dn, [ ]: move, turn".to_string(),
            "Ctrl Z/Y: undo/redo. Ctrl S: save".to_string(),
            "Ctrl D: duplicate. Ctrl C/V: copy, paste".to_string(),
            "Del: delete. Ctrl 1-4, 1-4: bookmarks".to_string(),
            "Tab: play".to_string(),
        ]
    }

    /// The panel's place on screen (on the right): as wide as its widest line.
    pub fn layout(&self, width: usize, height: usize) -> Layout {
        let scale = if height >= 600 { 2 } else { 1 };
        let pad = 6 * scale;
        let gap = Canvas::text_width("  ", scale);
        let panel = self.panel();
        let rows = panel
            .iter()
            .map(|r| Canvas::text_width(&r.label, scale) + gap + Canvas::text_width(&r.value, scale));
        let hints = self.hints().map(|h| Canvas::text_width(&h, scale));
        let panel_w = rows.chain(hints).max().unwrap_or(0) + 2 * pad;
        Layout {
            x: width.saturating_sub(panel_w),
            y: 0,
            width: panel_w,
            height,
            pad,
            scale,
        }
    }

    /// The panel row under framebuffer point `(x, y)`, if any.
    pub fn row_at(&self, x: f32, y: f32, width: usize, height: usize) -> Option<usize> {
        let layout = self.layout(width, height);
        if (x as usize) < layout.x {
            return None;
        }
        let line = Canvas::line_height(layout.scale);
        let k = ((y as usize).checked_sub(layout.row_y(0))?) / line;
        (k < self.panel().len()).then_some(k)
    }

    /// Whether framebuffer point `(x, _)` is over the panel.
    pub fn over_panel(&self, x: f32, width: usize, height: usize) -> bool {
        x as usize >= self.layout(width, height).x
    }
}

/// Where the panel is.
pub struct Layout {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
    pub pad: usize,
    pub scale: usize,
}

impl Layout {
    /// The top of panel row `k`.
    pub fn row_y(&self, k: usize) -> usize {
        self.y + self.pad + Canvas::line_height(self.scale) * (2 + k)
    }
}

/// How far `p` is from segment `a`-`b`.
fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// The sector containing `p`.
fn sector_of(world: &World, p: Vec3) -> Result<usize, String> {
    world
        .find_sector(p)
        .map(|s| s as usize)
        .ok_or_else(|| "that's outside the level".to_string())
}

/// A new level named `title`: one room, `LEVEL_ROOM` (centered on the origin, its floor
/// at 0), every surface drawn with the default material and textured flat (`planar_uv`), a
/// spawn point near one wall looking across it, a light under the ceiling and some ambient
/// light.
pub fn new_level_doc(title: &str) -> Result<LevelDoc, String> {
    let mut doc = LevelDoc {
        name: title.to_string(),
        name_options: Vec::new(),
        vertices: Vec::new(),
        attributes: vec![moose_assets::AttributeDoc {
            name: "uv".into(),
            format: "f32".into(),
            count: 2,
            values: vec![vec!["0".into(), "0".into()]],
        }],
        sectors: Vec::new(),
        surfaces: Vec::new(),
        adjoins: Vec::new(),
        templates: Vec::new(),
        entities: Vec::new(),
        ambient: Some(Vec3::splat(0.1)),
        lights: Vec::new(),
        directional: Vec::new(),
    };
    let min = Vec3::new(-LEVEL_ROOM.x / 2.0, 0.0, -LEVEL_ROOM.z / 2.0);
    let sector = doc.add_box(min, min + LEVEL_ROOM, "room")?;
    for i in doc.sector_surfaces(sector) {
        let normal = doc.surface_normal(i);
        doc.set_corner_values(i, "uv", |_, p, _| planar_uv(p, normal).to_array().to_vec());
        doc.surfaces[i].options = vec![format!("material={DEFAULT_MATERIAL}")];
    }
    // The first values row, which only `add_box` used, goes: rows after it move down one.
    doc.attributes[0].values.remove(0);
    for s in &mut doc.surfaces {
        for (_, rows) in &mut s.corners {
            rows[0] -= 1;
        }
    }
    doc.entities.push(EntityDoc {
        kind: EntityKind::Spawn,
        sector,
        model: None,
        position: Vec3::new(0.0, 0.0, LEVEL_ROOM.z / 2.0 - 1.5),
        rotation: Quat::IDENTITY,
        scale: 1.0,
        name: "player_start".into(),
        options: Vec::new(),
    });
    doc.lights.push(LightDoc {
        sector,
        position: Vec3::new(0.0, LEVEL_ROOM.y - 0.5, 0.0),
        color: Vec3::splat(1.0),
        range: 12.0,
        spot: None,
        options: vec!["radius=0.05".into()],
    });
    Ok(doc)
}

/// A surface's texture mapping as numbers: its coordinates are `offset + R(turn) [[1 /
/// size.x, shear], [0, 1 / size.y]] q`, for `q` the point's flat coordinates in meters (see
/// [`flat_coords`]). Mapped flat, `offset` is 0, `size` (meters a repeat) 2 each way and
/// `turn` (degrees) 0; a negative size is a flipped one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextureMapping {
    pub offset: Vec2,
    pub size: Vec2,
    pub turn: f32,
    pub shear: f32,
}

impl TextureMapping {
    /// The texture coordinates at flat coordinates `q`.
    pub fn at(&self, q: Vec2) -> Vec2 {
        let (sin, cos) = self.turn.to_radians().sin_cos();
        let (u, v) = (Vec2::new(1.0 / self.size.x, 0.0), Vec2::new(self.shear, 1.0 / self.size.y));
        let rotate = |c: Vec2| Vec2::new(cos * c.x - sin * c.y, sin * c.x + cos * c.y);
        self.offset + rotate(u) * q.x + rotate(v) * q.y
    }
}

/// Point `p`'s flat coordinates in meters on a surface facing `normal`: [`planar_uv`]'s axes,
/// unscaled.
fn flat_coords(p: Vec3, normal: Vec3) -> Vec2 {
    planar_uv(p, normal) * 2.0
}

/// Texture coordinates for point `p` on a surface facing `normal`, as the test levels are
/// made (tools/gen_test_assets.py): along the normal's strongest axis, a tile every 2 m, v
/// growing down on walls.
fn planar_uv(p: Vec3, normal: Vec3) -> Vec2 {
    let a = normal.abs();
    let (u, v) = if a.y >= a.x && a.y >= a.z {
        (p.x, p.z)
    } else if a.x >= a.z {
        (p.z, -p.y)
    } else {
        (p.x, -p.y)
    };
    Vec2::new(u, v) / 2.0
}

/// A light's option `key` (`key=value`), if it has one.
fn light_option<'a>(l: &'a LightDoc, key: &str) -> Option<&'a str> {
    l.options.iter().find_map(|o| o.strip_prefix(key)?.strip_prefix('='))
}

/// Sets a light's option `key` to `value`, or removes it.
fn set_light_option(l: &mut LightDoc, key: &str, value: Option<String>) {
    l.options.retain(|o| !o.starts_with(&format!("{key}=")));
    if let Some(value) = value {
        l.options.push(format!("{key}={value}"));
    }
}

/// A direction's yaw (degrees, turning left from -Z) and pitch (up from level).
fn aim_of(d: Vec3) -> (f32, f32) {
    let d = d.normalize_or_zero();
    let yaw = (-d.x).atan2(-d.z).to_degrees();
    let pitch = d.y.clamp(-1.0, 1.0).asin().to_degrees();
    (yaw, pitch)
}

/// The direction of yaw and pitch (degrees; see [`aim_of`]).
fn aim(yaw: f32, pitch: f32) -> Vec3 {
    let (yaw, pitch) = (yaw.to_radians(), pitch.to_radians());
    Vec3::new(-yaw.sin() * pitch.cos(), pitch.sin(), -yaw.cos() * pitch.cos())
}

/// `v` on the grid of `step`.
fn snap(v: f32, step: f32) -> f32 {
    (v / step).round() * step
}

const TITLE: u32 = 0xFF_D4_7A;
const TEXT: u32 = 0xD8_D8_D8;
const DIM: u32 = 0x8C_8C_8C;
/// Outline colors: the selection, and what the pointer is over.
pub(crate) const SELECTED: u32 = 0xFF_E0_40;
pub(crate) const HOVER: u32 = 0x60_A0_FF;
const VERTEX: u32 = 0xB0_B0_C0;
const LIGHT: u32 = 0xFF_B0_40;
const SPAWN: u32 = 0x40_E0_E0;
const RANGE: u32 = 0x80_60_20;
const CUT: u32 = 0xFF_50_50;
const PROBLEM: u32 = 0xFF_20_20;

/// Whether options in effect make a prop static (`static`, or `static=on`).
fn is_static(options: &[String]) -> bool {
    options.iter().rev().find(|o| moose_assets::option_key(o) == "static").is_some_and(|o| o != "static=off")
}

/// Makes an entity not static, over its template's options (`template`).
fn not_static(e: &mut moose_assets::EntityDoc, template: &[String]) {
    e.options.retain(|o| moose_assets::option_key(o) != "static");
    if is_static(template) {
        e.options.push("static=off".into());
    }
}

/// Steps the `material=` among `options` through `materials` (and none, first), `dir`'s way,
/// and returns the one now named.
fn cycle_material(options: &mut Vec<String>, materials: &[String], dir: f32) -> String {
    let now = options.iter().find_map(|o| o.strip_prefix("material=")).map(String::from);
    let names: Vec<Option<&String>> = std::iter::once(None).chain(materials.iter().map(Some)).collect();
    let k = names.iter().position(|n| n.map(String::as_str) == now.as_deref()).unwrap_or(0);
    let next = names[(k as isize + dir.signum() as isize).rem_euclid(names.len() as isize) as usize];
    options.retain(|o| !o.starts_with("material="));
    match next {
        Some(name) => {
            options.push(format!("material={name}"));
            name.clone()
        }
        None => "none".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::cycle_material;

    #[test]
    fn material_cycles_through_none_and_the_materials() {
        let materials = ["brick".to_string(), "stone".to_string()];
        let mut options = vec!["filter0=nearest".to_string()];
        assert_eq!(cycle_material(&mut options, &materials, 1.0), "brick");
        assert_eq!(cycle_material(&mut options, &materials, 1.0), "stone");
        assert_eq!(options, ["filter0=nearest", "material=stone"]);
        assert_eq!(cycle_material(&mut options, &materials, 1.0), "none");
        assert_eq!(options, ["filter0=nearest"]);
        assert_eq!(cycle_material(&mut options, &materials, -1.0), "stone");
    }
}
