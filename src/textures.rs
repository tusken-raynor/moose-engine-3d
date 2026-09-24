use image::ColorType;

use crate::{
  resources, colors::to_rgb
};

#[derive(Debug, Clone)]
pub struct Bitmap {
    pub data: Vec<u8>,
    pub width: u16,
    pub height: u16,
    pub log2x: u8,
    pub log2y: u8,
    pub linear_offsetx: u16,
    pub linear_offsety: u16,
}

impl Bitmap {
    pub fn from(filename: &String) -> Bitmap {
        let filepath = resources::get_resource_filepath(filename);
        let err_msg = format!("Failed to open image from: '{}'", filepath);
        // Check if filepath ends in '.mat'
        if filepath.ends_with(".mat") || filepath.ends_with(".mat") {
          panic!(
              "Files ending in .MTL and .MAT must be loaded as materials: {}",
              filepath
          );
        }
        let img = image::open(filepath.clone()).expect(err_msg.as_str());

        let width = img.width();
        let height = img.height();

        // Image can't be wider than 65K, which is already too big, let's be honest
        if width > 65535 || height > 65535 {
            panic!(
                "Image located at: {} has dimensions that are too large!",
                filepath
            );
        }

        let mut data: Vec<u8> = vec![];
        // Check the color mode and process it appropriately
        if img.color() == ColorType::Rgb8 {
            let colordata = img.as_rgb8().unwrap().to_vec();
            let mut i = 0;
            let len = colordata.len();
            while i < len {
                data.push(colordata[i]);
                data.push(colordata[i + 1]);
                data.push(colordata[i + 2]);
                i += 3;
            }
        } else if img.color() == ColorType::Rgba8 {
            let colordata = img.as_rgba8().unwrap().to_vec();
            let mut i = 0;
            let len = colordata.len();
            while i < len {
                data.push(colordata[i]);
                data.push(colordata[i + 1]);
                data.push(colordata[i + 2]);
                i += 4;
            }
        }

        // For now just created a bitmap by hand
        return Bitmap {
            data,
            width: width as u16,
            height: height as u16,
            log2x: (width as f32).log2().round() as u8, 
            log2y: (height as f32).log2().round() as u8,
            linear_offsetx: 32768 / width as u16,
            linear_offsety: 32768 / height as u16,
        };
    }
}
pub struct Texture {
    pub lods: Vec<Bitmap>,
    pub has_alpha: bool,
    pub color: u32,
}

impl Texture {
    pub fn empty() -> Texture {
        return Texture {
            lods: vec![Bitmap {
                width: 0,
                height: 0,
                data: vec![0; 0],
                log2x: 0,
                log2y: 0,
                linear_offsetx: 0,
                linear_offsety: 0,
            }],
            has_alpha: false,
            color: 0,
        };
    }
    pub fn default() -> Texture {
        return Texture {
            lods: vec![get_dflt_bitmap()],
            has_alpha: false,
            color: 4673355,
        };
    }
}

fn get_dflt_bitmap() -> Bitmap {
    let mut data: Vec<u32> = vec![4673355; 256];
    for (i, pixel) in data.iter_mut().enumerate() {
        if i < 16 || i % 16 == 0 {
            *pixel = 13063478;
        }
    }
    data[119] = 0;
    data[120] = 0;
    data[135] = 0;
    let mut new_data: Vec<u8> = vec![];
    for pixel in data {
        let rgb = to_rgb(pixel);
        new_data.push(rgb[0]);
        new_data.push(rgb[1]);
        new_data.push(rgb[2]);
    }
    return Bitmap {
        width: 16,
        height: 16,
        data: new_data,
        log2x: 4,
        log2y: 4,
        linear_offsetx: 2048,
        linear_offsety: 2048,
    };
}