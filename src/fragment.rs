/* FRAGMENT FUNCTION */
// This file contains the fragment (per pixel) functionality
// for the rasterizer. This is the only part of the pipeline
// that is not fixed-function, which is scary. But we make it
// work by not having more than one level of function pointer
// The function pointers have a performance impact because 
// the compiler cannot do branch prediction with it. For any
// additional function pointer used within fragment pipline,
// the cost adds up. It is done because it is meaningfully
// faster than putting conditions (branching) all through
// the fragment code. But this means that every combination
// of color channel format, filter method, mipmap method and
// lighting technique needs to be represented as a single 
// function pointer instead of a compound of various function
// pointers. 

use crate::{setup::{TEXTURE_FILTER_MODE_NEAREST, TEXTURE_FILTER_MODE_DITHERED, TEXTURE_FILTER_MODE_BILINEAR}, materials::ChannelFormat, textures::{Texture, Bitmap}, resources, utils::{mod_from_range_usize, q_interpolate_u8}};

// This is the matrix used to do dither texture sample like
// what is done in Unreal's software mode, to approximate
// bilinear filtering
const FILTER_DITHER_MATRIX: [[i16; 4]; 4] = [
  [ -32768,   0, -28672,  4096 ],
  [   16384, -16384,   20480, -12288 ],
  [  -20480,  12288,  -24576,  8192 ],
  [  28672, -4096,   24576, -8192 ]
];
// This is the matrix used to apply an ordered dither transition
// to textures for interpolating between mipmap levels, which
// creates an approximation of trilinear mipmap blending
// const MIPMAP_DITHER_MATRIX: [[i8; 4]; 4] = [
//   [7, 12, 4, 14],
//   [11, 2, 9, 1],
//   [5, 15, 6, 13],
//   [8, 0, 10, 3],
// ];

static mut CURRENT_COLORMAP_INDEX: usize = usize::MAX;
static mut CURRENT_COLORMAP_PALETTE: [u8; 768] = [0; 768];
static mut CURRENT_COLORMAP_TABLES: Vec<u8> = vec![];
static mut CURRENT_COLORMAP_TRANSPARENCY: bool = true;

pub fn set_current_colormap(colormap: usize) {
  unsafe {
    if CURRENT_COLORMAP_INDEX != colormap {
      CURRENT_COLORMAP_INDEX = colormap;
      let colormap = resources::get_colormap(colormap);
      CURRENT_COLORMAP_PALETTE = colormap.palette.clone();
      CURRENT_COLORMAP_TABLES = colormap.tables.clone();
      CURRENT_COLORMAP_TRANSPARENCY = colormap.transparency;
    }
  }
}

/********* THIS IS ABOUT TO GET MESSY *********/
/** THIS IS ALL THE TEXTURE SMAPLE FUNCTIONS **/
/* THERES A LOT TO GAZE ON HERE SO BE CAREFUL */

pub type FragmentFunction = fn(&Texture, i32, i32, usize, usize) -> [u8; 4];

/* DEFINE HERE THE FRAGMENT FUNCTIONS */
// Each function must follow FragmentFunction structure, and
// they are the functions that the function pointer will 
// point to

