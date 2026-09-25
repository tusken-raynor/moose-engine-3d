use std::ops::Range;

use glam::Vec3;

use crate::geom::{Aabb, Plane, convex_polygon_plane};
use crate::half::{f16_bits_to_f32, f32_to_f16_bits};

/// How an attribute's components are stored in memory. The interpolation format
/// (16.16, 8.8 or f32) is chosen by the shader, not the mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageFormat {
    U8,
    I8,
    I16,
    F16,
    F32,
}

impl StorageFormat {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "u8" => Self::U8,
            "i8" => Self::I8,
            "i16" => Self::I16,
            "f16" => Self::F16,
            "f32" => Self::F32,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::U8 => "u8",
            Self::I8 => "i8",
            Self::I16 => "i16",
            Self::F16 => "f16",
            Self::F32 => "f32",
        }
    }
}

/// Typed component storage for one attribute.
#[derive(Clone, Debug, PartialEq)]
pub enum AttribData {
    U8(Vec<u8>),
    I8(Vec<i8>),
    I16(Vec<i16>),
    /// IEEE 754 half-precision bit patterns.
    F16(Vec<u16>),
    F32(Vec<f32>),
}

impl AttribData {
    pub fn new(format: StorageFormat) -> Self {
        match format {
            StorageFormat::U8 => Self::U8(Vec::new()),
            StorageFormat::I8 => Self::I8(Vec::new()),
            StorageFormat::I16 => Self::I16(Vec::new()),
            StorageFormat::F16 => Self::F16(Vec::new()),
            StorageFormat::F32 => Self::F32(Vec::new()),
        }
    }

    pub fn format(&self) -> StorageFormat {
        match self {
            Self::U8(_) => StorageFormat::U8,
            Self::I8(_) => StorageFormat::I8,
            Self::I16(_) => StorageFormat::I16,
            Self::F16(_) => StorageFormat::F16,
            Self::F32(_) => StorageFormat::F32,
        }
    }

    /// Total number of components stored.
    pub fn len(&self) -> usize {
        match self {
            Self::U8(v) => v.len(),
            Self::I8(v) => v.len(),
            Self::I16(v) => v.len(),
            Self::F16(v) => v.len(),
            Self::F32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Component `index` as an f32 holding its stored numeric value: a u8 of 255 reads
    /// as 255.0 (not normalized), and f16 bits are decoded.
    pub fn get_f32(&self, index: usize) -> f32 {
        match self {
            Self::U8(v) => v[index] as f32,
            Self::I8(v) => v[index] as f32,
            Self::I16(v) => v[index] as f32,
            Self::F16(v) => f16_bits_to_f32(v[index]),
            Self::F32(v) => v[index],
        }
    }

    /// Parses one component written exactly as stored and appends it.
    pub(crate) fn push_token(&mut self, token: &str) -> Result<(), String> {
        let format = self.format().name();
        let bad = || format!("'{token}' is not a valid {format} value");
        match self {
            Self::U8(v) => v.push(token.parse().map_err(|_| bad())?),
            Self::I8(v) => v.push(token.parse().map_err(|_| bad())?),
            Self::I16(v) => v.push(token.parse().map_err(|_| bad())?),
            Self::F16(v) => {
                let x: f32 = token
                    .parse()
                    .ok()
                    .filter(|x: &f32| x.abs() <= 65504.0)
                    .ok_or_else(bad)?;
                v.push(f32_to_f16_bits(x));
            }
            Self::F32(v) => v.push(
                token
                    .parse()
                    .ok()
                    .filter(|x: &f32| x.is_finite())
                    .ok_or_else(bad)?,
            ),
        }
        Ok(())
    }

    /// Appends components `range` of `src`, which must have the same format.
    pub(crate) fn extend_from(&mut self, src: &AttribData, range: Range<usize>) {
        match (self, src) {
            (Self::U8(d), Self::U8(s)) => d.extend_from_slice(&s[range]),
            (Self::I8(d), Self::I8(s)) => d.extend_from_slice(&s[range]),
            (Self::I16(d), Self::I16(s)) => d.extend_from_slice(&s[range]),
            (Self::F16(d), Self::F16(s)) => d.extend_from_slice(&s[range]),
            (Self::F32(d), Self::F32(s)) => d.extend_from_slice(&s[range]),
            _ => unreachable!("attribute format mismatch"),
        }
    }
}

/// A named per-vertex attribute (varying). The name exists only so shaders can
/// match it; the data carries no meaning of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Attrib {
    pub name: String,
    /// Components per vertex.
    pub count: u8,
    /// `count` components for each polygon vertex, parallel to `Mesh::vertex_positions`.
    pub data: AttribData,
}

impl Attrib {
    pub fn format(&self) -> StorageFormat {
        self.data.format()
    }

    pub fn vertex_count(&self) -> usize {
        self.data.len() / self.count as usize
    }
}

/// Polygon-wide flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PolyFlags(pub u32);

impl PolyFlags {
    /// The surface reflects its sector like a mirror (level surfaces only). It is drawn
    /// over its reflection, typically translucent with a Fresnel falloff.
    pub const REFLECTIVE: u32 = 0x1;
    /// Every flag the level format defines.
    pub const ALL: u32 = Self::REFLECTIVE;

