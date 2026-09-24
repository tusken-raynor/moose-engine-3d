use std::{fs};

use rand::Rng;
use tobj::{load_obj, LoadOptions};

use crate::{
    colors::from_rgb,
    materials::{load_materials},
    jk_3do::{JKGeometryMode, load_3do_file}, setup::{TEXTURE_MAP_MODE_REDUCED, LIGHT_MODE_PER_VERTEX}, resources::get_resource_filepath,
};

#[derive(Debug, Clone)]
pub struct Face {
    pub verts: Vec<usize>,
    pub uvs: Vec<usize>,
    pub normals: Vec<usize>,
    pub normal: [f32; 3],
    pub center: [f32; 3],
    pub material: u32,
    pub color: u32,
    pub translucency: u8,
    pub blendmode: u8,
    pub dualsided: bool,
    pub tilemode: i8,
    pub filtermode: i8,
    pub extralight: u8,
    pub jk_no_collision: bool
}
#[derive(Debug, Clone)]
pub struct Mesh {
    pub rect: [f32; 6],
    pub verts: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub faces: Vec<Face>,
    pub name: String,
    pub radius: f32,
    pub texmapmode: u8,
    pub lightingmode: u8,
    pub jk_geometry_mode: JKGeometryMode,
    pub jk_vert_intensity: Vec<u8>,

}

#[derive(Debug, Clone)]
pub struct Model {
    pub meshes: Vec<Mesh>,
    pub name: String,
    pub radius: f32,
    pub rect: [f32; 6],
    pub jk_insert_offset: [f32; 3]
}

impl Model {
    pub fn load<'a>(filename: &String) -> Self {
      if filename.ends_with(".obj") {
        let meshes = load_obj_file(filename);
        let rect = calculate_rect_from_meshes(&meshes);
        return Self { 
          meshes, 
          name: filename.clone(), 
          radius: 0.0, 
          rect, 
          jk_insert_offset: [0.0; 3] 
        };
      } else if filename.ends_with(".3do") {
        return load_3do_file(filename);
      } else {
        let filepath = get_resource_filepath(filename);
        panic!("Invalid file type was provided: {}", filepath);
      }
    }
}

