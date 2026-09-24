use std::collections::HashMap;
use std::fs;

use regex::Regex;

use crate::materials::load_materials;
use crate::resources;
use crate::setup::{TEXTURE_MAP_MODE_REDUCED, LIGHT_MODE_PER_VERTEX, LIGHT_MODE_FULLY_LIT, LIGHT_MODE_PER_UNIT, LIGHT_MODE_PER_FACE, LIGHT_MODE_PER_PIXEL, LIGHT_MODE_PER_REDUCED, TEXTURE_MAP_MODE_AFFINE, TEXTURE_MAP_MODE_PERSPECTIVE, TEXTURE_MAP_MODE_SCREEN, TEXTURE_TILE_MODE_REPEAT};
use crate::threedee::{Mesh, Model, Face, calculate_rect_from_meshes};
use crate::utils::parse_numbers_from_line;

#[derive(Debug, Clone)]
pub enum JKGeometryMode {
  DoNotDraw,
  DrawVertices,
  DrawWireframe,
  DrawSolid,
  DrawTextured,
}

pub fn load_3do_file(filename: &String) -> Model {
  let mut model = Model { 
    meshes: vec![], 
    name: filename.clone(), 
    radius: 0.0, 
    rect: [0.0; 6], 
    jk_insert_offset: [0.0; 3]
  };
  // Hold the model's material names and their indices so we
  // can reference them when we normalize the uv coordinates
  let mut material_dimension_data: HashMap<usize, (u16, u16, u32)> = HashMap::new();

  // Load the 3DO file as a string
  // Split the lines, and remove comments and empty lines
  // Join the lines back together
  let filepath = resources::get_resource_filepath(filename);
  let text_3do = fs::read_to_string(&filepath)
      .expect(format!("Unable to read 3do file: {:?}", filepath).as_str()).to_lowercase().split('\n')
      .filter(|line| line.trim().len() > 0 && !line.starts_with('#'))
      .map(|line| {
          if line.contains('#') {
              let (line, _) = line.split_once('#').unwrap();
              return line.trim();
          }
          return line.trim();
      }).collect::<Vec<&str>>().join("\n");

  let mut version = 2.1; // Default to 2.1

  // Split the file into its base sections
  let sections_text = text_3do.split("section:");

  // Loop through the sections text
  for text in sections_text {
    let section_text = text.trim();
    // Grab the section name, it will be the first word
    let section_name = get_3do_section_name(section_text).unwrap();
    // Parse the 3DO version from the header
    if section_name.eq("header") {
      let lines = section_text.lines().collect::<Vec<&str>>();
      for line in lines {
        if line.starts_with("3do") {
          version = parse_numbers_from_line::<f32>(line, 1)
            .unwrap_or(vec![2.1])[0];
        }
      }
    } else if section_name.eq("modelresource") {
      let (material_names, material_indices) = parse_model_resource(&section_text.lines().collect::<Vec<&str>>());
      // Load the materials
      let global_mat_indices = load_materials(&material_names);
      
      // If the global_mat_indeces and the material_indices are not the same
      // length, then we have a problem
      if global_mat_indices.len() != material_indices.len() {
          panic!("The number of materials loaded does not match the number of materials referenced in the 3DO file: {}", filepath);
      }
      // Use the global_mat_indices to grab the dimensions of the
      // materials needed for this model because we have to use
      // them to normalize the UVs defined in the 3DO file
      let len = global_mat_indices.len();
      let mut i = 0;
      while i < len {
        let global_mat_index = global_mat_indices[i] as usize;
        let local_mat_index = material_indices[i];
        let mat = resources::get_material(global_mat_index);
        let texture = resources::get_texture(mat.texture as usize);
        // Grab the dimension from the first LOD
        let width = texture.lods[0].width;
        let height = texture.lods[0].height;
        material_dimension_data.insert(local_mat_index, (width, height, global_mat_index as u32));
        i += 1;
      }
    } else if section_name.eq("geometrydef") {
      // Split the geoset (LOD) text into its sub sections
      let (geometrydef_text, geoset_text) = section_text
        .split_once("geosets")
        .expect(&format!("Unable to find geosets in 3DO file: {}", filepath));
      // Grab the radius of the model AND the offset of the model
      let (radius, offset) = parse_geometrydef(geometrydef_text).expect(&format!("Unable to parse geometrydef in 3DO file: {}", filepath));
      model.radius = radius;
      model.jk_insert_offset = offset;
      
      // Split the geoset text into its sub sections
      let geoset_1_index = geoset_text.find("geoset").expect(&format!("Unable to find geoset 1 in 3DO file: {}", filepath));
      let mut geoset_chunks = geoset_text.split_at(geoset_1_index).1.split("geoset").map(|text| text.trim()).filter(|text| text.len() > 0).collect::<Vec<&str>>();
      sort_by_start_number(&mut geoset_chunks, "\n");

      // For now just process the first LOD
      let geoset_chunks = vec![geoset_chunks[0]];
      // Now loop through each LOD and process them
      for geoset_chunk in geoset_chunks {
        // We need a flag to track the first mesh chunk 
        // because it holds useless information
        let mut first_mesh_chunk = true;
        // Split the geoset chunk into its mesh sub sections
        let geoset_chunk = geoset_chunk.replacen("meshes", "", 1);
        let mut mesh_chunks = geoset_chunk
          .split("mesh")
          .map(|text| text.trim())
          .filter(|text| {
            let is_first = first_mesh_chunk;
            first_mesh_chunk = false;
            !is_first && text.len() > 0
          })
          .collect::<Vec<&str>>();
        sort_by_start_number(&mut mesh_chunks, "\n");
        
        // Now loop through each mesh and process them
        for mesh_chunk in mesh_chunks {
          let mut mesh = Mesh {
            name: String::new(),
            verts: vec![],
            uvs: vec![],
            normals: vec![],
            faces: vec![],
            rect: [0.0; 6],
            radius: 0.0,
            texmapmode: TEXTURE_MAP_MODE_REDUCED as u8,
            lightingmode: LIGHT_MODE_PER_VERTEX as u8,
            jk_geometry_mode: JKGeometryMode::DrawTextured,
            jk_vert_intensity: vec![],
          };
          let mut mesh_tilemode = TEXTURE_TILE_MODE_REPEAT as u8;
          // Grab the local index of the mesh
          // The chunk should start with the index
          let (lmi, mesh_chunk) = mesh_chunk.split_once("\n").expect(&format!("Unable to find local mesh index in 3DO file: {}", filepath));
          let local_mesh_index = lmi.parse::<usize>().unwrap();
          let (mesh_headers, mesh_body) = mesh_chunk
            .split_once("vertices")
            .expect(
              &format!("Unable to find vertices of mesh {} in 3DO file: {}", local_mesh_index, filepath)
            );
          // Let's parse the mesh headers
          let header_lines = mesh_headers
            .lines()
            .collect::<Vec<&str>>();
          for line in header_lines {
            let trimmed_line = line.trim();
            if trimmed_line.starts_with("name") {
              let name = line.split_once("name").unwrap().1.trim().to_string();
              mesh.name = name;
            } else if trimmed_line.starts_with("radius") {
              let radius = parse_numbers_from_line::<f32>(line, 1)
                .expect(&format!("Unable to parse radius of mesh {} in 3DO file: {}", local_mesh_index, filepath))[0];
              mesh.radius = radius;
            } else if trimmed_line.starts_with("geometrymode") {
              let mode = parse_numbers_from_line::<u8>(line, 1)
                .expect(&format!("Unable to parse geometry mode of mesh {} in 3DO file: {}", local_mesh_index, filepath))[0];
              match mode {
                0 => mesh.jk_geometry_mode = JKGeometryMode::DoNotDraw,
                1 => mesh.jk_geometry_mode = JKGeometryMode::DrawVertices,
                2 => mesh.jk_geometry_mode = JKGeometryMode::DrawWireframe,
                3 => mesh.jk_geometry_mode = JKGeometryMode::DrawSolid,
                4 => mesh.jk_geometry_mode = JKGeometryMode::DrawTextured,
                _ => mesh.jk_geometry_mode = JKGeometryMode::DrawTextured,
              }
            } else if trimmed_line.starts_with("lightingmode") {
              let mode = parse_numbers_from_line::<u8>(line, 1)
                .expect(&format!("Unable to parse lighting mode of mesh {} in 3DO file: {}", local_mesh_index, filepath))[0];
              match mode {
                0 => mesh.lightingmode = LIGHT_MODE_FULLY_LIT as u8,
                1 => mesh.lightingmode = LIGHT_MODE_PER_UNIT as u8,
                2 => mesh.lightingmode = LIGHT_MODE_PER_FACE as u8,
                3 => mesh.lightingmode = LIGHT_MODE_PER_VERTEX as u8,
                4 => mesh.lightingmode = LIGHT_MODE_PER_PIXEL as u8,
                5 => mesh.lightingmode = LIGHT_MODE_PER_REDUCED as u8,
                _ => mesh.lightingmode = LIGHT_MODE_PER_VERTEX as u8,
              }
            } else if trimmed_line.starts_with("texturemode") {
              let mode = parse_numbers_from_line::<u8>(line, 1)
                .expect(&format!("Unable to parse texture mode of mesh {} in 3DO file: {}", local_mesh_index, filepath))[0];
              match mode {
                0 => mesh.texmapmode = TEXTURE_MAP_MODE_AFFINE as u8,
                1 => mesh.texmapmode = TEXTURE_MAP_MODE_REDUCED as u8,
                2 => mesh.texmapmode = TEXTURE_MAP_MODE_PERSPECTIVE as u8,
                3 => mesh.texmapmode = TEXTURE_MAP_MODE_SCREEN as u8,
                _ => mesh.texmapmode = TEXTURE_MAP_MODE_REDUCED as u8,
              }
            } else if line.starts_with("textilemode") {
              let mode = parse_numbers_from_line::<u8>(line, 1)
                .expect(&format!("Unable to parse texture mode of mesh {} in 3DO file: {}", local_mesh_index, filepath))[0];
              mesh_tilemode = mode;
            }
          }

          // Let's split the mesh body into its sub sections
          // Veritices, UVs, Normals, Faces and Face Normals
          let (vertices_text, mesh_body) = mesh_body
            .split_once("texture vertices")
            .expect(
              &format!("Unable to find uvs of mesh {} in 3DO file: {}", local_mesh_index, filepath)
            );
          let (uvs_text, mesh_body) = mesh_body
            .split_once("vertex normals")
            .expect(
              &format!("Unable to find normals of mesh {} in 3DO file: {}", local_mesh_index, filepath)
            );
          let (normals_text, mesh_body) = mesh_body
            .split_once("faces")
            .expect(
              &format!("Unable to find faces of mesh {} in 3DO file: {}", local_mesh_index, filepath)
            );
          let (faces_text, face_normals_text) = mesh_body
            .split_once("face normals")
            .expect(
              &format!("Unable to find face normals of mesh {} in 3DO file: {}", local_mesh_index, filepath)
            );
          // Now loop through the lines of each section
          // Every line that has information we need will
          // start with a number followed by a colon
          // VERTICES
          let mut vertex_lines = vertices_text
            .lines()
            .filter(|line| line.contains(":"))
            .map(|line| line.trim())
            .collect::<Vec<&str>>();
          sort_by_start_number(&mut vertex_lines, ":");
          for line in vertex_lines {
            let (_, numbers) = line.split_once(":").unwrap();
            // let index = index.trim().parse::<usize>().expect(&format!("Unable to parse vertex index of mesh {} in 3DO file: {}", local_mesh_index, filepath));
            let numbers = parse_numbers_from_line::<f32>(numbers, 0).expect(&format!("Unable to parse vertex of mesh {} in 3DO file: {}", local_mesh_index, filepath));
            if numbers.len() < 3 {
              panic!("Unable to parse vertex of mesh {} in 3DO file: {}", local_mesh_index, filepath);
            }
            // Scale the vertices by 10.0 to match the scale of Moose Engine Standard
            mesh.verts.push([numbers[0] * 10.0, numbers[1] * 10.0, numbers[2] * 10.0]);
            if numbers.len() > 3 {
              mesh.jk_vert_intensity.push((numbers[3].clamp(0.0, 1.0) * 255.9) as u8);
            } else {
              mesh.jk_vert_intensity.push(0);
            }
          }
          // TEXTURE VERTICES
          let mut uv_lines = uvs_text
            .lines()
            .filter(|line| line.contains(":"))
            .map(|line| line.trim())
            .collect::<Vec<&str>>();
          sort_by_start_number(&mut uv_lines, ":");
          for line in uv_lines {
            let (_, numbers) = line.split_once(":").unwrap();
            // let index = index.trim().parse::<usize>().expect(&format!("Unable to parse uv index of mesh {} in 3DO file: {}", local_mesh_index, filepath));
            let numbers = parse_numbers_from_line::<f32>(numbers, 0).expect(&format!("Unable to parse uv of mesh {} in 3DO file: {}", local_mesh_index, filepath));
            if numbers.len() < 2 {
              panic!("Unable to parse uv of mesh {} in 3DO file: {}", local_mesh_index, filepath);
            }
            mesh.uvs.push([numbers[0], numbers[1]]);
          }
          // Use this to track which uvs have been normalized!
          let mut uv_normalize_tracker = vec![false; mesh.uvs.len()];
          // VERTEX NORMALS
          let mut normal_lines = normals_text
            .lines()
            .filter(|line| line.contains(":"))
            .map(|line| line.trim())
            .collect::<Vec<&str>>();
          sort_by_start_number(&mut normal_lines, ":");
          for line in normal_lines {
            let (_, numbers) = line.split_once(":").unwrap();
            // let index = index.trim().parse::<usize>().expect(&format!("Unable to parse normal index of mesh {} in 3DO file: {}", local_mesh_index, filepath));
            let numbers = parse_numbers_from_line::<f32>(numbers, 0).expect(&format!("Unable to parse normal of mesh {} in 3DO file: {}", local_mesh_index, filepath));
            if numbers.len() < 3 {
              panic!("Unable to parse normal of mesh {} in 3DO file: {}", local_mesh_index, filepath);
            }
            mesh.normals.push([numbers[0], numbers[1], numbers[2]]);
          }
          // FACE NORMALS
          let mut face_normals: Vec<[f32; 3]> = vec![];
          let mut face_normal_lines = face_normals_text
            .lines()
            .filter(|line| line.contains(":"))
            .map(|line| line.trim())
            .collect::<Vec<&str>>();
          sort_by_start_number(&mut face_normal_lines, ":");
          for line in face_normal_lines {
            let (_, numbers) = line.split_once(":").unwrap();
            // let index = index.trim().parse::<usize>().expect(&format!("Unable to parse face normal index of mesh {} in 3DO file: {}", local_mesh_index, filepath));
            let numbers = parse_numbers_from_line::<f32>(numbers, 0).expect(&format!("Unable to parse face normal of mesh {} in 3DO file: {}", local_mesh_index, filepath));
            if numbers.len() < 3 {
              panic!("Unable to parse face normal of mesh {} in 3DO file: {}", local_mesh_index, filepath);
            }
            face_normals.push([numbers[0], numbers[1], numbers[2]]);
          }
          // FACES
          let mut face_lines = faces_text
            .lines()
            .filter(|line| line.contains(":"))
            .map(|line| line.trim())
            .collect::<Vec<&str>>();
          if face_lines.len() != face_normals.len() {
            panic!("The number of faces does not match the number of face normals of mesh {} in 3DO file: {}", local_mesh_index, filepath);
          }
          sort_by_start_number(&mut face_lines, ":");
          let mut i = 0;
          for line in face_lines {
            let face = parse_face_definition_line(line);
            if face.is_err() {
              panic!("Unable to parse face of mesh {} in 3DO file: {}. Message: {}", local_mesh_index, filepath, face.err().unwrap());
            }
            let mut face = face.unwrap();
            // Let's clean up the face, add the normal and a few other things
            face.normal = face_normals[i];
            // Calculate the center of the face
            let mut center = [0.0; 3];
            for vert_index in face.verts.iter() {
              let vert = mesh.verts[*vert_index];
              center[0] += vert[0];
              center[1] += vert[1];
              center[2] += vert[2];
            }
            center[0] /= face.verts.len() as f32;
            center[1] /= face.verts.len() as f32;
            center[2] /= face.verts.len() as f32;
            face.center = center;
            // Calculate the color of the face
            // Just set it to a random u32 for now
            face.color = rand::random::<u32>();
            // Set the tilemode of the face
            face.tilemode = mesh_tilemode as i8;
            // Now for the difficult part, we need to normalize the UVs
            // The UVs are defined in the 3DO file using the literal pixel
            // coordinates of the texture. We need to convert them to a
            // normalized value between 0.0 and 1.0
            // Grab the dimensions of the largest LOD used by the face
            let local_material_index = face.material;
            let option = material_dimension_data
              .get(&(local_material_index as usize));
            let (width, height, global_mat_index): &(u16, u16, u32) = if option.is_none() {
              // return the dimensions and index for the default material
              &(16, 16, 1)
            } else {
              option.unwrap()
            };
            face.material = *global_mat_index;
            let mut width = *width as f32;
            let mut height = *height as f32;
            // Jedi Knight originally did not support textures larger than 256 in either dimension
            // Patches have been released to support larger textures, but textures larger than 256
            // still used the same UV coordinates as if they were 256. If the 3do version is 2.1,
            // then assume we need to follow this setup. If the width or height is larger than 256,
            // then clamp it down to 256
            if version == 2.1 {
              if width > 256.0 {
                width = 256.0;
              }
              if height > 256.0 {
                height = 256.0;
              }
            }
            // Now loop through the UVs and normalize them
            let mut j = 0;
            let uv_len = face.uvs.len();
            while j < uv_len {
              let uv_index = face.uvs[j];
              // If the UV has already been normalized, then skip it
              if uv_normalize_tracker[uv_index] {
                j += 1;
                continue;
              }
              // Grab the UV
              let uv = mesh.uvs[uv_index];
              // Normalize & Update the UV
              mesh.uvs[uv_index] = [uv[0] / width, uv[1] / height];
              // Update the tracker
              uv_normalize_tracker[uv_index] = true;
              j += 1;
            }

            mesh.faces.push(face);
            i += 1;
          }
          // Now that we have all the data, let's calculate the rect
          let mut lowest_x = f32::MAX;
          let mut lowest_y = f32::MAX;
          let mut lowest_z = f32::MAX;
          let mut highest_x = f32::MIN;
          let mut highest_y = f32::MIN;
          let mut highest_z = f32::MIN;

          for vert in mesh.verts.iter() {
            if vert[0] < lowest_x {
              lowest_x = vert[0];
            }
            if vert[1] < lowest_y {
              lowest_y = vert[1];
            }
            if vert[2] < lowest_z {
              lowest_z = vert[2];
            }
            if vert[0] > highest_x {
              highest_x = vert[0];
            }
            if vert[1] > highest_y {
              highest_y = vert[1];
            }
            if vert[2] > highest_z {
              highest_z = vert[2];
            }
          }
          mesh.rect = [lowest_x, lowest_y, lowest_z, highest_x, highest_y, highest_z];
          // Add the mesh to the model
          model.meshes.push(mesh);
        }
      }
    }
  }
  // Calculate the rect for the whole model
  let rect = calculate_rect_from_meshes(&model.meshes);
  model.rect = rect;

  return model;
}

