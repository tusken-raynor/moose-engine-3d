
// #[rustversion::nightly]
// use packed_simd::u16x4;

use crate::{
    vertex::{ProjectedVertex, interpolate_projected_vertex},
    colors::{from_rgba},
    interpolate::{
        interpolate_i32,
        interpolate_i32_pc, interpolate_u32, interpolate_u32_pc, interpolate_usize,
    },
    materials::Material,
    setup::{
        TEXTURE_MAP_MODE_AFFINE,
        TEXTURE_MAP_MODE_PERSPECTIVE, TEXTURE_MAP_MODE_REDUCED, TEXTURE_MAP_MODE_SCREEN,
    },
    textures::Texture,
    resources, fragment::FRAGMENT_FUNCTIONS, utils::mod_from_range_usize,
};

static mut SAMPLE_TEXTURE_INDEX: usize = 0;
static mut SAMPLE_TEXTURE: fn(&Texture, i32, i32, usize, usize) -> [u8; 4] = FRAGMENT_FUNCTIONS[0];

pub fn set_texture_sample_function(index: usize) {
  unsafe {
    if SAMPLE_TEXTURE_INDEX != index {
      SAMPLE_TEXTURE_INDEX = index;
      SAMPLE_TEXTURE = FRAGMENT_FUNCTIONS[index];
    }
  }
}

pub fn fill_pixel(buffer: &mut Vec<u32>, x: usize, y: usize, color: u32, width: usize) {
    let pos = width as usize * y + x;
    buffer[pos] = color;
}
pub fn fetch_pixel(buffer: &mut Vec<u32>, x: usize, y: usize, width: usize) -> u32 {
    let pos = width as usize * y + x;
    return buffer[pos];
}
pub fn qfill_pixel(buffer: &mut Vec<u32>, index: usize, color: u32) {
    // if index >= buffer.len() {
    //     println!("INDEX: {}, BUFFER: {}", index, buffer.len());
    //     return;
    // }
    buffer[index] = color;
}
pub fn qfetch_pixel(buffer: &mut Vec<u32>, index: usize) -> u32 {
    // if index >= buffer.len() {
    //     println!("INDEX: {}, BUFFER: {}", index, buffer.len());
    //     return 0;
    // }
    return buffer[index];
}
pub fn fill_pixel_safe(
    buffer: &mut Vec<u32>,
    x: isize,
    y: isize,
    color: u32,
    width: u32,
    height: u32,
) {
    if x >= 0 && x < width as isize && y >= 0 && y < height as isize {
        let pos = (width as isize * y + x) as usize;
        buffer[pos] = color;
    }
}
pub fn fetch_pixel_safe(buffer: &mut Vec<u32>, x: isize, y: isize, width: u32, height: u32) -> u32 {
    if x >= 0 && x < width as isize && y >= 0 && y < height as isize {
        let pos = (width as isize * y + x) as usize;
        return buffer[pos];
    }
    return 0;
}

fn draw_line(
    frame_buff: &mut Vec<u32>,
    x1: isize,
    y1: isize,
    x2: isize,
    y2: isize,
    color: u32,
    width: u32,
    height: u32,
) {
    let mut x = x1;
    let mut y = y1;
    let stepx = (x2 - x1).signum();
    let stepy = (y2 - y1).signum();
    if stepx == 0 {
        // Draw a vertical line
        loop {
            fill_pixel_safe(frame_buff, x1, y, color, width, height);
            if y == y2 {
                break;
            }
            y += stepy;
        }
    } else if stepy == 0 {
        // Draw a horizontal line
        loop {
            fill_pixel_safe(frame_buff, x, y1, color, width, height);
            if x == x2 {
                break;
            }
            x += stepx;
        }
    } else if (x2 - x1).abs() == (y2 - y1).abs() {
        // Perfectly diagonal
        loop {
            fill_pixel_safe(frame_buff, x, y, color, width, height);
            if x == x2 {
                break;
            }
            x += stepx;
            y += stepy;
        }
    } else if (x2 - x1).abs() > (y2 - y1).abs() {
        // More horizontal diagonal
        let diff = (y2 - y1) as f32;
        loop {
            let i = (x - x1) as f32 / (x2 - x1) as f32;
            fill_pixel_safe(
                frame_buff,
                x,
                y1 + (diff * i).round() as isize,
                color,
                width,
                height,
            );
            if x == x2 {
                break;
            }
            x += stepx;
        }
    } else {
        // More vertical diagonal
        let diff = (x2 - x1) as f32;
        loop {
            let i = (y - y1) as f32 / (y2 - y1) as f32;
            fill_pixel_safe(
                frame_buff,
                x1 + (diff * i).round() as isize,
                y,
                color,
                width,
                height,
            );
            if y == y2 {
                break;
            }
            y += stepy;
        }
    }
}

