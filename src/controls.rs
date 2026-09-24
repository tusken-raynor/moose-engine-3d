use std::{f32::consts::PI, fs, sync::Mutex};

use lazy_static::lazy_static;

use minifb::{Key, KeyRepeat, Window};

use crate::{
    setup::{Vars, State},
    trig::{cos, sin},
};

pub struct ControlKeysConfig {
    move_forward: Key,
    move_backward: Key,
    move_left: Key,
    move_right: Key,
    pitch_up: Key,
    pitch_down: Key,
    yaw_left: Key,
    yaw_right: Key,
    fast: Key,
}

lazy_static! {
    static ref CONTROL_KEYS_CONFIG: Mutex<ControlKeysConfig> = Mutex::new(ControlKeysConfig {
        move_forward: Key::W,
        move_backward: Key::S,
        move_left: Key::A,
        move_right: Key::D,
        pitch_up: Key::Up,
        pitch_down: Key::Down,
        yaw_left: Key::Left,
        yaw_right: Key::Right,
        fast: Key::LeftShift,
    });
}

static mut MOUSE_X: f32 = 0.0;
static mut MOUSE_Y: f32 = 0.0;

pub fn get_mouse_pos() -> (f32, f32) {
    unsafe { (MOUSE_X, MOUSE_Y) }
}
pub fn set_mouse_pos(x: f32, y: f32) {
    unsafe {
        MOUSE_X = x;
        MOUSE_Y = y;
    }
}

pub fn handle(window: &Window, state: &mut State, vars: &mut Vars) {
    let control_keys = CONTROL_KEYS_CONFIG.lock().unwrap();

    let keys_pressed = window.get_keys_pressed(KeyRepeat::No);

    if keys_pressed.contains(&control_keys.move_backward) {
        state.moving_backward = true;
    }
    if keys_pressed.contains(&control_keys.move_forward) {
        state.moving_forward = true;
    }
    if keys_pressed.contains(&control_keys.move_left) {
        state.moving_left = true;
    }
    if keys_pressed.contains(&control_keys.move_right) {
        state.moving_right = true;
    }
    if keys_pressed.contains(&control_keys.pitch_up) {
        state.pitching_up = true;
    }
    if keys_pressed.contains(&control_keys.pitch_down) {
        state.pitching_down = true;
    }
    if keys_pressed.contains(&control_keys.yaw_left) {
        state.yawing_left = true;
    }
    if keys_pressed.contains(&control_keys.yaw_right) {
        state.yawing_right = true;
    }
    if keys_pressed.contains(&control_keys.fast) {
        vars.movespeed *= 1.5;
    }

    let keys_released = window.get_keys_released();
    if keys_released.contains(&control_keys.move_backward) {
        state.moving_backward = false;
    }
    if keys_released.contains(&control_keys.move_forward) {
        state.moving_forward = false;
    }
    if keys_released.contains(&control_keys.move_left) {
        state.moving_left = false;
    }
    if keys_released.contains(&control_keys.move_right) {
        state.moving_right = false;
    }
    if keys_released.contains(&control_keys.pitch_up) {
        state.pitching_up = false;
    }
    if keys_released.contains(&control_keys.pitch_down) {
        state.pitching_down = false;
    }
    if keys_released.contains(&control_keys.yaw_left) {
        state.yawing_left = false;
    }
    if keys_released.contains(&control_keys.yaw_right) {
        state.yawing_right = false;
    }
    if keys_released.contains(&control_keys.fast) {
        vars.movespeed /= 1.5;
    }
}

