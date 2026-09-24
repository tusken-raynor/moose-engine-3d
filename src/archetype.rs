use std::{f32::consts::PI, fs};

use crate::{
    matrix::Transformation,
    threedee::{Model}, resources::{has_model, get_model_index, has_resource, add_model, add_archetype},
};

#[derive(Debug, Clone)]
pub struct MeshNode {
    pub mesh: usize,
    pub children: Vec<MeshNode>,
    pub transformation: Transformation,
}

impl MeshNode {
    pub fn new(mesh: usize) -> Self {
        Self {
            mesh,
            children: Vec::new(),
            transformation: Transformation::new(0.0, 0.0, 0.0),
        }
    }
}

const PI2: f32 = PI * 2.0;
#[derive(Debug, Clone)]
// This struct is TEMPORARY
pub struct Archetype {
    pub name: String,
    pub model: usize,
    pub hierarchy: MeshNode,
}

pub fn load_archetypes(
    filepath: String,
) {
    let arch_json =
        fs::read_to_string(filepath).expect("Unable to read archetypes declaration file");
    let archetypes = json::parse(arch_json.as_str()).unwrap();

    if archetypes.is_array() {
        for archetype in archetypes.members() {
            let name = archetype["name"].as_str().unwrap().to_string();
            let mut model: usize = 0;
            if archetype.has_key("model") {
                let model_name = archetype["model"].as_str().unwrap().to_string();
                // Get the model index from the map if it exists, else load the model
                if has_model(&model_name) {
                    // Get the model index from the map
                    model = get_model_index(&model_name);
                } else {
                  let filename = archetype["model"].as_str().unwrap().to_string();
                  if has_resource(&filename) {
                    // Load the 3D model and add it to the list
                    let modelobj = Model::load(&model_name);
                    model = add_model(&model_name, modelobj);
                  }
                }
            }
            let archie = Archetype {
                name: name.clone(),
                model,
                hierarchy: MeshNode::new(0),
            };
            add_archetype(&name, archie);
        }
    }
}