pub fn draw_polygon(
    frame_buff: &mut Vec<u32>,
    points: Vec<(isize, isize, u32)>,
    color: u32,
    width: u32,
    height: u32,
) {
    let mut i = 0;
    while i < points.len() {
      let j = (i + 1) % points.len();
      let point1 = points[i];
      let point2 = points[j];
      draw_line(
          frame_buff, point1.0, point1.1, point2.0, point2.1, color, width, height,
      );
      i += 1;
    }
}

fn fill_scanline(
  frame_buff: &mut Vec<u32>,
  depth_buff: &mut Vec<u32>,
  buffer_width: usize,
  y: usize,
  x1: usize,
  x2: usize,
  z1: u32,
  z2: u32,
  u1: i32,
  u2: i32,
  v1: i32,
  v2: i32,
  texture: &Texture,
  depth_test: bool,
) {
    let buffer_row = buffer_width * y;
    let xdiff = (x2 - x1) as f32;
    let z_step = (((z2 as i64) - (z1 as i64)) as f32) / xdiff;
    let u_step = ((u2 - u1) as f32) / xdiff;
    let v_step = ((v2 - v1) as f32) / xdiff;
    let mut x = x1;
    let mut z = z1 as f32;
    let mut u = u1 as f32;
    let mut v = v1 as f32;
    while x < x2 {
      let index = buffer_row + x;
      let zu32 = z as u32;
      if depth_test && zu32 > qfetch_pixel(depth_buff, index) {
        x += 1;
        z += z_step;
        u += u_step;
        v += v_step;
        continue;
      }

      unsafe {
        let [r, g, b, a] = SAMPLE_TEXTURE(&texture, u as i32, v as i32, x, y);
        qfill_pixel(frame_buff, index, from_rgba(r, g, b, a));
      }
      qfill_pixel(depth_buff, index, zu32);
      x += 1;
      z += z_step;
      u += u_step;
      v += v_step;
    }
}

fn pfill_scanline(
  frame_buff: &mut Vec<u32>,
  depth_buff: &mut Vec<u32>,
  buffer_width: usize,
  y: usize,
  x1: usize,
  x2: usize,
  z1: u32,
  z2: u32,
  u1: i32,
  u2: i32,
  v1: i32,
  v2: i32,
  texture: &Texture,
  depth_test: bool,
) {
    let buffer_row = buffer_width * y;
    let xdiff = (x2 - x1) as f32;
    let mut x = x1;
    while x < x2 {
      let index = buffer_row + x;
      let alpha = (x as f32 - x1 as f32) / xdiff;
      let z = interpolate_u32(z1, z2, alpha);
      let u = interpolate_i32(u1, u2, alpha);
      let v = interpolate_i32(v1, v2, alpha);
      if depth_test && z > qfetch_pixel(depth_buff, index) {
        x += 1;
        continue;
      }

      unsafe {
        let [r, g, b, a] = SAMPLE_TEXTURE(&texture, u as i32, v as i32, x, y);
        qfill_pixel(frame_buff, index, from_rgba(r, g, b, a));
      }
      qfill_pixel(depth_buff, index, z);
      x += 1;
    }
}


fn sfill_scanline(
  frame_buff: &mut Vec<u32>,
  depth_buff: &mut Vec<u32>,
  buffer_width: usize,
  y: usize,
  x1: usize,
  x2: usize,
  z1: u32,
  z2: u32,
  u1: i32,
  u2: i32,
  v1: i32,
  v2: i32,
  texture: &Texture,
  depth_test: bool,
) {
    let buffer_row = buffer_width * y;
    let xdiff = (x2 - x1) as i32;
    let mut x = x1;
    let mut z = (z1 as i64) << 16;
    let mut u = u1;
    let mut v = v1;
    let z_step = (((z2 as i64) << 16) - z) / (xdiff as i64);
    let u_step = (u2 - u) / xdiff;
    let v_step = (v2 - v) / xdiff;
    // let (z_step, u_step, v_step) =  (
    //   z_diff.div_euclid(xdiff),
    //   u_diff.div_euclid(xdiff),
    //   v_diff.div_euclid(xdiff),
    // );
    while x < x2 {
      let index = buffer_row + x;
      let zu32 = (z >> 16) as u32;
      if depth_test && zu32 > qfetch_pixel(depth_buff, index) {
        x += 1;
        z += z_step;
        u += u_step;
        v += v_step;
        continue;
      }

      unsafe {
        let [r, g, b, a] = SAMPLE_TEXTURE(&texture, u, v, x, y);
        qfill_pixel(frame_buff, index, from_rgba(r, g, b, a));
      }
      qfill_pixel(depth_buff, index, zu32);
      x += 1;
      z += z_step;
      u += u_step;
      v += v_step;
    }
}

