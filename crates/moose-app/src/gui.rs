//! The editor's widgets: a list of what's on screen (rows, buttons, fields, check boxes,
//! icons, texture previews), each with where it is and what clicking or scrolling it does.
//! The editor lays the list out from its state; input finds the widget under the pointer in
//! it, and drawing draws it, so what's drawn is what's clicked.

use crate::ui::{Canvas, Rect};

/// What a widget looks like.
#[derive(Clone, Debug, PartialEq)]
pub enum Look {
    /// A panel's background (the frame darkened).
    Panel,
    /// Text that does nothing.
    Label { text: String, color: u32, scale: usize },
    /// A row: its label on the left, its value on the right; `hot` (the keys' row in a
    /// list) as if pointed at.
    Row { label: String, value: String, hot: bool, dim: bool },
    /// A boxed button.
    Button { text: String, on: bool },
    /// A line of text being typed (or to type into, when clicked).
    Field { text: String, placeholder: String, focused: bool },
    /// A check box and its label, a value on the right.
    Check { label: String, value: String, on: bool, hot: bool },
    Icon { icon: Icon, on: bool },
    /// A texture's preview (from `Editor::thumbs`), and what it is under it.
    Thumb { key: String, label: String, on: bool },
    /// A rectangle's outline (a box being dragged).
    Frame(u32),
}

/// The icons on the editor's right edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Add,
    World,
    Gear,
    Material,
    Sector,
    Face,
    Vertex,
}

/// Something on screen.
#[derive(Clone, Debug)]
pub struct Widget<A> {
    pub rect: Rect,
    pub look: Look,
    /// What a click does.
    pub act: Option<A>,
    /// What the wheel does over it (with how far it turned).
    pub wheel: Option<A>,
}

impl<A: Clone> Widget<A> {
    pub fn new(rect: Rect, look: Look) -> Widget<A> {
        Widget { rect, look, act: None, wheel: None }
    }

    pub fn act(mut self, act: A) -> Widget<A> {
        self.act = Some(act);
        self
    }

    pub fn wheel(mut self, act: A) -> Widget<A> {
        self.wheel = Some(act);
        self
    }
}

/// The topmost widget under `at` that does something when clicked (or, with `wheel`, when
/// scrolled), last drawn first.
pub fn under<A>(widgets: &[Widget<A>], at: (f32, f32), wheel: bool) -> Option<&Widget<A>> {
    widgets
        .iter()
        .rev()
        .find(|w| w.rect.contains(at) && if wheel { w.wheel.is_some() } else { w.act.is_some() })
}

/// Whether `at` is over any widget (the pointer is on the editor's panels, not the view).
pub fn over<A>(widgets: &[Widget<A>], at: (f32, f32)) -> bool {
    widgets.iter().any(|w| w.rect.contains(at) && !matches!(w.look, Look::Frame(_)))
}

pub const TEXT: u32 = 0xD8_D8_D8;
pub const DIM: u32 = 0x8C_8C_8C;
pub const TITLE: u32 = 0xFF_D4_7A;
/// Stats: what a selection is, not something to change.
pub const STAT: u32 = 0x7C_C4_D8;
const HOT: u32 = 0x3A_4E_6E;
const HOVER: u32 = 0x2A_34_44;
const BORDER: u32 = 0x5A_62_70;
const ON: u32 = 0x50_70_A8;

/// A texture's preview: its pixels (XRGB), `size` square.
#[derive(Clone, Debug)]
pub struct Thumb {
    pub size: usize,
    pub pixels: Vec<u32>,
}

