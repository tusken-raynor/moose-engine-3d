use std::{collections::HashMap, fmt::Debug, fs};

use crate::colors::from_rgb;
use crate::{resources, jk_mat};
use crate::textures::{Bitmap, Texture};

#[derive(Debug)]
#[derive(PartialEq)]
pub enum ChannelFormat {
    INDEXED,
    RGB565,
    ARGB1555,
    RGB,
    RGBA,
    CUSTOM,
}

pub enum ChannelEffect {
    R,
    G,
    B,
    A,
    NX,
    NY,
    NZ,
    DU,
    DV,
    S,
    E,
    CN,
    NONE,
}

pub struct Material {
    pub texture: u32,
    pub channel_format: ChannelFormat,
    pub custom_effects: Vec<ChannelEffect>,
    pub palette: u32,
    pub indexed_transparency: bool,
    pub displace_u_strength: f32,
    pub displace_v_strength: f32,
    pub opacity: f32,
    pub emissive_strenth: f32,
    pub specular_strength: f32,
    pub compact_normal_precision: f32,
}

impl Material {
    pub fn default() -> Material {
        return Material {
            texture: 1,
            channel_format: ChannelFormat::RGB,
            custom_effects: vec![],
            palette: 0,
            indexed_transparency: false,
            displace_u_strength: 0.5,
            displace_v_strength: 0.5,
            opacity: 1.0,
            emissive_strenth: 1.0,
            specular_strength: 1.0,
            compact_normal_precision: 4.0,
        };
    }

    pub fn load(filename: &String) -> Material {
        let filepath = resources::get_resource_filepath(filename);
        if filepath.ends_with(".mat") {
            jk_mat::load_file_mat(filename)
              .expect(format!("Failed to load material {}", filepath).as_str())
        } else if filepath.ends_with(".mtl") {
            load_file_mtl(filename)
        } else {
            panic!("Invalid material file extension: {}", filepath);
        }
    }
}

pub fn load_materials(filenames: &Vec<String>) -> Vec<u32> {
    let mut indices: Vec<u32> = vec![];

    for filename in filenames {
      // Check if the filename has even been registered as a resource
      if !resources::has_resource(filename) {
        // Return the index for the default material
        indices.push(1);
        continue;
      }
      // Check and make sure this material hasn't already been processed
      if resources::has_material(filename) {
        // This also means the textures of the material have also been processeed
        // Just grab the index and get outta here
        let idx = resources::get_material_index(filename);
        indices.push(idx as u32);
        continue;
      }

      // Resource hasn't been loaded yet? Let's do it
      let material = Material::load(filename);

      let idx = resources::add_material(filename, material);
      indices.push(idx as u32);
    }

    return indices;
}

