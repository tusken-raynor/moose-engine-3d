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
//! or part sectors with openings, cleave a sector along a line drawn in a 2D view, add a
//! room, delete a sector; vertices move on the grid.

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
    /// A surface's texture: shifted across, shifted down, scaled, turned a quarter, or
    /// mapped flat from its corners' positions.
    TextureU,
    TextureV,
    TextureScale,
    TextureTurn,
    TextureProject,
    /// Puts what was copied (Ctrl+C) where the view is.
    Paste,
    /// Opens (or leaves) the mesh editor on an entity's model (M).
    EditModel,
    /// Mesh editor: steps through the model's polygons.
    PolygonPick,
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
    /// Whether it changes the level (rather than what the editor is doing).
    pub fn is_edit(self) -> bool {
        !matches!(
            self,
            Field::SelectSector | Field::ExtrudeBy | Field::Cleave | Field::EditModel | Field::PolygonPick | Field::PickProxies
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

type Snapshot = (LevelDoc, Option<Selection>);

pub struct Editor {
    /// Editing (Tab), as opposed to playing.
    pub on: bool,
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
    /// Where new things go: the middle of the 2D view, or in front of the camera. Set by
    /// the app each frame.
    pub anchor: Vec3,
    /// The world point under the pointer in a 2D view (for the cut's preview).
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

    pub fn dirty(&self) -> bool {
        self.doc != self.saved
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
        self.say(what);
    }

    /// After an edit that failed: puts the tables back.
    pub fn revert(&mut self, before: Snapshot, why: &str) {
        (self.doc, self.selection) = before;
        self.say(why);
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
        if let Some(model) = &self.model {
            return model.panel();
        }
        let row = |label: &str, value: String, field: Option<Field>| PanelRow {
            label: label.to_string(),
            value,
            field,
        };
        let n = moose_assets::number;
        match self.selection {
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
                        rows.push(row("Reflective", on(0x1), Some(Field::Reflective)));
                        rows.push(row("Sky", on(0x2), Some(Field::Sky)));
                        rows.push(row("Extrude by", format!("{} m", n(self.extrude_by)), Some(Field::ExtrudeBy)));
                        rows.push(row("Extrude", String::new(), Some(Field::Extrude)));
                        rows.push(row("Push/pull", "scroll".into(), Some(Field::Push)));
                        rows.push(row("Open to a match", String::new(), Some(Field::Adjoin)));
                        if self.doc.attributes.iter().any(|a| a.name == "uv") {
                            rows.push(row("Texture across", "scroll".into(), Some(Field::TextureU)));
                            rows.push(row("Texture down", "scroll".into(), Some(Field::TextureV)));
                            rows.push(row("Texture scale", "scroll".into(), Some(Field::TextureScale)));
                            rows.push(row("Texture turn", String::new(), Some(Field::TextureTurn)));
                            rows.push(row("Texture flat", String::new(), Some(Field::TextureProject)));
                        }
                    }
                }
                rows
            }
            Some(Selection::Sector(s)) => {
                let range = self.doc.sector_surfaces(s);
                let openings = self.doc.surfaces[range.clone()].iter().filter(|x| x.adjoin.is_some()).count();
                vec![
                    row("Sector", self.doc.sectors[s].name.clone(), None),
                    row("Surfaces", range.len().to_string(), None),
                    row("Openings", openings.to_string(), None),
                    row("Cleave", "in 2D".into(), Some(Field::Cleave)),
                    row("Delete", String::new(), Some(Field::Delete)),
                ]
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
                rows.push(row("Sector", self.doc.sectors[e.sector].name.clone(), None));
                rows.push(row("Duplicate", String::new(), Some(Field::Duplicate)));
                rows.push(row("Delete", String::new(), Some(Field::Delete)));
                rows
            }
        }
    }

    // ---- Changes

    /// Carries out `field` when it isn't an edit (see [`Field::is_edit`]).
    pub fn command(&mut self, field: Field, dir: f32) {
        match (self.selection, field) {
            (Some(Selection::Surface(i)), Field::SelectSector) => {
                self.selection = Some(Selection::Sector(self.doc.surfaces[i].sector));
            }
            (_, Field::ExtrudeBy) => {
                let step = self.step();
                self.extrude_by = snap((self.extrude_by + step * dir.signum()).max(step), step);
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
            (
                Some(Selection::Surface(i)),
                Field::TextureU | Field::TextureV | Field::TextureScale | Field::TextureTurn | Field::TextureProject,
            ) => {
                let normal = self.doc.surface_normal(i);
                let uvs = self.corner_values(i, "uv");
                if uvs.is_empty() {
                    return Err("this level has no texture coordinates".into());
                }
                let center = uvs.iter().fold(Vec2::ZERO, |c, uv| c + Vec2::new(uv[0], uv[1])) / uvs.len() as f32;
                self.doc.set_corner_values(i, "uv", |_, p, uv| {
                    let t = Vec2::new(uv[0], uv[1]);
                    let t = match field {
                        Field::TextureU => t + Vec2::X * 0.125 * dir,
                        Field::TextureV => t + Vec2::Y * 0.125 * dir,
                        Field::TextureScale => center + (t - center) * 1.25f32.powf(-dir),
                        Field::TextureTurn => center + (t - center).perp(),
                        _ => planar_uv(p, normal),
                    };
                    vec![t.x, t.y]
                });
                Ok(format!("surface {i}'s texture moved"))
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
            (Some(Selection::Light(i)), Field::Duplicate) => {
                let mut copy = self.doc.lights[i].clone();
                copy.position.x += step;
                copy.sector = sector_of(world, copy.position)?;
                self.doc.lights.push(copy);
                self.selection = Some(Selection::Light(self.doc.lights.len() - 1));
                Ok("duplicated the light".into())
            }
            (Some(Selection::Light(i)), Field::Delete) => {
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
    pub fn cut_point(&mut self, p: Vec3) -> bool {
        let step = self.step();
        let Some(points) = &mut self.cutting else {
            return false;
        };
        points.push(p.map(|v| snap(v, step)));
        points.len() == 2
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
            match selection {
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
        for (k, row) in self.panel().iter().enumerate() {
            let y = layout.row_y(k);
            let color = if row.field.is_some() { TEXT } else { DIM };
            canvas.text(layout.x + layout.pad, y, &row.label, color, layout.scale);
            let vx = layout.x + layout.width - layout.pad - Canvas::text_width(&row.value, layout.scale);
            canvas.text(vx, y, &row.value, color, layout.scale);
        }
        let hints = self.hints();
        let line = Canvas::line_height(layout.scale);
        let mut y = layout.y + layout.height - layout.pad - hints.len() * line;
        for hint in &hints {
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
            "Scroll or click a row to change it".to_string(),
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