fn frag_f_indexed_nearest(
  texture: &Texture,
  u: i32,
  v: i32,
  _: usize,
  _: usize,
) -> [u8; 4] {
  // For now ull from the first LOD
  let bitmap = &texture.lods[0];
  let [x, y] = derive_coords_from_uv_fixed(u, v, bitmap);
  let idx = x + y * bitmap.width as usize;
  let idx = bitmap.data[idx] as usize;
  let idx = (idx << 1) + idx;
  unsafe {
    return [
      CURRENT_COLORMAP_PALETTE[idx],
      CURRENT_COLORMAP_PALETTE[idx + 1],
      CURRENT_COLORMAP_PALETTE[idx + 2],
      255
    ];
  }
}
fn frag_f_indexed_dithered(
  texture: &Texture,
  u: i32,
  v: i32,
  screenx: usize,
  screeny: usize,
) -> [u8; 4] {
  // Pull from the first LOD
  let bitmap = &texture.lods[0];
  // Mask out the lowest 2 bits of the screen coordinates
  let ix = screenx & 3;
  let iy = screeny & 3;
  let dither_error = FILTER_DITHER_MATRIX[ix][iy] as i32;
  let [x, y] = derive_coords_from_uv_fixed(u - (dither_error >> bitmap.log2x), v + (dither_error >> bitmap.log2y), &bitmap);
  let idx = x + y * bitmap.width as usize;
  let idx = bitmap.data[idx] as usize;
  let idx = (idx << 1) + idx;
  unsafe {
    return [
      CURRENT_COLORMAP_PALETTE[idx],
      CURRENT_COLORMAP_PALETTE[idx + 1],
      CURRENT_COLORMAP_PALETTE[idx + 2],
      255
    ];
  }
}
fn frag_f_indexed_linear(
  texture: &Texture,
  u: i32,
  v: i32,
  _: usize,
  _: usize,
) -> [u8; 4] {
  // For now pull from the first LOD
  let bitmap = &texture.lods[0];
  let [(x, alpha_x), (y, alpha_y)] = derive_coords_and_alpha_from_uv_fixed(u, v, bitmap);
  return bilinear_sample_palette(bitmap, x,  y, alpha_x, alpha_y);
}
fn frag_f_rgb_nearest(
    texture: &Texture,
    u: i32,
    v: i32,
    _: usize,
    _: usize,
) -> [u8; 4] {
  let bitmap = &texture.lods[0];
  let [x, y] = derive_coords_from_uv_fixed(u, v, bitmap);
  let idx = x + y * bitmap.width as usize;
  let idx = (idx << 1) + idx;
  return [bitmap.data[idx], bitmap.data[idx + 1], bitmap.data[idx + 2], 255];
}
fn frag_f_rgb_dithered(
    texture: &Texture,
    u: i32,
    v: i32,
    screenx: usize,
    screeny: usize,
) -> [u8; 4] {
  let bitmap = &texture.lods[0];
  // Mask out the lowest 2 bits of the screen coordinates
  let ix = screenx & 3;
  let iy = screeny & 3;
  let dither_error = FILTER_DITHER_MATRIX[ix][iy] as i32;
  let [x, y] = derive_coords_from_uv_fixed(u - (dither_error >> bitmap.log2x), v + (dither_error >> bitmap.log2y), &bitmap);
  let idx = x + y * bitmap.width as usize;
  let idx = (idx << 1) + idx;
  return [bitmap.data[idx], bitmap.data[idx + 1], bitmap.data[idx + 2], 255];
}
fn frag_f_rgb_linear(
    texture: &Texture,
    u: i32,
    v: i32,
    _: usize,
    _: usize,
) -> [u8; 4] {
  let bitmap = &texture.lods[0];
  let [(x, alpha_x), (y, alpha_y)] = derive_coords_and_alpha_from_uv_fixed(u, v, bitmap);
  return bilinear_sample(bitmap, x,  y, alpha_x, alpha_y);
}

/* STORE THE FRAGMENT FUNCTION IN THIS ARRAY SO THEY CAN BE POINTED TO */

pub const FRAGMENT_FUNCTIONS: [FragmentFunction; 6] = [
  frag_f_indexed_nearest,
  frag_f_indexed_dithered,
  frag_f_indexed_linear,
  frag_f_rgb_nearest,
  frag_f_rgb_dithered,
  frag_f_rgb_linear
];