    pub fn reflective(self) -> bool {
        self.0 & Self::REFLECTIVE != 0
    }
}

/// A planar, convex n-gon. It owns a contiguous run of the mesh's polygon vertices,
/// listed counter-clockwise as seen from the front.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Polygon {
    pub first_vertex: u32,
    pub vertex_count: u16,
    pub plane: Plane,
    pub flags: PolyFlags,
}

impl Polygon {
    /// Range of this polygon's vertices in `Mesh::vertex_positions` and every attribute.
    pub fn vertices(&self) -> Range<usize> {
        self.first_vertex as usize..self.first_vertex as usize + self.vertex_count as usize
    }
}

/// Renderable geometry: positions as authored plus polygons that own their vertices.
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub name: String,
    /// Positions as authored, in file order. Polygons that reference the same index share
    /// that position, and each position is transformed once. Separate positions at the same
    /// coordinates are never merged; positions no polygon uses are dropped.
    pub positions: Vec<Vec3>,
    /// For each polygon vertex, an index into `positions`.
    pub vertex_positions: Vec<u32>,
    /// Per-polygon-vertex attribute values, parallel to `vertex_positions`.
    pub attribs: Vec<Attrib>,
    pub polygons: Vec<Polygon>,
    pub bounds: Aabb,
}

impl Mesh {
    pub fn attrib(&self, name: &str) -> Option<&Attrib> {
        self.attribs.iter().find(|a| a.name == name)
    }

    /// The polygon's points in order.
    pub fn polygon_points(&self, polygon: &Polygon) -> impl Iterator<Item = Vec3> + '_ {
        self.vertex_positions[polygon.vertices()]
            .iter()
            .map(|&i| self.positions[i as usize])
    }
}

/// Builds a mesh over a fixed position list, validating each polygon.
pub(crate) struct MeshBuilder {
    mesh: Mesh,
}

impl MeshBuilder {
    pub fn new(
        name: &str,
        positions: Vec<Vec3>,
        attribs: impl IntoIterator<Item = (String, u8, StorageFormat)>,
    ) -> Self {
        let attribs = attribs
            .into_iter()
            .map(|(name, count, format)| Attrib {
                name,
                count,
                data: AttribData::new(format),
            })
            .collect();
        Self {
            mesh: Mesh {
                name: name.to_string(),
                positions,
                vertex_positions: Vec::new(),
                attribs,
                polygons: Vec::new(),
                bounds: Aabb::from_points([]),
            },
        }
    }

    pub fn polygon_count(&self) -> u32 {
        self.mesh.polygons.len() as u32
    }

    /// Adds a polygon over the given position indices, which the caller has range-checked.
    /// The caller then appends each vertex's values to every attribute.
    pub fn push_polygon(&mut self, indices: &[u32], flags: PolyFlags) -> Result<(), String> {
        let vertex_count = u16::try_from(indices.len())
            .map_err(|_| format!("polygon has {} vertices, max is 65535", indices.len()))?;
        let points: Vec<Vec3> = indices
            .iter()
            .map(|&i| self.mesh.positions[i as usize])
            .collect();
        let plane = convex_polygon_plane(&points)?;
        let first_vertex = self.mesh.vertex_positions.len() as u32;
        self.mesh.vertex_positions.extend_from_slice(indices);
        self.mesh.polygons.push(Polygon {
            first_vertex,
            vertex_count,
            plane,
            flags,
        });
        Ok(())
    }

    pub fn attrib_data(&mut self, index: usize) -> &mut AttribData {
        &mut self.mesh.attribs[index].data
    }

    /// Drops positions nothing uses, keeping the rest in order, and returns the mesh.
    /// `extra` index lists (such as portal outlines) also keep their positions and are
    /// remapped in place along with the polygons.
    pub fn finish(mut self, extra: &mut [&mut Vec<u32>]) -> Mesh {
        let vertices = self.mesh.vertex_positions.len();
        for a in &self.mesh.attribs {
            debug_assert_eq!(
                a.data.len(),
                vertices * a.count as usize,
                "attribute '{}' is incomplete",
                a.name
            );
        }
        const UNUSED: u32 = u32::MAX;
        let mut remap = vec![UNUSED; self.mesh.positions.len()];
        for &i in self
            .mesh
            .vertex_positions
            .iter()
            .chain(extra.iter().flat_map(|l| l.iter()))
        {
            remap[i as usize] = 0;
        }
        let mut kept = Vec::with_capacity(remap.len());
        for (i, r) in remap.iter_mut().enumerate() {
            if *r != UNUSED {
                *r = kept.len() as u32;
                kept.push(self.mesh.positions[i]);
            }
        }
        self.mesh.positions = kept;
        for i in self
            .mesh
            .vertex_positions
            .iter_mut()
            .chain(extra.iter_mut().flat_map(|l| l.iter_mut()))
        {
            *i = remap[*i as usize];
        }
        self.mesh.bounds = Aabb::from_points(self.mesh.positions.iter().copied());
        self.mesh
    }
}
