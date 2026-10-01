//! Bakes the brick wall's bump data, the test surface for bump and normal mapping, from
//! its height, `brick_wall_height.png` (grey: 0 in the mortar, the bricks rising over 2
//! texels from their edges to 1; see `moose_assets::bump::bevel_heights`):
//!
//! - `brick_wall.png`'s alpha: the height's normals, packed (see `moose_assets::bump::pack`),
//!   for the radiosity basis shader, which reads its bumps there.
//! - `brick_wall_normal.png`: a tangent-space normal map, `(n + 1) / 2` in its red, green
//!   and blue (its alpha 255), for the normal map shader, which leaves the brick texture's
//!   alpha free.
//!
//! Without the height file, it is made from `brick_wall.png`'s alpha as a mask of the bricks
//! (above 0) and the mortar (0). Run from the repository:
//! `cargo run -p moose-assets --example bake_brick`.

use std::path::Path;

use moose_assets::Assets;
use moose_assets::bump::{bevel_heights, normals, pack};

/// How many texels the bricks' edges round over.
const BEVEL: f32 = 2.0;
/// How steep the normals are: texels of height per unit of height (bricks stand this many
/// texels proud of the mortar).
const STRENGTH: f32 = 2.0;

fn main() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let textures = root.join("textures");
    let mut assets = Assets::new(&root);
    let id = assets.load_texture("brick_wall.png").map_err(|e| e.to_string())?;
    let base = assets.texture(id).base().clone();
    let (w, h) = (base.width, base.height);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    // The height: its file's, or made from the mask.
    let heights: Vec<f32> = match assets.load_texture("brick_wall_height.png") {
        Ok(id) => assets.texture(id).base().texels.iter().map(|t| (t & 255) as f32 / 255.0).collect(),
        Err(_) => {
            let mask: Vec<bool> = base.texels.iter().map(|t| t >> 24 > 0).collect();
            let heights = bevel_heights(&mask, w, h, BEVEL);
            let grey: Vec<u8> = heights.iter().map(|&v| byte(v)).collect();
            write(&textures.join("brick_wall_height.png"), w, h, &grey, png::ColorType::Grayscale)?;
            heights
        }
    };
    let n = normals(&heights, w, h, STRENGTH);
    let color: Vec<u8> = base
        .texels
        .iter()
        .zip(&n)
        .flat_map(|(&t, &n)| [(t >> 16) as u8, (t >> 8) as u8, t as u8, pack(n)])
        .collect();
    let normal: Vec<u8> = n
        .iter()
        .flat_map(|n| [byte(n[0] * 0.5 + 0.5), byte(n[1] * 0.5 + 0.5), byte(n[2] * 0.5 + 0.5), 255])
        .collect();
    write(&textures.join("brick_wall.png"), w, h, &color, png::ColorType::Rgba)?;
    write(&textures.join("brick_wall_normal.png"), w, h, &normal, png::ColorType::Rgba)?;
    println!("baked brick_wall.png's packed normals and brick_wall_normal.png ({w}x{h})");
    Ok(())
}

/// Writes 8-bit pixels of `color` type as a PNG.
fn write(path: &Path, w: u32, h: u32, data: &[u8], color: png::ColorType) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    encoder.set_color(color);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(data).map_err(|e| e.to_string())
}