// This function takes in important paramters about the face about to be drawn, and the
// material/texture that will be applied to it, and returns the index for the appropriate 
// function pointer. This is run before the face is rasterized so that branching is 
// handled at a higher level, and not at the fragment level. This allows for decent level
// performance with high levels of flexability.
pub fn get_fragment_function_index(channel_format: &ChannelFormat, tex_filter_mode: u8) -> usize {
  if *channel_format == ChannelFormat::INDEXED {
    if tex_filter_mode == TEXTURE_FILTER_MODE_NEAREST {
      return 0;
    } else if tex_filter_mode == TEXTURE_FILTER_MODE_DITHERED {
      return 1;
    } else if tex_filter_mode == TEXTURE_FILTER_MODE_BILINEAR {
      return 2;
    }
    return 0;
  } else {
    if tex_filter_mode == TEXTURE_FILTER_MODE_NEAREST {
      return 3;
    } else if tex_filter_mode == TEXTURE_FILTER_MODE_DITHERED {
      return 4;
    } else if tex_filter_mode == TEXTURE_FILTER_MODE_BILINEAR {
      return 5;
    }
  }
  return 3;
}

// Take in a fixed point U or V coordinate and convert it
// into a coordinate for a pixel within a texture
fn derive_tex_s_coord(s: i32, size: u16) -> usize {
  return ((s & 65535) * (size as i32) >> 16) as usize;
}

// Take in a fixed point U or V coordinate and return a
// coordinate for a pixel within a texture and the 
// fractional portion (alpha) used for sub pixel inter-
// polation. Alpha is represented by a u8 integer.
fn derive_tex_s_coord_with_alpha(s: i32, size: u16, offset: u16) -> (usize, u8) {
  let s = s - (offset as i32);
  let scaled = (s & 65535) * (size as i32);
  return ((scaled >> 16) as usize, (scaled >> 8 & 255) as u8);
}

// Returns the x,y values for texture pixel based on the inserted uv
// coordinates and texture dimensions
fn derive_coords_from_uv_fixed(u: i32, v: i32, bitmap: &Bitmap) -> [usize; 2] {
  return [
      derive_tex_s_coord(u, bitmap.width),
      derive_tex_s_coord(v, bitmap.height),
  ];
}

// Returns the x,y values for texture pixel and the sub-pixel alphas
// based on the inserted uv coordinates, texture dimensions and an
// offset to correctly align the sampling
fn derive_coords_and_alpha_from_uv_fixed(u: i32, v: i32, bitmap: &Bitmap) -> [(usize, u8); 2] {
  return [
    derive_tex_s_coord_with_alpha(u, bitmap.width, bitmap.linear_offsetx),
    derive_tex_s_coord_with_alpha(v, bitmap.height, bitmap.linear_offsety),
  ];
}