pub fn load_controls_config(filepath: String) {
    // Load the config json file and parse it
    let cfg_json = fs::read_to_string(filepath).ok();
    let cfg = json::parse(cfg_json.unwrap_or(String::from("{}")).as_str()).unwrap();

    let has_key_controls = cfg.has_key("KeyboardControls") && cfg["KeyboardControls"].is_object();

    let mut control_keys_config = CONTROL_KEYS_CONFIG.lock().unwrap();

    /* KEYBOARD CONTROLS */
    if has_key_controls && cfg["KeyboardControls"].has_key("MOVE_FORWARD") {
        control_keys_config.move_forward = get_key_from_string(
            cfg["KeyboardControls"]["MOVE_FORWARD"]
                .as_str()
                .expect("Invalid data type for MOVE_FORWARD. Must be string."),
        )
        .expect("Invalid key mapping for MOVE_FORWARD")
    }
    if has_key_controls && cfg["KeyboardControls"].has_key("MOVE_BACKWARD") {
        control_keys_config.move_backward = get_key_from_string(
            cfg["KeyboardControls"]["MOVE_BACKWARD"]
                .as_str()
                .expect("Invalid data type for MOVE_BACKWARD. Must be string."),
        )
        .expect("Invalid key mapping for MOVE_BACKWARD")
    }
    if has_key_controls && cfg["KeyboardControls"].has_key("MOVE_LEFT") {
        control_keys_config.move_left = get_key_from_string(
            cfg["KeyboardControls"]["MOVE_LEFT"]
                .as_str()
                .expect("Invalid data type for MOVE_LEFT. Must be string."),
        )
        .expect("Invalid key mapping for MOVE_LEFT")
    }
    if has_key_controls && cfg["KeyboardControls"].has_key("MOVE_RIGHT") {
        control_keys_config.move_right = get_key_from_string(
            cfg["KeyboardControls"]["MOVE_RIGHT"]
                .as_str()
                .expect("Invalid data type for MOVE_RIGHT. Must be string."),
        )
        .expect("Invalid key mapping for MOVE_RIGHT")
    }
    if has_key_controls && cfg["KeyboardControls"].has_key("PITCH_UP") {
        control_keys_config.pitch_up = get_key_from_string(
            cfg["KeyboardControls"]["PITCH_UP"]
                .as_str()
                .expect("Invalid data type for PITCH_UP. Must be string."),
        )
        .expect("Invalid key mapping for PITCH_UP")
    }
    if has_key_controls && cfg["KeyboardControls"].has_key("PITCH_DOWN") {
        control_keys_config.pitch_down = get_key_from_string(
            cfg["KeyboardControls"]["PITCH_DOWN"]
                .as_str()
                .expect("Invalid data type for PITCH_DOWN. Must be string."),
        )
        .expect("Invalid key mapping for PITCH_DOWN")
    }
    if has_key_controls && cfg["KeyboardControls"].has_key("YAW_LEFT") {
        control_keys_config.yaw_left = get_key_from_string(
            cfg["KeyboardControls"]["YAW_LEFT"]
                .as_str()
                .expect("Invalid data type for YAW_LEFT. Must be string."),
        )
        .expect("Invalid key mapping for YAW_LEFT")
    }
    if has_key_controls && cfg["KeyboardControls"].has_key("YAW_RIGHT") {
        control_keys_config.yaw_right = get_key_from_string(
            cfg["KeyboardControls"]["YAW_RIGHT"]
                .as_str()
                .expect("Invalid data type for YAW_RIGHT. Must be string."),
        )
        .expect("Invalid key mapping for YAW_RIGHT")
    }
    if has_key_controls && cfg["KeyboardControls"].has_key("FAST") {
        control_keys_config.fast = get_key_from_string(
            cfg["KeyboardControls"]["FAST"]
                .as_str()
                .expect("Invalid data type for FAST. Must be string."),
        )
        .expect("Invalid key mapping for FAST")
    }
}

pub fn move_forward(secs: f32, state: &mut State, vars: &Vars) {
    let move_x = sin(state.dude_yaw);
    let move_z = cos(state.dude_yaw);

    state.dude_x += move_x * vars.movespeed * secs;
    state.dude_z += move_z * vars.movespeed * secs;

    if state.flying {
        let move_y = sin(state.dude_pitch);
        state.dude_y -= move_y * vars.movespeed * secs;
    }
}

pub fn move_backward(secs: f32, state: &mut State, vars: &Vars) {
    let move_x = sin(state.dude_yaw);
    let move_z = cos(state.dude_yaw);

    state.dude_x -= move_x * vars.movespeed * secs;
    state.dude_z -= move_z * vars.movespeed * secs;

    if state.flying {
        let move_y = sin(state.dude_pitch);
        state.dude_y += move_y * vars.movespeed * secs;
    }
}

pub fn move_right(secs: f32, state: &mut State, vars: &Vars) {
    let move_x = cos(state.dude_yaw);
    let move_z = sin(state.dude_yaw);

    state.dude_x -= move_x * 0.9 * vars.movespeed * secs;
    state.dude_z += move_z * 0.9 * vars.movespeed * secs;
}

pub fn move_left(secs: f32, state: &mut State, vars: &Vars) {
    let move_x = cos(state.dude_yaw);
    let move_z = sin(state.dude_yaw);

    state.dude_x += move_x * 0.9 * vars.movespeed * secs;
    state.dude_z -= move_z * 0.9 * vars.movespeed * secs;
}

pub fn yaw_right(secs: f32, state: &mut State, vars: &Vars) {
    let mut turn_ang = state.dude_yaw - PI * vars.turnspeed * secs;
    if turn_ang >= 2.0 * PI {
        turn_ang = turn_ang - 2.0 * PI;
    } else if turn_ang < 0.0 {
        turn_ang = turn_ang + 2.0 * PI;
    }

    state.dude_yaw = turn_ang;
}

