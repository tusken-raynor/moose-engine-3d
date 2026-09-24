use std::{collections::HashMap, fs};

use crate::entity::Entity;

pub fn sort_things(things: &mut Vec<Entity>, closure: &dyn Fn(&Entity, &Entity) -> bool) {
    let len = things.len();
    _quick_sort_things(things, 0, (len - 1) as isize, closure);
}

fn _quick_sort_things(
    vec: &mut Vec<Entity>,
    low: isize,
    high: isize,
    closure: &dyn Fn(&Entity, &Entity) -> bool,
) {
    if low < high {
        let p = partition(vec, low, high, closure);
        _quick_sort_things(vec, low, p - 1, closure);
        _quick_sort_things(vec, p + 1, high, closure);
    }
}

fn partition(
    arr: &mut Vec<Entity>,
    low: isize,
    high: isize,
    closure: &dyn Fn(&Entity, &Entity) -> bool,
) -> isize {
    let pivot = high as usize;
    let mut store_index = low - 1;
    let mut last_index = high;

    loop {
        store_index += 1;
        while store_index < high && !closure(&arr[store_index as usize], &arr[pivot]) {
            store_index += 1;
        }
        last_index -= 1;
        while last_index >= low && closure(&arr[last_index as usize], &arr[pivot]) {
            last_index -= 1;
        }
        if store_index >= last_index {
            break;
        } else {
            arr.swap(store_index as usize, last_index as usize);
        }
    }
    arr.swap(store_index as usize, pivot as usize);
    store_index
}

// This is used to shift a color channel value from 0-255 to -128 to 127
// for displacement mapping and other effects without using a type cast
// operation or a subtraction operation
pub fn bmpc_shift(a: u8) -> i8 {
  unsafe { std::mem::transmute::<u8, i8>(a ^ 128) }
}
// Mutliply a u8 by a u8 scalar and return the result as a scaled u8
pub fn approx_multiply_u8(a: u8, b: u8) -> u8 {
  ((((a as u16) * (b as u16))) >> 8) as u8
}
pub fn approx_multiply_u8_u16(a: u8, b: u16) -> u8 {
  ((((a as u32) * (b as u32))) >> 16) as u8
}
// Mutliply an i32 by a u16 scalar and return the result as a scaled i32
pub fn approx_multiply_i32_u16(a: i32, b: u16) -> i32 {
    ((((a as i64) * (b as i64)) + 65535) >> 16) as i32
}
// Mutliply a u32 by a u16 scalar and return the result as a scaled u32
pub fn approx_multiply_u32_u16(a: u32, b: u16) -> u32 {
    ((((a as u64) * (b as u64)) + 65535) >> 16) as u32
}
// Optimized method to interpolate between two u8 values using a u8 alpha
pub fn q_interpolate_u8(c1: u8, c2: u8, f: u8) -> u8 {
    // let c1 = if c1 > c2 { c2 } else { c1 };
    // let c2 = if c1 > c2 { c1 } else { c2 };
    // approx_multiply_u8(c2 - c1, f) + c1
    // For some reason the following is faster than the above
    approx_multiply_u8(c1, 255 - f) + approx_multiply_u8(c2, f)
}
// Optimized method to interpolate between two u8 values using a u8 alpha
pub fn q_interpolate_u16(c1: u8, c2: u8, a: u16) -> u8 {
    approx_multiply_u8_u16(c1, 255 - a) + approx_multiply_u8_u16(c2, a)
}
pub fn scaled_rgb(r: u8, g: u8, b: u8, scale: u8) -> u32 {
    let cm: u64 = ((b as u64) + ((g as u64) << 16) + ((r as u64) << 32)) * ((scale as u64) + 1);
    let sh = (cm) >> 8;
    (sh & 255 | sh >> 8 & 65280 | sh >> 16 & 16711680) as u32
    // println!("{}, {}, {}, {}", sh & 255, sh >> 8 & 255, sh >> 24, cm);
}
// Optimized version of modulus 1.0 for f32 values
pub fn unit_mod_f32(num: f32) -> f32 {
    return num - num.floor();
}

pub fn mod_from_range_usize(num: usize, modulus: usize) -> usize {
    if num >= modulus {
        return num - modulus;
    }
    return num as usize;
}
// Transform a unit coordinate so that odd integered values
// are flipped across their respective axis
pub fn tex_coord_odd_flip_f32(val: f32) -> f32 {
    let v_int = val as i32;
    let num = val - (v_int as f32);
    if (v_int & 1) == 1 {
        0.9999 - num
    } else {
        num
    }
}
pub fn is_power_of_two(n: u16) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

pub fn starts_with_number(s: &String) -> bool {
    match s.parse::<usize>() {
        Ok(_) => true,
        Err(_) => false,
    }
}

pub fn parse_numbers_from_line<T: std::str::FromStr>(line: &str, skip_count: usize) -> Result<Vec<T>, &'static str> {
  let parts = line.split_whitespace().skip(skip_count);

  let mut numbers = Vec::new();

  for part in parts {
      if let Ok(number) = part.parse::<T>() {
          numbers.push(number);
      } else {
          return Err("Failed to parse number");
      }
  }

  Ok(numbers)
}