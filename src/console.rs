use crate::{colors::{from_rgba, q_blend_colors}, commands::CommandExecutor};


static mut CONSOLE_BUFFER: String = String::new();
static mut CONSOLE_LENGTH: usize = 1024;
static mut CONSOLE_INPUT: String = String::new();
static mut CONSOLE_CURSOR: usize = 0;
static mut CONSOLE_UPDATE_PENDING: bool = false;
static mut CONSOLE_OPEN: bool = false;

static mut CONSOLE_SCREEN_BUFFER: Vec<u32> = vec![];
static mut CONSOLE_SCREEN_WIDTH: usize = 0;
const CONSOLE_ASPECT_RATIO: f32 = 7.0;

static mut FONT_MAP: Vec<u32> = vec![];
static mut FONT_MAP_WIDTH: usize = 4096;
static mut LINE_HEIGHT: usize = 16;
static mut CHAR_WIDTH: usize = 16;
const CHAR_COUNT: usize = 256;

static mut COMMAND_EXECUTOR: Option<CommandExecutor> = None;

pub fn init(console_length: usize, font_map: String) {
  unsafe {
    // Set the console length
    CONSOLE_LENGTH = console_length;
    // Set the capactiy of the console buffer
    CONSOLE_BUFFER = String::with_capacity(CONSOLE_LENGTH);
    // Load the font map image and store it in the FONT_MAP buffer
    let font_map = image::open(font_map).unwrap().to_rgba8();
    FONT_MAP = vec![0; font_map.width() as usize * font_map.height() as usize];
    // Set the font map width and line height
    FONT_MAP_WIDTH = font_map.width() as usize;
    LINE_HEIGHT = font_map.height() as usize;
    // Set the char width
    CHAR_WIDTH = FONT_MAP_WIDTH / CHAR_COUNT;

    let font_map = font_map.as_flat_samples();
    let font_map = font_map.as_slice();
    let mut mpi = 0;
    let mut i = 0;
    let len = font_map.len();
    while i < len {
      let r = font_map[i];
      let g = font_map[i + 1];
      let b = font_map[i + 2];
      let a = font_map[i + 3];
      FONT_MAP[mpi] = from_rgba(r, g, b, a);
      i += 4;
      mpi += 1;
    }
    // Mark the console for rendering
    CONSOLE_UPDATE_PENDING = true;
    // Create the instance of the command executor
    COMMAND_EXECUTOR = Some(CommandExecutor::new());
  }
}

pub fn log<S: AsRef<str>>(message: S) {
  let message = message.as_ref();
  unsafe {
    CONSOLE_BUFFER.push_str(message);
    CONSOLE_BUFFER.push_str("\n");
    // Check the length of the buffer and truncate if necessary
    if CONSOLE_BUFFER.len() > CONSOLE_LENGTH {
      CONSOLE_BUFFER = CONSOLE_BUFFER[CONSOLE_BUFFER.len() - CONSOLE_LENGTH..].to_string();
    }
    CONSOLE_UPDATE_PENDING = true;
  }
}

pub fn input(char: char) {
  unsafe {
    CONSOLE_INPUT.push(char);
    CONSOLE_CURSOR += 1;
    CONSOLE_UPDATE_PENDING = true;
  }
}

pub fn delete() {
  unsafe {
    // Remove the last character from the input buffer
    if CONSOLE_INPUT.len() > 0 {
      CONSOLE_INPUT.pop();
      CONSOLE_CURSOR -= 1;
      CONSOLE_UPDATE_PENDING = true;
    }
  }
}

pub fn enter() {
  unsafe {
    // Add the input buffer to the console buffer
    CONSOLE_BUFFER.push_str("> ");
    CONSOLE_BUFFER.push_str(CONSOLE_INPUT.as_str());
    CONSOLE_BUFFER.push_str("\n");
    // Attempt to execute the command
    COMMAND_EXECUTOR.as_mut().unwrap().execute(CONSOLE_INPUT.clone());
    // Clear the input buffer
    CONSOLE_INPUT = String::new();
    CONSOLE_CURSOR = 0;
    // Mark the console for rendering
    CONSOLE_UPDATE_PENDING = true;
  }
}

pub fn toggle() {
  unsafe {
    CONSOLE_OPEN = !CONSOLE_OPEN;
    CONSOLE_UPDATE_PENDING = true;
  }
}

pub fn open() {
  unsafe {
    CONSOLE_OPEN = true;
    CONSOLE_UPDATE_PENDING = true;
  }
}

pub fn close() {
  unsafe {
    CONSOLE_OPEN = false;
  }
}

pub fn clear() {
  unsafe {
    CONSOLE_BUFFER = String::new();
    CONSOLE_UPDATE_PENDING = true;
  }
}

pub fn is_open() -> bool {
  unsafe {
    return CONSOLE_OPEN;
  }
}

