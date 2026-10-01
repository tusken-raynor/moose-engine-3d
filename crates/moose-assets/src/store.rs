use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::LoadError;
use crate::level::Level;
use crate::mesh::Mesh;
use crate::texture::{Texture, decode_png};
use crate::{mmp, obj};

/// Handle to a mesh owned by [`Assets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeshId(pub u32);

/// Handle to a texture owned by [`Assets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TextureId(pub u32);

/// Owns every loaded mesh and texture. Everything else refers to them by [`MeshId`] and
/// [`TextureId`].
///
/// Load between frames through `&mut Assets`; during a frame, threads share `&Assets`.
pub struct Assets {
    root: PathBuf,
    meshes: Vec<Mesh>,
    by_name: HashMap<String, MeshId>,
    textures: Vec<Texture>,
    textures_by_name: HashMap<String, TextureId>,
    /// Textures whose alpha is a packed normal (see [`Assets::packed_alpha`]).
    packed_alpha: std::collections::HashSet<String>,
}

impl Assets {
    /// `root` is the asset directory: models are read from `root/models`, levels from
    /// `root/levels`, textures from `root/textures`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            meshes: Vec::new(),
            by_name: HashMap::new(),
            textures: Vec::new(),
            textures_by_name: HashMap::new(),
            packed_alpha: Default::default(),
        }
    }

    /// Says texture `name`'s alpha is a packed normal (see [`crate::bump::pack`]), before
    /// it is loaded: its mip levels' alpha is then made from the normals it packs, not by
    /// averaging the bytes (see [`crate::bump::repack_mips`]), when loaded or reloaded.
    pub fn packed_alpha(&mut self, name: &str) {
        self.packed_alpha.insert(name.to_string());
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn mesh(&self, id: MeshId) -> &Mesh {
        &self.meshes[id.0 as usize]
    }

    pub fn mesh_mut(&mut self, id: MeshId) -> &mut Mesh {
        &mut self.meshes[id.0 as usize]
    }

    pub fn meshes(&self) -> &[Mesh] {
        &self.meshes
    }

    pub fn mesh_id(&self, name: &str) -> Option<MeshId> {
        self.by_name.get(name).copied()
    }

    /// The file name a mesh was loaded from (see [`load_mesh`](Self::load_mesh)), if it
    /// was loaded from one.
    pub fn mesh_name(&self, id: MeshId) -> Option<&str> {
        self.by_name
            .iter()
            .find(|&(_, &m)| m == id)
            .map(|(name, _)| name.as_str())
    }

    /// Loads `root/models/<name>`, or returns the existing handle if it is already loaded.
    pub fn load_mesh(&mut self, name: &str) -> Result<MeshId, LoadError> {
        if let Some(id) = self.mesh_id(name) {
            return Ok(id);
        }
        let mesh = self.read_mesh(name)?;
        let id = self.add_mesh(mesh);
        self.by_name.insert(name.to_string(), id);
        Ok(id)
    }

    fn read_mesh(&self, name: &str) -> Result<Mesh, LoadError> {
        let path = self.root.join("models").join(name);
        let src = read(&path)?;
        match path.extension().and_then(|e| e.to_str()) {
            Some("obj") => obj::parse_obj(&path, name, &src),
            Some("mmdl") => crate::mmdl::parse_mmdl(&path, name, &src),
            _ => Err(LoadError::new(
                &path,
                None,
                "unsupported model format (expected .obj or .mmdl)",
            )),
        }
    }

    /// Reads a loaded model's file again, into the same handle (for a file changed on
    /// disk). Returns whether it was loaded.
    pub fn reload_mesh(&mut self, name: &str) -> Result<bool, LoadError> {
        let Some(id) = self.mesh_id(name) else {
            return Ok(false);
        };
        self.meshes[id.0 as usize] = self.read_mesh(name)?;
        Ok(true)
    }

    /// Replaces loaded model `name` with one read from `src` (its file's text, as the
    /// mesh editor has it), keeping its handle. Nothing changes if `src` doesn't load.
    pub fn set_model_text(&mut self, name: &str, src: &str) -> Result<(), LoadError> {
        let id = self
            .mesh_id(name)
            .ok_or_else(|| LoadError::new(Path::new(name), None, "that model isn't loaded"))?;
        let path = self.root.join("models").join(name);
        self.meshes[id.0 as usize] = crate::mmdl::parse_mmdl(&path, name, src)?;
        Ok(())
    }

    /// The names of the models and textures loaded from files.
    pub fn loaded_files(&self) -> (Vec<String>, Vec<String>) {
        (
            self.by_name.keys().cloned().collect(),
            self.textures_by_name.keys().cloned().collect(),
        )
    }

    /// Adds a mesh without a name; it can only be reached through the returned handle.
    pub fn add_mesh(&mut self, mesh: Mesh) -> MeshId {
        self.meshes.push(mesh);
        MeshId((self.meshes.len() - 1) as u32)
    }

    pub fn texture(&self, id: TextureId) -> &Texture {
        &self.textures[id.0 as usize]
    }

    /// For textures that change while running, like [`Ripples`](crate::Ripples) water.
    pub fn texture_mut(&mut self, id: TextureId) -> &mut Texture {
        &mut self.textures[id.0 as usize]
    }

    pub fn textures(&self) -> &[Texture] {
        &self.textures
    }

    pub fn texture_id(&self, name: &str) -> Option<TextureId> {
        self.textures_by_name.get(name).copied()
    }

    /// Loads `root/textures/<name>` (PNG), or returns the existing handle if it is already
    /// loaded.
    pub fn load_texture(&mut self, name: &str) -> Result<TextureId, LoadError> {
        if let Some(id) = self.texture_id(name) {
            return Ok(id);
        }
        let texture = self.read_texture(name)?;
        let id = self.add_texture(texture);
        self.textures_by_name.insert(name.to_string(), id);
        Ok(id)
    }

    /// Reads a loaded texture's file again, into the same handle. Returns whether it was
    /// loaded.
    pub fn reload_texture(&mut self, name: &str) -> Result<bool, LoadError> {
        let Some(id) = self.texture_id(name) else {
            return Ok(false);
        };
        self.textures[id.0 as usize] = self.read_texture(name)?;
        Ok(true)
    }

    fn read_texture(&self, name: &str) -> Result<Texture, LoadError> {
        let path = self.root.join("textures").join(name);
        let bytes = std::fs::read(&path)
            .map_err(|e| LoadError::new(&path, None, format!("cannot read file: {e}")))?;
        match path.extension().and_then(|e| e.to_str()) {
            Some("png") if self.packed_alpha.contains(name) => decode_png(&path, name, &bytes).map(|mut t| {
                crate::bump::repack_mips(&mut t);
                t
            }),
            Some("png") => decode_png(&path, name, &bytes),
            _ => Err(LoadError::new(
                &path,
                None,
                "unsupported texture format (expected .png)",
            )),
        }
    }

    /// Adds a texture without a name; it can only be reached through the returned handle.
    pub fn add_texture(&mut self, texture: Texture) -> TextureId {
        self.textures.push(texture);
        TextureId((self.textures.len() - 1) as u32)
    }

    /// Loads `root/levels/<name>`, loading any models its entities use.
    /// Each call builds a new level and adds a new geometry mesh.
    pub fn load_level(&mut self, name: &str) -> Result<Level, LoadError> {
        let path = self.root.join("levels").join(name);
        let src = read(&path)?;
        mmp::parse_mmp(self, &path, &src)
    }

    /// Like [`load_level`](Self::load_level), but parses `src` instead of reading the file.
    pub fn parse_level(&mut self, name: &str, src: &str) -> Result<Level, LoadError> {
        let path = self.root.join("levels").join(name);
        mmp::parse_mmp(self, &path, src)
    }
}

fn read(path: &Path) -> Result<String, LoadError> {
    std::fs::read_to_string(path)
        .map_err(|e| LoadError::new(path, None, format!("cannot read file: {e}")))
}
