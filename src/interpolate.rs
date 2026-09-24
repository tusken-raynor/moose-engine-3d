use crate::{
    colors::{from_rgb, to_rgb},
    threedee::normalize_vec,
    utils::{approx_multiply_i32_u16, approx_multiply_u32_u16},
};

// interpolate between two usize numbers
pub fn interpolate_usize(a: usize, b: usize, alpha: f32) -> usize {
    let a = a as f32;
    let b = b as f32;
    let diff = b - a;
    let result = diff * alpha + a;
    result as usize
}
// interpolate between two u32 numbers
pub fn interpolate_u32(a: u32, b: u32, alpha: f32) -> u32 {
    let a = a as f32;
    let b = b as f32;
    let diff = b - a;
    let result = diff * alpha + a;
    result as u32
}
// quickly approximate interpolation between two u32 numbers
pub fn q_interpolate_u32(a: u32, b: u32, alpha: u16) -> u32 {
    approx_multiply_u32_u16(a, 65535 - alpha) + approx_multiply_u32_u16(b, alpha)
}
// interpolate between two u16 number using an alpha
pub fn interpolate_u16(a: u16, b: u16, alpha: f32) -> u16 {
    let a = a as f32;
    let b = b as f32;
    let diff = b - a;
    let result = diff * alpha + a;
    result as u16
}
// interpolate between two i32 numbers
pub fn interpolate_i32(a: i32, b: i32, alpha: f32) -> i32 {
    let a = a as f32;
    let b = b as f32;
    let diff = b - a;
    let result = diff * alpha + a;
    result as i32
}
// quickly approximate interpolation between two i32 numbers
pub fn q_interpolate_i32(a: i32, b: i32, alpha: u16) -> i32 {
    approx_multiply_i32_u16(a, 65535 - alpha) + approx_multiply_i32_u16(b, alpha)
}
// interpolate between two u8 numbers
pub fn interpolate_u8(a: u8, b: u8, alpha: f32) -> u8 {
    let a = a as f32;
    let b = b as f32;
    let diff = b - a;
    let result = diff * alpha + a;
    result as u8
}
// interpolate between two f32 numbers
pub fn interpolate_f32(a: f32, b: f32, alpha: f32) -> f32 {
    let diff = b - a;
    let result = diff * alpha + a;
    result
}
// interpolate between two colors
pub fn interpolate_color(a: u32, b: u32, alpha: f32) -> u32 {
    let a = to_rgb(a);
    let b = to_rgb(b);
    return from_rgb(
        interpolate_u8(a[0], b[0], alpha),
        interpolate_u8(a[1], b[1], alpha),
        interpolate_u8(a[2], b[2], alpha),
    );
}
// interpolate between two vectors
pub fn interpolate_vec(a: [f32; 3], b: [f32; 3], alpha: f32) -> [f32; 3] {
    if a[0] == b[0] && a[1] == b[1] && a[2] == b[2] {
        return a;
    }
    return normalize_vec([
        interpolate_f32(a[0], b[0], alpha),
        interpolate_f32(a[1], b[1], alpha),
        interpolate_f32(a[2], b[2], alpha),
    ]);
}
// interpolate between two vectors, vector components are spread as arguments
pub fn interpolate_vec_s(
    ax: f32,
    ay: f32,
    az: f32,
    bx: f32,
    by: f32,
    bz: f32,
    alpha: f32,
) -> [f32; 3] {
    if ax == bx && ay == by && az == bz {
        return [ax, ay, az];
    }
    return normalize_vec([
        interpolate_f32(ax, bx, alpha),
        interpolate_f32(ay, by, alpha),
        interpolate_f32(az, bz, alpha),
    ]);
}
// interpolate between two positions
pub fn interpolate_pos(a: [f32; 3], b: [f32; 3], alpha: f32) -> [f32; 3] {
    return [
        interpolate_f32(a[0], b[0], alpha),
        interpolate_f32(a[1], b[1], alpha),
        interpolate_f32(a[2], b[2], alpha),
    ];
}
// interpolate between two u32 numbers with a perspective correction
pub fn interpolate_u32_pc(a: u32, b: u32, aw: f32, bw: f32, alpha: f32) -> u32 {
    let a = a as f32;
    let b = b as f32;
    let ialpha = 1.0 - alpha;
    let aw = aw * ialpha;
    let bw = bw * alpha;
    let result = (a * aw + b * bw) / (aw + bw);
    result as u32
}
// interpolate between two i32 numbers with a perspective correction
pub fn interpolate_i32_pc(a: i32, b: i32, aw: f32, bw: f32, alpha: f32) -> i32 {
    let a = a as f32;
    let b = b as f32;
    let ialpha = 1.0 - alpha;
    let aw = aw * ialpha;
    let bw = bw * alpha;
    let result = (a * aw + b * bw) / (aw + bw);
    result as i32
}
// interpolate between two f32 numbers with a perspective correction
pub fn interpolate_f32_pc(a: f32, b: f32, aw: f32, bw: f32, alpha: f32) -> f32 {
    let ialpha = 1.0 - alpha;
    let aw = aw * ialpha;
    let bw = bw * alpha;
    (a * aw + b * bw) / (aw + bw)
}
// Get the alpha (normalized intermediary) between points 'a' and 'b' at intermediate point 'i'
pub fn derive_alpha_f32(a: f32, b: f32, i: f32) -> f32 {
  (i - a) / (b - a)
}