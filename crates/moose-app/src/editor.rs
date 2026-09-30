//! The level editor: a mode of the app (Tab) for picking what's in the level with the
//! mouse and changing it.
//!
//! The editor works on the level's own tables ([`LevelDoc`]): every edit changes them,
//! and the app then rebuilds the level from them, through the level loader, which checks
//! the edit (an edit it rejects is undone, with the loader's reason). Undo and redo are
//! snapshots of the tables; saving writes them to the level's file.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use glam::{EulerRot, Quat, Vec2, Vec3};
use moose_assets::{Assets, EntityKind, LevelDoc};
use moose_scene::World;
use moose_view::{PolygonSource, ViewGeometry};

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

/// What is selected: a surface or an entity, by its row in the level's tables (so it
/// survives rebuilding the level).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    Surface(usize),
    Entity(usize),
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
    Static,
    Occluder,
    Reflective,
    Sky,
    Duplicate,
    Delete,
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

pub struct Editor {
    /// Editing (Tab), as opposed to playing.
    pub on: bool,
    pub doc: LevelDoc,
    /// Where the level is saved.
    pub path: PathBuf,
    pub selection: Option<Selection>,
    /// What the pointer is over.
    pub hover: Option<Selection>,
    undo: Vec<(LevelDoc, Option<Selection>)>,
    redo: Vec<(LevelDoc, Option<Selection>)>,
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
            undo: Vec::new(),
            redo: Vec::new(),
            status: None,
            step: 2,
            view: ViewMode::Perspective,
            wire: false,
            ortho_center: Vec3::ZERO,
            ortho_scale: 40.0,
        }
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

    /// The level polygon (in the level mesh) drawing surface `surface`, if it is solid.
    fn mesh_polygon(&self, surface: usize) -> Option<usize> {
        self.doc.surfaces[surface].adjoin.is_none().then(|| {
            self.doc.surfaces[..surface].iter().filter(|s| s.adjoin.is_none()).count()
        })
    }

    /// The world entity row `row` is, if it isn't a spawn point.
    fn world_entity(&self, row: usize) -> Option<usize> {
        (self.doc.entities[row].kind != EntityKind::Spawn).then(|| {
            self.doc.entities[..row].iter().filter(|e| e.kind != EntityKind::Spawn).count()
        })
    }

    /// What is under screen point `(x, y)` in a 2D view: an entity whose outline (its box
    /// on screen) holds it, the smallest; otherwise the surface with an edge nearest it,
    /// within a few pixels.
    pub fn pick_2d(&self, ortho: &Ortho, world: &World, assets: &Assets, x: f32, y: f32) -> Option<Selection> {
        let at = Vec2::new(x, y);
        let mut best: Option<(f32, Selection)> = None;
        for (i, e) in world.entities.iter().enumerate() {
            let corners = (0..8).map(|k| {
                let pick = |bit: usize, lo: f32, hi: f32| if k & bit == 0 { lo } else { hi };
                ortho.to_screen(Vec3::new(
                    pick(1, e.bounds.min.x, e.bounds.max.x),
                    pick(2, e.bounds.min.y, e.bounds.max.y),
                    pick(4, e.bounds.min.z, e.bounds.max.z),
                ))
            });
            let (lo, hi) = corners.fold((Vec2::INFINITY, Vec2::NEG_INFINITY), |(lo, hi), c| (lo.min(c), hi.max(c)));
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
        let geometry = assets.mesh(world.geometry);
        let mut nearest: Option<(f32, usize)> = None;
        for (i, polygon) in geometry.polygons.iter().enumerate() {
            let points: Vec<Vec2> = geometry.polygon_points(polygon).map(|p| ortho.to_screen(p)).collect();
            for k in 0..points.len() {
                let d = distance_to_segment(at, points[k], points[(k + 1) % points.len()]);
                if d < 6.0 && nearest.is_none_or(|(n, _)| d < n) {
                    nearest = Some((d, i));
                }
            }
        }
        let (_, polygon) = nearest?;
        self.selection_of(PolygonSource::World { sector: 0, polygon: polygon as u32 })
    }

    /// Draws a 2D view: the grid, the level's wireframe, and the selection and what the
    /// pointer is over, outlined.
    pub fn draw_2d(&self, canvas: &mut Canvas, ortho: &Ortho, world: &World, assets: &Assets) {
        wire::draw_grid(canvas, ortho, self.step());
        let projection = Projection::Ortho(ortho);
        wire::draw_level(canvas, &projection, world, assets);
        self.draw_selected(canvas, &projection, world, assets);
    }

    /// Outlines the selection and what the pointer is over, through `projection`.
    pub fn draw_selected(&self, canvas: &mut Canvas, projection: &Projection, world: &World, assets: &Assets) {
        for (selection, color) in [(self.hover, HOVER), (self.selection, SELECTED)] {
            match selection {
                Some(Selection::Surface(i)) => {
                    let Some(polygon) = self.mesh_polygon(i) else { continue };
                    let geometry = assets.mesh(world.geometry);
                    let points: Vec<Vec3> = geometry.polygon_points(&geometry.polygons[polygon]).collect();
                    wire::outline(canvas, projection, &points, color);
                }
                Some(Selection::Entity(row)) => {
                    let Some(e) = self.world_entity(row).and_then(|k| world.entities.get(k)) else { continue };
                    let mesh = assets.mesh(e.mesh);
                    let transform = e.transform();
                    for polygon in &mesh.polygons {
                        let points: Vec<Vec3> = mesh.polygon_points(polygon).map(|p| transform.transform_point3(p)).collect();
                        wire::outline(canvas, projection, &points, color);
                    }
                }
                None => {}
            }
        }
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

    /// Before an edit: remembers the tables for undo. Returns them, to put back if the
    /// edit fails.
    pub fn begin(&mut self) -> (LevelDoc, Option<Selection>) {
        (self.doc.clone(), self.selection)
    }

    /// After an edit the level accepted.
    pub fn commit(&mut self, before: (LevelDoc, Option<Selection>), what: &str) {
        self.undo.push(before);
        self.redo.clear();
        self.say(what);
    }

    /// After an edit that failed: puts the tables back.
    pub fn revert(&mut self, before: (LevelDoc, Option<Selection>), why: &str) {
        (self.doc, self.selection) = before;
        self.say(why);
    }

    /// Steps back (or forward, `redo`) through the history: the tables to rebuild from, if
    /// there was a step.
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
            PolygonSource::Entity { entity, .. } => {
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

    /// Whether a drawn polygon is part of `selection`.
    pub fn shows(&self, selection: Selection, source: PolygonSource) -> bool {
        self.selection_of(source) == Some(selection)
    }

    /// The properties panel's rows for the selection.
    pub fn panel(&self) -> Vec<PanelRow> {
        let row = |label: &str, value: String, field: Option<Field>| PanelRow {
            label: label.to_string(),
            value,
            field,
        };
        let n = |v: f32| moose_assets::number(v);
        match self.selection {
            None => vec![row("Nothing selected", String::new(), None)],
            Some(Selection::Surface(i)) => {
                let s = &self.doc.surfaces[i];
                let sector = &self.doc.sectors[s.sector].name;
                let mut rows = vec![
                    row("Surface", i.to_string(), None),
                    row("Sector", sector.clone(), None),
                    row("Corners", s.corners.len().to_string(), None),
                ];
                if s.adjoin.is_some() {
                    rows.push(row("Portal", String::new(), None));
                } else {
                    let on = |bit: u32| if s.flags & bit != 0 { "on" } else { "off" }.to_string();
                    rows.push(row("Reflective", on(0x1), Some(Field::Reflective)));
                    rows.push(row("Sky", on(0x2), Some(Field::Sky)));
                }
                rows
            }
            Some(Selection::Entity(i)) => {
                let e = &self.doc.entities[i];
                let (yaw, pitch, roll) = e.rotation.to_euler(EulerRot::YXZ);
                let kind = match e.kind {
                    EntityKind::Spawn => "spawn",
                    EntityKind::Prop => "prop",
                    EntityKind::Actor => "actor",
                };
                let mut rows = vec![
                    row("Name", e.name.clone(), None),
                    row("Kind", kind.into(), (e.kind != EntityKind::Spawn).then_some(Field::Kind)),
                ];
                if let Some(model) = &e.model {
                    rows.push(row("Model", model.clone(), Some(Field::Model)));
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
                    let on = if e.has_option("static") { "on" } else { "off" };
                    rows.push(row("Static", on.into(), Some(Field::Static)));
                }
                if e.kind != EntityKind::Spawn {
                    let occluder = e
                        .options
                        .iter()
                        .find_map(|o| o.strip_prefix("occluder="))
                        .unwrap_or("mesh");
                    rows.push(row("Occluder", occluder.into(), Some(Field::Occluder)));
                }
                rows.push(row("Sector", self.doc.sectors[e.sector].name.clone(), None));
                rows.push(row("Duplicate", String::new(), Some(Field::Duplicate)));
                rows.push(row("Delete", String::new(), Some(Field::Delete)));
                rows
            }
        }
    }

    /// Changes the selection's `field` (`dir` is the way: +1 or -1, or scroll lines), in
    /// the tables. `world` finds the sector a moved entity is in; `models` are the model
    /// files to cycle through. Returns what was done, or why it can't be.
    pub fn change(&mut self, field: Field, dir: f32, world: &World, models: &[String]) -> Result<String, String> {
        let step = self.step();
        match (self.selection, field) {
            (Some(Selection::Surface(i)), Field::Reflective | Field::Sky) => {
                let bit = if field == Field::Reflective { 0x1 } else { 0x2 };
                let s = &mut self.doc.surfaces[i];
                s.flags ^= bit;
                // A surface can't be both.
                s.flags &= !(0x3 & !bit);
                Ok(format!("surface {i}: flags {:#x}", s.flags))
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
                let e = &mut self.doc.entities[i];
                let turn = |q: Quat, axis: EulerRot| {
                    let (yaw, pitch, roll) = q.to_euler(EulerRot::YXZ);
                    let t = (TURN * dir).to_radians();
                    let (yaw, pitch, roll) = match axis {
                        EulerRot::YXZ => (yaw + t, pitch, roll),
                        EulerRot::XYZ => (yaw, pitch + t, roll),
                        _ => (yaw, pitch, roll + t),
                    };
                    Quat::from_euler(EulerRot::YXZ, yaw, pitch, roll)
                };
                match field {
                    Field::X => e.position.x = snap(e.position.x + step * dir, step),
                    Field::Y => e.position.y = snap(e.position.y + step * dir, step),
                    Field::Z => e.position.z = snap(e.position.z + step * dir, step),
                    Field::Yaw => e.rotation = turn(e.rotation, EulerRot::YXZ),
                    Field::Pitch => e.rotation = turn(e.rotation, EulerRot::XYZ),
                    Field::Roll => e.rotation = turn(e.rotation, EulerRot::ZYX),
                    Field::Scale => e.scale = (e.scale * 1.1f32.powf(dir)).clamp(0.01, 100.0),
                    Field::Kind => {
                        e.kind = match e.kind {
                            EntityKind::Prop => EntityKind::Actor,
                            _ => EntityKind::Prop,
                        };
                        if e.kind == EntityKind::Actor {
                            e.set_option("static", false);
                        }
                    }
                    Field::Model => {
                        let now = models.iter().position(|m| Some(m) == e.model.as_ref());
                        let next = match now {
                            Some(k) => (k as isize + dir.signum() as isize).rem_euclid(models.len() as isize),
                            None => 0,
                        };
                        e.model = models.get(next as usize).cloned().or(e.model.take());
                    }
                    Field::Static => {
                        let on = !e.has_option("static");
                        e.set_option("static", on);
                    }
                    Field::Occluder => {
                        let now = e.options.iter().position(|o| o.starts_with("occluder="));
                        let value = now.map(|k| e.options[k].clone());
                        if let Some(k) = now {
                            e.options.remove(k);
                        }
                        // mesh (the default) → none → back.
                        if value.is_none() {
                            e.options.push("occluder=none".into());
                        }
                    }
                    _ => {}
                }
                if matches!(field, Field::X | Field::Y | Field::Z) {
                    e.sector = sector_of(world, e.position)?;
                }
                let (name, p) = (e.name.clone(), e.position);
                let n = moose_assets::number;
                Ok(format!("{name} at {}, {}, {}", n(p.x), n(p.y), n(p.z)))
            }
            _ => Err("select something first".into()),
        }
    }

    /// `name` with a number after it that no entity has.
    fn unique_name(&self, name: &str) -> String {
        let base = name.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end_matches('_');
        (2..)
            .map(|k| format!("{base}_{k}"))
            .find(|n| self.doc.entities.iter().all(|e| &e.name != n))
            .unwrap()
    }

    /// Draws the editor over the frame: outlines of the selection and what's under the
    /// pointer, the properties panel, and the status line.
    pub fn draw(&self, canvas: &mut Canvas, view: &ViewGeometry, camera: &moose_scene::View, world: &World, assets: &Assets) {
        if self.view == ViewMode::Perspective {
            if self.wire {
                wire::draw_level(canvas, &Projection::Perspective(camera), world, assets);
            }
            for (selection, color) in [(self.hover, HOVER), (self.selection, SELECTED)] {
                let Some(selection) = selection else { continue };
                for p in &view.polygons {
                    if p.mirror.is_some() || !self.shows(selection, p.source) {
                        continue;
                    }
                    let v = &view.vertices[p.vertices()];
                    for i in 0..v.len() {
                        let (a, b) = (v[i], v[(i + 1) % v.len()]);
                        canvas.line(a.x, a.y, b.x, b.y, color);
                    }
                }
            }
        }
        let layout = self.layout(canvas.width, canvas.height);
        canvas.shade(layout.x, layout.y, layout.width, layout.height, 70);
        let view = match self.view {
            ViewMode::Perspective => "3D",
            ViewMode::Ortho(axis) => axis.name(),
        };
        let title = format!("EDITOR  {view}{}", if self.dirty() { "  (unsaved)" } else { "" });
        let title = title.as_str();
        canvas.text(layout.x + layout.pad, layout.y + layout.pad, title, TITLE, layout.scale);
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
            canvas.shade(0, y - layout.pad / 2, Canvas::text_width(message, layout.scale) + 2 * layout.pad, line + layout.pad, 90);
            canvas.text(layout.pad, y, message, TEXT, layout.scale);
        }
    }

    /// The key hints at the bottom of the panel.
    fn hints(&self) -> [String; 7] {
        [
            format!("Grid {} m (G)", moose_assets::number(self.step())),
            "F6: 3D/top/front/side. F5: wire".to_string(),
            "Click: select. Hold right: look".to_string(),
            "Scroll or click a row to change it".to_string(),
            "Arrows, PgUp/Dn, [ ]: move, turn".to_string(),
            "Ctrl Z/Y: undo/redo. Ctrl S: save".to_string(),
            "Ctrl D: copy. Del: delete. Tab: play".to_string(),
        ]
    }

    /// The panel's place on screen (on the right): as wide as its widest line.
    pub fn layout(&self, width: usize, height: usize) -> Layout {
        let scale = if height >= 600 { 2 } else { 1 };
        let pad = 6 * scale;
        let gap = Canvas::text_width("  ", scale);
        let panel = self.panel();
        let rows = panel.iter().map(|r| {
            Canvas::text_width(&r.label, scale) + gap + Canvas::text_width(&r.value, scale)
        });
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
        let top = layout.row_y(0);
        let k = ((y as usize).checked_sub(top)?) / line;
        (k < self.panel().len()).then_some(k)
    }

    /// Whether framebuffer point `(x, y)` is over the panel.
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
        let line = Canvas::line_height(self.scale);
        self.y + self.pad + line * (2 + k)
    }
}

/// How far `p` is from segment `a`-`b`.
fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    p.distance(a + ab * t)
}

/// The sector containing `p`.
fn sector_of(world: &World, p: Vec3) -> Result<usize, String> {
    world
        .find_sector(p)
        .map(|s| s as usize)
        .ok_or_else(|| "that's outside the level".to_string())
}

/// `v` on the grid of `step`.
fn snap(v: f32, step: f32) -> f32 {
    (v / step).round() * step
}

const TITLE: u32 = 0xFF_D4_7A;
const TEXT: u32 = 0xD8_D8_D8;
const DIM: u32 = 0x8C_8C_8C;
/// Outline colors: the selection, and what the pointer is over.
const SELECTED: u32 = 0xFF_E0_40;
const HOVER: u32 = 0x60_A0_FF;