fn parse_model_resource(lines: &[&str]) -> (Vec<String>, Vec<usize>) {
  let re = Regex::new(r"(\d+):\s+(.+\.(mat|mtl))").unwrap();

  let mut mat_files: Vec<(u32, String)> = lines
      .iter()
      .map(|&line| {
          if let Some(captures) = re.captures(line) {
              let index: u32 = captures[1].parse().ok()?;
              let file_name = captures[2].to_string();
              Some((index, file_name))
          } else {
              None
          }
      }).filter(|opt| opt.is_some())
    .map(|opt| opt.unwrap()).collect();

  mat_files.sort_by_key(|(index, _)| *index);

  let mut names: Vec<String> = vec![];
  let mut indices: Vec<usize> = vec![];
  for (index, name) in mat_files {
      names.push(name);
      indices.push(index as usize);
  }
  (names, indices)
}

fn parse_geometrydef(text: &str) -> Option<(f32, [f32; 3])> {
  let mut vals = (0.0, [0.0; 3]);
  let lines  = text.lines().collect::<Vec<&str>>();
  for line in lines {
    if line.starts_with("radius") {
      let nums = parse_numbers_from_line::<f32>(line, 1);
      // If the result is Err, then return None
      if nums.is_err() {
        return None;
      }
      let nums = nums.unwrap();
      vals.0 = nums[0];
    } else if line.starts_with("insert") {
      let nums = parse_numbers_from_line::<f32>(line, 2);
      // If the result is Err, then return None
      if nums.is_err() {
        return None;
      }
      let nums = nums.unwrap();
      vals.1 = [nums[0], nums[1], nums[2]];
    }
  }
  return Some(vals);
}