fn load_obj_file(filename: &String) -> Vec<Mesh> {
    let filepath = get_resource_filepath(filename);
    let (models, _materials) = load_obj(&filepath, &LoadOptions::default())
        .expect(format!("Failed to OBJ load file: {}", filepath).as_str());

    let model_materials = parse_face_materials(filename, models.len());

    if models.len() == 0 || models[0].mesh.positions.len() == 0 {
        panic!(
            "{} {}",
            "No valid model data could be found in file", filepath
        );
    }
    let mut meshes: Vec<Mesh> = vec![];
    let mut i = 0;
    let len = models.len();
    // Loop through all models (groups) in the file
    while i < len {
        let model = &models[i];
        // Declare the material data for this model
        let (material_names, material_indices) = &model_materials[i];
        let global_mat_indices = load_materials(&material_names);
        let material_indices: Vec<u32> = material_indices
            .iter()
            .map(|i| global_mat_indices[*i])
            .collect();
        // Skip any that have less than 3 vertices
        if model.mesh.positions.len() < 9 {
            i += 1;
            continue;
        }
        let mut verts: Vec<[f32; 3]> = vec![];
        let mut uvs: Vec<[f32; 2]> = vec![];
        let mut normals: Vec<[f32; 3]> = vec![];
        // If all faces are triangulated
        let tr = model.mesh.face_arities.len() == 0;
        let num_faces = if tr {
            model.mesh.indices.len() / 3
        } else {
            model.mesh.face_arities.len()
        };
        let mut j = 0;
        while j < model.mesh.positions.len() {
            verts.push([
                model.mesh.positions[j],
                model.mesh.positions[j + 1],
                model.mesh.positions[j + 2],
            ]);
            j += 3;
        }
        j = 0;
        while j < model.mesh.texcoords.len() {
            uvs.push([model.mesh.texcoords[j], model.mesh.texcoords[j + 1]]);
            j += 2;
        }
        j = 0;
        while j < model.mesh.normals.len() {
            normals.push([
                model.mesh.normals[j],
                model.mesh.normals[j + 1],
                model.mesh.normals[j + 2],
            ]);
            j += 3;
        }
        // Create the faces of the mesh by looping through the indices
        let mut faces: Vec<Face> = vec![];
        j = 0;
        let mut fp = 0;
        let mut rng = rand::thread_rng();
        while j < num_faces {
            let mut face: Face = Face {
                verts: vec![],
                uvs: vec![],
                normals: vec![],
                normal: [0.0, 0.0, 0.0],
                center: [0.0, 0.0, 0.0],
                material: if material_indices.len() > j {
                    material_indices[j]
                } else {
                    1
                },
                color: from_rgb(
                    rng.gen_range(0..255),
                    rng.gen_range(0..255),
                    rng.gen_range(0..255),
                ),
                translucency: 0,
                blendmode: 0,
                dualsided: false,
                tilemode: 0,
                filtermode: if filename.eq("cube.obj") {
                    3
                } else {
                    0
                },
                extralight: 0,
                jk_no_collision: false
            };
            let num_verts = if tr {
                3
            } else {
                model.mesh.face_arities[j] as usize
            };
            face.verts = vec_u32_to_vec_usize(model.mesh.indices[fp..fp + num_verts].to_vec());
            if model.mesh.texcoord_indices.len() > 0 {
                face.uvs =
                    vec_u32_to_vec_usize(model.mesh.texcoord_indices[fp..fp + num_verts].to_vec());
            }
            if model.mesh.normal_indices.len() > 0 {
                face.normals =
                    vec_u32_to_vec_usize(model.mesh.normal_indices[fp..fp + num_verts].to_vec());
            }
            let face_points: Vec<[f32; 3]> = face.verts.iter().map(|i| verts[*i]).collect();
            // Generate the normal of the face from the points of the face
            face.normal = normalize_vec(get_face_normal(&face_points));
            // Generate the center of the face from the points of the face
            let mut k = 0;
            while k < face_points.len() {
                face.center[0] += face_points[k][0];
                face.center[1] += face_points[k][1];
                face.center[2] += face_points[k][2];
                k += 1;
            }
            face.center[0] /= face_points.len() as f32;
            face.center[1] /= face_points.len() as f32;
            face.center[2] /= face_points.len() as f32;
            // Add the face to the mesh
            faces.push(face);
            j += 1;
            fp += num_verts;
        }
        // Create a 'bounding-box' rect for the mesh
        let mut x1 = f32::MAX;
        let mut y1 = f32::MAX;
        let mut z1 = f32::MAX;
        let mut x2 = f32::MIN;
        let mut y2 = f32::MIN;
        let mut z2 = f32::MIN;
        let verts_len = verts.len();
        j = 0;
        while j < verts_len {
            if verts[j][0] < x1 {
                x1 = verts[j][0];
            }
            if verts[j][0] > x2 {
                x2 = verts[j][0];
            }
            if verts[j][1] < y1 {
                y1 = verts[j][1];
            }
            if verts[j][1] > y2 {
                y2 = verts[j][1];
            }
            if verts[j][2] < z1 {
                z1 = verts[j][2];
            }
            if verts[j][2] > z2 {
                z2 = verts[j][2];
            }
            j += 1;
        }
        let rect: [f32; 6] = [x1, y1, z1, x2, y2, z2];
        // Create the mesh
        let mesh = Mesh {
            verts,
            uvs,
            normals,
            faces,
            rect,
            name: model.name.clone(),
            radius: 0.0,
            texmapmode: TEXTURE_MAP_MODE_REDUCED as u8,
            lightingmode: LIGHT_MODE_PER_VERTEX as u8,
            jk_geometry_mode: JKGeometryMode::DrawTextured,
            jk_vert_intensity: vec![0; verts_len],
        };
        meshes.push(mesh);
        i += 1;
    }
    return meshes;
}

pub fn get_face_normal(points: &Vec<[f32; 3]>) -> [f32; 3] {
    let mut i = 0;
    let mut t_normal = cross_product(
        [
            points[i + 1][0] - points[i][0],
            points[i + 1][1] - points[i][1],
            points[i + 1][2] - points[i][2],
        ],
        [
            points[i + 2][0] - points[i + 1][0],
            points[i + 2][1] - points[i + 1][1],
            points[i + 2][2] - points[i + 1][2],
        ],
    );
    while (i as isize) < (points.len() as isize) - 3
        && t_normal[0] == 0.0
        && t_normal[1] == 0.0
        && t_normal[2] == 0.0
    {
        i += 1;
        t_normal = cross_product(
            [
                points[i + 1][0] - points[i][0],
                points[i + 1][1] - points[i][1],
                points[i + 1][2] - points[i][2],
            ],
            [
                points[i + 2][0] - points[i + 1][0],
                points[i + 2][1] - points[i + 1][1],
                points[i + 2][2] - points[i + 1][2],
            ],
        );
    }
    return t_normal;
}