// Fill a polygon with perspective correction
fn fill_scanline_pc(
  frame_buff: &mut Vec<u32>,
  depth_buff: &mut Vec<u32>,
  buffer_width: usize,
  y: usize,
  x1: usize,
  x2: usize,
  w1: f32,
  w2: f32,
  z1: u32,
  z2: u32,
  u1: i32,
  u2: i32,
  v1: i32,
  v2: i32,
  texture: &Texture,
  depth_test: bool,
) {
    let buffer_row = buffer_width * y;
    let mut x = x1;
    let mut alpha: f32 = 0.0;
    let alpha_step = 1.0 / (x2 - x1) as f32;
    // OPT There are a few operations that done everytime we do perspective
    // correction, this could all be batched by have one function that does
    // perspective correct interpolation for an entire vertex
    while x < x2 {
        let index = buffer_row + x;
        let z = interpolate_u32_pc(z1, z2, w1, w2, alpha);
        if depth_test && z > qfetch_pixel(depth_buff, index) {
            x += 1;
            alpha += alpha_step;
            continue;
        }
        let u = interpolate_i32_pc(u1, u2, w1, w2, alpha);
        let v = interpolate_i32_pc(v1, v2, w1, w2, alpha);

        unsafe {
          let [r, g, b, a] = SAMPLE_TEXTURE(&texture, u as i32, v as i32, x, y);
          qfill_pixel(frame_buff, index, from_rgba(r, g, b, a));
        }
        qfill_pixel(depth_buff, index, z);
        x += 1;
        alpha += alpha_step;
    }
}

// Fill a polygon with perspective correction reduced to every Nth pixel
// Linearly interpolate in between
fn fill_scanline_reduced(
  frame_buff: &mut Vec<u32>,
  depth_buff: &mut Vec<u32>,
  buffer_width: usize,
  y: usize,
  x1: usize,
  x2: usize,
  w1: f32,
  w2: f32,
  z1: u32,
  z2: u32,
  u1: i32,
  u2: i32,
  v1: i32,
  v2: i32,
  texture: &Texture,
  depth_test: bool,
  perspective_reduction: usize,
) {
    let diff_x = x2 - x1;
    // If the difference between the x's is less than or equal to the reduction gap, just do a linear fill
    if diff_x <= perspective_reduction {
        fill_scanline(
          frame_buff,
          depth_buff,
          buffer_width,
          y,
          x1,
          x2,
          z1,
          z2,
          u1,
          u2,
          v1,
          v2,
          texture,
          depth_test,
        );
        return;
    }
    let step = perspective_reduction;
    let mut x = x1;
    let mut next_x = x1 + step;
    let alpha_step = (1.0 / diff_x as f32) * (step as f32);
    let mut alpha = alpha_step;

    let mut prev_z = z1;
    let mut prev_u = u1;
    let mut prev_v = v1;

    while x < x2 {
        let z = interpolate_u32_pc(z1, z2, w1, w2, alpha);
        let u = interpolate_i32_pc(u1, u2, w1, w2, alpha);
        let v = interpolate_i32_pc(v1, v2, w1, w2, alpha);
        fill_scanline(
          frame_buff,
          depth_buff,
          buffer_width,
          y,
          x,
          next_x,
          prev_z,
          z,
          prev_u,
          u,
          prev_v,
          v,
          texture,
          depth_test,
        );
        x += step; // Don't just use the last x, otherwise we'll end up in an infinite loop
        next_x += step;
        alpha += alpha_step;
        if next_x > x2 {
            next_x = x2;
            alpha = 1.0;
        }
        prev_z = z;
        prev_u = u;
        prev_v = v;
    }
}

