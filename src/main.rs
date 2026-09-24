// PROFILE: cargo instruments -t 'Game Performance' --release
// #![feature(stdsimd, platform_intrinsics)]

use controls::set_mouse_pos;
use fragment::set_current_colormap;
use minifb::{Key, KeyRepeat, MouseMode, Window, WindowOptions};
use std::env;
use std::f32::consts::PI;
use std::time::{SystemTime, UNIX_EPOCH};

use colors::from_rgb;
use matrix::Transformation;

use crate::archetype::load_archetypes;
use crate::camera::Camera;
use crate::clipplane::ClipPlane;
use crate::controls::char_from_key;
use crate::entity::Entity;
use crate::jk_cmp::{load_cmp_file, ColorMap};
use crate::materials::{Material, ChannelFormat};
use crate::rasterizer::{fetch_pixel_safe, fill_pixel_safe};
use crate::resources::{add_texture, add_material, get_model, get_model_index, get_archetype, get_material, get_texture, add_colormap};
use crate::setup::DRAW_MODE_SOLID;
use crate::ss::{save_frame_buffer, buffer_to_image_file};
use crate::textures::{Bitmap, Texture};

mod archetype;
mod camera;
mod clipplane;
mod colors;
mod controls;
mod entity;
mod interpolate;
mod lights;
mod materials;
mod matrix;
mod movement;
mod rasterizer;
mod setup;
mod shapes;
mod textures;
mod threedee;
mod trig;
mod utils;
mod jk_3do;
mod renderer;
mod jk_cmp;
mod resources;
mod jk_mat;
mod spans;
mod fragment;
mod vertex;
pub mod console;
pub mod commands;
pub mod ss;

