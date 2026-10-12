//! The editor's panels: a column of icons on the right (add, world, controls, materials,
//! and the three modes), and beside it the panel of the active selection, or of an icon's
//! menu, with submenus (the lights kept off, meta values, the material browser) opened in
//! the same frame, a Back button on top. Laid out as widgets (`crate::gui`) from the
//! editor's state each frame, for drawing and for the mouse alike.

use glam::Vec3;

use crate::editor::{Editor, Excluder, Field, LightRef, Mode, Pick, Selection, ViewMode};
use crate::editor_keys::{Action, Context, FIXED};
use crate::gui::{DIM, Icon, Look, STAT, TEXT, TITLE, Widget};

/// A warning's color: something amiss, not an option.
const WARNING: u32 = 0xFF_A0_40;
use crate::ui::{Canvas, Rect};

/// A menu on the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    /// The plus icon: what to add.
    Add,
    /// Add > Prop: the models.
    AddProp,
    /// The world icon: the level's own settings.
    World,
    /// The gear: the editor's keys and settings.
    Controls,
    /// The lights a surface, sector or entity is kept from (checked).
    Lights(Excluder, usize),
    /// Meta values (`$KEY=VALUE`) of the level, a sector or a surface.
    Meta(Owner),
    /// The materials, to give the selected faces one.
    Materials,
}

/// What has meta values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Level,
    Sector(usize),
    Surface(usize),
    /// A vertex: values a material reads per vertex (`vertex:KEY`), blended across faces.
    Vertex(usize),
}

/// What something on the panel does.
#[derive(Clone, Debug, PartialEq)]
pub enum Act {
    /// An edit or command (`App::edit`), the wheel's way or 1 for a click.
    Edit(Field),
    /// Types a number into a field (the wheel still steps it: `Edit`).
    Type(Field),
    /// Opens a submenu over this panel.
    Open(Menu),
    /// An icon: opens its menu, or closes it if it's open.
    Icon(Menu),
    /// The material editor (the fourth icon).
    MaterialEditor,
    Back,
    Mode(Mode),
    /// Picks a material in the browser (to apply).
    PickMaterial(String),
    /// Adds a thing where the view is.
    Add(Field),
    /// Adds a prop of model `k` (`App::models`).
    AddModel(usize),
    /// Waits for a key to bind to an action.
    Rebind(Action),
    /// Steps the 2D views' turn.
    TurnStep,
    MetaAdd,
    MetaKey(usize),
    MetaValue(usize),
    MetaRemove(usize),
    /// Types into the list's search.
    Search,
    /// Types a number (or name) to find.
    Find,
    /// The wheel over a list scrolls it.
    Scroll,
    /// The confirm box's yes or no.
    Confirm(bool),
    /// Deletes what's selected, after asking.
    Delete,
    /// Selects something and brings the view to it.
    GoTo(Selection),
    /// Saves the level (asked first while it won't load).
    Save,
}

/// What's being typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typing {
    Number(Field),
    Search,
    Find,
    MetaKey(usize),
    MetaValue(usize),
}

/// A list's scroll (its first row shown) and the keys' row in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ListState {
    pub scroll: usize,
    pub cursor: Option<usize>,
}

/// A question waiting for yes or no: the keys choose with Left/Right and Enter (Esc: no).
#[derive(Clone, Debug, PartialEq)]
pub struct Confirm {
    pub text: String,
    pub yes: Act,
    /// Yes is chosen (the keys' button).
    pub choice: bool,
}

/// What the panel needs from outside the editor.
pub struct Ctx<'a> {
    /// The model files (Add > Prop).
    pub models: &'a [String],
    pub width: usize,
    pub height: usize,
}

/// The panels laid out: the widgets, and the list on the top menu (its rows' acts, for the
/// keys, and how many show at once).
pub struct Laid {
    pub widgets: Vec<Widget<Act>>,
    pub list: Vec<Act>,
    pub shown: usize,
}

/// Font scale for a screen `height` tall, as the menus use.
pub fn scale_for(height: usize) -> usize {
    if height >= 600 { 2 } else { 1 }
}

/// Lays out the panel, going down a column of `w` pixels from `x`.
struct Column {
    widgets: Vec<Widget<Act>>,
    x: usize,
    y: usize,
    w: usize,
    scale: usize,
    line: usize,
    pad: usize,
}

impl Column {
    fn push(&mut self, w: Widget<Act>) {
        self.widgets.push(w);
    }

    /// A title, with a Back button before it (and an Apply after it).
    fn header(&mut self, title: &str, back: bool, apply: Option<Act>) {
        let (s, line, pad) = (self.scale, self.line, self.pad);
        let mut x = self.x + pad;
        if back {
            let w = Canvas::text_width("< Back", s) + 2 * pad;
            self.push(Widget::new(Rect::new(x, self.y, w, line), Look::Button { text: "< Back".into(), on: false }).act(Act::Back));
            x += w + pad;
        }
        let right = self.x + self.w - pad;
        if let Some(apply) = apply {
            let w = Canvas::text_width("Apply", s) + 2 * pad;
            self.push(Widget::new(Rect::new(right - w, self.y, w, line), Look::Button { text: "Apply".into(), on: false }).act(apply));
        }
        self.push(Widget::new(Rect::new(x, self.y + 2 * s, right.saturating_sub(x), line), Look::Label { text: title.into(), color: TITLE, scale: s }));
        self.y += line + pad;
    }

    /// Lines about the selection: smaller and in their own color, so they read as stats,
    /// not things to change.
    fn stats(&mut self, lines: &[String]) {
        let s = if self.scale > 1 { self.scale - 1 } else { 1 };
        let line = Canvas::line_height(s);
        for l in lines {
            self.push(Widget::new(Rect::new(self.x + self.pad, self.y, self.w, line), Look::Label { text: l.clone(), color: STAT, scale: s }));
            self.y += line;
        }
        if !lines.is_empty() {
            self.y += self.pad;
        }
    }

    fn label(&mut self, text: &str, color: u32) {
        self.push(Widget::new(Rect::new(self.x + self.pad, self.y + 2 * self.scale, self.w, self.line), Look::Label { text: text.into(), color, scale: self.scale }));
        self.y += self.line;
    }