fn load_file_mtl(filename: &String) -> Material {
    let filepath = resources::get_resource_filepath(filename);
    let material_string =
        fs::read_to_string(&filepath).expect("Unable to read material file");
    let lines: Vec<&str> = material_string
        .split("\n")
        .into_iter()
        .filter(|x| x.len() > 0 && !x.starts_with("#"))
        .map(|x| x.trim())
        .collect();

    // 0 -> textures lines
    // 1 -> output lines
    // 2 -> properties lines
    let mut section: usize = 0;

    let mut tex_map: HashMap<String, (Vec<String>, u8)> = HashMap::new();
    let mut output_map: HashMap<String, (String, String)> = HashMap::new();
    let mut properties: HashMap<String, f32> = HashMap::new();

    // Default with custom channel format, and set it to specific
    // format if it is found
    let mut channel_format = ChannelFormat::CUSTOM;

    for line in lines {
        if line.to_lowercase().starts_with("textures") {
            section = 0;
            continue;
        }
        if line.to_lowercase().starts_with("output") {
            section = 1;
            continue;
        }
        if line.to_lowercase().starts_with("properties") {
            section = 2;
            continue;
        }

        let mut sanitized_line = line;
        if line.contains("#") {
            sanitized_line = sanitized_line.split_once("#").unwrap().0;
        }
        // Process each line of each section
        if section == 0 {
            // TEXTURES section
            let option = sanitized_line.split_once(" ");
            if option.is_none() {
                continue;
            }
            let parts = option.unwrap();
            // Attempt to split again by comma for texture lods
            tex_map.insert(
                parts.0.trim().to_lowercase().to_string(),
                (
                    parts
                        .1
                        .trim()
                        .to_string()
                        .split(',')
                        .map(|x| x.trim().to_string())
                        .collect(),
                    0,
                ),
            );
        } else if section == 1 {
            // OUTPUT section
            if !matches!(channel_format, ChannelFormat::CUSTOM) {
                // If we have determined the channel format to be anything other
                // than custom, then there is no point in processing the output
                // any further
                continue;
            }
            if sanitized_line.contains("->") {
                let parts = sanitized_line.split_once(" ").unwrap();
                let tpt = parts.1.split_once("->").unwrap();
                let texture_ref = tpt.0.trim().to_lowercase().to_string();
                // Look for the texture ref in the texture map
                // If it is found, add 1 to the number of channels it has.
                if tex_map.contains_key(&texture_ref) {
                    let mut tex = tex_map.get_mut(&texture_ref).unwrap();
                    tex.1 += 1;
                }

                output_map.insert(
                    parts.0.trim().to_lowercase().to_string(),
                    (texture_ref, tpt.1.trim().to_lowercase().to_string()),
                );
            } else {
                channel_format = get_channel_format_from_name(&sanitized_line.to_string());
                if !matches!(channel_format, ChannelFormat::CUSTOM) {
                    // Clear the entries from the output_map just in
                    // case there were any
                    output_map.clear();
                }
            }
        } else if section == 2 {
            // PROPERTIES section
            let parts = sanitized_line.split_once("->").unwrap();
            properties.insert(
                parts.0.trim().to_lowercase().to_string(),
                parts.1.trim().parse::<f32>().unwrap(),
            );
        }
    }
    // Now that the sections have been parsed, go through and map the data
    let mut channel_map: HashMap<String, (&Vec<String>, u16)> = HashMap::new();
    for (key, value) in output_map.iter() {
        let tex_key = &value.0;
        let content = tex_map.get(tex_key).unwrap();
        let filename = &content.0;
        let channel_index = if value.1.eq("r") {
            0
        } else if value.1.eq("g") {
            1
        } else if value.1.eq("b") {
            2
        } else if value.1.eq("a") {
            3
        } else {
            panic!(
                "Character \"{}\" was used to reference color channel but is invalid. FOUND IN: {}",
                value.1, filepath
            );
        };
        channel_map.insert(key.to_string(), (filename, channel_index));
    }

    let tex_images: Vec<Vec<String>> = tex_map
        .into_values()
        .map(|x| x.0)
        .collect();

    if tex_images.len() == 0 {
        // Must have a texture
        panic!(
            "Material from file: '{}' does not have a texture defined",
            filepath
        );
    }

    // tex_images is a vector of vectors of strings
    // Loop through the top level and make sure all the sub-vectors are the same length
    let mut tex_image_len = 0;
    for tex_filepaths in tex_images.iter() {
        if tex_image_len == 0 {
            tex_image_len = tex_filepaths.len();
        } else if tex_image_len != tex_filepaths.len() {
            panic!(
                "Material from file: '{}' has a different number of texture LODs for each texture",
                filepath
            );
        }
    }

    let mut texture_indices = vec![];

    for tex_filepaths in tex_images {
        // Use the first image path as the filepath for the texture
        let main_tex_filepath = tex_filepaths[0].clone();

        let mut lods = vec![];
        for tex_filepath in tex_filepaths {
            // Check and make sure the texture file isn't already loaded
            // If it is, grab the index and get the hell outta here
            if resources::has_texture(&tex_filepath) {
                let idx = resources::get_texture_index(&tex_filepath);
                texture_indices.push(idx);
                continue;
            }

            /* THIS IS WHERE WE ACTUALLY LOAD THE IMAGE FILE AND CREATE A BITMAP */
            lods.push(Bitmap::from(&tex_filepath));
            /* THIS IS WHERE WE ACTUALLY LOAD THE IMAGE FILE AND CREATE A BITMAP */
        }

        // Make sure the lods have the same aspect ratio
        let aspect_ratio = lods[0].width as f32 / lods[0].height as f32;
        let mut i = 1;
        while i < lods.len() {
            let lod_aspect_ratio = lods[i].width as f32 / lods[i].height as f32;
            if (lod_aspect_ratio - aspect_ratio).abs() > 0.0001 {
                panic!(
                    "Aspect ratio of texture lods used in material {} must be the same",
                    filepath
                );
            }
            i += 1;
        }
        // Sort the lods by size, largest to smallest
        lods.sort_by(|a, b| b.width.cmp(&a.width));

        // Get the avaerage color of the texture using the smallest lod
        let mut avg_color: [u32; 3] = [0, 0, 0];
        let mut t = 0;
        let lod = &lods[lods.len() - 1];
        let sl_len = lod.data.len();
        while t < sl_len {
            avg_color[0] += lod.data[t] as u32;
            avg_color[1] += lod.data[t + 1] as u32;
            avg_color[2] += lod.data[t + 2] as u32;
            t += 3;
        }
        // Let's derive the average color
        let avg_color = from_rgb(
            (avg_color[0] / (lod.width as u32 * lod.height as u32)) as u8,
            (avg_color[1] / (lod.width as u32 * lod.height as u32)) as u8,
            (avg_color[2] / (lod.width as u32 * lod.height as u32)) as u8,
        );

        // Right now, we only support one texture per material, so break the loop after this
        let texture = Texture {
            lods,
            color: avg_color,
            ..Texture::default()
        };
        let idx = resources::add_texture(filename, texture);
        texture_indices.push(idx);
    }

    let material = Material {
        texture: texture_indices[0] as u32,
        // Let's just roll with RGB for now
        channel_format,
        custom_effects: vec![],
        palette: 0,
        indexed_transparency: false,
        displace_u_strength: 0.5,
        displace_v_strength: 0.5,
        opacity: 1.0,
        emissive_strenth: 1.0,
        specular_strength: 1.0,
        compact_normal_precision: 4.0,
    };

    return material;
}

