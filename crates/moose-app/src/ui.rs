//! On-screen text: a small pixel font, and the options menu and debug HUD drawn with it
//! over a finished frame.

use std::sync::LazyLock;

/// Glyph size in font pixels: 7 rows above the baseline, and 2 below it for descenders.
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 9;

/// The font: each line a character, a space, then its rows of 5 (`#` on, `.` off),
/// separated by `/`: 7 down to the baseline, and 2 more for a descender. ASCII 32 to 126,
/// plus `°`.
const FONT: &str = "
  ...../...../...../...../...../...../.....
! ..#../..#../..#../..#../..#../...../..#..
\" .#.#./.#.#./...../...../...../...../.....
# .#.#./.#.#./#####/.#.#./#####/.#.#./.#.#.
$ ..#../.####/#.#../.###./..#.#/####./..#..
% ##.../##..#/...#./..#../.#.../#..##/...##
& .##../#..#./#.#../.#.../#.#.#/#..#./.##.#
' ..#../..#../.#.../...../...../...../.....
( ...#./..#../.#.../.#.../.#.../..#../...#.
) .#.../..#../...#./...#./...#./..#../.#...
* ...../..#../#.#.#/.###./#.#.#/..#../.....
+ ...../..#../..#../#####/..#../..#../.....
, ...../...../...../...../.##../..#../.#...
- ...../...../...../#####/...../...../.....
. ...../...../...../...../...../.##../.##..
/ ...../....#/...#./..#../.#.../#..../.....
0 .###./#...#/#..##/#.#.#/##..#/#...#/.###.
1 ..#../.##../..#../..#../..#../..#../.###.
2 .###./#...#/....#/...#./..#../.#.../#####
3 #####/...#./..#../...#./....#/#...#/.###.
4 ...#./..##./.#.#./#..#./#####/...#./...#.
5 #####/#..../####./....#/....#/#...#/.###.
6 ..##./.#.../#..../####./#...#/#...#/.###.
7 #####/....#/...#./..#../.#.../.#.../.#...
8 .###./#...#/#...#/.###./#...#/#...#/.###.
9 .###./#...#/#...#/.####/....#/...#./.##..
: ...../.##../.##../...../.##../.##../.....
; ...../.##../.##../...../.##../..#../.#...
< ...#./..#../.#.../#..../.#.../..#../...#.
= ...../...../#####/...../#####/...../.....
> .#.../..#../...#./....#/...#./..#../.#...
? .###./#...#/....#/...#./..#../...../..#..
@ .###./#...#/....#/.##.#/#.#.#/#.#.#/.###.
A .###./#...#/#...#/#####/#...#/#...#/#...#
B ####./#...#/#...#/####./#...#/#...#/####.
C .###./#...#/#..../#..../#..../#...#/.###.
D ###../#..#./#...#/#...#/#...#/#..#./###..
E #####/#..../#..../####./#..../#..../#####
F #####/#..../#..../####./#..../#..../#....
G .###./#...#/#..../#.###/#...#/#...#/.####
H #...#/#...#/#...#/#####/#...#/#...#/#...#
I .###./..#../..#../..#../..#../..#../.###.
J ..###/...#./...#./...#./...#./#..#./.##..
K #...#/#..#./#.#../##.../#.#../#..#./#...#
L #..../#..../#..../#..../#..../#..../#####
M #...#/##.##/#.#.#/#.#.#/#...#/#...#/#...#
N #...#/#...#/##..#/#.#.#/#..##/#...#/#...#
O .###./#...#/#...#/#...#/#...#/#...#/.###.
P ####./#...#/#...#/####./#..../#..../#....
Q .###./#...#/#...#/#...#/#.#.#/#..#./.##.#
R ####./#...#/#...#/####./#.#../#..#./#...#
S .####/#..../#..../.###./....#/....#/####.
T #####/..#../..#../..#../..#../..#../..#..
U #...#/#...#/#...#/#...#/#...#/#...#/.###.
V #...#/#...#/#...#/#...#/#...#/.#.#./..#..
W #...#/#...#/#...#/#.#.#/#.#.#/#.#.#/.#.#.
X #...#/#...#/.#.#./..#../.#.#./#...#/#...#
Y #...#/#...#/.#.#./..#../..#../..#../..#..
Z #####/....#/...#./..#../.#.../#..../#####
[ .###./.#.../.#.../.#.../.#.../.#.../.###.
\\ ...../#..../.#.../..#../...#./....#/.....
] .###./...#./...#./...#./...#./...#./.###.
^ ..#../.#.#./#...#/...../...../...../.....
_ ...../...../...../...../...../...../#####
` .#.../..#../...#./...../...../...../.....
a ...../...../.###./....#/.####/#...#/.####
b #..../#..../#.##./##..#/#...#/#...#/####.
c ...../...../.###./#..../#..../#...#/.###.
d ....#/....#/.##.#/#..##/#...#/#...#/.####
e ...../...../.###./#...#/#####/#..../.###.
f ..##./.#..#/.#.../###../.#.../.#.../.#...
g ...../...../.####/#...#/#...#/#...#/.####/....#/.###.
h #..../#..../#.##./##..#/#...#/#...#/#...#
i ..#../...../.##../..#../..#../..#../.###.
j ...#./...../..##./...#./...#./...#./...#./#..#./.##..
k #..../#..../#..#./#.#../##.../#.#../#..#.
l .##../..#../..#../..#../..#../..#../.###.
m ...../...../##.#./#.#.#/#.#.#/#...#/#...#
n ...../...../#.##./##..#/#...#/#...#/#...#
o ...../...../.###./#...#/#...#/#...#/.###.
p ...../...../####./#...#/#...#/#...#/####./#..../#....
q ...../...../.####/#...#/#...#/#...#/.####/....#/....#
r ...../...../#.##./##..#/#..../#..../#....
s ...../...../.###./#..../.###./....#/####.
t .#.../.#.../###../.#.../.#.../.#..#/..##.
u ...../...../#...#/#...#/#...#/#..##/.##.#
v ...../...../#...#/#...#/#...#/.#.#./..#..
w ...../...../#...#/#...#/#.#.#/#.#.#/.#.#.
x ...../...../#...#/.#.#./..#../.#.#./#...#
y ...../...../#...#/#...#/#...#/#...#/.####/....#/.###.
z ...../...../#####/...#./..#../.#.../#####
{ ...#./..#../..#../.#.../..#../..#../...#.
| ..#../..#../..#../..#../..#../..#../..#..
} .#.../..#../..#../...#./..#../..#../.#...
~ ...../...../.#.../#.#.#/...#./...../.....
° .##../#..#./.##../...../...../...../.....
";

/// Each glyph's rows (bit 4 is the leftmost column), by character.
static GLYPHS: LazyLock<Vec<(char, [u8; GLYPH_H])>> = LazyLock::new(|| {
    FONT.lines()
        .filter_map(|line| {
            let mut chars = line.chars();
            let c = chars.next()?;
            let mut rows = [0u8; GLYPH_H];
            let given = chars.as_str().trim_start().split('/');
            for (k, row) in given.enumerate() {
                *rows.get_mut(k)? = row.chars().fold(0u8, |bits, p| bits << 1 | (p == '#') as u8);
            }
            Some((c, rows))
        })
        .collect()
});

fn glyph(c: char) -> [u8; GLYPH_H] {
    GLYPHS
        .iter()
        .find(|&&(g, _)| g == c)
        .map_or_else(|| glyph('?'), |&(_, rows)| rows)
}

/// A framebuffer to draw text and panels on: `width` x `height` XRGB pixels.
pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: usize,
    pub height: usize,
}

impl Canvas<'_> {
    /// How wide `text` is drawn at `scale`, in pixels.
    pub fn text_width(text: &str, scale: usize) -> usize {
        text.chars().count() * (GLYPH_W + 1) * scale
    }

    /// Line height at `scale`, in pixels.
    pub fn line_height(scale: usize) -> usize {
        (GLYPH_H + 2) * scale
    }

    /// Draws `text` with its top left at `(x, y)`, each font pixel `scale` pixels
    /// square, in `color`, with a dark shadow one pixel down and right for legibility.
    pub fn text(&mut self, x: usize, y: usize, text: &str, color: u32, scale: usize) {
        for (shadow, color) in [(1, 0x00_0000), (0, color)] {
            for (i, c) in text.chars().enumerate() {
                let rows = glyph(c);
                let gx = x + i * (GLYPH_W + 1) * scale + shadow;
                for (row, bits) in rows.iter().enumerate() {
                    for col in 0..GLYPH_W {
                        if bits >> (GLYPH_W - 1 - col) & 1 != 0 {
                            self.fill(gx + col * scale, y + shadow + row * scale, scale, scale, color);
                        }
                    }
                }
            }
        }
    }

    /// Fills a rectangle (clipped to the canvas).
    pub fn fill(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        for row in y.min(self.height)..(y + h).min(self.height) {
            let line = &mut self.pixels[row * self.width..(row + 1) * self.width];
            for p in &mut line[x.min(self.width)..(x + w).min(self.width)] {
                *p = color;
            }
        }
    }

    /// Draws a one-pixel line from `(x0, y0)` to `(x1, y1)` (pixel coordinates, clipped to
    /// the canvas).
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
        let (dx, dy) = (x1 - x0, y1 - y0);
        let steps = dx.abs().max(dy.abs()).ceil().max(1.0);
        if !steps.is_finite() || steps > 100_000.0 {
            return;
        }
        let (sx, sy) = (dx / steps, dy / steps);
        for i in 0..=steps as usize {
            let (x, y) = (x0 + sx * i as f32, y0 + sy * i as f32);
            if x >= 0.0 && y >= 0.0 && (x as usize) < self.width && (y as usize) < self.height {
                self.pixels[y as usize * self.width + x as usize] = color;
            }
        }
    }

    /// Fills a `size` pixel square centered on `(x, y)` (clipped to the canvas).
    pub fn fill_centered(&mut self, x: f32, y: f32, size: usize, color: u32) {
        let half = size as f32 / 2.0;
        let (left, top) = ((x - half).round(), (y - half).round());
        if left + size as f32 <= 0.0 || top + size as f32 <= 0.0 {
            return;
        }
        let (l, t) = (left.max(0.0) as usize, top.max(0.0) as usize);
        let (w, h) = ((left + size as f32) as usize - l, (top + size as f32) as usize - t);
        self.fill(l, t, w, h, color);
    }

    /// Darkens a rectangle (clipped to the canvas) to `keep` 256ths of its brightness.
    pub fn shade(&mut self, x: usize, y: usize, w: usize, h: usize, keep: u32) {
        for row in y.min(self.height)..(y + h).min(self.height) {
            let line = &mut self.pixels[row * self.width..(row + 1) * self.width];
            for p in &mut line[x.min(self.width)..(x + w).min(self.width)] {
                let c = *p;
                let channel = |shift: u32| (((c >> shift & 0xFF) * keep) >> 8) << shift;
                *p = channel(16) | channel(8) | channel(0);
            }
        }
    }
}

/// One row of a menu page: a label and, for a setting, its value.
pub struct Row {
    pub label: String,
    pub value: Option<String>,
}

/// Colors of the menu and HUD.
const TITLE: u32 = 0xFF_D4_7A;
const TEXT: u32 = 0xD8_D8_D8;
const DIM: u32 = 0x8C_8C_8C;
const SELECTED: u32 = 0xFF_FF_FF;
const HIGHLIGHT: u32 = 0x3A_4E_6E;

/// Draws a menu page over the frame: the frame darkened, and a panel in the middle with
/// its title, its rows (the selected one highlighted, values on the right), notes below
/// them, and a line of key hints.
pub fn draw_menu(canvas: &mut Canvas, title: &str, rows: &[Row], selected: usize, notes: &[&str], hint: &str) {
    let scale = if canvas.height >= 600 { 2 } else { 1 };
    let line = Canvas::line_height(scale);
    canvas.shade(0, 0, canvas.width, canvas.height, 90);
    let label_w = rows.iter().map(|r| Canvas::text_width(&r.label, scale)).max().unwrap_or(0);
    let value_w = rows
        .iter()
        .filter_map(|r| r.value.as_ref())
        .map(|v| Canvas::text_width(v, scale))
        .max()
        .unwrap_or(0);
    let notes_w = notes.iter().chain([&hint]).map(|n| Canvas::text_width(n, scale)).max().unwrap_or(0);
    let pad = 8 * scale;
    let width = (label_w + 12 * scale + value_w).max(notes_w).max(Canvas::text_width(title, scale)) + 2 * pad;
    let spacer = if notes.is_empty() { 0 } else { 1 };
    let height = line * (rows.len() + notes.len() + spacer + 4) + 2 * pad;
    let (x0, y0) = (
        canvas.width.saturating_sub(width) / 2,
        canvas.height.saturating_sub(height) / 2,
    );
    canvas.fill(x0, y0, width, height, 0x14_16_1C);
    canvas.text(x0 + pad, y0 + pad, title, TITLE, scale);
    let mut y = y0 + pad + line * 2;
    for (i, row) in rows.iter().enumerate() {
        let on = i == selected;
        if on {
            canvas.fill(x0 + pad / 2, y - scale * 2, width - pad, line, HIGHLIGHT);
        }
        let color = if on { SELECTED } else { TEXT };
        canvas.text(x0 + pad, y, &row.label, color, scale);
        if let Some(value) = &row.value {
            let vx = x0 + width - pad - Canvas::text_width(value, scale);
            canvas.text(vx, y, value, color, scale);
        }
        y += line;
    }
    y += line;
    for note in notes {
        canvas.text(x0 + pad, y, note, DIM, scale);
        y += line;
    }
    canvas.text(x0 + pad, y0 + height - pad - line + 2 * scale, hint, DIM, scale);
}

/// Draws the debug HUD's lines in the top left corner, on a dark band.
pub fn draw_hud(canvas: &mut Canvas, lines: &[String]) {
    let scale = if canvas.height >= 600 { 2 } else { 1 };
    let line = Canvas::line_height(scale);
    let pad = 4 * scale;
    let width = lines.iter().map(|l| Canvas::text_width(l, scale)).max().unwrap_or(0) + 2 * pad;
    canvas.shade(0, 0, width, line * lines.len() + 2 * pad, 110);
    for (i, l) in lines.iter().enumerate() {
        canvas.text(pad, pad + i * line, l, TEXT, scale);
    }
}

/// A short note at the bottom of the screen, centered (a hotkey's new setting).
pub fn draw_toast(canvas: &mut Canvas, text: &str) {
    let scale = if canvas.height >= 600 { 2 } else { 1 };
    let line = Canvas::line_height(scale);
    let pad = 6 * scale;
    let width = Canvas::text_width(text, scale) + 2 * pad;
    let (x, y) = (canvas.width.saturating_sub(width) / 2, canvas.height.saturating_sub(line + 2 * pad + 16 * scale));
    canvas.shade(x, y, width, line + 2 * pad, 140);
    canvas.text(x + pad, y + pad, text, TEXT, scale);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_printable_character_has_a_glyph() {
        for c in (32u8..127).map(char::from).chain(['°']) {
            assert!(GLYPHS.iter().any(|&(g, _)| g == c), "no glyph for {c:?}");
        }
        assert_eq!(GLYPHS.len(), 96, "one glyph per character");
        // Descenders only where they belong.
        for &(c, rows) in GLYPHS.iter() {
            assert_eq!(rows[7] | rows[8] != 0, "gjpqy".contains(c), "{c:?}");
        }
    }
}
