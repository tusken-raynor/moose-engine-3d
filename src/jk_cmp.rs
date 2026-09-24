use std::fs::File;
use std::io::Read;

use crate::resources;

const PALETTE_SIZE: usize = 256;
const LIGHT_TABLES: usize = 64;
const TRANSPARENCY_TABLES: usize = 256;
const TABLE_SIZE: usize = 256;
const BYTES_PER_COLOR: usize = 3;

#[derive(Debug, Clone)]
pub struct ColorMap {
  pub palette: [u8; 768],
  pub tables: Vec<u8>,
  pub transparency: bool,
}

pub fn load_cmp_file(filename: &String) -> Result<ColorMap, std::io::Error> {
    let file_path = resources::get_resource_filepath(filename);
    // Read the entire file into a buffer
    let mut file = File::open(&file_path)?;
    let mut buffer = vec![0u8; file.metadata()?.len() as usize];
    file.read_exact(&mut buffer)?;

    // Does the color map have tranparent tables?
    let has_transparency = buffer[8] == 1;

    // Extract the color palette table
    let palette_start = 64;
    let palette_end = palette_start + PALETTE_SIZE * BYTES_PER_COLOR;
    let palette: [u8; 768] = buffer[palette_start..palette_end].try_into().expect(&format!("Invalid palette length in file: {}", file_path));

    // Extract the light level tables
    let light_start = 832;
    let light_end = light_start + LIGHT_TABLES * TABLE_SIZE;
    let mut tables = buffer[light_start..light_end].to_vec();

    // Extract the transparency tables (if available)
    if has_transparency {
      let transparency_start = 17216;
      let transparency_end = transparency_start + TRANSPARENCY_TABLES * TABLE_SIZE;
      let transparency_tables = buffer[transparency_start..transparency_end].to_vec();

      // Combine the light and transparency tables
      tables = tables
          .iter()
          .chain(transparency_tables.iter())
          .cloned()
          .collect::<Vec<u8>>();
    }

    Ok(ColorMap {
        palette,
        tables,
        transparency: has_transparency
    })
}