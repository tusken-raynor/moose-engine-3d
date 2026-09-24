use std::{fs, collections::HashMap, sync::Mutex};

use lazy_static::lazy_static;

use crate::{textures::{Texture}, materials::Material, jk_cmp::ColorMap, archetype::Archetype, threedee::Model};

/* RESOURCE FILES MAP */
lazy_static! {
  static ref RESOURCE_FILES: Mutex<HashMap<String, String>> = create_resource_map();
}
/* RESOURCE FILES MAP */

/* TEXTURE STORAGE */
lazy_static! {
  static ref TEXTURE_NAMES: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
}
static mut TEXTURES: Vec<Texture> = vec![];
/* TEXTURE STORAGE */

/* MATERIAL STORAGE */
lazy_static! {
  static ref MATERIAL_NAMES: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
}
static mut MATERIALS: Vec<Material> = vec![];
/* MATERIAL STORAGE */

/* COLOR MAP STORAGE */
lazy_static! {
  static ref COLORMAP_NAMES: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
}
static mut COLORMAPS: Vec<ColorMap> = vec![];
/* COLOR MAP STORAGE */

/* 3D ARCHETYPE STORAGE */
lazy_static! {
  static ref MODELS_NAMES: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
}
static mut MODELS: Vec<Model> = vec![];
/* 3D ARCHETYPE STORAGE */

/* GAME ARCHETYPE STORAGE */
lazy_static! {
  static ref ARCHETYPE_NAMES: Mutex<HashMap<String, usize>> = Mutex::new(HashMap::new());
}
static mut ARCHETYPES: Vec<Archetype> = vec![];
/* GAME ARCHETYPE STORAGE */

/* GETTERS */
pub fn get_resource_filepath(name: &String) -> String {
  let binding = RESOURCE_FILES.lock().unwrap();
  let option = binding.get(name);
  if option.is_some() {
    return option.unwrap().to_string();
  } else {
    panic!("Resource file {} not found!", name);
  }
}
pub fn get_texture(index: usize) -> &'static Texture {
  unsafe {
    if TEXTURES.len() > index {
      return &TEXTURES[index];
    } else {
      // Return the default texture
      return &TEXTURES[1];
    }
  }
}
pub fn get_texture_index(name: &String) -> usize {
  if TEXTURE_NAMES.lock().unwrap().contains_key(name) {
    return *TEXTURE_NAMES.lock().unwrap().get(name).unwrap();
  } else {
    return 1;
  }
}
pub fn get_material(index: usize) -> &'static Material {
  unsafe {
    if MATERIALS.len() > index {
      return &MATERIALS[index];
    } else {
      // Return the default material
      return &MATERIALS[1];
    }
  }
}
pub fn get_material_index(name: &String) -> usize {
  if MATERIAL_NAMES.lock().unwrap().contains_key(name) {
    return *MATERIAL_NAMES.lock().unwrap().get(name).unwrap();
  } else {
    return 1;
  }
}
pub fn get_colormap(index: usize) -> &'static ColorMap {
  unsafe {
    if COLORMAPS.len() > index {
      return &COLORMAPS[index];
    } else {
      // Return the default colormap
      return &COLORMAPS[0];
    }
  }
}
pub fn get_colormap_index(name: &String) -> usize {
  if COLORMAP_NAMES.lock().unwrap().contains_key(name) {
    return *COLORMAP_NAMES.lock().unwrap().get(name).unwrap();
  } else {
    return 0;
  }
}
pub fn get_model(index: usize) -> &'static Model {
  unsafe {
    if MODELS.len() > index {
      return &MODELS[index];
    } else {
      // Return the default model
      return &MODELS[0];
    }
  }
}
pub fn get_model_index(name: &String) -> usize {
  if MODELS_NAMES.lock().unwrap().contains_key(name) {
    return *MODELS_NAMES.lock().unwrap().get(name).unwrap();
  } else {
    return 0;
  }
}
pub fn get_archetype(index: usize) -> &'static Archetype {
  unsafe {
    if ARCHETYPES.len() > index {
      return &ARCHETYPES[index];
    } else {
      // Return the default archetype
      return &ARCHETYPES[0];
    }
  }
}
pub fn get_archetype_index(name: &String) -> usize {
  if ARCHETYPE_NAMES.lock().unwrap().contains_key(name) {
    return *ARCHETYPE_NAMES.lock().unwrap().get(name).unwrap();
  } else {
    return 0;
  }
}
/* GETTERS */

