use crate::{threedee::normalize_vec, interpolate::{interpolate_f32_pc, interpolate_f32, interpolate_u32, interpolate_i32, interpolate_u16}};



#[derive(Clone, Copy, Debug)]
pub struct ProjectedVertex {
    pub x: usize, // x
    pub y: usize, // y
    pub z: u32,   // z
    pub w: f32,   // w
    pub nx: f32,  // normalx
    pub ny: f32,  // normaly
    pub nz: f32,  // normalz
    pub u: i32,   // u
    pub v: i32,   // v
    pub wx: f32,  // posx
    pub wy: f32,  // posy
    pub wz: f32,  // posz
    pub mm: u16,  // mipmap level
}

impl ProjectedVertex {
  pub fn new() -> ProjectedVertex {
    ProjectedVertex {
      x: 0,
      y: 0,
      z: 0,
      w: 0.0,
      nx: 0.0,
      ny: 0.0,
      nz: 0.0,
      u: 0,
      v: 0,
      wx: 0.0,
      wy: 0.0,
      wz: 0.0,
      mm: 0,
    }
  }
  pub fn for_span(x: usize) -> ProjectedVertex {
    ProjectedVertex {
      x,
      y: 0,
      z: 0,
      w: 0.0,
      nx: 0.0,
      ny: 0.0,
      nz: 0.0,
      u: 0,
      v: 0,
      wx: 0.0,
      wy: 0.0,
      wz: 0.0,
      mm: 0,
    }
  }
}

pub fn interpolate_projected_vertex(vert1: ProjectedVertex, vert2: ProjectedVertex, alpha: f32, x: usize, y: usize, w: f32) -> ProjectedVertex {
  // We are passing in x, y and w because they are always linearly
  // interpolated, so they will have already been calculated outside
  // the function using a cheaper method

  // Derive the perspective correct alpha value by using the w of our two vertices
  // to get a number between 0 and 1. This number can then be used to linearly
  // interpolate the rest of the vertex attributes
  let pc_alpha = interpolate_f32_pc(0.0, 1.0, vert1.w, vert2.w, alpha);
  // let [nx, ny, nz] = normalize_vec([
  //   interpolate_f32(vert1.nx, vert2.nx, pc_alpha),
  //   interpolate_f32(vert1.ny, vert2.ny, pc_alpha),
  //   interpolate_f32(vert1.nz, vert2.nz, pc_alpha),
  // ]);
  ProjectedVertex {
    x,
    y,
    z: interpolate_u32(vert1.z, vert2.z, pc_alpha),
    w,
    nx: 0.0,
    ny: 0.0,
    nz: 0.0,
    u: interpolate_i32(vert1.u, vert2.u, pc_alpha),
    v: interpolate_i32(vert1.v, vert2.v, pc_alpha),
    wx: interpolate_f32(vert1.wx, vert2.wx, pc_alpha),
    wy: interpolate_f32(vert1.wy, vert2.wy, pc_alpha),
    wz: interpolate_f32(vert1.wz, vert2.wz, pc_alpha),
    mm: interpolate_u16(vert1.mm, vert2.mm, pc_alpha),
  }
}