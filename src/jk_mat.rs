
use std::{fs::File, io::Read};

use crate::{Material, resources, materials::ChannelFormat, colors::{argb1555_to_rgba, rgb565_to_rgb, from_rgb}, textures::{Texture, Bitmap}};

pub fn load_file_mat(filename: &String) -> Result<Material, String> {
  let filepath = resources::get_resource_filepath(filename);
  let mut file = File::open(&filepath).map_err(|e| format!("{}", e))?;

  let mut buffer = Vec::new();
  file.read_to_end(&mut buffer)
      .map_err(|e| format!("{}", e))?;

  let bpp = buffer[24] as u32;

  let width = buffer[116] as u32 | ((buffer[117] as u32) << 8);
  let height = buffer[120] as u32 | ((buffer[121] as u32) << 8);

  // Image can't be wider than 32K, which is already too big, let's be honest
  if width > 32767 || height > 32767 {
      panic!(
          "Image located at: {} has dimensions that are too large!",
          filepath
      );
  }
  let width = width as u16;
  let height = height as u16;

  let mut indexed_transparency = false;
  let mut format = ChannelFormat::RGB565;
  if bpp == 8 {
      format = ChannelFormat::INDEXED;
  }
  let has_alpha = buffer[124] == 1;
  if has_alpha {
      if bpp == 16 {
          format = ChannelFormat::ARGB1555;
      } else {
          indexed_transparency = true;
      }
  }

  let lod_count = buffer[136];

  // Create bitmaps for the lods
  let mut lods = vec![];
  let mut data_offset = 140;
  // Calculate how many bytes are in the next lod
  let mut data_len = width as usize * height as usize * if bpp == 8 { 1 } else { 2 };

  let mut avg_color: [u32; 3] = [0, 0, 0];
  let mut j = 0;
  while j < lod_count {
      // Calculate where to end by adding the byte count of current lod to the offset
      let data_end = data_len + data_offset;
      let mut data = Vec::new();

      let mut i = data_offset;

      if matches!(format, ChannelFormat::ARGB1555) {
        while i < data_end {
          let b0 = buffer[i];
          let b1 = buffer[i + 1];
          i += 2;
          let [red, green, blue, alpha] = argb1555_to_rgba(b0, b1);

          data.push(red);
          data.push(green);
          data.push(blue);
          data.push(alpha);
        }
      } else if matches!(format, ChannelFormat::INDEXED) {
        while i < data_end {
          data.push(buffer[i]);
          i += 1;
        }
      } else {
        while i < data_end {
          let b0 = buffer[i];
          let b1 = buffer[i + 1];
          i += 2;
          let [red, green, blue] = rgb565_to_rgb(b0, b1);

          data.push(red);
          data.push(green);
          data.push(blue);
        }
      }
      // Add the numbers of bits of the lod we just read to the offset
      data_offset += data_len;
      // LODs are half the size of the previous
      // LOD bi-directionally, divide by 4
      data_len >>= 2;

      // Let's flip the image horizontally
      // let mut flipped_data = vec![];
      // let mut i = 0;
      // while i < data.len() {
      //     let mut j = i + width as usize * 3;
      //     while j > i {
      //         j -= 3;
      //         flipped_data.push(data[j]);
      //         flipped_data.push(data[j + 1]);
      //         flipped_data.push(data[j + 2]);
      //     }
      //     i += width as usize * 3;
      // }
      // data = flipped_data;

      // Create the bitmap. Make sure to divide the width and height by 2^j
      let bitmap_width = width >> j;
      let bitmap_height = height >> j;
      let bitmap = Bitmap {
        data,
        width: bitmap_width,
        height: bitmap_height,
        log2x: (bitmap_width as f32).log2().round() as u8, 
        log2y: (bitmap_height as f32).log2().round() as u8, 
        linear_offsetx: 32768 / bitmap_width as u16,
        linear_offsety: 32768 / bitmap_height as u16,
      };
      lods.push(bitmap);
      j += 1;
  }
  // Let's get the avg color from the smallest LOD
  let mut average_color: u32 = 0;
  if format != ChannelFormat::INDEXED {
    let mut t = 0;
    let bitmap = &lods[lods.len() - 1];
    let sl_len = bitmap.data.len();
    while t < sl_len {
      avg_color[0] += bitmap.data[t] as u32;
      avg_color[1] += bitmap.data[t + 1] as u32;
      avg_color[2] += bitmap.data[t + 2] as u32;
      t += 3;
    }
    // Let's derive the average color 
    average_color = from_rgb(
      (avg_color[0] / (width as u32 * height as u32)) as u8,
      (avg_color[1] / (width as u32 * height as u32)) as u8,
      (avg_color[2] / (width as u32 * height as u32)) as u8,
    );
  }
  // Create the texture
  let texture = Texture { lods, has_alpha, color: average_color };

  // Add the texture to the global list
  let idx = resources::add_texture(filename, texture);

  // Create the material
  let material = Material {
      texture: idx as u32,
      channel_format: format,
      custom_effects: vec![],
      palette: 0,
      indexed_transparency,
      displace_u_strength: 0.0,
      displace_v_strength: 0.0,
      opacity: 1.0,
      emissive_strenth: 0.0,
      specular_strength: 0.0,
      compact_normal_precision: 4.0,
  };
  return Ok(material);
}
