//! The material editor (the editor's fourth icon): the whole window, with the materials on
//! the left (a search, a new one) and the open one's rows on the right (`material_edit`'s
//! draft), each shader and input with what it does. The shader and the textures are picked
//! from lists of their own, with a search, a preview (the shader's description, the
//! texture) and Select. Mouse and keys alike: Up/Down and Enter, Left/Right to step a row,
//! Esc back.

use crate::editor_panel::ListState;
use crate::gui::{DIM, Look, STAT, TEXT, TITLE, Widget};
use crate::material_edit::{Draft, Known, Row};
use crate::materials::{SHADERS, shader_info};
use crate::ui::{Canvas, Rect};

/// What's picked from a list of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    Shader,
    Texture(u8),
}

/// What's being typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Typing {
    /// The materials' search.
    Search,
    /// The picker's search.
    PickerSearch,
    /// A row of the material (its name, a value).
    Row(Row),
}

/// What something on the screen does.
#[derive(Clone, Debug, PartialEq)]
pub enum MAct {
    Close,
    New,
    /// Opens material `id` of the library.
    Open(u32),
    Search,
    PickerSearch,
    /// A row clicked: typed, stepped, or its picker opened.
    Row(Row),
    /// A row stepped (the wheel, a right click).
    Step(Row),
    PickerBack,
    /// A choice in the picker, to preview.
    Pick(String),
    /// The picker's choice taken.
    Select,
    ScrollMaterials,
    ScrollPicker,
    ScrollRows,
}

#[derive(Clone, Debug, Default)]
pub struct MaterialScreen {
    pub search: String,
    pub list: ListState,
    /// The material's rows: their scroll, and the keys' row.
    pub rows: ListState,
    pub picker: Option<Picker>,
    /// The picker's choice so far, its search and its list.
    pub pick: Option<String>,
    pub picker_search: String,
    pub picker_list: ListState,
    pub typing: Option<(Typing, String)>,
}

/// What the screen shows, from outside it.
pub struct Ctx<'a> {
    /// The library's materials: id, name, shader.
    pub materials: &'a [(u32, String, String)],
    pub draft: Option<&'a Draft>,
    pub known: &'a Known,
    /// What texture slots can be (see `material_edit::texture_choices`).
    pub textures: &'a [String],
    pub width: usize,
    pub height: usize,
}

/// The screen laid out: its widgets, and the list the keys move in (its acts).
pub struct Laid {
    pub widgets: Vec<Widget<MAct>>,
    pub keys_list: Vec<MAct>,
}

/// `text` broken into lines of at most `width` characters, at spaces.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn matches(search: &str, name: &str) -> bool {
    search.is_empty() || name.to_lowercase().contains(&search.to_lowercase())
}

impl MaterialScreen {
    /// The typing text, if `t` is being typed.
    fn typed(&self, t: Typing) -> Option<&str> {
        match &self.typing {
            Some((typing, text)) if *typing == t => Some(text),
            _ => None,
        }
    }