    fn gap(&mut self) {
        self.y += self.pad;
    }

    /// A row: clicked does `act`, the wheel does `wheel`.
    fn row(&mut self, label: &str, value: String, act: Option<Act>, wheel: Option<Act>) {
        let mut w = Widget::new(Rect::new(self.x, self.y, self.w, self.line), Look::Row { label: label.into(), value, hot: false, dim: false });
        (w.act, w.wheel) = (act, wheel);
        self.push(w);
        self.y += self.line;
    }

    /// A row whose label does one thing (`act`) and whose value another (`value_act`; the
    /// wheel: `value_wheel`): a value drawn over the row's right part.
    fn row_split(&mut self, label: &str, act: Act, value: String, value_act: Act, value_wheel: Option<Act>) {
        let vw = (Canvas::text_width(&value, self.scale) + 6 * self.scale).min(self.w / 2);
        self.push(Widget::new(Rect::new(self.x, self.y, self.w - vw, self.line), Look::Row { label: label.into(), value: String::new(), hot: false, dim: false }).act(act));
        let mut w = Widget::new(Rect::new(self.x + self.w - vw, self.y, vw, self.line), Look::Row { label: String::new(), value, hot: false, dim: false }).act(value_act);
        w.wheel = value_wheel;
        self.push(w);
        self.y += self.line;
    }

    /// Two half-width rows side by side.
    fn pair(&mut self, a: (&str, String, Option<Act>, Option<Act>), b: (&str, String, Option<Act>, Option<Act>)) {
        let half = self.w / 2;
        for (k, (label, value, act, wheel)) in [a, b].into_iter().enumerate() {
            let mut w = Widget::new(Rect::new(self.x + k * half, self.y, half, self.line), Look::Row { label: label.into(), value, hot: false, dim: false });
            (w.act, w.wheel) = (act, wheel);
            self.push(w);
        }
        self.y += self.line;
    }

    /// A line to type into.
    fn field(&mut self, text: &str, placeholder: &str, focused: bool, act: Act) {
        let r = Rect::new(self.x + self.pad, self.y, self.w - 2 * self.pad, self.line);
        self.push(Widget::new(r, Look::Field { text: text.into(), placeholder: placeholder.into(), focused }).act(act));
        self.y += self.line + self.pad;
    }

    /// A list of `rows` (each its look and what clicking it does) in the room left above
    /// `reserve` pixels from the bottom (`bottom`), scrolled as `state` says; the keys' row
    /// hot. Returns the rows' acts and how many show.
    fn list(&mut self, rows: Vec<(Look, Act)>, state: &ListState, bottom: usize, reserve: usize) -> (Vec<Act>, usize) {
        let room = bottom.saturating_sub(self.y + reserve) / self.line;
        let shown = room.max(1).min(rows.len().max(1));
        let first = state.scroll.min(rows.len().saturating_sub(shown));
        let top = self.y;
        let acts: Vec<Act> = rows.iter().map(|(_, a)| a.clone()).collect();
        for (k, (mut look, act)) in rows.into_iter().enumerate().skip(first).take(shown) {
            let hot = state.cursor == Some(k);
            match &mut look {
                Look::Row { hot: h, .. } | Look::Check { hot: h, .. } => *h = hot,
                _ => {}
            }
            let r = Rect::new(self.x, self.y, self.w, self.line);
            // Headings and rows that only show something do nothing when clicked.
            let w = if matches!(look, Look::Label { .. }) || act == Act::Scroll { Widget::new(r, look) } else { Widget::new(r, look).act(act) };
            self.push(w.wheel(Act::Scroll));
            self.y += self.line;
        }
        // The wheel scrolls anywhere over the list.
        let area = Rect::new(self.x, top, self.w, shown * self.line);
        self.widgets.insert(0, Widget::new(area, Look::Frame(0)).wheel(Act::Scroll));
        if acts.len() > shown {
            let more = format!("{}-{} of {}", first + 1, first + shown, acts.len());
            self.push(Widget::new(Rect::new(self.x + self.pad, self.y, self.w, self.line), Look::Label { text: more, color: DIM, scale: 1 }));
            self.y += Canvas::line_height(1);
        }
        (acts, shown)
    }
}

/// The value a typed field shows: the text so far, while it's being typed.
fn typed_value(editor: &Editor, field: Field, value: String) -> String {
    match &editor.typing {
        Some((Typing::Number(f), text)) if *f == field => format!("{text}_"),
        _ => value,
    }
}