fn main() {
    env::set_var("RUST_BACKTRACE", "1");
    // Load controls data
    controls::load_controls_config(String::from("config/controls.json"));
    // Load config data and create settings structs
    let (
      mut vars, 
      resolution, 
      mut state
    ) = setup::load_config(String::from("config/cfg.json"));
    // let mut settings = config.0;
    // let mut vars = config.2;
    // let mut vars = config.3;
    // let mut vars = config.4;
    // let mut state = config.5;

    /* Create the buffers */
    let mut frame_buff: Vec<u32> = vec![0; (vars.width * vars.height) as usize];
    let mut depth_buff: Vec<u32> = vec![0; (vars.width * vars.height) as usize];
    // let mut span_buff: Vec<Vec<Span>> = vec![vec![]; settings.height as usize];
    /* Create the buffers */

    // Initialize the command line interface
    console::init(1024, "res/font/orbitron-fm.png".to_string());
    console::log("Welcome to the Moose Engine!");
    console::log("Type 'help' for a list of commands.");

    // Initalize the dflt materials, textures and colormaps
    initialize_material_graphics(
      vars.width, 
      vars.height, 
      &vars.defaultpalette
    );

    load_archetypes("res/archetypes.json".to_string());

    let mut entities: Vec<Entity> = vec![];

    // Create the view struct which holds the perspective projection matrix
    let camera = Camera::new(
      vars.width,
      vars.height,
      vars.fov,
      vars.znear,
      vars.zfar,
    );
    // Create the struct that builds the view transformation matrix
    let mut v_transform = Transformation::new(0.0, 0.0, 0.0);
    let mut v_rotate = Transformation::new(0.0, 0.0, 0.0);

    // Create the window for rendering
    let mut window = Window::new(
        "Moose Engine - ESC to exit",
        (vars.width as f32 / resolution) as usize,
        (vars.height as f32 / resolution) as usize,
        WindowOptions::default(),
    )
    .unwrap_or_else(|e| {
        panic!("{}", e);
    });

    // Limit fps update rate
    window.limit_update_rate(Some(std::time::Duration::from_micros(vars.frametime)));
    let mut frame_count: u32 = 0;

    let mut start = SystemTime::now();
    let mut then = start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let mut fpstracker = (0.0, 0 as usize);

    // manually create entities for now

    entities.push(Entity::new(
        0,
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        -1,
        true,
    ));
    // entities.push(Entity::new(
    //     1,
    //     [0.0, -0.2, 1.6],
    //     [0.0, 0.0, 0.0],
    //     [1.0, 1.0, 1.0],
    //     2,
    //     true,
    // ));
    entities.push(Entity::new(
      3,
      [1.0, -0.2, -1.59],
      [0.0, 0.0, 0.0],
      [1.0, 1.0, 1.0],
      -1,
      true,
    ));
    entities.push(Entity::new(
        2,
        [0.0, -0.2, 1.6],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        -1,
        true,
    ));
    entities.push(Entity::new(
        2,
        [2.0, 0.2, 1.3],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        -1,
        true,
    ));
    entities.push(Entity::new(
        2,
        [0.0, -2.2, 0.6],
        [0.0, 0.0, 0.0],
        [1.5, 1.5, 1.5],
        -1,
        true,
    ));
    entities.push(Entity::new(
        2,
        [-1.0, -0.5, -0.03],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        -1,
        true,
    ));
    entities.push(Entity::new(
        2,
        [0.2, 2.2, 2.0],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        -1,
        true,
    ));

    let shift = false;
    let mouse_pos = window
        .get_mouse_pos(MouseMode::Pass)
        .expect("Invalid mouse input");
    controls::set_mouse_pos(mouse_pos.0, mouse_pos.1);
    let mut depth_test = true;
    let mut is_esc_down = false;

    while window.is_open() && !is_esc_down {
        /* Scale time sensitive values by this number to make movement independent of framerate */
        start = SystemTime::now();
        let now = start
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        let mut tprl = (now.as_millis() - then.as_millis()) as f32 / 1000.0;
        /* Scale time sensitive values by this number to make movement independent of framerate */
        then = now;
        // Track the framerate
        fpstracker.0 += tprl;
        fpstracker.1 += 1;
        if fpstracker.0 >= 1.0 {
            window.set_title(
                format!(
                    "Moose Engine - ESC to exit - {}FPS - {:.2} {:.2} {:.2} - {} {}",
                    (fpstracker.1 as f32 / fpstracker.0) as u32,
                    state.dude_x,
                    state.dude_y,
                    state.dude_z,
                    vars.width,
                    vars.height
                )
                .as_str(),
            );
            fpstracker.0 = 0.0;
            fpstracker.1 = 0;
        }

        // Change the tprl speed to factor in the user and environment rate of time passage
        let utprl = tprl * vars.userrate;
        tprl = tprl * vars.envrate;

        // This section of keybinds only exists for testing purposes
        if !console::is_open() {
          window
            .get_keys_pressed(KeyRepeat::No)
            .iter()
            .for_each(|key| match key {
                Key::Escape => is_esc_down = true,
                Key::L => {
                    vars.entlightmode =
                        (vars.entlightmode + if shift { -1 } else { 1 }).rem_euclid(4)
                }
                Key::O => vars.occlusionquerying = !vars.occlusionquerying,
                Key::Z => vars.show_depth_buffer = !vars.show_depth_buffer,
                Key::K => depth_test = !depth_test,
                Key::G => vars.coloredlighting = !vars.coloredlighting,
                Key::Y => state.flying = !state.flying,
                Key::P => {
                    vars.texmapmode =
                        ((vars.texmapmode + 1) + if window.is_key_down(Key::LeftShift) { -1 } else { 1 }).rem_euclid(5) - 1
                }
                Key::F => {
                    vars.drawmode =
                        ((vars.drawmode + 1) + if window.is_key_down(Key::LeftShift) { -1 } else { 1 }).rem_euclid(4) - 1
                },
                Key::M => {
                  vars.texfiltermode =
                      ((vars.texfiltermode + 1) + if window.is_key_down(Key::LeftShift) { -1 } else { 1 }).rem_euclid(10) - 1;
                }
                Key::Equal => vars.texmapmodereduction += 1,
                Key::Minus => vars.texmapmodereduction -= 1,
                Key::Backspace => {
                    state.dude_pitch = 0.0;
                    state.dude_yaw = 0.0;
                    state.dude_x = 0.0;
                    state.dude_y = 0.0;
                    state.dude_z = 0.0;
                },
                Key::T => console::open(),
                _ => (),
            });
            window.get_keys_released().iter().for_each(|key| match key {
                Key::Escape => is_esc_down = false,
                _ => (),
            });

            // Call the handler for the controls state manager
            controls::handle(&window, &mut state, &mut vars);
    
            // Call the handler for the camera movement
            movement::handle(&window, utprl, &mut state, &vars);
        } else {
          // Determine if the shift key is down
          let shift = window.is_key_down(Key::LeftShift) || window.is_key_down(Key::RightShift);
          window
            .get_keys_pressed(KeyRepeat::No)
            .iter()
            .for_each(|key| {
                if key == &Key::Escape {
                  console::close();
                } else if key == &Key::Backspace || key == &Key::Delete {
                  console::delete();
                } else if key == &Key::Enter {
                  console::enter();
                } else {
                  // Get any characters that were typed and add them to the console input buffer
                  let char_opt = char_from_key(*key, shift);
                  if char_opt.is_some() {
                    let char = char_opt.unwrap();
                    console::input(char);
                  }
                }
            });
        }
        // print the each of the keys that are currently pressed
        // window.get_keys().iter().for_each(|key| println!("{:?}", key));

        // Reset the buffers
        // This is redundant for the frame buffer
        for i in frame_buff.iter_mut() {
            *i = 0;
        }
        for i in depth_buff.iter_mut() {
            *i = 4294967295;
        }

        // Apply the global transformation
        v_transform.reset();
        v_rotate.reset();
        // println!("{}", state.dude_yaw);
        v_transform.translate(-state.dude_x, -state.dude_y, -state.dude_z);
        v_transform.rotate_y(2.0 * PI - state.dude_yaw);
        v_transform.rotate_x(2.0 * PI - state.dude_pitch);
        v_transform.build_matrix();
        v_rotate.rotate_y(2.0 * PI - state.dude_yaw);
        v_rotate.rotate_x(2.0 * PI - state.dude_pitch);
        v_rotate.build_matrix();
        

        // Use the render sector entitites function to render the test entities
        renderer::render_sector_entities(
          &camera, 
          &mut depth_buff, 
          &mut frame_buff, 
          &v_transform, 
          &v_rotate, 
          entities.clone(), 
          &vars
        );

        // Do a screen capture if the one was requested
        if save_frame_buffer() {
          buffer_to_image_file(&frame_buff, vars.width, vars.height);
        }

        // Render the console if it's open
        if console::is_open() {
          console::update(vars.width as usize);
          let console_screen = console::get_screen_buffer();
          let mut row_index = 0;
          for y in 0..console_screen.height {
            for x in 0..console_screen.width {
              let pixel = console_screen.buffer[row_index + x];
              fill_pixel_safe(
                &mut frame_buff,
                x as isize,
                y as isize,
                pixel,
                vars.width,
                vars.height,
              );
            }
            row_index += console_screen.width;
          }
        }

        // We unwrap here as we want this code to exit if it fails. Real applications may want to handle this in a different way
        if !vars.show_depth_buffer {
            window
                .update_with_buffer(
                    &frame_buff,
                    vars.width as usize,
                    vars.height as usize,
                )
                .unwrap();
        } else {
            if vars.drawmode != DRAW_MODE_SOLID {
                let mut y = 0;
                while y < vars.height as usize {
                    let mut x = 0;
                    while x < vars.width as usize {
                        let depth = fetch_pixel_safe(
                            &mut depth_buff,
                            x as isize,
                            y as isize,
                            vars.width,
                            vars.height,
                        );
                        let color = depth >> 24;
                        let left_over = depth - (color << 24);
                        let new_depth_down = fetch_pixel_safe(
                            &mut depth_buff,
                            x as isize,
                            y as isize + 1,
                            vars.width,
                            vars.height,
                        ) + (left_over >> 2)
                            + (left_over >> 3);
                        let new_depth_right = fetch_pixel_safe(
                            &mut depth_buff,
                            x as isize + 1,
                            y as isize,
                            vars.width,
                            vars.height,
                        ) + (left_over >> 2)
                            + (left_over >> 3);
                        let new_depth_diag = fetch_pixel_safe(
                            &mut depth_buff,
                            x as isize + 1,
                            y as isize + 1,
                            vars.width,
                            vars.height,
                        ) + (left_over >> 2);
                        fill_pixel_safe(
                            &mut depth_buff,
                            x as isize,
                            y as isize,
                            from_rgb(color as u8, color as u8, color as u8),
                            vars.width,
                            vars.height,
                        );
                        fill_pixel_safe(
                            &mut depth_buff,
                            x as isize,
                            y as isize + 1,
                            new_depth_down,
                            vars.width,
                            vars.height,
                        );
                        fill_pixel_safe(
                            &mut depth_buff,
                            x as isize + 1,
                            y as isize,
                            new_depth_right,
                            vars.width,
                            vars.height,
                        );
                        fill_pixel_safe(
                            &mut depth_buff,
                            x as isize + 1,
                            y as isize + 1,
                            new_depth_diag,
                            vars.width,
                            vars.height,
                        );
                        x += 1;
                    }
                    y += 1;
                }
            } else {
                for i in depth_buff.iter_mut() {
                    let v = (*i >> 24) as u8;
                    *i = from_rgb(v, v, v);
                }
            }
            window
                .update_with_buffer(
                    &depth_buff,
                    vars.width as usize,
                    vars.height as usize,
                )
                .unwrap();
        }
        frame_count += 1;

        // let col1: [u8; 3] = [255, 0, 0];
        // let col2: [u8; 3] = [0, 255, 0];

        // let alpha = 64;

        // let mut col3: [u8; 3] = [
        //   q_interpolate_u8(col1[0], col2[0], alpha),
        //   q_interpolate_u8(col1[1], col2[1], alpha),
        //   q_interpolate_u8(col1[2], col2[2], alpha),
        // ];
        // println!("{:?}", col3);
        // panic!("");
    }
    println!("FRAME COUNT: {}", frame_count);
}