fn fill_scanline_scrn(
  frame_buff: &mut Vec<u32>,
  depth_buff: &mut Vec<u32>,
  buffer_width: usize,
  y: usize,
  x1: usize,
  x2: usize,
  z1: u32,
  z2: u32,
  texture: &Texture,
  depth_test: bool,
) {
    let buffer_row = buffer_width * y;
    let xdiff = (x2 - x1) as f32;
    let z_step = (((z2 as i64) - (z1 as i64)) as f32) / xdiff;
    let mut x = x1;
    let mut z = z1 as f32;
    while x < x2 {
      let index = buffer_row + x;
      let zu32 = z as u32;
      if depth_test && zu32 > qfetch_pixel(depth_buff, index) {
        x += 1;
        z += z_step;
        continue;
      }

      let lod = &texture.lods[0];
      let lod_width = lod.width as usize;
      let index = (lod_width * (y % (lod.height as usize))) + (x % lod_width);
      let index = index << 1 + index;
      let r = lod.data[index];
      let g = lod.data[index + 1];
      let b = lod.data[index + 2];
      qfill_pixel(frame_buff, index, from_rgba(r, g, b, 255));
      qfill_pixel(depth_buff, index, zu32);
      x += 1;
      z += z_step;
    }
}

fn plot_ends_1(
    point1: ProjectedVertex,
    point2: ProjectedVertex,
    /* (x1, x2, w1, w2, z1, z2) */
    spans: &mut Vec<(usize, usize, f32, f32, u32, u32)>,
    y_top: usize,
    width: usize,
    perspective: bool,
) {
    let ord = point1.y < point2.y;
    let p1 = if ord { point1 } else { point2 };
    let p2 = if ord { point2 } else { point1 };
    let y1 = p1.y;
    let y2 = p2.y;
    // Ignore horizontal lines
    if y1 < y2 {
        let diffy = y2 - y1;
        let diffx = p2.x as isize - p1.x as isize;
        let y_end = y2 - y_top;
        let mut y = y1 - y_top;
        // Change the alpha using addition instead of using division to calculate it each time
        let alpha_step = 1.0 / diffy as f32;
        let mut alpha = 0.0;
        // Change the x using addition instead of interpolating it each time
        let x_step = diffx as f32 / diffy as f32;
        let mut xf32 = p1.x as f32;
        // Change the w using addition instead of interpolating it each time
        let w_step = (p2.w - p1.w) / diffy as f32;
        let mut w = p1.w;
        while y < y_end {
            let x = xf32.round() as usize;
            let z = if perspective {
                interpolate_u32_pc(p1.z, p2.z, p1.w, p2.w, alpha)
            } else {
                interpolate_u32(p1.z, p2.z, alpha)
            };
            if spans[y].1 > width {
                spans[y].0 = x;
                spans[y].1 = x;
                spans[y].2 = w;
                spans[y].3 = w;
                spans[y].4 = z;
                spans[y].5 = z;
            } else {
                // println!("DOES THIS EVER HAPPEN?");
                if x < spans[y].1 {
                    spans[y].0 = x;
                    spans[y].2 = w;
                    spans[y].4 = z;
                } else if x > spans[y].0 {
                    spans[y].1 = x;
                    spans[y].3 = w;
                    spans[y].5 = z;
                }
            }
            y += 1;
            alpha += alpha_step;
            xf32 += x_step;
            w += w_step;
        }
    }
}
// Fill a polygon with a solid color
pub fn fill_polygon_1(
    points: Vec<ProjectedVertex>,
    color: u32,
    pc: bool,
    dt: bool,
    width: usize,
    y_bounds: (usize, usize),
    frame_buff: &mut Vec<u32>,
    depth_buff: &mut Vec<u32>,
) {
    let y_start = y_bounds.0;
    let y_end = y_bounds.1;
    let y_diff = y_end - y_start;
    let mut spans = vec![(width + 1, width + 1, 0.0, 0.0, 0, 0,); y_diff];

    let len = points.len();
    let mut i = 0;
    while i < len {
        let point1 = points[i];
        let point2 = points[mod_from_range_usize(i + 1, len)];
        plot_ends_1(point1, point2, &mut spans, y_start, width, pc);
        i += 1;
    }

    i = 0;
    while i < y_diff {
        let y = i + y_start;
        let row = spans[i];
        let mut x = row.0;
        let end = row.1;
        if x == end {
            i += 1;
            continue;
        }
        let mut alpha = 0.0;
        let alpha_step = 1.0 / (row.1 - row.0) as f32;
        let buffer_row = y * width;
        while x < end {
            let index = buffer_row + x;
            let z = if pc {
                interpolate_u32_pc(row.4, row.5, row.2, row.3, alpha)
            } else {
                interpolate_u32(row.4, row.5, alpha)
            };
            let depth = if dt {
                qfetch_pixel(depth_buff, index)
            } else {
                u32::MAX
            };
            if z < depth {
                qfill_pixel(frame_buff, index, color);
                qfill_pixel(depth_buff, index, z);
            }
            x += 1;
            alpha += alpha_step;
        }
        i += 1;
    }
}