/// Draws `widgets`, highlighting what the pointer (`cursor`) is over; previews come from
/// `thumb`.
pub fn draw<A>(canvas: &mut Canvas, widgets: &[Widget<A>], cursor: Option<(f32, f32)>, scale: usize, thumb: &dyn Fn(&str) -> Option<Thumb>) {
    let pointed = |w: &Widget<A>| w.act.is_some() && cursor.is_some_and(|c| w.rect.contains(c));
    for w in widgets {
        let r = w.rect;
        let hover = pointed(w);
        let text_y = r.y + scale * 2;
        match &w.look {
            Look::Panel => canvas.shade(r.x, r.y, r.w, r.h, 70),
            Look::Label { text, color, scale } => canvas.text(r.x, r.y, text, *color, *scale),
            Look::Row { label, value, hot, dim } => {
                if *hot {
                    canvas.fill(r.x, r.y, r.w, r.h, HOT);
                } else if hover {
                    canvas.fill(r.x, r.y, r.w, r.h, HOVER);
                }
                let color = if *dim || w.act.is_none() && w.wheel.is_none() { DIM } else { TEXT };
                let pad = 3 * scale;
                canvas.text(r.x + pad, text_y, label, color, scale);
                let vw = Canvas::text_width(value, scale);
                canvas.text((r.x + r.w).saturating_sub(pad + vw), text_y, value, color, scale);
            }
            Look::Button { text, on } => {
                canvas.fill(r.x, r.y, r.w, r.h, if *on { ON } else if hover { HOT } else { 0x1E_22_2A });
                outline(canvas, r, BORDER);
                let tw = Canvas::text_width(text, scale);
                canvas.text(r.x + r.w.saturating_sub(tw) / 2, text_y, text, TEXT, scale);
            }
            Look::Field { text, placeholder, focused } => {
                canvas.fill(r.x, r.y, r.w, r.h, 0x0C_0E_12);
                outline(canvas, r, if *focused { TITLE } else if hover { TEXT } else { BORDER });
                let pad = 3 * scale;
                let shown = if text.is_empty() && !*focused { placeholder.as_str() } else { text.as_str() };
                // The end of a long text shows.
                let room = (r.w.saturating_sub(2 * pad) / Canvas::text_width("m", scale)).max(1);
                let chars: Vec<char> = shown.chars().collect();
                // A placeholder from its start; typed text to its end.
                let tail: String = if text.is_empty() && !*focused {
                    chars.iter().take(room).collect()
                } else {
                    chars[chars.len().saturating_sub(room - usize::from(*focused))..].iter().collect()
                };
                let color = if text.is_empty() && !*focused { DIM } else { TEXT };
                let shown = if *focused { format!("{tail}_") } else { tail };
                canvas.text(r.x + pad, text_y, &shown, color, scale);
            }
            Look::Check { label, value, on, hot } => {
                if *hot {
                    canvas.fill(r.x, r.y, r.w, r.h, HOT);
                } else if hover {
                    canvas.fill(r.x, r.y, r.w, r.h, HOVER);
                }
                let pad = 3 * scale;
                let side = 7 * scale;
                let (bx, by) = (r.x + pad, r.y + (r.h.saturating_sub(side)) / 2);
                outline(canvas, Rect::new(bx, by, side, side), TEXT);
                if *on {
                    canvas.fill(bx + 2 * scale, by + 2 * scale, side - 4 * scale, side - 4 * scale, TITLE);
                }
                canvas.text(bx + side + pad, text_y, label, TEXT, scale);
                let vw = Canvas::text_width(value, scale);
                canvas.text((r.x + r.w).saturating_sub(pad + vw), text_y, value, DIM, scale);
            }
            Look::Icon { icon, on } => {
                canvas.fill(r.x, r.y, r.w, r.h, if *on { ON } else if hover { HOT } else { 0x16_19_1F });
                outline(canvas, r, BORDER);
                draw_icon(canvas, *icon, r, scale);
            }
            Look::Thumb { key, label, on } => {
                let lh = Canvas::line_height(1);
                let side = r.w.min(r.h.saturating_sub(lh));
                match thumb(key) {
                    Some(t) => blit(canvas, &t, r.x, r.y, side),
                    None => {
                        canvas.fill(r.x, r.y, side, side, 0x20_20_24);
                        canvas.text(r.x + 2, r.y + side / 2, "?", DIM, 1);
                    }
                }
                outline(canvas, Rect::new(r.x, r.y, side, side), if *on { TITLE } else { BORDER });
                let shown: String = label.chars().take(side / Canvas::text_width("m", 1)).collect();
                canvas.text(r.x, r.y + side + 1, &shown, DIM, 1);
            }
            Look::Frame(color) => outline(canvas, r, *color),
        }
    }
}

/// A rectangle's one-pixel border.
pub fn outline(canvas: &mut Canvas, r: Rect, color: u32) {
    if r.w == 0 || r.h == 0 {
        return;
    }
    canvas.fill(r.x, r.y, r.w, 1, color);
    canvas.fill(r.x, r.y + r.h - 1, r.w, 1, color);
    canvas.fill(r.x, r.y, 1, r.h, color);
    canvas.fill(r.x + r.w - 1, r.y, 1, r.h, color);
}

/// Draws preview `t` with its top left at `(x, y)`, `side` pixels square (nearest texel).
fn blit(canvas: &mut Canvas, t: &Thumb, x: usize, y: usize, side: usize) {
    for row in 0..side {
        let ty = row * t.size / side.max(1);
        let py = y + row;
        if py >= canvas.height {
            break;
        }
        for col in 0..side {
            let px = x + col;
            if px >= canvas.width {
                break;
            }
            let tx = col * t.size / side.max(1);
            canvas.pixels[py * canvas.width + px] = t.pixels[ty * t.size + tx];
        }
    }
}