pub fn cross_product(vec1: [f32; 3], vec2: [f32; 3]) -> [f32; 3] {
    return [
        vec1[1] * vec2[2] - vec1[2] * vec2[1],
        vec1[2] * vec2[0] - vec1[0] * vec2[2],
        vec1[0] * vec2[1] - vec1[1] * vec2[0],
    ];
}

pub fn dot_product(vec1: [f32; 3], vec2: [f32; 3]) -> f32 {
    return vec1[0] * vec2[0] + vec1[1] * vec2[1] + vec1[2] * vec2[2];
}

pub fn normalize_vec(vec: [f32; 3]) -> [f32; 3] {
    let length = (vec[0] * vec[0] + vec[1] * vec[1] + vec[2] * vec[2]).sqrt();
    return [vec[0] / length, vec[1] / length, vec[2] / length];
}

pub fn pythag(vec: [f32; 3]) -> f32 {
    return (vec[0] * vec[0] + vec[1] * vec[1] + vec[2] * vec[2]).sqrt();
}

pub fn point_pythag(vec1: [f32; 3], vec2: [f32; 3]) -> f32 {
    let diffx = vec1[0] - vec2[0];
    let diffy = vec1[1] - vec2[1];
    let diffz = vec1[2] - vec2[2];
    return (diffx * diffx + diffy * diffy + diffz * diffz).sqrt();
}

pub fn point_pythag_soft(vec1: [f32; 3], vec2: [f32; 3]) -> f32 {
    let diffx = vec1[0] - vec2[0];
    let diffy = vec1[1] - vec2[1];
    let diffz = vec1[2] - vec2[2];
    return diffx * diffx + diffy * diffy + diffz * diffz;
}

fn vec_u32_to_vec_usize(vec: Vec<u32>) -> Vec<usize> {
    let mut new_vec: Vec<usize> = Vec::new();
    for i in vec {
        new_vec.push(i as usize);
    }
    return new_vec;
}

fn parse_face_materials(filename: &String, model_count: usize) -> Vec<(Vec<String>, Vec<usize>)> {
    let filepath = get_resource_filepath(filename);
    let mut data: Vec<(Vec<String>, Vec<usize>)> = vec![];

    let obj_string = fs::read_to_string(filepath.to_string())
        .expect(format!("Unable to read obj file: {:?}", filepath).as_str());
    // Get the lines and remove the comments
    let lines = obj_string.split('\n').map(|x| {
        if x.contains('#') {
            x.split_once('#').unwrap().0.trim()
        } else {
            x.trim()
        }
    });

    let material_names: Vec<String> = lines
        .clone()
        .filter(|x| x.starts_with("usemtl"))
        .map(|x| x.split_once(' ').unwrap().1.to_string())
        .collect();

    if material_names.len() == 0 {
        return vec![(vec![], vec![]); model_count];
    }

    let lines: Vec<&str> = lines.collect();

    let mut i = 0;
    let len = lines.len();
    let mut curr_mat_idx = 0;
    let mut curr_obj_idx = -1;
    while i < len {
        let line = lines[i];
        if line.starts_with("o ") || line.starts_with("g ") {
            // Start the object iterating
            curr_obj_idx += 1;
            data.push((vec![], vec![]));
        } else if line.starts_with("usemtl") {
            let mat_name = line.split_once(' ').unwrap().1.to_string();
            let idx = material_names.iter().position(|x| x.eq(&mat_name)).unwrap();
            data[curr_obj_idx as usize].0.push(mat_name.clone());
            curr_mat_idx = idx;
        } else if line.starts_with("f ") {
            data[curr_obj_idx as usize].1.push(curr_mat_idx);
        }
        i += 1;
    }

    return data;
}

pub fn calculate_rect_from_meshes(meshes: &Vec<Mesh>) -> [f32; 6] {
    let mut x1 = f32::MAX;
    let mut y1 = f32::MAX;
    let mut z1 = f32::MAX;
    let mut x2 = f32::MIN;
    let mut y2 = f32::MIN;
    let mut z2 = f32::MIN;
    let mut i = 0;
    let len = meshes.len();
    while i < len {
        let mesh = &meshes[i];
        if mesh.rect[0] < x1 {
            x1 = mesh.rect[0];
        }
        if mesh.rect[3] > x2 {
            x2 = mesh.rect[3];
        }
        if mesh.rect[1] < y1 {
            y1 = mesh.rect[1];
        }
        if mesh.rect[4] > y2 {
            y2 = mesh.rect[4];
        }
        if mesh.rect[2] < z1 {
            z1 = mesh.rect[2];
        }
        if mesh.rect[5] > z2 {
            z2 = mesh.rect[5];
        }
        i += 1;
    }
    return [x1, y1, z1, x2, y2, z2];
}