impl Editor {
    /// The editor's panels and icons for a `ctx.width` x `ctx.height` screen.
    pub fn layout(&self, ctx: &Ctx) -> Laid {
        let (w, h) = (ctx.width, ctx.height);
        let scale = scale_for(h);
        let line = Canvas::line_height(scale);
        let pad = 3 * scale;
        let icon = 12 * scale + 6 * scale;
        let mut widgets: Vec<Widget<Act>> = Vec::new();
        // The icons, down the right edge: the menus, then the modes.
        let ix = w.saturating_sub(icon);
        let top_menu = self.menus.first().copied();
        let icons = [
            (Icon::Add, Act::Icon(Menu::Add), top_menu == Some(Menu::Add) || top_menu == Some(Menu::AddProp)),
            (Icon::World, Act::Icon(Menu::World), top_menu == Some(Menu::World)),
            (Icon::Gear, Act::Icon(Menu::Controls), top_menu == Some(Menu::Controls)),
            (Icon::Material, Act::MaterialEditor, false),
        ];
        widgets.push(Widget::new(Rect::new(ix, 0, icon, h), Look::Panel));
        let mut y = pad;
        for (i, act, on) in icons {
            widgets.push(Widget::new(Rect::new(ix + 1, y, icon - 2, icon - 2), Look::Icon { icon: i, on }).act(act));
            y += icon;
        }
        y += icon / 2;
        if self.model.is_none() {
            for (i, mode) in [(Icon::Sector, Mode::Sector), (Icon::Face, Mode::Face), (Icon::Vertex, Mode::Vertex)] {
                widgets.push(Widget::new(Rect::new(ix + 1, y, icon - 2, icon - 2), Look::Icon { icon: i, on: self.mode == mode }).act(Act::Mode(mode)));
                y += icon;
            }
        }
        // The panel, left of the icons.
        let pw = (Canvas::text_width(&"m".repeat(34), scale)).min(w * 9 / 20);
        let px = ix.saturating_sub(pw);
        let mut c = Column { widgets: Vec::new(), x: px, y: pad, w: pw, scale, line, pad };
        let (list, shown) = self.fill(&mut c, ctx, h);
        let bottom = c.y + pad;
        let mut panel = vec![Widget::new(Rect::new(px, 0, pw, bottom.min(h)), Look::Panel)];
        panel.extend(c.widgets);
        widgets.splice(0..0, panel);
        // A box being dragged.
        if let Some((a, b)) = self.boxing {
            let (lo, hi) = (a.min(b), a.max(b));
            let r = Rect::new(lo.x.max(0.0) as usize, lo.y.max(0.0) as usize, (hi.x - lo.x) as usize, (hi.y - lo.y) as usize);
            widgets.push(Widget::new(r, Look::Frame(crate::editor::SELECTED)));
        }
        // The question, in the middle, over everything.
        if let Some(confirm) = &self.confirm {
            let tw = Canvas::text_width(&confirm.text, scale).max(Canvas::text_width("Yes   No", scale)) + 6 * pad;
            let bh = line * 3 + 4 * pad;
            let (bx, by) = (w.saturating_sub(tw) / 2, h.saturating_sub(bh) / 2);
            widgets.push(Widget::new(Rect::new(bx, by, tw, bh), Look::Panel));
            widgets.push(Widget::new(Rect::new(bx, by, tw, bh), Look::Frame(TITLE)));
            widgets.push(Widget::new(Rect::new(bx + 3 * pad, by + 2 * pad, tw, line), Look::Label { text: confirm.text.clone(), color: TEXT, scale }));
            let bw = Canvas::text_width("Yes", scale) + 6 * pad;
            let yb = by + bh - line - 2 * pad;
            widgets.push(Widget::new(Rect::new(bx + tw / 2 - bw - pad, yb, bw, line), Look::Button { text: "Yes".into(), on: confirm.choice }).act(Act::Confirm(true)));
            widgets.push(Widget::new(Rect::new(bx + tw / 2 + pad, yb, bw, line), Look::Button { text: "No".into(), on: !confirm.choice }).act(Act::Confirm(false)));
        }
        Laid { widgets, list, shown }
    }