// Fill a polygon with a texture
pub fn fill_polygon_2(
    frame_buff: &mut Vec<u32>,
    depth_buff: &mut Vec<u32>,
    buffer_width: usize,
    points: Vec<ProjectedVertex>,
    y_bounds: (usize, usize),
    material: &Material,
    texture_map_mode: i8,
    texture_map_reduction: usize,
    depth_test: bool,
) {
    let y_start = y_bounds.0;
    let y_end = y_bounds.1;
    let y_diff = y_end - y_start;
    // Create a vector of spans which is the height of the polygon in screen space
    let mut spans = vec![[ProjectedVertex::for_span(buffer_width + 1); 2]; y_diff];

    let len = points.len();
    let mut i = 0;
    while i < len {
        let point1 = points[i];
        let point2 = points[mod_from_range_usize(i + 1, len)];
        plot_ends_2(
          point1,
          point2,
          &mut spans,
          y_start,
          buffer_width
        );
        i += 1;
    }
    // println!("SPANS: {:?}", spans);
    // panic!("");

    let texture = resources::get_texture(material.texture as usize);
    i = 0;
    if texture_map_mode == TEXTURE_MAP_MODE_AFFINE {
      while i < y_diff {
        let y = i + y_start;
        let span = spans[i];
        let x1 = span[0].x;
        let x2 = span[1].x;
        if x1 == x2 {
            i += 1;
            continue;
        }
        fill_scanline(
          frame_buff,
          depth_buff,
          buffer_width,
            y,
            x1,
            x2,
            span[0].z,
            span[1].z,
            span[0].u,
            span[1].u,
            span[0].v,
            span[1].v,
            texture,
            depth_test,
        );
        i += 1;
      }
    } else if texture_map_mode == TEXTURE_MAP_MODE_PERSPECTIVE || texture_map_reduction == 0 {
      while i < y_diff {
        let y = i + y_start;
        let span = spans[i];
        let x1 = span[0].x;
        let x2 = span[1].x;
        if x1 == x2 {
            i += 1;
            continue;
        }
        fill_scanline_pc(
          frame_buff,
          depth_buff,
          buffer_width,
          y,
          x1,
          x2,
          span[0].w,
          span[1].w,
          span[0].z,
          span[1].z,
          span[0].u,
          span[1].u,
          span[0].v,
          span[1].v,
          texture,
          depth_test,
        );
        i += 1;
      }
    } else if texture_map_mode == TEXTURE_MAP_MODE_REDUCED {
      while i < y_diff {
        let y = i + y_start;
        let span = spans[i];
        let x1 = span[0].x;
        let x2 = span[1].x;
        if x1 == x2 {
            i += 1;
            continue;
        }
        fill_scanline_reduced(
          frame_buff,
          depth_buff,
          buffer_width,
            y,
            x1,
            x2,
            span[0].w,
            span[1].w,
            span[0].z,
            span[1].z,
            span[0].u,
            span[1].u,
            span[0].v,
            span[1].v,
            texture,
            depth_test,
            texture_map_reduction
        );
        i += 1;
      }
    } else if texture_map_mode == TEXTURE_MAP_MODE_SCREEN {
      while i < y_diff {
        let y = i + y_start;
        let span = spans[i];
        let x1 = span[0].x;
        let x2 = span[1].x;
        if x1 == x2 {
            i += 1;
            continue;
        }
        fill_scanline_scrn(
          frame_buff,
          depth_buff,
          buffer_width,
            y,
            x1,
            x2,
            span[0].z,
            span[1].z,
            texture,
            depth_test,
        );
        i += 1;
      }
    }
}

