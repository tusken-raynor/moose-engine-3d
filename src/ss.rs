use std::time;

static mut SAVE_FRAME_BUFFER: bool = false;

pub fn set_save_frame_buffer() {
  unsafe {
    SAVE_FRAME_BUFFER = true;
  }
}

pub fn save_frame_buffer() -> bool {
  unsafe {
    return SAVE_FRAME_BUFFER;
  }
}

// Save the frame buffer to a png file
pub fn buffer_to_image_file(buffer: &Vec<u32>, width: u32, height: u32) {
  // Create a new image buffer
  let mut image_buffer = image::ImageBuffer::new(width, height);
  // Iterate over the buffer and set the pixels
  for (x, y, pixel) in image_buffer.enumerate_pixels_mut() {
    *pixel = image::Rgb([
      ((buffer[(y * width + x) as usize] >> 16) & 0xFF) as u8,
      ((buffer[(y * width + x) as usize] >> 8) & 0xFF) as u8,
      (buffer[(y * width + x) as usize] & 0xFF) as u8,
    ]);
  }
  // Seconds since epoch
  let now = time::SystemTime::now().duration_since(time::UNIX_EPOCH).unwrap().as_secs();
  // Save the image
  let path = format!("frame_{}.png", now);
  image_buffer.save(path).unwrap();
  unsafe {
    SAVE_FRAME_BUFFER = false;
  }
}