/// The icons, 12 by 12 (`#` lit). The material icon is drawn: a square border around a
/// 1-bit gradient, dithered.
const ICON_ADD: &str = "\
............
.....##.....
.....##.....
.....##.....
.....##.....
.##########.
.##########.
.....##.....
.....##.....
.....##.....
.....##.....
............";
const ICON_WORLD: &str = "\
....####....
..##.##.##..
.#...##...#.
.#..#..#..#.
############
#...#..#...#
#...#..#...#
############
.#..#..#..#.
.#...##...#.
..##.##.##..
....####....";
const ICON_GEAR: &str = "\
.....##.....
..#..##..#..
.###.##.###.
..########..
...##..##...
####....####
####....####
...##..##...
..########..
.###.##.###.
..#..##..#..
.....##.....";
const ICON_SECTOR: &str = "\
...########.
..#.......##
.#.......#.#
#########..#
#.......#..#
#.......#..#
#.......#..#
#.......#..#
#.......#.#.
#.......##..
#########...
............";
const ICON_FACE: &str = "\
............
...#########
..#########.
..#########.
.#########..
.#########..
#########...
#########...
............
............
............
............";
const ICON_VERTEX: &str = "\
............
.##.......#.
.##......#..
........#...
.......#....
......#.....
.....#......
....#.......
...#........
..#.........
.#........##
..........##";

/// Draws `icon` in the middle of `r`, each icon pixel `scale` pixels square.
fn draw_icon(canvas: &mut Canvas, icon: Icon, r: Rect, scale: usize) {
    let size = 12 * scale;
    let (x0, y0) = (r.x + r.w.saturating_sub(size) / 2, r.y + r.h.saturating_sub(size) / 2);
    let lit = |x: usize, y: usize| -> bool {
        let art = match icon {
            Icon::Add => ICON_ADD,
            Icon::World => ICON_WORLD,
            Icon::Gear => ICON_GEAR,
            Icon::Sector => ICON_SECTOR,
            Icon::Face => ICON_FACE,
            Icon::Vertex => ICON_VERTEX,
            Icon::Material => {
                // The border, then a gradient from all lit (top left) to none (bottom
                // right), as a 4x4 ordered dither.
                if x == 0 || y == 0 || x == 11 || y == 11 {
                    return true;
                }
                const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
                let t = (x + y - 2) as f32 / 18.0;
                return (BAYER[y % 4][x % 4] as f32 + 0.5) / 16.0 >= t;
            }
        };
        art.lines().nth(y).and_then(|l| l.as_bytes().get(x)).is_some_and(|&c| c == b'#')
    };
    for y in 0..12 {
        for x in 0..12 {
            if lit(x, y) {
                canvas.fill(x0 + x * scale, y0 + y * scale, scale, scale, TEXT);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_are_twelve_square_and_the_material_one_fades_down_and_right() {
        for art in [ICON_ADD, ICON_WORLD, ICON_GEAR, ICON_SECTOR, ICON_FACE, ICON_VERTEX] {
            let lines: Vec<&str> = art.lines().collect();
            assert_eq!(lines.len(), 12);
            assert!(lines.iter().all(|l| l.len() == 12), "{art}");
        }
        let mut pixels = vec![0u32; 24 * 24];
        let mut canvas = Canvas { pixels: &mut pixels, width: 24, height: 24 };
        draw_icon(&mut canvas, Icon::Material, Rect::new(0, 0, 24, 24), 2);
        let lit = |x0: usize, y0: usize| (y0..y0 + 8).flat_map(|y| (x0..x0 + 8).map(move |x| (x, y))).filter(|&(x, y)| pixels[y * 24 + x] != 0).count();
        assert!(lit(4, 4) > lit(12, 12));
    }

    #[test]
    fn the_widget_under_the_pointer_is_the_last_drawn_that_does_something() {
        let ws = vec![
            Widget::new(Rect::new(0, 0, 100, 100), Look::Panel),
            Widget::new(Rect::new(10, 10, 50, 10), Look::Button { text: "a".into(), on: false }).act(1),
            Widget::new(Rect::new(10, 10, 50, 10), Look::Button { text: "b".into(), on: false }).act(2),
        ];
        assert_eq!(under(&ws, (20.0, 15.0), false).and_then(|w| w.act), Some(2));
        assert!(under(&ws, (80.0, 80.0), false).is_none());
        assert!(over(&ws, (80.0, 80.0)));
    }
}