pub fn yaw_left(secs: f32, state: &mut State, vars: &Vars) {
    let mut turn_ang = state.dude_yaw + PI * vars.turnspeed * secs;

    if turn_ang >= 2.0 * PI {
        turn_ang = turn_ang - 2.0 * PI;
    } else if turn_ang < 0.0 {
        turn_ang = turn_ang + 2.0 * PI;
    }

    state.dude_yaw = turn_ang;
}

pub fn pitch_up(secs: f32, state: &mut State, vars: &Vars) {
    let max = vars.maxpitch / 180.0 * PI;
    let mut turn_ang = state.dude_pitch - PI * vars.turnspeed * secs;
    if turn_ang >= 2.0 * PI {
        turn_ang = turn_ang - 2.0 * PI;
    } else if turn_ang < 0.0 {
        turn_ang = turn_ang + 2.0 * PI;
    }

    if turn_ang <= max || turn_ang >= PI * 2.0 - max {
        state.dude_pitch = turn_ang;
    }
}

pub fn pitch_down(secs: f32, state: &mut State, vars: &Vars) {
    let max = vars.maxpitch / 180.0 * PI;
    let mut turn_ang = state.dude_pitch + PI * vars.turnspeed * secs;

    if turn_ang >= 2.0 * PI {
        turn_ang = turn_ang - 2.0 * PI;
    } else if turn_ang < 0.0 {
        turn_ang = turn_ang + 2.0 * PI;
    }

    if turn_ang <= max || turn_ang >= PI * 2.0 - max {
        state.dude_pitch = turn_ang;
    }
}

fn get_key_from_string(string: &str) -> Option<Key> {
    match string {
        "Digit0" => Some(Key::Key0),
        "Digit1" => Some(Key::Key1),
        "Digit2" => Some(Key::Key2),
        "Digit3" => Some(Key::Key3),
        "Digit4" => Some(Key::Key4),
        "Digit5" => Some(Key::Key5),
        "Digit6" => Some(Key::Key6),
        "Digit7" => Some(Key::Key7),
        "Digit8" => Some(Key::Key8),
        "Digit9" => Some(Key::Key9),
        "KeyA" => Some(Key::A),
        "KeyB" => Some(Key::B),
        "KeyC" => Some(Key::C),
        "KeyD" => Some(Key::D),
        "KeyE" => Some(Key::E),
        "KeyF" => Some(Key::F),
        "KeyG" => Some(Key::G),
        "KeyH" => Some(Key::H),
        "KeyI" => Some(Key::I),
        "KeyJ" => Some(Key::J),
        "KeyK" => Some(Key::K),
        "KeyL" => Some(Key::L),
        "KeyM" => Some(Key::M),
        "KeyN" => Some(Key::N),
        "KeyO" => Some(Key::O),
        "KeyP" => Some(Key::P),
        "KeyQ" => Some(Key::Q),
        "KeyR" => Some(Key::R),
        "KeyS" => Some(Key::S),
        "KeyT" => Some(Key::T),
        "KeyU" => Some(Key::U),
        "KeyV" => Some(Key::V),
        "KeyW" => Some(Key::W),
        "KeyX" => Some(Key::X),
        "KeyY" => Some(Key::Y),
        "KeyZ" => Some(Key::Z),
        "Escape" => Some(Key::Escape),
        "F1" => Some(Key::F1),
        "F2" => Some(Key::F2),
        "F3" => Some(Key::F3),
        "F4" => Some(Key::F4),
        "F5" => Some(Key::F5),
        "F6" => Some(Key::F6),
        "F7" => Some(Key::F7),
        "F8" => Some(Key::F8),
        "F9" => Some(Key::F9),
        "F10" => Some(Key::F10),
        "F11" => Some(Key::F11),
        "F12" => Some(Key::F12),
        "ScrollLock" => Some(Key::ScrollLock),
        "Pause" => Some(Key::Pause),
        "Insert" => Some(Key::Insert),
        "Home" => Some(Key::Home),
        "PageUp" => Some(Key::PageUp),
        "Delete" => Some(Key::Delete),
        "End" => Some(Key::End),
        "PageDown" => Some(Key::PageDown),
        "ArrowUp" => Some(Key::Up),
        "ArrowLeft" => Some(Key::Left),
        "ArrowRight" => Some(Key::Right),
        "ArrowDown" => Some(Key::Down),
        "Backspace" => Some(Key::Backspace),
        "Enter" => Some(Key::Enter),
        "Tab" => Some(Key::Tab),
        "Space" => Some(Key::Space),
        "Minus" => Some(Key::Minus),
        "Equal" => Some(Key::Equal),
        "BracketLeft" => Some(Key::LeftBracket),
        "BracketRight" => Some(Key::RightBracket),
        "Backslash" => Some(Key::Backslash),
        "Semicolon" => Some(Key::Semicolon),
        "Quote" => Some(Key::Apostrophe),
        "Backquote" => Some(Key::Backslash),
        "Comma" => Some(Key::Comma),
        "Period" => Some(Key::Period),
        "Slash" => Some(Key::Slash),
        "ShiftLeft" => Some(Key::LeftShift),
        "ShiftRight" => Some(Key::RightShift),
        "ControlLeft" => Some(Key::LeftCtrl),
        "ControlRight" => Some(Key::RightCtrl),
        _ => None,
    }
}