fn plot_ends_2(
    point1: ProjectedVertex,
    point2: ProjectedVertex,
    spans: &mut Vec<[ProjectedVertex; 2]>,
    y_top: usize,
    width: usize,
) {
    let ord = point1.y < point2.y;
    // Sort the vertices by y value
    let p1 = if ord { point1 } else { point2 };
    let p2 = if ord { point2 } else { point1 };
    let y1 = p1.y;
    let y2 = p2.y;
    // Ignore horizontal lines
    if y1 < y2 {
      let diffy = y2 - y1;
      let mut y = y1 - y_top;
      let y_end = y2 - y_top;
      // Scale the x by 65536 and cast to i32 so that we can mimic decimal precision
      // without using floating point numbers to interpolate integers
      let mut xi32 = (p1.x as i32) << 16;
      let diffx = ((p2.x as i32) << 16) - xi32;
      // Change the alpha using addition instead of using division to calculate it each time
      let alpha_step = 1.0 / diffy as f32;
      let mut alpha = 0.0;
      // Change the x using addition instead of interpolating it each time
      let x_step = diffx / diffy as i32;
      // Change the w using addition instead of interpolating it each time
      let w_step = (p2.w - p1.w) / diffy as f32;
      // The w must be floating-point number for perspective corrrection
      let mut w = p1.w;
      while y < y_end {
        let x = (xi32 >> 16) as usize;
        // Set the y to zero since y is just an index into a vec in this context
        let vert = interpolate_projected_vertex(p1, p2, alpha, x, y, w);
        
        if spans[y][1].x > width {
          spans[y][0] = vert;
          spans[y][1] = vert;

          // spans[y][0].x = x;
          // spans[y][0].w = w;
          // spans[y][0].z = vert.z;
          // spans[y][0].u = vert.u;
          // spans[y][0].v = vert.v;
          // spans[y][0].nx = vert.nx;
          // spans[y][0].ny = vert.ny;
          // spans[y][0].nz = vert.nz;
          // spans[y][0].wx = vert.wx;
          // spans[y][0].wy = vert.wy;
          // spans[y][0].wz = vert.wz;
          // spans[y][0].mm = vert.mm;
          // spans[y][1].x = x;
          // spans[y][1].w = w;
          // spans[y][1].z = vert.z;
          // spans[y][1].u = vert.u;
          // spans[y][1].v = vert.v;
          // spans[y][1].nx = vert.nx;
          // spans[y][1].ny = vert.ny;
          // spans[y][1].nz = vert.nz;
          // spans[y][1].wx = vert.wx;
          // spans[y][1].wy = vert.wy;
          // spans[y][1].wz = vert.wz;
          // spans[y][1].mm = vert.mm;
        } else {
          if x < spans[y][1].x {
            spans[y][0] = vert;

            // spans[y][0].x = x;
            // spans[y][0].w = w;
            // spans[y][0].z = vert.z;
            // spans[y][0].u = vert.u;
            // spans[y][0].v = vert.v;
            // spans[y][0].nx = vert.nx;
            // spans[y][0].ny = vert.ny;
            // spans[y][0].nz = vert.nz;
            // spans[y][0].wx = vert.wx;
            // spans[y][0].wy = vert.wy;
            // spans[y][0].wz = vert.wz;
            // spans[y][0].mm = vert.mm;
          } else if x > spans[y][0].x {
            spans[y][1] = vert;

            // spans[y][1].x = x;
            // spans[y][1].w = w;
            // spans[y][1].z = vert.z;
            // spans[y][1].u = vert.u;
            // spans[y][1].v = vert.v;
            // spans[y][1].nx = vert.nx;
            // spans[y][1].ny = vert.ny;
            // spans[y][1].nz = vert.nz;
            // spans[y][1].wx = vert.wx;
            // spans[y][1].wy = vert.wy;
            // spans[y][1].wz = vert.wz;
            // spans[y][1].mm = vert.mm;
          }
        }
        y += 1;
        alpha += alpha_step;
        xi32 += x_step;
        w += w_step;
      }
    }
}