    /// Fills the panel's column: the top menu, or the active selection's panel. Returns its
    /// list's rows' acts and how many show (none without a list).
    fn fill(&self, c: &mut Column, ctx: &Ctx, bottom: usize) -> (Vec<Act>, usize) {
        let back = !self.menus.is_empty();
        let thumb_room = 32 * c.scale + Canvas::line_height(1) + 2 * c.pad;
        match self.menus.last() {
            Some(Menu::Add) => {
                c.header("Add", back, None);
                c.stats(&["Where the view is (2D: its middle)".into()]);
                c.row("New room", "4x3x4 m".into(), Some(Act::Add(Field::NewRoom)), None);
                c.row("Light", String::new(), Some(Act::Add(Field::AddLight)), None);
                c.row("Prop", ">".into(), Some(Act::Open(Menu::AddProp)), None);
                c.row("Spawn point", String::new(), Some(Act::Add(Field::AddSpawn)), None);
            }
            Some(Menu::AddProp) => {
                c.header("Add a prop", back, None);
                c.field(&self.search, "search models", self.typing_is(Typing::Search), Act::Search);
                let rows = ctx
                    .models
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| self.matches_search(m))
                    .map(|(k, m)| (Look::Row { label: m.clone(), value: String::new(), hot: false, dim: false }, Act::AddModel(k)))
                    .collect();
                return c.list(rows, &self.list, bottom, c.pad);
            }
            Some(Menu::World) => self.world_menu(c, back),
            Some(Menu::Controls) => {
                c.header("Editor controls", back, None);
                c.stats(&["Click one, then press its new key".into()]);
                let mut rows: Vec<(Look, Act)> = vec![(
                    Look::Row { label: "2D view turn".into(), value: format!("{}°", moose_assets::number(self.keys.turn_step)), hot: false, dim: false },
                    Act::TurnStep,
                )];
                for context in Context::ALL {
                    rows.push((Look::Label { text: context.name().into(), color: TITLE, scale: c.scale }, Act::Scroll));
                    for a in Action::ALL.iter().filter(|a| a.context() == context) {
                        let value = if self.rebinding == Some(*a) { "press a key".into() } else { self.keys.key(*a).name() };
                        rows.push((Look::Row { label: a.label().into(), value, hot: false, dim: false }, Act::Rebind(*a)));
                    }
                }
                rows.push((Look::Label { text: "Always".into(), color: TITLE, scale: c.scale }, Act::Scroll));
                for (key, what) in FIXED {
                    rows.push((Look::Row { label: key.into(), value: what.into(), hot: false, dim: true }, Act::Scroll));
                }
                return c.list(rows, &self.list, bottom, c.pad);
            }
            Some(&Menu::Lights(thing, i)) => {
                c.header("Lights kept off", back, None);
                c.stats(&["Checked: kept off. Only lights that reach it".into()]);
                let rows = self
                    .light_choices(thing, i)
                    .into_iter()
                    .map(|(light, label, value, on)| (Look::Check { label, value, on, hot: false }, Act::Edit(Field::ToggleLight(light))))
                    .collect();
                return c.list(rows, &self.list, bottom, c.pad);
            }
            Some(&Menu::Meta(owner)) => {
                let title = match owner {
                    Owner::Level => "Level meta values".to_string(),
                    Owner::Sector(s) => format!("Sector {s} meta values"),
                    Owner::Surface(f) => format!("Face {f} meta values"),
                    Owner::Vertex(v) => format!("Vertex {v} meta values"),
                };
                c.header(&title, back, None);
                c.stats(&["$KEY=VALUE: numbers for materials to read".into(), "Click a key or value to type it".into()]);
                for (k, (key, value)) in self.meta_values(owner).into_iter().enumerate() {
                    let third = c.w / 3;
                    let shown = |t: Typing, v: String| match &self.typing {
                        Some((typing, text)) if *typing == t => format!("{text}_"),
                        _ => v,
                    };
                    let (x, y, line) = (c.x, c.y, c.line);
                    let key_row = Look::Row { label: shown(Typing::MetaKey(k), key), value: String::new(), hot: false, dim: false };
                    c.push(Widget::new(Rect::new(x, y, third, line), key_row).act(Act::MetaKey(k)));
                    let value_row = Look::Row { label: shown(Typing::MetaValue(k), value), value: String::new(), hot: false, dim: false };
                    c.push(Widget::new(Rect::new(x + third, y, third + third / 2, line), value_row).act(Act::MetaValue(k)));
                    let bw = Canvas::text_width("x", c.scale) + 2 * c.pad;
                    c.push(Widget::new(Rect::new(x + c.w - bw - c.pad, y, bw, line), Look::Button { text: "x".into(), on: false }).act(Act::MetaRemove(k)));
                    c.y += line;
                }
                c.gap();
                c.row("Add a value", "+".into(), Some(Act::MetaAdd), None);
            }
            Some(Menu::Materials) => {
                c.header("Material", back, Some(Act::Edit(Field::SetMaterial)));
                let picked = self.material_pick.clone();
                c.stats(&[match &picked {
                    Some(m) => format!("{m}: Apply gives the selected faces it"),
                    None => "Click one to see it; Apply gives it".into(),
                }]);
                c.field(&self.search, "search materials", self.typing_is(Typing::Search), Act::Search);
                let rows = self
                    .materials
                    .iter()
                    .filter(|m| self.matches_search(m))
                    .map(|m| {
                        let on = picked.as_deref() == Some(m.as_str());
                        let look = Look::Row { label: m.clone(), value: if on { "<".into() } else { String::new() }, hot: false, dim: false };
                        (look, Act::PickMaterial(m.clone()))
                    })
                    .collect();
                let laid = c.list(rows, &self.list, bottom, thumb_room);
                // Its textures, in slot order.
                if let Some(k) = picked.and_then(|m| self.materials.iter().position(|n| *n == m)) {
                    c.gap();
                    let side = 32 * c.scale;
                    let mut x = c.x + c.pad;
                    for t in self.material_textures.get(k).into_iter().flatten() {
                        if x + side > c.x + c.w {
                            break;
                        }
                        c.push(Widget::new(Rect::new(x, c.y, side, side + Canvas::line_height(1)), Look::Thumb { key: t.clone(), label: t.clone(), on: false }));
                        x += side + c.pad;
                    }
                    c.y += side + Canvas::line_height(1);
                }
                return laid;
            }
            None => self.selection_panel(c, ctx),
        }
        (Vec::new(), 0)
    }

    fn typing_is(&self, t: Typing) -> bool {
        matches!(&self.typing, Some((typing, _)) if *typing == t)
    }

    /// Whether `name` fits what's typed in the search (any part of it, in any case).
    pub fn matches_search(&self, name: &str) -> bool {
        self.search.is_empty() || name.to_lowercase().contains(&self.search.to_lowercase())
    }

    fn world_menu(&self, c: &mut Column, back: bool) {
        let d = &self.doc;
        let n = moose_assets::number;
        c.header("World", back, None);
        let file = self.path.file_name().map_or(String::new(), |f| f.to_string_lossy().into_owned());
        let mut stats = vec![
            format!("{} ({file}){}", d.name, if self.dirty() { ", unsaved" } else { "" }),
            format!("{} sectors, {} faces, {} vertices", d.sectors.len(), d.surfaces.len(), d.vertices.len()),
            format!("{} entities, {} lights", d.entities.len(), d.lights.len()),
        ];
        if let Some(why) = &self.broken {
            stats.push(format!("won't load: {}", why.chars().take(60).collect::<String>()));
        }
        c.stats(&stats);
        let ambient = d.ambient.unwrap_or(Vec3::ONE);
        let typed = |f: Field, v: String| (typed_value(self, f, v), Some(Act::Type(f)), Some(Act::Edit(f)));
        let (v, a, w) = typed(Field::AmbientRed, n(ambient.x));
        c.row("Ambient red", v, a, w);
        let (v, a, w) = typed(Field::AmbientGreen, n(ambient.y));
        c.row("Ambient green", v, a, w);
        let (v, a, w) = typed(Field::AmbientBlue, n(ambient.z));
        c.row("Ambient blue", v, a, w);
        if let Some(sun) = d.directional.first() {
            let (yaw, pitch) = crate::editor::aim_of(-sun.direction);
            let (v, a, w) = typed(Field::SunAngle, format!("{}°", n(sun.angle)));
            c.row("Sun size", v, a, w);
            // With a day and night, where it is at noon.
            let noon = if self.sun_option("cycle").is_some() { " at noon" } else { "" };
            let (v, a, w) = typed(Field::SunYaw, format!("{}°", n(yaw.round())));
            c.row(&format!("Sun from (yaw){noon}"), v, a, w);
            let (v, a, w) = typed(Field::SunPitch, format!("{}°", n(pitch.round())));
            c.row(&format!("Sun height{noon}"), v, a, w);
            // Its day and night, and clouds.
            let option = |key: &str| self.sun_option(key).map(str::to_string);
            let length = option("cycle").unwrap_or("none".into());
            let (v, a, w) = typed(Field::DayLength, if length == "none" { length } else { format!("{length} s") });
            c.row("Day lasts", v, a, w);
            let start = option("time").and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.5);
            let (v, a, w) = typed(Field::DayStart, crate::daynight::clock(start));
            c.row("Day starts at (hours)", v, a, w);
            let (v, a, w) = typed(Field::CloudSeed, option("clouds").unwrap_or("none (-1)".into()));
            c.row("Clouds (seed)", v, a, w);
            let (v1, a1, w1) = typed(Field::Weather, option("weather").unwrap_or("0.3".into()));
            let (v2, a2, w2) = typed(Field::Wind, format!("{} m/s", option("wind").unwrap_or("8".into())));
            c.pair(("Weather", v1, a1, w1), ("Wind", v2, a2, w2));
            let (v, a, w) = typed(Field::WindFrom, format!("{}°", option("wind_from").unwrap_or("0".into())));
            c.row("Wind from", v, a, w);
            c.row("Sky faces: day and night sky", String::new(), Some(Act::Edit(Field::SkyCycleFaces)), None);
        } else {
            c.row("Add a sun", "+".into(), Some(Act::Edit(Field::AddSun)), None);
        }
        c.row("Meta values", format!("{} >", self.meta_values(Owner::Level).len()), Some(Act::Open(Menu::Meta(Owner::Level))), None);
        c.gap();
        c.label("Find by number", DIM);
        let find = match &self.typing {
            Some((Typing::Find, text)) => text.clone(),
            _ => String::new(),
        };
        c.field(&find, "sector 3, face 12, vertex 7, light 2", self.typing_is(Typing::Find), Act::Find);
        if self.broken.is_some()
            && let Some(problem) = self.problem_selection()
        {
            c.row("Go to the problem", describe(problem), Some(Act::GoTo(problem)), None);
        }
    }

    /// The panel of the active selection (or saying there's none, in the mode).
    fn selection_panel(&self, c: &mut Column, _ctx: &Ctx) {
        let n = moose_assets::number;
        let typed = |f: Field, v: String| (typed_value(self, f, v), Some(Act::Type(f)), Some(Act::Edit(f)));
        let selected = self.selected().len();
        let also = |stats: &mut Vec<String>| {
            if selected > 1 {
                stats.push(format!("{selected} selected (Shift: add or drop)"));
            }
        };
        if let Some(model) = &self.model {
            c.header(&format!("Mesh editor{}", if model.dirty() { " (unsaved)" } else { "" }), false, None);
            for r in model.panel() {
                c.row(&r.label, r.value, r.field.map(Act::Edit), r.field.map(Act::Edit));
            }
            return;
        }
        match self.selection.filter(|&s| self.exists(s)) {
            None => {
                let text = match self.mode {
                    Mode::Vertex if !self.vertex_faces.is_empty() => "No vertex selected".to_string(),
                    Mode::Vertex => "No face to pick corners of".to_string(),
                    m => format!("No {} selected", m.name()),
                };
                c.header(&text, false, None);
                let hint = match self.mode {
                    Mode::Sector => "Click a face of one; Shift adds",
                    Mode::Face => "Click a face; Shift adds; drag a box",
                    Mode::Vertex if self.vertex_faces.is_empty() => "Click a face, then its corners",
                    Mode::Vertex => "Click a corner (dots); a face: its corners",
                };
                c.stats(&[hint.into()]);
            }
            Some(Selection::Sector(s)) => {
                let d = &self.doc;
                let range = d.sector_surfaces(s);
                let openings = d.surfaces[range.clone()].iter().filter(|x| x.adjoin.is_some()).count();
                let (lo, hi) = self.sector_bounds(s);
                let size = hi - lo;
                let mut stats = vec![
                    format!("sector {s} of {}", d.sectors.len()),
                    format!("{} faces, {openings} openings", range.len()),
                    format!(
                        "{} entities, {} lights",
                        d.entities.iter().filter(|e| e.sector == s).count(),
                        d.lights.iter().filter(|l| l.sector == s).count()
                    ),
                    format!("{} x {} x {} m", n(size.x), n(size.y), n(size.z)),
                ];
                also(&mut stats);
                c.header(&format!("Sector {}", d.sectors[s].name), false, None);
                c.stats(&stats);
                let keys = &self.keys;
                let cleave = if self.cutting.is_some() { "click two points in 2D".into() } else { format!("{}, then the cut", keys.key(Action::Cleave).name()) };
                c.row("Cleave", cleave, Some(Act::Edit(Field::Cleave)), None);
                let merge = if self.picking == Some((s, Pick::MergeSector)) {
                    "click a face of it".to_string()
                } else if selected > 1 {
                    format!("the selected ({})", keys.key(Action::MergeSectors).name())
                } else {
                    format!("click one ({})", keys.key(Action::MergeSectors).name())
                };
                let merge_field = if selected > 1 { Field::MergeSelected } else { Field::MergeSector };
                c.row("Merge", merge, Some(Act::Edit(merge_field)), None);
                let middle = (lo + hi) * 0.5;
                c.label("Its middle (moves it)", DIM);
                let (vx, ax, wx) = typed(Field::X, n(middle.x));
                let (vy, ay, wy) = typed(Field::Y, n(middle.y));
                c.pair(("X", vx, ax, wx), ("Y", vy, ay, wy));
                let (vz, az, wz) = typed(Field::Z, n(middle.z));
                c.pair(("Z", vz, az, wz), ("Turn", "[ ]".into(), None, Some(Act::Edit(Field::Yaw))));
                self.exclusion_row(c, Excluder::Sector, s);
                c.row("Meta values", format!("{} >", self.meta_values(Owner::Sector(s)).len()), Some(Act::Open(Menu::Meta(Owner::Sector(s)))), None);
                c.gap();
                c.row("Delete", keys.key(Action::DeleteSector).name(), Some(Act::Delete), None);
            }
            Some(Selection::Surface(i)) => self.face_panel(c, i, selected),
            Some(Selection::Vertex(v)) => {
                let p = self.doc.vertices[v];
                let faces = self.doc.surfaces.iter().filter(|s| s.corners.iter().any(|c| c.0 == v)).count();
                let mut stats = vec![format!("vertex {v}, a corner of {faces} faces")];
                also(&mut stats);
                c.header(&format!("Vertex {v}"), false, None);
                c.stats(&stats);
                for (label, f, value) in [("X", Field::X, p.x), ("Y", Field::Y, p.y), ("Z", Field::Z, p.z)] {
                    let (v, a, w) = typed(f, n(value));
                    c.row(label, v, a, w);
                }
                // What materials read per vertex (vertex:KEY), blended across its faces.
                let metas = self.meta_values(Owner::Vertex(v));
                let shown = match metas.len() {
                    0 => "none >".to_string(),
                    1 => format!("${}={} >", metas[0].0, metas[0].1),
                    k => format!("{k} >"),
                };
                c.row("Meta values", shown, Some(Act::Open(Menu::Meta(Owner::Vertex(v)))), None);
                c.gap();
                if selected > 1 {
                    c.row("Weld the selected into this one", String::new(), Some(Act::Edit(Field::Weld)), None);
                }
                let near = self.doc.vertices_near(v, self.step() + 1e-3).len() - 1;
                c.row(
                    &format!("Weld nearby ({} m)", moose_assets::number(self.step())),
                    if near > 0 { format!("{near} near") } else { "none near".into() },
                    Some(Act::Edit(Field::WeldNear)),
                    None,
                );
            }
            Some(Selection::Entity(_) | Selection::Light(_)) => {
                let rows = self.panel();
                let (title, rest) = rows.split_first().map_or(("".to_string(), &rows[..]), |(t, r)| (format!("{} {}", t.label, t.value), r));
                c.header(&title, false, None);
                if selected > 1 {
                    c.stats(&[format!("{selected} selected")]);
                }
                for r in rest {
                    match r.field {
                        Some(Field::Delete) => c.row(&r.label, String::new(), Some(Act::Delete), None),
                        Some(Field::ExcludeLights) => {
                            if let Some(Selection::Entity(e)) = self.selection {
                                self.exclusion_row(c, Excluder::Entity, e);
                            }
                        }
                        Some(Field::FlashlightLights | Field::SunLights) => {}
                        Some(f) if f.typed() => {
                            let (v, a, w) = typed(f, r.value.clone());
                            c.row(&r.label, v, a, w);
                        }
                        Some(f) => c.row(&r.label, r.value.clone(), Some(Act::Edit(f)), Some(Act::Edit(f))),
                        None => c.row(&r.label, r.value.clone(), None, None),
                    }
                }
            }
        }
    }

    fn face_panel(&self, c: &mut Column, i: usize, selected: usize) {
        let n = moose_assets::number;
        let typed = |f: Field, v: String| (typed_value(self, f, v), Some(Act::Type(f)), Some(Act::Edit(f)));
        let d = &self.doc;
        let s = &d.surfaces[i];
        let keys = &self.keys;
        let normal = d.surface_normal(i);
        let mut stats = vec![
            format!("face {i} of sector {}", d.sectors[s.sector].name),
            format!("{} corners, facing {}, {}, {}", s.corners.len(), n((normal.x * 100.0).round() / 100.0), n((normal.y * 100.0).round() / 100.0), n((normal.z * 100.0).round() / 100.0)),
        ];
        if selected > 1 {
            stats.push(format!("{selected} selected (Shift: add or drop)"));
        }
        if let Some(a) = s.adjoin {
            let other = d.surfaces[d.adjoins[d.adjoins[a].mirror].surface].sector;
            stats.push(format!("an opening to {}", d.sectors[other].name));
            c.header(&format!("Opening {i}"), false, None);
            c.stats(&stats);
            c.row("Its sector", d.sectors[s.sector].name.clone(), Some(Act::Edit(Field::SelectSector)), None);
            c.row("Wall it up (both sides)", String::new(), Some(Act::Edit(Field::Unadjoin)), None);
            c.row("Meta values", format!("{} >", self.meta_values(Owner::Surface(i)).len()), Some(Act::Open(Menu::Meta(Owner::Surface(i)))), None);
            return;
        }
        let material = s.options.iter().find_map(|o| o.strip_prefix("material=")).unwrap_or("none");
        stats.push(format!("material {material}"));
        c.header(&format!("Face {i}"), false, None);
        c.stats(&stats);
        // Vertex meta some corners have and some don't: those use the material's fallback.
        let warnings = self.vertex_meta_warnings(i);
        if !warnings.is_empty() {
            let s = if c.scale > 1 { c.scale - 1 } else { 1 };
            for w in warnings {
                c.push(Widget::new(Rect::new(c.x + c.pad, c.y, c.w, Canvas::line_height(s)), Look::Label { text: w, color: WARNING, scale: s }));
                c.y += Canvas::line_height(s);
            }
            c.gap();
        }
        let on = |bit: u32| if s.flags & bit != 0 { "on" } else { "off" }.to_string();
        let pick = |kind: Pick, idle: String| if self.picking == Some((i, kind)) { "click a face".into() } else { idle };
        c.row("Its sector", d.sectors[s.sector].name.clone(), Some(Act::Edit(Field::SelectSector)), None);
        c.row("Material", format!("{material} >"), Some(Act::Open(Menu::Materials)), None);
        c.row("Stitch from a touching face", pick(Pick::Stitch, String::new()), Some(Act::Edit(Field::Stitch)), None);
        let merge_key = keys.key(Action::MergeFaces).name();
        let (merge_field, merge) = if selected > 1 {
            (Field::MergeSelectedFaces, format!("the selected ({merge_key})"))
        } else {
            (Field::Merge, pick(Pick::Merge, format!("({merge_key})")))
        };
        c.row("Merge with touching faces", merge, Some(Act::Edit(merge_field)), None);
        let cut = if self.cutting.is_some() { "drag or click in 2D".into() } else { format!("{}, then the cut", keys.key(Action::CutFace).name()) };
        c.row("Cut", cut, Some(Act::Edit(Field::Cleave)), None);
        c.pair(("Reflective", on(0x1), Some(Act::Edit(Field::Reflective)), None), ("Sky", on(0x2), Some(Act::Edit(Field::Sky)), None));
        let by = typed_value(self, Field::ExtrudeBy, format!("by {} m", n(self.extrude_by)));
        c.row_split(&format!("Extrude ({})", keys.key(Action::Extrude).name()), Act::Edit(Field::Extrude), by, Act::Type(Field::ExtrudeBy), Some(Act::Edit(Field::ExtrudeBy)));
        let (v, a, w) = typed(Field::Push, "type/scroll".into());
        c.row("Push/pull", v, a, w);
        let open = if selected == 2 { "the two selected" } else { "" };
        c.row("Open to a match", format!("{open} ({})", keys.key(Action::OpenToMatch).name()), Some(Act::Edit(Field::Adjoin)), None);
        c.row("Split into triangles", String::new(), Some(Act::Edit(Field::Triangulate)), None);
        c.row("Vary tint ($tint on its vertices)", String::new(), Some(Act::Edit(Field::VaryTint)), None);
        self.exclusion_row(c, Excluder::Surface, i);
        c.row("Meta values", format!("{} >", self.meta_values(Owner::Surface(i)).len()), Some(Act::Open(Menu::Meta(Owner::Surface(i)))), None);
        if d.attributes.iter().any(|a| a.name == "uv") {
            c.gap();
            c.label("Texture", DIM);
            let m = self.texture_mapping(i);
            let shown = |f: fn(&crate::editor::TextureMapping) -> String| m.as_ref().map_or("scroll".into(), f);
            let (v1, a1, w1) = typed(Field::TextureU, shown(|m| format!("{:.3}", m.offset.x)));
            let (v2, a2, w2) = typed(Field::TextureV, shown(|m| format!("{:.3}", m.offset.y)));
            c.pair(("Across", v1, a1, w1), ("Down", v2, a2, w2));
            let (v1, a1, w1) = typed(Field::TextureSizeU, shown(|m| format!("{:.2} m", m.size.x)));
            let (v2, a2, w2) = typed(Field::TextureSizeV, shown(|m| format!("{:.2} m", m.size.y)));
            c.pair(("Width", v1, a1, w1), ("Height", v2, a2, w2));
            let (v1, a1, w1) = typed(Field::TextureTurn, shown(|m| format!("{:.1}°", m.turn)));
            c.pair(("Turn", v1, a1, w1), ("Scale", "scroll".into(), None, Some(Act::Edit(Field::TextureScale))));
            c.pair(
                ("Flip across", String::new(), Some(Act::Edit(Field::TextureFlipAcross)), None),
                ("Flip down", String::new(), Some(Act::Edit(Field::TextureFlipDown)), None),
            );
            c.row("Map flat", String::new(), Some(Act::Edit(Field::TextureProject)), None);
        }
    }

    /// The row opening the lights a thing is kept from, with what they are.
    fn exclusion_row(&self, c: &mut Column, thing: Excluder, i: usize) {
        let (names, template) = self.excluded(thing, i);
        let value = match names.len() {
            0 => "none >".to_string(),
            1 => format!("{} >", names[0]),
            k => format!("{k} >"),
        };
        let value = if template { format!("(template) {value}") } else { value };
        c.row("Lights kept off", value, Some(Act::Open(Menu::Lights(thing, i))), None);
    }

    /// The lights that can reach `thing` `i` (within their range of it; for a surface, in
    /// front of it), nearest first, then the flashlight and the sun: each its label, how far,
    /// and whether it's kept off.
    pub fn light_choices(&self, thing: Excluder, i: usize) -> Vec<(LightRef, String, String, bool)> {
        let d = &self.doc;
        let (names, _) = self.excluded(thing, i);
        let off = |name: Option<&str>| name.is_some_and(|n| names.iter().any(|m| m == n));
        let (lo, hi, plane) = match thing {
            Excluder::Surface => {
                let p = d.surface_points(i);
                let (lo, hi) = p.iter().fold((Vec3::INFINITY, Vec3::NEG_INFINITY), |(lo, hi), &q| (lo.min(q), hi.max(q)));
                (lo, hi, Some((d.surface_normal(i), p[0])))
            }
            Excluder::Sector => {
                let (lo, hi) = self.sector_bounds(i);
                (lo, hi, None)
            }
            Excluder::Entity => {
                let e = &d.entities[i];
                (e.position - Vec3::splat(1.0), e.position + Vec3::splat(1.0), None)
            }
        };
        let mut out: Vec<(f32, LightRef, String, String, bool)> = Vec::new();
        for (k, l) in d.lights.iter().enumerate() {
            let near = l.position.clamp(lo, hi);
            let distance = near.distance(l.position);
            let in_front = plane.is_none_or(|(n, p)| n.dot(l.position - p) > -0.05);
            if distance >= l.range || !in_front {
                continue;
            }
            let name = l.options.iter().find_map(|o| o.strip_prefix("name=")).map(str::to_string);
            let label = match &name {
                Some(n) => format!("{n} (light {k})"),
                None => format!("light {k}"),
            };
            out.push((distance, LightRef::Level(k), label, format!("{} m", moose_assets::number((distance * 10.0).round() / 10.0)), off(name.as_deref())));
        }
        out.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut list: Vec<(LightRef, String, String, bool)> = out.into_iter().map(|(_, r, l, v, o)| (r, l, v, o)).collect();
        list.push((LightRef::Flashlight, "the flashlight".into(), String::new(), off(Some("flashlight"))));
        if let Some(sun) = d.directional.first() {
            let name = sun.options.iter().find_map(|o| o.strip_prefix("name="));
            list.push((LightRef::Sun, "the sun".into(), String::new(), off(name)));
        }
        list
    }

    /// What's amiss with face `f`'s vertices' meta values: a key some of its corners have
    /// and others don't (they use the material's fallback there), or one with different
    /// counts of numbers on different corners.
    pub fn vertex_meta_warnings(&self, f: usize) -> Vec<String> {
        let corners: Vec<usize> = self.doc.surfaces[f].corners.iter().map(|c| c.0).collect();
        let metas: Vec<Vec<(String, String)>> = corners.iter().map(|&v| self.meta_values(Owner::Vertex(v))).collect();
        let mut keys: Vec<String> = metas.iter().flatten().map(|(k, _)| k.clone()).collect();
        keys.sort();
        keys.dedup();
        let mut out = Vec::new();
        for key in keys {
            let values: Vec<&String> = metas.iter().filter_map(|m| m.iter().find(|(k, _)| *k == key).map(|(_, v)| v)).collect();
            if values.len() < corners.len() {
                out.push(format!("! ${key} on {} of {} corners (the rest: the material's fallback)", values.len(), corners.len()));
            }
            let counts: Vec<usize> = values.iter().map(|v| v.split(',').count()).collect();
            if counts.iter().any(|&c| c != counts[0]) {
                out.push(format!("! ${key}'s corners have different counts of numbers"));
            }
        }
        out
    }

    /// The meta values (`$KEY=VALUE`) of `owner`, as key and value.
    pub fn meta_values(&self, owner: Owner) -> Vec<(String, String)> {
        let options = match owner {
            Owner::Level => &self.doc.name_options,
            Owner::Sector(s) if s < self.doc.sectors.len() => &self.doc.sectors[s].options,
            Owner::Surface(f) if f < self.doc.surfaces.len() => &self.doc.surfaces[f].options,
            Owner::Vertex(v) => match self.doc.vertex_options.get(v) {
                Some(options) => options,
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        options
            .iter()
            .filter_map(|o| o.strip_prefix('$')?.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// `owner`'s options, to change its meta values.
    pub fn meta_options(&mut self, owner: Owner) -> Option<&mut Vec<String>> {
        match owner {
            Owner::Level => Some(&mut self.doc.name_options),
            Owner::Sector(s) => self.doc.sectors.get_mut(s).map(|s| &mut s.options),
            Owner::Surface(f) => self.doc.surfaces.get_mut(f).map(|s| &mut s.options),
            Owner::Vertex(v) if v < self.doc.vertices.len() => Some(self.doc.vertex_options_mut(v)),
            Owner::Vertex(_) => None,
        }
    }

    /// Sets meta value `k` of `owner` (its key, or its value), or with `None` for both,
    /// adds a new one or removes it (see `Act::MetaRemove`). Checked as the level loader
    /// would: keys are letters, digits and `_`, values one or more numbers.
    pub fn set_meta(&mut self, owner: Owner, k: Option<usize>, key: Option<&str>, value: Option<&str>) -> Result<String, String> {
        if let Some(key) = key
            && (key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
        {
            return Err("a key is letters, digits and '_'".into());
        }
        if let Some(value) = value {
            moose_assets::meta::parse_numbers(value)?;
        }
        let values = self.meta_values(owner);
        let options = self.meta_options(owner).ok_or("that's gone")?;
        let metas: Vec<usize> = (0..options.len()).filter(|&j| options[j].starts_with('$')).collect();
        match (k, key, value) {
            // A new one.
            (None, Some(key), Some(value)) => {
                if values.iter().any(|(k, _)| k == key) {
                    return Err(format!("there is a ${key} already"));
                }
                options.push(format!("${key}={value}"));
                Ok(format!("${key}={value}"))
            }
            (Some(k), None, None) => {
                let gone = options.remove(*metas.get(k).ok_or("no such value")?);
                Ok(format!("removed {gone}"))
            }
            (Some(k), key, value) => {
                let j = *metas.get(k).ok_or("no such value")?;
                let (old_key, old_value) = values[k].clone();
                let key = key.unwrap_or(&old_key);
                if key != old_key && values.iter().any(|(k, _)| k == key) {
                    return Err(format!("there is a ${key} already"));
                }
                options[j] = format!("${key}={}", value.unwrap_or(&old_value));
                Ok(options[j].clone())
            }
            _ => Err("nothing to set".into()),
        }
    }

    /// The text a line's typing starts with: what it is now (a meta key or value), or empty
    /// (a number to type, a search).
    pub fn typing_start(&self, t: Typing) -> String {
        let owner = match self.menus.last() {
            Some(Menu::Meta(owner)) => *owner,
            _ => Owner::Level,
        };
        match t {
            Typing::MetaKey(k) => self.meta_values(owner).get(k).map_or(String::new(), |(key, _)| key.clone()),
            Typing::MetaValue(k) => self.meta_values(owner).get(k).map_or(String::new(), |(_, v)| v.clone()),
            Typing::Search => self.search.clone(),
            _ => String::new(),
        }
    }

    /// What `text` (typed into the find line) names: `sector 3` (or `s3`), `face 12`
    /// (`surface`, `f`), `vertex 7` (`v`), `light 2` (`l`), or an entity's name.
    pub fn find(&self, text: &str) -> Result<Selection, String> {
        let text = text.trim();
        let (word, number) = match text.find(|c: char| c.is_ascii_digit()) {
            Some(k) => (text[..k].trim().to_lowercase(), text[k..].trim().parse::<usize>().ok()),
            None => (text.to_lowercase(), None),
        };
        let s = match (word.as_str(), number) {
            ("s" | "sector", Some(k)) => Selection::Sector(k),
            ("f" | "face" | "surface", Some(k)) => Selection::Surface(k),
            ("v" | "vertex", Some(k)) => Selection::Vertex(k),
            ("l" | "light", Some(k)) => Selection::Light(k),
            _ => Selection::Entity(
                self.doc.entities.iter().position(|e| e.name == text).ok_or(format!("nothing called '{text}' (try sector 3, face 12)"))?,
            ),
        };
        if !self.exists(s) {
            return Err(format!("there is no {}", describe(s)));
        }
        Ok(s)
    }

    /// Where `s` is: the middle of its points.
    pub fn place_of(&self, s: Selection) -> Vec3 {
        match s {
            Selection::Sector(k) => {
                let (lo, hi) = self.sector_bounds(k);
                (lo + hi) * 0.5
            }
            Selection::Surface(f) => {
                let p = self.doc.surface_points(f);
                p.iter().copied().sum::<Vec3>() / p.len().max(1) as f32
            }
            Selection::Vertex(v) => self.doc.vertices[v],
            Selection::Entity(e) => self.doc.entities[e].position,
            Selection::Light(l) => self.doc.lights[l].position,
        }
    }

    /// Whether the editor's panel is over framebuffer point `at` (so clicks there aren't the
    /// view's).
    pub fn over_ui(&self, laid: &Laid, at: (f32, f32)) -> bool {
        self.confirm.is_some() || crate::gui::over(&laid.widgets, at)
    }

    /// The view's name, for the status line.
    pub fn view_name(&self) -> String {
        match self.view {
            ViewMode::Perspective => "3D".into(),
            ViewMode::Ortho(axis) => axis.name(),
        }
    }
}

/// How a selection is named in messages: `sector 3`, `face 12`.
pub fn describe(s: Selection) -> String {
    match s {
        Selection::Sector(k) => format!("sector {k}"),
        Selection::Surface(k) => format!("face {k}"),
        Selection::Vertex(k) => format!("vertex {k}"),
        Selection::Light(k) => format!("light {k}"),
        Selection::Entity(k) => format!("entity {k}"),
    }
}