pub fn char_from_key(key: Key, shift_key: bool) -> Option<char> {
    if !shift_key {
      match key {
          Key::Key0 => Some('0'),
          Key::Key1 => Some('1'),
          Key::Key2 => Some('2'),
          Key::Key3 => Some('3'),
          Key::Key4 => Some('4'),
          Key::Key5 => Some('5'),
          Key::Key6 => Some('6'),
          Key::Key7 => Some('7'),
          Key::Key8 => Some('8'),
          Key::Key9 => Some('9'),
          Key::A => Some('a'),
          Key::B => Some('b'),
          Key::C => Some('c'),
          Key::D => Some('d'),
          Key::E => Some('e'),
          Key::F => Some('f'),
          Key::G => Some('g'),
          Key::H => Some('h'),
          Key::I => Some('i'),
          Key::J => Some('j'),
          Key::K => Some('k'),
          Key::L => Some('l'),
          Key::M => Some('m'),
          Key::N => Some('n'),
          Key::O => Some('o'),
          Key::P => Some('p'),
          Key::Q => Some('q'),
          Key::R => Some('r'),
          Key::S => Some('s'),
          Key::T => Some('t'),
          Key::U => Some('u'),
          Key::V => Some('v'),
          Key::W => Some('w'),
          Key::X => Some('x'),
          Key::Y => Some('y'),
          Key::Z => Some('z'),
          Key::Tab => Some('\t'),
          Key::Space => Some(' '),
          Key::Minus => Some('-'),
          Key::Equal => Some('='),
          Key::LeftBracket => Some('['),
          Key::RightBracket => Some(']'),
          Key::Backslash => Some('\\'),
          Key::Semicolon => Some(';'),
          Key::Apostrophe => Some('\''),
          Key::Backquote => Some('`'),
          Key::Comma => Some(','),
          Key::Period => Some('.'),
          Key::Slash => Some('/'),
          _ => None,
      }
    } else {
      match key {
          Key::Key0 => Some(')'),
          Key::Key1 => Some('!'),
          Key::Key2 => Some('@'),
          Key::Key3 => Some('#'),
          Key::Key4 => Some('$'),
          Key::Key5 => Some('%'),
          Key::Key6 => Some('^'),
          Key::Key7 => Some('&'),
          Key::Key8 => Some('*'),
          Key::Key9 => Some('('),
          Key::A => Some('A'),
          Key::B => Some('B'),
          Key::C => Some('C'),
          Key::D => Some('D'),
          Key::E => Some('E'),
          Key::F => Some('F'),
          Key::G => Some('G'),
          Key::H => Some('H'),
          Key::I => Some('I'),
          Key::J => Some('J'),
          Key::K => Some('K'),
          Key::L => Some('L'),
          Key::M => Some('M'),
          Key::N => Some('N'),
          Key::O => Some('O'),
          Key::P => Some('P'),
          Key::Q => Some('Q'),
          Key::R => Some('R'),
          Key::S => Some('S'),
          Key::T => Some('T'),
          Key::U => Some('U'),
          Key::V => Some('V'),
          Key::W => Some('W'),
          Key::X => Some('X'),
          Key::Y => Some('Y'),
          Key::Z => Some('Z'),
          Key::Tab => Some('\t'),
          Key::Space => Some(' '),
          Key::Minus => Some('_'),
          Key::Equal => Some('+'),
          Key::LeftBracket => Some('{'),
          Key::RightBracket => Some('}'),
          Key::Backslash => Some('|'),
          Key::Semicolon => Some(':'),
          Key::Apostrophe => Some('"'),
          Key::Backquote => Some('~'),
          Key::Comma => Some('<'),
          Key::Period => Some('>'),
          Key::Slash => Some('?'),
          _ => None,
      }
    }
}
