//! Bakes the crate's color texture for the normal map and specular shader
//! (`moose_raster::shaders::textured_lit`): `new_crate.png`, the color of
//! `new_crate_diff.png` with the specular map `new_crate_spec.png` (grey) in its alpha,
//! which scales the highlight. Its normal map, `new_crate_norm.png`, is used as it is (x
//! along u, y down the rows, as the shaders read them). Run from the repository:
//! `cargo run -p moose-assets --example bake_crate`.

use std::path::Path;

use moose_assets::Assets;

fn main() -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut assets = Assets::new(&root);
    let mut load = |name: &str| {
        let id = assets.load_texture(name).map_err(|e| e.to_string())?;
        Ok::<_, String>(assets.texture(id).base().clone())
    };
    let (color, specular) = (load("new_crate_diff.png")?, load("new_crate_spec.png")?);
    if (color.width, color.height) != (specular.width, specular.height) {
        return Err(format!(
            "the specular map is {}x{}, the color {}x{}",
            specular.width, specular.height, color.width, color.height
        ));
    }
    let data: Vec<u8> = color
        .texels
        .iter()
        .zip(&specular.texels)
        .flat_map(|(&c, &s)| [(c >> 16) as u8, (c >> 8) as u8, c as u8, (s >> 16) as u8])
        .collect();
    let path = root.join("textures/new_crate.png");
    let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), color.width, color.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&data).map_err(|e| e.to_string())?;
    println!("baked new_crate.png ({}x{}): color, specular in alpha", color.width, color.height);
    Ok(())
}