/* CHECKERS */
pub fn has_resource(name: &String) -> bool {
  return RESOURCE_FILES.lock().unwrap().contains_key(name);
}
pub fn has_texture(name: &String) -> bool {
  return TEXTURE_NAMES.lock().unwrap().contains_key(name);
}
pub fn has_material(name: &String) -> bool {
  return MATERIAL_NAMES.lock().unwrap().contains_key(name);
}
pub fn has_colormap(name: &String) -> bool {
  return COLORMAP_NAMES.lock().unwrap().contains_key(name);
}
pub fn has_model(name: &String) -> bool {
  return MODELS_NAMES.lock().unwrap().contains_key(name);
}
pub fn has_archetype(name: &String) -> bool {
  return ARCHETYPE_NAMES.lock().unwrap().contains_key(name);
}
/* CHECKERS */

/* SETTERS */
pub fn add_texture(name: &String, texture: Texture) -> usize {
  unsafe {
    if TEXTURE_NAMES.lock().unwrap().contains_key(name) {
      println!("Warning: Texture with name {} has already been created. Wasteful operation.", name);
      return *TEXTURE_NAMES.lock().unwrap().get(name).unwrap();
    } else {
      let index = TEXTURES.len();
      TEXTURE_NAMES.lock().unwrap().insert(name.clone(), index);
      TEXTURES.push(texture);
      return index;
    }
  }
}
pub fn add_material(name: &String, material: Material) -> usize {
  unsafe {
    if MATERIAL_NAMES.lock().unwrap().contains_key(name) {
      println!("Warning: Material with name {} has already been created. Wasteful operation.", name);
      return *MATERIAL_NAMES.lock().unwrap().get(name).unwrap();
    } else {
      let index = MATERIALS.len();
      MATERIAL_NAMES.lock().unwrap().insert(name.clone(), index);
      MATERIALS.push(material);
      return index;
    }
  }
}
pub fn add_colormap(name: &String, colormap: ColorMap) -> usize {
  unsafe {
    if COLORMAP_NAMES.lock().unwrap().contains_key(name) {
      println!("Warning: Colormap with name {} has already been created. Wasteful operation.", name);
      return *COLORMAP_NAMES.lock().unwrap().get(name).unwrap();
    } else {
      let index = COLORMAPS.len();
      COLORMAP_NAMES.lock().unwrap().insert(name.clone(), index);
      COLORMAPS.push(colormap);
      return index;
    }
  }
}
pub fn add_model(name: &String, model: Model) -> usize {
  unsafe {
    if MODELS_NAMES.lock().unwrap().contains_key(name) {
      println!("Warning: Model with name {} has already been created. Wasteful operation.", name);
      return *MODELS_NAMES.lock().unwrap().get(name).unwrap();
    } else {
      let index = MODELS.len();
      MODELS_NAMES.lock().unwrap().insert(name.clone(), index);
      MODELS.push(model);
      return index;
    }
  }
}
pub fn add_archetype(name: &String, archetype: Archetype) -> usize {
  unsafe {
    if ARCHETYPE_NAMES.lock().unwrap().contains_key(name) {
      println!("Warning: Archetype with name {} has already been created. Wasteful operation.", name);
      return *ARCHETYPE_NAMES.lock().unwrap().get(name).unwrap();
    } else {
      let index = ARCHETYPES.len();
      ARCHETYPE_NAMES.lock().unwrap().insert(name.clone(), index);
      ARCHETYPES.push(archetype);
      return index;
    }
  }
}
/* SETTERS */


fn create_resource_map() -> Mutex<HashMap<String, String>> {
  let mut hashmap: HashMap<String, String> = HashMap::new();
  traverse_resource_directory("res".to_string(), &mut hashmap);
  return Mutex::new(hashmap);
}

fn traverse_resource_directory(dir: String, map: &mut HashMap<String, String>) {
    let dir = fs::read_dir(dir).expect("Error reading resource directory");

    for path in dir {
        let pathdata = path.as_ref().unwrap();
        let pathname = pathdata.path().display().to_string();
        let is_dir = pathdata.metadata().unwrap().is_dir();
        let filename = pathdata.file_name().to_str().unwrap().to_string();
        // Skip files that start with an underscore
        if filename.starts_with("_") {
            continue;
        }
        if !is_dir {
            if !map.contains_key(&filename) {
                map.insert(filename, pathname);
            }
        } else {
            traverse_resource_directory(pathname, map);
        }
    }
}