fn initialize_material_graphics(buffer_width: u32, buffer_height: u32, dflt_palette: &String) {
  // Load the default cmp file
  let colormap = load_cmp_file(dflt_palette).expect("Unable to load default palette");
  let dflt_cmp_index = add_colormap(dflt_palette, colormap);
  set_current_colormap(dflt_cmp_index);
  
  let empty_mat_name = String::from("empty");
  let default_mat_name = String::from("default");
  let lastframe_mat_name = String::from("lastframe");
  // Add the default textures
  add_texture(&empty_mat_name, Texture::empty());
  add_texture(&default_mat_name, Texture::default());
  add_texture(
    &lastframe_mat_name,
      Texture {
          lods: vec![Bitmap {
              data: vec![0; (buffer_width * buffer_height) as usize],
              width: buffer_width as u16,
              height: buffer_height as u16,
              log2x: (buffer_width as f32).log2().round() as u8,
              log2y: (buffer_height as f32).log2().round() as u8,
              linear_offsetx: 32768 / buffer_width as u16,
              linear_offsety: 32768 / buffer_height as u16,
          }],
          has_alpha: false,
          color: 200,
      },
  );
  // Add the default materials
  add_material(&empty_mat_name, 
    Material {
      texture: 0,
      ..Material::default()
    });
  add_material(&default_mat_name, Material::default());
  add_material(
      &lastframe_mat_name,
      Material {
          texture: 2,
          ..Material::default()
      },
  );
}