    /// The shaders the picker lists (the search's).
    pub fn shaders(&self) -> Vec<&'static str> {
        SHADERS.iter().map(|s| s.name).filter(|n| matches(&self.picker_search, n)).collect()
    }

    pub fn layout(&self, ctx: &Ctx) -> Laid {
        let (w, h) = (ctx.width, ctx.height);
        let scale = crate::editor_panel::scale_for(h);
        let small = if scale > 1 { scale - 1 } else { 1 };
        let line = Canvas::line_height(scale);
        let sline = Canvas::line_height(small);
        let pad = 3 * scale;
        let mut ws: Vec<Widget<MAct>> = vec![Widget::new(Rect::new(0, 0, w, h), Look::Panel)];
        let mut keys_list = Vec::new();
        // The top: back to the editor, the title, what the last change said.
        let bw = Canvas::text_width("< Editor", scale) + 2 * pad;
        ws.push(Widget::new(Rect::new(pad, pad, bw, line), Look::Button { text: "< Editor".into(), on: false }).act(MAct::Close));
        ws.push(Widget::new(Rect::new(2 * pad + bw, pad + 2 * scale, w, line), Look::Label { text: "Material editor".into(), color: TITLE, scale }));
        let top = line + 3 * pad;
        // The materials, on the left.
        let lw = (w * 3 / 10).max(Canvas::text_width(&"m".repeat(20), scale)).min(w / 2);
        let mut y = top;
        let field = |ws: &mut Vec<Widget<MAct>>, x: usize, y: usize, width: usize, text: &str, placeholder: &str, focused: bool, act: MAct| {
            ws.push(Widget::new(Rect::new(x, y, width, line), Look::Field { text: text.into(), placeholder: placeholder.into(), focused }).act(act));
        };
        let search = self.typed(Typing::Search).unwrap_or(&self.search).to_string();
        field(&mut ws, pad, y, lw - 2 * pad, &search, "search materials", self.typed(Typing::Search).is_some(), MAct::Search);
        y += line + pad;
        ws.push(Widget::new(Rect::new(pad, y, lw - 2 * pad, line), Look::Button { text: "New material".into(), on: false }).act(MAct::New));
        y += line + pad;
        let open = ctx.draft.and_then(|d| d.original.clone());
        let shown: Vec<&(u32, String, String)> = ctx.materials.iter().filter(|(_, n, _)| matches(&self.search, n)).collect();
        let room = (h.saturating_sub(y + pad) / line).max(1);
        let first = self.list.scroll.min(shown.len().saturating_sub(room));
        ws.push(Widget::new(Rect::new(0, y, lw, room * line), Look::Frame(0)).wheel(MAct::ScrollMaterials));
        for (id, name, shader) in shown.iter().skip(first).take(room) {
            let hot = open.as_deref() == Some(name.as_str());
            // Its shader where it fits beside it.
            let fits = Canvas::text_width(&format!("{name}  {shader}"), scale) + 6 * scale <= lw;
            let shader = if fits { shader.clone() } else { String::new() };
            ws.push(
                Widget::new(Rect::new(0, y, lw, line), Look::Row { label: name.clone(), value: shader.clone(), hot, dim: false })
                    .act(MAct::Open(*id))
                    .wheel(MAct::ScrollMaterials),
            );
            y += line;
        }
        // The right: the material, or a picker.
        let rx = lw + pad;
        let rw = w.saturating_sub(rx + pad);
        let chars = rw / Canvas::text_width("m", small).max(1);
        let Some(draft) = ctx.draft else {
            ws.push(Widget::new(Rect::new(rx, top, rw, line), Look::Label { text: "Open a material, or make a new one".into(), color: DIM, scale }));
            return Laid { widgets: ws, keys_list };
        };
        match self.picker {
            Some(picker) => {
                let mut y = top;
                let bw = Canvas::text_width("< Back", scale) + 2 * pad;
                ws.push(Widget::new(Rect::new(rx, y, bw, line), Look::Button { text: "< Back".into(), on: false }).act(MAct::PickerBack));
                let sw = Canvas::text_width("Select", scale) + 2 * pad;
                ws.push(Widget::new(Rect::new(rx + rw - sw, y, sw, line), Look::Button { text: "Select".into(), on: self.pick.is_some() }).act(MAct::Select));
                let title = match picker {
                    Picker::Shader => "Shader program".to_string(),
                    Picker::Texture(k) => {
                        let slots = shader_info(&draft.def.shader).map_or(&[][..], |s| s.slots);
                        format!("Texture {k}: {}", slots.get(k as usize).unwrap_or(&""))
                    }
                };
                ws.push(Widget::new(Rect::new(rx + bw + pad, y + 2 * scale, rw, line), Look::Label { text: title, color: TITLE, scale }));
                y += line + pad;
                let search = self.typed(Typing::PickerSearch).unwrap_or(&self.picker_search).to_string();
                let half = rw / 2;
                field(&mut ws, rx, y, half - pad, &search, "search", self.typed(Typing::PickerSearch).is_some(), MAct::PickerSearch);
                y += line + pad;
                let choices: Vec<String> = match picker {
                    Picker::Shader => self.shaders().into_iter().map(String::from).collect(),
                    Picker::Texture(_) => {
                        std::iter::once("none".to_string()).chain(ctx.textures.iter().filter(|t| matches(&self.picker_search, t)).cloned()).collect()
                    }
                };
                let room = (h.saturating_sub(y + pad) / line).max(1);
                let first = self.picker_list.scroll.min(choices.len().saturating_sub(room));
                ws.push(Widget::new(Rect::new(rx, y, half, room * line), Look::Frame(0)).wheel(MAct::ScrollPicker));
                let list_top = y;
                for (k, c) in choices.iter().enumerate().skip(first).take(room) {
                    let hot = self.pick.as_deref() == Some(c.as_str()) || self.picker_list.cursor == Some(k);
                    ws.push(
                        Widget::new(Rect::new(rx, y, half, line), Look::Row { label: c.clone(), value: String::new(), hot, dim: false })
                            .act(MAct::Pick(c.clone()))
                            .wheel(MAct::ScrollPicker),
                    );
                    y += line;
                }
                keys_list = choices.iter().map(|c| MAct::Pick(c.clone())).collect();
                // The preview, right of the list.
                let px = rx + half + pad;
                let pw = rw.saturating_sub(half + pad);
                let mut y = list_top;
                match (picker, &self.pick) {
                    (Picker::Shader, Some(name)) => {
                        if let Some(info) = shader_info(name) {
                            ws.push(Widget::new(Rect::new(px, y, pw, line), Look::Label { text: info.name.into(), color: TITLE, scale }));
                            y += line;
                            let pchars = pw / Canvas::text_width("m", small).max(1);
                            for l in wrap(info.about, pchars) {
                                ws.push(Widget::new(Rect::new(px, y, pw, sline), Look::Label { text: l, color: TEXT, scale: small }));
                                y += sline;
                            }
                            y += sline;
                            let slots = if info.slots.is_empty() { "none".to_string() } else { info.slots.join(", ") };
                            for l in wrap(&format!("Textures: {slots}"), pchars) {
                                ws.push(Widget::new(Rect::new(px, y, pw, sline), Look::Label { text: l, color: STAT, scale: small }));
                                y += sline;
                            }
                            y += sline;
                            for input in info.all_inputs() {
                                for l in wrap(&format!("{} ({}): {}", input.name, input.ty.name(), input.about), pchars) {
                                    ws.push(Widget::new(Rect::new(px, y, pw, sline), Look::Label { text: l, color: STAT, scale: small }));
                                    y += sline;
                                }
                            }
                        }
                    }
                    (Picker::Texture(_), Some(name)) if name != "none" => {
                        let side = pw.min(h.saturating_sub(y + 2 * line)).min(256 * scale / 2);
                        ws.push(Widget::new(Rect::new(px, y, side, side + Canvas::line_height(1)), Look::Thumb { key: name.clone(), label: name.clone(), on: false }));
                    }
                    _ => {
                        ws.push(Widget::new(Rect::new(px, y, pw, line), Look::Label { text: "Click one to see it".into(), color: DIM, scale }));
                    }
                }
            }
            None => {
                // The material's rows, each shader and input with what it does under it.
                let rows = draft.rows();
                let info = shader_info(&draft.def.shader);
                let mut y = top;
                let first = self.rows.scroll.min(rows.len().saturating_sub(1));
                ws.push(Widget::new(Rect::new(rx, top, rw, h - top), Look::Frame(0)).wheel(MAct::ScrollRows));
                for (k, &row) in rows.iter().enumerate().skip(first) {
                    let about: Vec<String> = match row {
                        Row::Shader => info.map_or(Vec::new(), |i| wrap(i.about, chars)),
                        Row::Source(i) => info.map_or(Vec::new(), |s| wrap(s.input(i as usize).0.about, chars)),
                        _ => Vec::new(),
                    };
                    if y + line + about.len() * sline > h {
                        break;
                    }
                    let value = match self.typed(Typing::Row(row)) {
                        Some(text) => format!("{text}_"),
                        None => draft.value(row, ctx.known).unwrap_or_default(),
                    };
                    let value = match row {
                        Row::Shader | Row::Texture(_) => format!("{value} >"),
                        _ => value,
                    };
                    let hot = self.rows.cursor == Some(k);
                    let look = Look::Row { label: draft.label(row), value, hot, dim: false };
                    let r = Rect::new(rx, y, rw, line);
                    let w = Widget::new(r, look).act(MAct::Row(row)).wheel(MAct::ScrollRows);
                    // The wheel steps what steps.
                    let w = if matches!(row, Row::Sampler(_) | Row::Source(_)) { w.wheel(MAct::Step(row)) } else { w };
                    ws.push(w);
                    y += line;
                    for l in about {
                        ws.push(Widget::new(Rect::new(rx + 3 * pad, y, rw, sline), Look::Label { text: l, color: STAT, scale: small }));
                        y += sline;
                    }
                }
                keys_list = rows.into_iter().map(MAct::Row).collect();
            }
        }
        Laid { widgets: ws, keys_list }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_wrap_at_spaces_and_every_shader_and_input_has_one() {
        assert_eq!(wrap("a bb ccc dddd", 6), ["a bb", "ccc", "dddd"]);
        for s in SHADERS {
            assert!(!s.about.is_empty(), "{}", s.name);
            for i in s.all_inputs() {
                assert!(!i.about.is_empty(), "{}: {}", s.name, i.name);
            }
        }
    }
}
