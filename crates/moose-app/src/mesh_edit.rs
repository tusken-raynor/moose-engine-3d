//! The mesh editor (M on an entity with a `.mmdl` model): sets what Blender doesn't know,
//! as the Model Format Spec says: which polygons are shadow proxies, proxy boxes per bone,
//! polygon colors. It edits the model's tables (`ModelDoc`), shown on the entity it was
//! opened on, with its own undo and saving, apart from the level's.

use std::path::PathBuf;

use glam::{Vec2, Vec3};
use moose_assets::{Assets, ModelDoc};
use moose_scene::World;

use crate::editor::{Field, PanelRow};
use crate::ui::Canvas;
use crate::wire::{self, Projection};

/// Proxy outlines.
const PROXY: u32 = 0xE0_60_E0;
/// How far a color row steps, of 1.
const COLOR_STEP: f32 = 1.0 / 16.0;

type Snapshot = (ModelDoc, Option<usize>);

pub struct ModelEdit {
    /// The model's file name in assets/models, and its path.
    pub file: String,
    pub path: PathBuf,
    pub doc: ModelDoc,
    saved: ModelDoc,
    /// The world entity (its index in `World::entities`) it is shown on.
    pub entity: usize,
    pub polygon: Option<usize>,
    pub hover: Option<usize>,
    /// Clicks pick shadow proxies (P), not drawn polygons.
    pub proxies: bool,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Leaving with unsaved changes was refused once (leaving again drops them).
    pub warned: bool,
}

impl ModelEdit {
    pub fn new(file: String, path: PathBuf, doc: ModelDoc, entity: usize) -> ModelEdit {
        ModelEdit {
            file,
            path,
            saved: doc.clone(),
            doc,
            entity,
            polygon: None,
            hover: None,
            proxies: false,
            undo: Vec::new(),
            redo: Vec::new(),
            warned: false,
        }
    }

    pub fn dirty(&self) -> bool {
        self.doc != self.saved
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.doc.clone();
        self.warned = false;
    }

    // ---- History (as the level's: snapshots of the tables)

    pub fn begin(&self) -> Snapshot {
        (self.doc.clone(), self.polygon)
    }

    pub fn commit(&mut self, before: Snapshot) {
        self.undo.push(before);
        self.redo.clear();
    }

    pub fn revert(&mut self, before: Snapshot) {
        (self.doc, self.polygon) = before;
    }

    /// Steps back (or forward, `redo`); returns whether there was a step.
    pub fn travel(&mut self, redo: bool) -> bool {
        let (from, to) = if redo { (&mut self.redo, &mut self.undo) } else { (&mut self.undo, &mut self.redo) };
        let Some(state) = from.pop() else {
            return false;
        };
        to.push((self.doc.clone(), self.polygon));
        (self.doc, self.polygon) = state;
        true
    }

    /// The bone the tools work on: the selected polygon's (none: the whole model).
    fn bone(&self) -> Option<usize> {
        self.polygon.and_then(|p| self.doc.polygon_bone(p))
    }

    fn bone_name(&self, bone: Option<usize>) -> String {
        match bone.and_then(|b| self.doc.bones.get(b)) {
            Some(b) => b.name.clone(),
            None if self.doc.bones.is_empty() => "model".into(),
            None => "all".into(),
        }
    }

    // ---- Panel

    pub fn panel(&self) -> Vec<PanelRow> {
        let row = |label: &str, value: String, field: Option<Field>| PanelRow {
            label: label.into(),
            value,
            field,
        };
        let proxies = self.doc.polygons.iter().filter(|p| p.proxy()).count();
        let mut rows = vec![
            row("Model", self.file.clone(), None),
            row("Polygons", format!("{} + {proxies} proxies", self.doc.polygons.len() - proxies), None),
            row("Bones", self.doc.bones.len().to_string(), None),
            row("Click picks", if self.proxies { "proxies" } else { "drawn" }.into(), Some(Field::PickProxies)),
            row("Polygon", self.polygon.map_or("none".into(), |p| p.to_string()), Some(Field::PolygonPick)),
        ];
        if let Some(p) = self.polygon {
            let polygon = &self.doc.polygons[p];
            rows.push(row("Kind", if polygon.proxy() { "proxy" } else { "drawn" }.into(), Some(Field::Proxy)));
            rows.push(row("Corners", polygon.corners.len().to_string(), None));
            if !self.doc.bones.is_empty() {
                rows.push(row("Bone", self.bone_name(self.doc.polygon_bone(p)), None));
            }
            // (A proxy isn't drawn: its color doesn't show.)
            if let Some(c) = self.doc.polygon_color(p).filter(|_| !polygon.proxy()) {
                let n = |v: f32| moose_assets::number(v);
                rows.push(row("Red", n(c.x), Some(Field::Red)));
                rows.push(row("Green", n(c.y), Some(Field::Green)));
                rows.push(row("Blue", n(c.z), Some(Field::Blue)));
            }
            rows.push(row("Delete polygon", String::new(), Some(Field::Delete)));
        }
        let bone = self.bone_name(self.bone());
        rows.push(row("Proxy box around", bone.clone(), Some(Field::ProxyBox)));
        rows.push(row("Remove proxies of", bone, Some(Field::RemoveProxies)));
        rows.push(row("Done", String::new(), Some(Field::EditModel)));
        rows
    }

    pub fn hints(&self) -> [String; 10] {
        [
            "Proxies are outlined in pink".to_string(),
            "Click: pick a polygon".to_string(),
            "P: pick proxies or drawn".to_string(),
            "Scroll a row to change it".to_string(),
            "Hold right: look".to_string(),
            "Del: delete the polygon".to_string(),
            "Ctrl Z/Y: undo/redo".to_string(),
            "Ctrl S: save the model".to_string(),
            "Esc or M: back to the level".to_string(),
            "Tab: play".to_string(),
        ]
    }