fn get_channel_format_from_name(name: &String) -> ChannelFormat {
    let name = name.to_lowercase();
    if name.eq("rgb") {
        return ChannelFormat::RGB;
    } else if name.eq("rgba") {
        return ChannelFormat::RGBA;
    } else if name.starts_with("indexed:") {
        return ChannelFormat::INDEXED;
    } else if name.eq("rgb565") {
        return ChannelFormat::RGB565;
    } else if name.eq("argb1555") {
        return ChannelFormat::ARGB1555;
    } else if name.eq("custom") {
        return ChannelFormat::CUSTOM;
    }
    return ChannelFormat::RGB;
}

fn get_channel_effect_from_name(name: &str) -> ChannelEffect {
    match name.to_lowercase().as_str() {
        "r" => ChannelEffect::R,
        "g" => ChannelEffect::G,
        "b" => ChannelEffect::B,
        "a" => ChannelEffect::A,
        "nx" => ChannelEffect::NX,
        "ny" => ChannelEffect::NY,
        "nz" => ChannelEffect::NZ,
        "du" => ChannelEffect::DU,
        "dv" => ChannelEffect::DV,
        "s" => ChannelEffect::S,
        "e" => ChannelEffect::E,
        "cn" => ChannelEffect::CN,
        _ => ChannelEffect::NONE,
    }
}