// fn plot_ends_3(
//     point1: ProjectedVertex,
//     point2: ProjectedVertex,
//     ends: &mut HashMap<usize, (usize, usize, f32, u32, u32, f32, f32, f32, f32)>,
// ) {
//     let ord = point1.1 < point2.1;
//     let p1 = if ord { point1 } else { point2 };
//     let p2 = if ord { point2 } else { point1 };
//     let y1 = p1.1;
//     let y2 = p2.1;
//     if y1 < y2 {
//         let diffy = p2.1 - p1.1;
//         let diffx = p2.0 - p1.0;
//         let mut i = p1.1.round() + 0.5 - p1.1;
//         let mut y = y1;
//         while y < y2 {
//             let alpha = i / diffy;
//             let x = diffx * alpha + p1.0;
//             let w = interpolate_f32(p1.3, p2.3, alpha);
//             let z = interpolate_u32(p1.2, p2.2, alpha);
//             let u = interpolate_f32(p1.7, p2.7, alpha);
//             let v = interpolate_f32(p1.8, p2.8, alpha);
//             if ends.contains_key(&y) {
//                 let mut row = ends.remove(&y).expect("Failed to extract row");
//                 if x < row.1 {
//                     row.0 = x;
//                     row.2 = w;
//                     row.4 = z;
//                     row.6 = u;
//                     row.8 = v;
//                 } else if x > row.0 {
//                     row.1 = x;
//                     row.3 = w;
//                     row.5 = z;
//                     row.7 = u;
//                     row.9 = v;
//                 }
//                 ends.insert(y, row);
//             } else {
//                 ends.insert(y, (x, x, w, w, z, z, u, u, v, v));
//             }
//             i += 1.0;
//             y += 1;
//         }
//     }
// }
// fn plot_ends_4(
//     point1: ProjectedVertex,
//     point2: ProjectedVertex,
//     ends: &mut HashMap<isize, (f32, f32, u32, u32, [f32; 3], [f32; 3], [f32; 3], [f32; 3])>,
// ) {
//     let ord = point1.1 < point2.1;
//     let p1 = if ord { point1 } else { point2 };
//     let p2 = if ord { point2 } else { point1 };
//     let y1 = p1.1.round() as isize;
//     let y2 = p2.1.round() as isize;
//     if y1 < y2 {
//         let diffy = p2.1 - p1.1;
//         let diffx = p2.0 - p1.0;
//         let mut i = p1.1.round() + 0.5 - p1.1;
//         let mut y = y1;
//         while y < y2 {
//             let alpha = i / diffy;
//             let x = diffx * alpha + p1.0;
//             let z = interpolate_u32(p1.2, p2.2, alpha);
//             let p = interpolate_f32(p1.7, p2.7, alpha);
//             let n = interpolate_f32(p1.8, p2.8, alpha);
//             if ends.contains_key(&y) {
//                 let mut row = ends.remove(&y).expect("Failed to extract row");
//                 if x < row.1 {
//                     row.0 = x;
//                     row.2 = z;
//                     // row.4 = p;
//                     // row.6 = n;
//                 } else if x > row.0 {
//                     row.1 = x;
//                     row.3 = z;
//                     // row.5 = p;
//                     // row.7 = n;
//                 }
//                 ends.insert(y, row);
//             } else {
//                 // ends.insert(y, (x, x, z, z, p, p, n, n));
//             }
//             i += 1.0;
//             y += 1;
//         }
//     }
// }
// // Fill a polygon with colors interpolated from the vertices
// fn fill_polygon_3(
//     frame_buff: &mut Vec<u32>,
//     depth_buff: &mut Vec<u32>,
//     points: Vec<ProjectedVertex>,
//     width: u32,
//     height: u32,
// ) {
//     let mut ends: HashMap<isize, (f32, f32, f32, f32, u32, u32, f32, f32, f32, f32)> =
//         HashMap::new();
//     let len = points.len();
//     let mut i = 0;
//     while i < len {
//         let point1 = points[i];
//         let point2 = points[index_loop_usize(i + 1, len)];
//         plot_ends_3(point1, point2, &mut ends);
//         i += 1;
//     }
//     for (y, row) in ends.iter() {
//         let mut x = row.0.round() as isize;
//         let end = row.1.round() as isize;
//         while x < end {
//             let alpha = (x as f32 - row.0) / (row.1 - row.0);
//             let z = interpolate_u32(row.4, row.5, alpha);
//             let depth = fetch_pixel(depth_buff, x, *y, width, height);
//             if z < depth {
//                 let color = from_rgb(255, 0, 0);
//                 fill_pixel(frame_buff, x, *y, color, width, height);
//                 fill_pixel(depth_buff, x, *y, z, width, height);
//             }
//             x += 1;
//         }
//     }
// }

// // Fill a polygon with normals interpolated from the vertices
// fn fill_polygon_4(
//     frame_buff: &mut Vec<u32>,
//     depth_buff: &mut Vec<u32>,
//     points: Vec<ProjectedVertex>,
//     color: &[u8; 3],
//     ambient: &mut [f32; 3],
//     direction_lights: &Vec<DirectionalLight>,
//     point_lights: &Vec<PointLight>,
//     coloredlighting: bool,
//     width: u32,
//     height: u32,
// ) {
//     let mut ends: HashMap<isize, (f32, f32, u32, u32, [f32; 3], [f32; 3], [f32; 3], [f32; 3])> =
//         HashMap::new();
//     let len = points.len();
//     let mut i = 0;
//     while i < len {
//         let point1 = points[i];
//         let point2 = points[index_loop_usize(i + 1, len)];
//         plot_ends_4(point1, point2, &mut ends);
//         i += 1;
//     }
//     for (y, row) in ends.iter() {
//         let mut x = row.0.round() as isize;
//         let end = row.1.round() as isize;
//         while x < end {
//             let alpha = (x as f32 - row.0) / (row.1 - row.0);
//             let z = interpolate_u32(row.2, row.3, alpha);
//             let depth = fetch_pixel(depth_buff, x, *y, width, height);
//             if z < depth {
//                 let pos = interpolate_pos(row.4, row.5, alpha);
//                 let normal = interpolate_vec(row.6, row.7, alpha);
//                 let color = get_vector_light(
//                     normal,
//                     pos,
//                     *color,
//                     &mut ambient.clone(),
//                     direction_lights,
//                     point_lights,
//                     coloredlighting,
//                 );
//                 fill_pixel(frame_buff, x, *y, color, width, height);
//                 fill_pixel(depth_buff, x, *y, z, width, height);
//             }
//             x += 1;
//         }
//     }
// }