    // ---- Changes

    /// Carries out `field` when it isn't an edit.
    pub fn command(&mut self, field: Field, dir: f32) {
        match field {
            Field::PickProxies => {
                self.proxies = !self.proxies;
                self.hover = None;
            }
            Field::PolygonPick => {
                let n = self.doc.polygons.len() as isize;
                if n > 0 {
                    let now = self.polygon.map_or(-1, |p| p as isize);
                    self.polygon = Some((now + dir.signum() as isize).rem_euclid(n) as usize);
                }
            }
            _ => {}
        }
    }

    /// Makes edit `field` in the tables; returns what it did. The app then loads them (which
    /// checks them) and rebuilds the level.
    pub fn change(&mut self, field: Field, dir: f32) -> Result<String, String> {
        match (field, self.polygon) {
            (Field::Proxy, Some(p)) => {
                let polygon = &mut self.doc.polygons[p];
                polygon.flags ^= moose_assets::PolyFlags::PROXY;
                let kind = if polygon.proxy() { "a shadow proxy" } else { "drawn" };
                Ok(format!("polygon {p} is {kind}"))
            }
            (Field::Red | Field::Green | Field::Blue, Some(p)) => {
                let mut c = self.doc.polygon_color(p).ok_or("the model has no colors")?;
                let channel = match field {
                    Field::Red => &mut c.x,
                    Field::Green => &mut c.y,
                    _ => &mut c.z,
                };
                *channel = ((*channel / COLOR_STEP).round() + dir.signum()) * COLOR_STEP;
                self.doc.set_polygon_color(p, c)?;
                Ok(format!("polygon {p} colored"))
            }
            (Field::Delete, Some(p)) => {
                self.doc.remove_polygon(p);
                self.polygon = None;
                Ok(format!("polygon {p} deleted"))
            }
            (Field::ProxyBox, _) => {
                let bone = self.bone();
                let first = self.doc.add_proxy_box(bone)?;
                self.polygon = Some(first);
                self.proxies = true;
                Ok(format!("a proxy box around {}", self.bone_name(bone)))
            }
            (Field::RemoveProxies, _) => {
                let bone = self.bone();
                let name = self.bone_name(bone);
                if self.polygon.is_some_and(|p| self.doc.polygons[p].proxy()) {
                    self.polygon = None;
                }
                let removed = self.doc.remove_proxies(bone);
                if removed == 0 {
                    return Err(format!("{name} has no proxies"));
                }
                // Polygon numbers after the removed ones moved down.
                self.polygon = None;
                Ok(format!("removed {removed} proxy polygons of {name}"))
            }
            _ => Err("pick a polygon first".into()),
        }
    }

    // ---- Picking and drawing

    /// The entity's polygon under screen point `at` among drawn polygons or proxies (as
    /// `proxies` says), facing the eye at `eye`, nearest it. For drawn polygons the app uses
    /// the renderer's picking instead, which knows what's hidden.
    pub fn pick_proxy(&self, projection: &Projection, eye: Vec3, at: Vec2, world: &World, assets: &Assets) -> Option<usize> {
        let e = world.entities.get(self.entity)?;
        let mesh = assets.mesh(e.mesh);
        let transform = e.transform();
        let mut best: Option<(usize, f32)> = None;
        for (i, polygon) in mesh.polygons.iter().enumerate() {
            if !polygon.flags.proxy() {
                continue;
            }
            let points: Vec<Vec3> = mesh.polygon_points(polygon).map(|p| transform.transform_point3(p)).collect();
            let normal = (e.rotation * polygon.plane.normal).normalize_or_zero();
            if (eye - points[0]).dot(normal) <= 0.0 {
                continue;
            }
            let Some(screen) = points.iter().map(|&p| projection.point(p)).collect::<Option<Vec<Vec2>>>() else {
                continue;
            };
            if !inside(&screen, at) {
                continue;
            }
            let center = points.iter().copied().sum::<Vec3>() / points.len() as f32;
            let d = center.distance(eye);
            if best.is_none_or(|(_, b)| d < b) {
                best = Some((i, d));
            }
        }
        best.map(|(i, _)| i)
    }

    /// The entity's proxies outlined, and the selected and pointed-at polygons.
    pub fn draw(&self, canvas: &mut Canvas, projection: &Projection, world: &World, assets: &Assets) {
        let Some(e) = world.entities.get(self.entity) else {
            return;
        };
        let mesh = assets.mesh(e.mesh);
        let transform = e.transform();
        let outline = |canvas: &mut Canvas, i: usize, color: u32| {
            if let Some(polygon) = mesh.polygons.get(i) {
                let points: Vec<Vec3> = mesh.polygon_points(polygon).map(|p| transform.transform_point3(p)).collect();
                wire::outline(canvas, projection, &points, color);
            }
        };
        for (i, polygon) in mesh.polygons.iter().enumerate() {
            if polygon.flags.proxy() {
                outline(canvas, i, PROXY);
            }
        }
        if let Some(i) = self.hover {
            outline(canvas, i, crate::editor::HOVER);
        }
        if let Some(i) = self.polygon {
            outline(canvas, i, crate::editor::SELECTED);
        }
    }
}

/// Whether `at` is inside convex outline `points` (either winding).
fn inside(points: &[Vec2], at: Vec2) -> bool {
    let n = points.len();
    let mut sign = 0.0f32;
    for i in 0..n {
        let (a, b) = (points[i], points[(i + 1) % n]);
        let side = (b - a).perp_dot(at - a);
        if side.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = side.signum();
        } else if side.signum() != sign {
            return false;
        }
    }
    sign != 0.0
}