pub fn update(screen_width: usize) {
  unsafe {
    // If there is no update pending, return
    if !CONSOLE_UPDATE_PENDING {
      return;
    }
    CONSOLE_UPDATE_PENDING = false;
    let screen_height = (screen_width as f32 / CONSOLE_ASPECT_RATIO) as usize;
    // If the screen width has changed, resize the screen buffer
    if CONSOLE_SCREEN_WIDTH != screen_width {
      CONSOLE_SCREEN_WIDTH = screen_width;
      CONSOLE_SCREEN_BUFFER = vec![0x463E3E; screen_width * screen_height];
    } else {
      // Clear the console background with black
      for i in 0..CONSOLE_SCREEN_BUFFER.len() {
        CONSOLE_SCREEN_BUFFER[i] = 0x463E3E;
      }
    }


    /* Render the console buffer */
    // Set 4 pixel padding on the left and right and bottom
    let padding = 4;
    let line_height = LINE_HEIGHT;
    let char_width = CHAR_WIDTH;

    // calculate the width we have to work with in chars, and then loop through
    // the lines and split any that are too long
    let max_width = (screen_width - padding * 2) / char_width;

    let x = padding;
    let mut y: isize = (screen_height - padding - line_height) as isize;

    // Render the input line
    let input_line = format!("> {}", CONSOLE_INPUT);
    // Split the input line into lines if it's too long
    let mut input_lines: Vec<String> = line_format(vec![input_line], max_width);
    input_lines.reverse();
    let mut i = 0;
    let len = input_lines.len();
    // Loop through the lines and render them
    while i < len && y >= 0 {
      let line = input_lines[i].clone();
      draw_line(line, x, y as usize, char_width);
      y -= line_height as isize;
      i += 1;
    }

    // Handle the output lines if there is room on the screen
    if y >= 0 {
      // Split the buffer into lines starting at the end, and then render
      // the lines from the bottom up, but leave space for the input line
      // at the bottom
      let lines: Vec<String> = CONSOLE_BUFFER.split("\n").into_iter().map(|s| s.to_string()).collect();
      // Now render the output lines
      let mut output_lines = line_format(lines, max_width);
      output_lines.reverse();
      let mut i = 0;
      let len = output_lines.len();
      while i < len && y > 0 {
        let line = output_lines[i].clone();
        draw_line(line, x, y as usize, char_width);
        y -= line_height as isize;
        i += 1;
      }
    }
  }
}

pub struct ConsoleScreenData<'a> {
  pub width: usize,
  pub height: usize,
  pub buffer: &'a Vec<u32>,
}

pub fn get_screen_buffer() -> ConsoleScreenData<'static> {
  unsafe {
    return ConsoleScreenData {
      width: CONSOLE_SCREEN_WIDTH,  
      height: (CONSOLE_SCREEN_WIDTH as f32 / CONSOLE_ASPECT_RATIO) as usize,
      buffer: &CONSOLE_SCREEN_BUFFER,
    }
  }
}

fn draw_line(line: String, x_pos: usize, y_pos: usize, char_width: usize) {
  let mut i = 0;
  let mut x_pos = x_pos;
  let line_len = line.len();
  let chars: Vec<char> = line.chars().into_iter().collect();
  while i < line_len {
    let character = chars[i];
    draw_char(character, x_pos, y_pos);
    x_pos += char_width;
    i += 1;
  }
}

fn draw_char(character: char, x: usize, y: usize) {
  let index = character as usize;
  unsafe {
    let char_width = CHAR_WIDTH;
    let char_height = LINE_HEIGHT;
    let offset = index * char_width;
    let mut sx = x;
    let mut sy = y * CONSOLE_SCREEN_WIDTH;
    let mut fy_row = 0;
    for _ in 0..char_height {
      for fx in 0..char_width {
        let scrn_index = sy + sx;
        let pix_index = fy_row + fx;
        let color = FONT_MAP[pix_index + offset];
        let alpha = (color >> 24) as u8;
        let color = color & 0x00FFFFFF;
        let curr_color = CONSOLE_SCREEN_BUFFER[scrn_index];
        // Blend the colors using the alpha
        let color = q_blend_colors(curr_color, color, alpha);
        CONSOLE_SCREEN_BUFFER[scrn_index] = color;
        sx += 1;
      }
      sx = x;
      sy += CONSOLE_SCREEN_WIDTH;
      fy_row += FONT_MAP_WIDTH;
    }
  }
}

// Send in lines, and break them up oif they're too long
fn line_format(lines: Vec<String>, max_width: usize) -> Vec<String> {
  let mut new_lines: Vec<String> = vec![];
  let mut i = 0;
  let len = lines.len();
  while i < len {
    let line = &lines[i];
    let line_width = line.len();
    if line_width > max_width {
      // Split the line by spaces so that we don't cut words in half
      let mut split = line.split(" ");
      let mut new_line = String::new();
      let mut new_line_width = 0;
      let mut word = split.next();
      while word != None {
        let word_val = word.unwrap();
        let word_width = word_val.len();
        if new_line_width + word_width > max_width {
          // Add the new line to the new lines
          new_lines.push(new_line);
          // Reset the new line
          new_line = String::new();
          new_line_width = 0;
        }
        // Add the word to the new line
        new_line.push_str(word_val);
        new_line.push_str(" ");
        new_line_width += word_width + 1;
        // Get the next word
        word = split.next();
      }
      // Add the final new line to the new lines
      new_lines.push(new_line);
    } else {
      // Add the line to the new lines as is
      new_lines.push(line.to_string());
    }
    i += 1;
  }
  return new_lines;
}