fn get_3do_section_name(text: &str) -> Option<String> {
  if let Some(index) = text.find('\n') {
      Some(text[..index].trim().to_string())
  } else {
      Some(text.trim().to_string())
  }
}

fn parse_face_definition_line(line: &str) -> Result<Face, &'static str> {
  let option = line.split_once(":");
  if option.is_none() {
    return Err("Unable to parse face definition line from explicit index");
  }
  let (_, line) = option.unwrap();
  // Remove whitespace between vert indices and uv indices. Only commas should be between them
  let line = line.split(',').map(|s| s.trim()).collect::<Vec<&str>>().join(",");
  let parts = line.split_whitespace().map(|p| p.trim()).filter(|p| p.len() > 0).collect::<Vec<&str>>();

  // Face local material index
  let material_index = parts[0].parse::<usize>();
  if material_index.is_err() {
    return Err("Unable to parse material index");
  }
  let material_index = material_index.unwrap();

  // Face Flags
  let hex = parts[1];
  let flags = u16::from_str_radix(&hex[2..], 16);
  if flags.is_err() {
    return Err("Unable to parse flags");
  }
  let flags = flags.unwrap();

  // Ignore the next three values (geomode, lightmode, texmode), they are not used on a per-face basis

  // Face Self Luminance
  let extralight = parts[5].parse::<f32>();
  if extralight.is_err() {
    return Err("Unable to parse extralight");
  }
  let extralight = (extralight.unwrap() * 255.0) as u8;

  // Face Vertices & UVs
  let vert_count = parts[6].parse::<usize>();
  if vert_count.is_err() {
    return Err("Unable to parse vertex count");
  }
  let vert_count = vert_count.unwrap();
  // Now grab the next N pairs of vertex indices
  let mut vert_indices: Vec<usize> = vec![];
  let mut uv_indices: Vec<usize> = vec![];
  let mut i = 0;
  while i < vert_count {
    let pair = parts[7 + i].split_once(",");
    if pair.is_none() {
      return Err("Unable to parse vertex index / uv index pair");
    }
    let (vertex_index, uv_index) = pair.unwrap();
    let vertex_index = vertex_index.parse::<usize>();
    if vertex_index.is_err() {
      return Err("Unable to parse vertex index");
    }
    let uv_index = uv_index.parse::<usize>();
    if uv_index.is_err() {
      return Err("Unable to parse uv index");
    }
    // Add the indices 
    vert_indices.push(vertex_index.unwrap());
    uv_indices.push(uv_index.unwrap());
    i += 1;
  }

  // Parse the flags
  let dualsided = flags & 1 != 0;
  let translucency = flags & 2 != 0;
  let collide_with_other_objects = flags & 4 != 0;
  let blendmode = ((flags & 0b00011000) >> 3) as u8; // Supports 4 blend modes
  let filtermode = ((flags & 0b111100000) >> 5) as i8; // Supports 16 filter modes

  // Let's create the face
  let face = Face {
    verts: vert_indices.clone(),
    uvs: uv_indices,
    // Apparently the vertex normals always correlate to the vertex indices
    // So each vertex normal is just the average of all normals of the
    // faces that use that vertex. Kinda disappointing, but whatever
    normals: vert_indices,
    normal: [0.0; 3],
    center: [0.0; 3],
    material: material_index as u32,
    color: 0,
    translucency: if translucency { 128 } else { 255 },
    dualsided,
    blendmode,
    tilemode: 0,
    filtermode,
    extralight,
    jk_no_collision: !collide_with_other_objects
  };

  return Ok(face);
}

fn sort_by_start_number(str_vec: &mut Vec<&str>, delimeter: &str) {
  // Sort the strings by the number at the start of the string
  str_vec.sort_by(|a, b| {
    let a = a.split_once(delimeter).unwrap();
    let b = b.split_once(delimeter).unwrap();
    let a = a.0.parse::<usize>().unwrap();
    let b = b.0.parse::<usize>().unwrap();
    a.cmp(&b)
  });
}