fn bilinear_sample_palette(
  bitmap: &Bitmap,
  x: usize,
  y: usize,
  alpha_x: u8,
  alpha_y: u8,
) -> [u8; 4] {
  let width = bitmap.width as usize;
  let height = bitmap.height as usize;
  let x_plus_1 = mod_from_range_usize(x + 1, width);
  let y_plus_1 = mod_from_range_usize(y + 1, height);
  let row = y * width as usize;
  let row_plus_1 = y_plus_1 * width as usize;
  let idx_tl = x + row;
  let idx_tr = x_plus_1 + row;
  let idx_bl = x + row_plus_1;
  let idx_br = x_plus_1 + row_plus_1;
  let mut color = [0; 4];
  unsafe {
    let idx_00 = bitmap.data[idx_tl] as usize;
    let idx_01 = bitmap.data[idx_tr] as usize;
    let idx_10 = bitmap.data[idx_bl] as usize;
    let idx_11 = bitmap.data[idx_br] as usize;

    let idx_00 = (idx_00 << 1) + idx_00;
    let idx_01 = (idx_01 << 1) + idx_01;
    let idx_10 = (idx_10 << 1) + idx_10;
    let idx_11 = (idx_11 << 1) + idx_11;

    let r00 = CURRENT_COLORMAP_PALETTE[idx_00];
    let r01 = CURRENT_COLORMAP_PALETTE[idx_01];
    let r10 = CURRENT_COLORMAP_PALETTE[idx_10];
    let r11 = CURRENT_COLORMAP_PALETTE[idx_11];

    let g00 = CURRENT_COLORMAP_PALETTE[idx_00 + 1];
    let g01 = CURRENT_COLORMAP_PALETTE[idx_01 + 1];
    let g10 = CURRENT_COLORMAP_PALETTE[idx_10 + 1];
    let g11 = CURRENT_COLORMAP_PALETTE[idx_11 + 1];

    let b00 = CURRENT_COLORMAP_PALETTE[idx_00 + 2];
    let b01 = CURRENT_COLORMAP_PALETTE[idx_01 + 2];
    let b10 = CURRENT_COLORMAP_PALETTE[idx_10 + 2];
    let b11 = CURRENT_COLORMAP_PALETTE[idx_11 + 2];

    let r0 = q_interpolate_u8(r00, r01, alpha_x);
    let r1 = q_interpolate_u8(r10, r11, alpha_x);
  
    let g0 = q_interpolate_u8(g00, g01, alpha_x);
    let g1 = q_interpolate_u8(g10, g11, alpha_x);
  
    let b0 = q_interpolate_u8(b00, b01, alpha_x);
    let b1 = q_interpolate_u8(b10, b11, alpha_x);
  
    color[0] = q_interpolate_u8(r0, r1, alpha_y);
    color[1] = q_interpolate_u8(g0, g1, alpha_y);
    color[2] = q_interpolate_u8(b0, b1, alpha_y);
    color[3] = 255;
  }

  return color;
}

fn bilinear_sample(
  bitmap: &Bitmap,
  x: usize,
  y: usize,
  alpha_x: u8,
  alpha_y: u8,
) -> [u8; 4] {
  let width = bitmap.width as usize;
  let height = bitmap.height as usize;
  let x_plus_1 = mod_from_range_usize(x + 1, width);
  let y_plus_1 = mod_from_range_usize(y + 1, height);
  let row = y * width as usize;
  let row_plus_1 = y_plus_1 * width as usize;
  let idx_tl = x + row;
  let idx_tr = x_plus_1 + row;
  let idx_bl = x + row_plus_1;
  let idx_br = x_plus_1 + row_plus_1;
  let mut color = [0; 4];
  let idx_00 = (idx_tl << 1) + idx_tl;
  let idx_01 = (idx_tr << 1) + idx_tr;
  let idx_10 = (idx_bl << 1) + idx_bl;
  let idx_11 = (idx_br << 1) + idx_br;

  let r00 = bitmap.data[idx_00];
  let r01 = bitmap.data[idx_01];
  let r10 = bitmap.data[idx_10];
  let r11 = bitmap.data[idx_11];

  let g00 = bitmap.data[idx_00 + 1];
  let g01 = bitmap.data[idx_01 + 1];
  let g10 = bitmap.data[idx_10 + 1];
  let g11 = bitmap.data[idx_11 + 1];

  let b00 = bitmap.data[idx_00 + 2];
  let b01 = bitmap.data[idx_01 + 2];
  let b10 = bitmap.data[idx_10 + 2];
  let b11 = bitmap.data[idx_11 + 2];

  let r0 = q_interpolate_u8(r00, r01, alpha_x);
  let r1 = q_interpolate_u8(r10, r11, alpha_x);

  let g0 = q_interpolate_u8(g00, g01, alpha_x);
  let g1 = q_interpolate_u8(g10, g11, alpha_x);

  let b0 = q_interpolate_u8(b00, b01, alpha_x);
  let b1 = q_interpolate_u8(b10, b11, alpha_x);

  color[0] = q_interpolate_u8(r0, r1, alpha_y);
  color[1] = q_interpolate_u8(g0, g1, alpha_y);
  color[2] = q_interpolate_u8(b0, b1, alpha_y);
  color[3] = 255;
  return color;
}