// // Fill a polygon with normals interpolated from the vertices
// // Scanlines only interpolate the normal every n pixels and
// // then interpolate the colors between those normals
// fn fill_polygon_5(
//     frame_buff: &mut Vec<u32>,
//     depth_buff: &mut Vec<u32>,
//     points: Vec<ProjectedVertex>,
//     color: &[u8; 3],
//     ambient: &mut [f32; 3],
//     direction_lights: &Vec<DirectionalLight>,
//     point_lights: &Vec<PointLight>,
//     coloredlighting: bool,
//     n: usize,
//     width: u32,
//     height: u32,
// ) {
//     let mut ends: HashMap<isize, (f32, f32, u32, u32, [f32; 3], [f32; 3], [f32; 3], [f32; 3])> =
//         HashMap::new();
//     let len = points.len();
//     let mut i = 0;
//     while i < len {
//         let point1 = points[i];
//         let point2 = points[index_loop_usize(i + 1, len)];
//         plot_ends_4(point1, point2, &mut ends);
//         i += 1;
//     }
//     for (y, row) in ends.iter() {
//         let mut x = row.0.round() as isize;
//         let end = row.1.round() as isize;
//         let mut last_color: u32 = 0;
//         let mut last_x = -1;
//         let mut last_z = u32::MAX;
//         let mut point_drawn = true;
//         while x < end {
//             let alpha = (x as f32 - row.0) / (row.1 - row.0);
//             let z = interpolate_u32(row.2, row.3, alpha);
//             let depth = fetch_pixel(depth_buff, x, *y, width, height);
//             if z < depth {
//                 let pos = interpolate_pos(row.4, row.5, alpha);
//                 let normal = interpolate_vec(row.6, row.7, alpha);
//                 let color = get_vector_light(
//                     normal,
//                     pos,
//                     *color,
//                     &mut ambient.clone(),
//                     direction_lights,
//                     point_lights,
//                     coloredlighting,
//                 );
//                 fill_pixel(frame_buff, x, *y, color, width, height);
//                 fill_pixel(depth_buff, x, *y, z, width, height);
//                 point_drawn = true;
//                 if last_x != -1 {
//                     let mut i = last_x + 1;
//                     while i < x {
//                         let alpha = (i as f32 - last_x as f32) / (x - last_x) as f32;
//                         let z = interpolate_u32(last_z, z, alpha);
//                         let depth = fetch_pixel(depth_buff, i, *y, width, height);
//                         if z < depth {
//                             let color = interpolate_color(last_color, color, alpha);
//                             fill_pixel(frame_buff, i, *y, color, width, height);
//                             fill_pixel(depth_buff, i, *y, z, width, height);
//                         }
//                         i += 1;
//                     }
//                 }
//                 last_color = color;
//                 last_x = x;
//                 last_z = z;
//                 if x + n as isize >= end {
//                     x = end - 1;
//                 } else {
//                     x += n as isize;
//                 }
//             } else {
//                 if point_drawn {
//                     // If we hit an occluded pixel, calc the color for when the scanline comes back into view
//                     let pos = interpolate_pos(row.4, row.5, alpha);
//                     let normal = interpolate_vec(row.6, row.7, alpha);
//                     let color = get_vector_light(
//                         normal,
//                         pos,
//                         *color,
//                         &mut ambient.clone(),
//                         direction_lights,
//                         point_lights,
//                         coloredlighting,
//                     );
//                     if last_x != -1 {
//                         let mut i = last_x + 1;
//                         while i < x {
//                             let alpha = (i as f32 - last_x as f32) / (x - last_x) as f32;
//                             let z = interpolate_u32(last_z, z, alpha);
//                             let depth = fetch_pixel(depth_buff, i, *y, width, height);
//                             if z < depth {
//                                 let color = interpolate_color(last_color, color, alpha);
//                                 fill_pixel(frame_buff, i, *y, color, width, height);
//                                 fill_pixel(depth_buff, i, *y, z, width, height);
//                             }
//                             i += 1;
//                         }
//                     }
//                     last_color = color;
//                     last_x = x;
//                     last_z = z;
//                     point_drawn = false;
//                 }
//                 x += 1;
//             }
//         }
//     }
// }
