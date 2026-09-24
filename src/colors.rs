use crate::{interpolate::interpolate_u8, utils::q_interpolate_u8};

pub fn from_rgb(r: u8, g: u8, b: u8) -> u32 {
    return ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
}
pub fn to_rgb(int: u32) -> [u8; 3] {
    return [
        (int >> 16 & 255) as u8,
        (int >> 8 & 255) as u8,
        (int & 255) as u8,
    ];
}
pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    return ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
}
pub fn to_rgba(int: u32) -> [u8; 4] {
    return [
        (int >> 16 & 255) as u8,
        (int >> 8 & 255) as u8,
        (int & 255) as u8,
        (int >> 24 & 255) as u8,
    ];
}
pub fn greyscale(r: u8, g: u8, b: u8) -> u8 {
    return ((r as u16 + g as u16 + b as u16) / 3) as u8;
}
pub fn q_blend_colors(color1: u32, color2: u32, alpha: u8) -> u32 {
    let c1 = to_rgb(color1);
    let c2 = to_rgb(color2);
    return from_rgb(
      q_interpolate_u8(c1[0], c2[0], alpha),
      q_interpolate_u8(c1[1], c2[1], alpha),
      q_interpolate_u8(c1[2], c2[2], alpha),
    );
}
pub fn blend_colors(color1: u32, color2: u32, alpha: f32) -> u32 {
    let c1 = to_rgb(color1);
    let c2 = to_rgb(color2);
    return from_rgb(
        interpolate_u8(c1[0], c2[0], alpha),
        interpolate_u8(c1[1], c2[1], alpha),
        interpolate_u8(c1[2], c2[2], alpha),
    );
}
pub fn rgb565_to_rgb(byte1: u8, byte2: u8) -> [u8; 3] {
    let red = byte2 & 0b11111000;
    let green = ((byte2 & 0b00000111) << 5) | ((byte1 & 0b11100000) >> 3);
    let blue = (byte1 & 0b00011111) << 3;
    return [red, green, blue];
}
pub fn argb1555_to_rgba(byte1: u8, byte2: u8) -> [u8; 4] {
    let alpha = ((byte1 & 0b10000000) != 0) as u8 * 255;
    let red = (byte2 & 0b01111100) << 1;
    let green = ((byte2 & 0b00000011) << 6) | ((byte1 & 0b11100000) >> 2);
    let blue = (byte1 & 0b00011111) << 3;
    return [red, green, blue, alpha];
}
