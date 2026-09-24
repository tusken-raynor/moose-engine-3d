use minifb::{MouseMode, Window};

use crate::{
    controls::{
        get_mouse_pos, move_backward, move_forward, move_left, move_right, pitch_down, pitch_up,
        set_mouse_pos, yaw_left, yaw_right,
    },
    setup::{Vars, State},
};

pub fn handle(window: &Window, utprl: f32, mut state: &mut State, vars: &Vars) {
    let new_pos = window
        .get_mouse_pos(MouseMode::Pass)
        .expect("Invalid mouse input");
    // Grab the mouse delta
    let mouse_pos = get_mouse_pos();
    let diffx = new_pos.0 - mouse_pos.0;
    let diffy = new_pos.1 - mouse_pos.1;
    // Save the new mouse position
    set_mouse_pos(new_pos.0, new_pos.1);
    if diffx < 0.0 {
        yaw_left(diffx / -1000.0, &mut state, &vars);
    } else if diffx > 0.0 {
        yaw_right(diffx / 1000.0, &mut state, &vars);
    }
    if diffy < 0.0 {
        pitch_up(diffy / -1000.0, &mut state, &vars);
    } else if diffy > 0.0 {
        pitch_down(diffy / 1000.0, &mut state, &vars);
    }

    /* MOVEMENT */
    if state.moving_forward {
        move_forward(utprl, &mut state, &vars);
    }
    if state.moving_backward {
        move_backward(utprl, &mut state, &vars);
    }
    if state.moving_left {
        move_left(utprl, &mut state, &vars);
    }
    if state.moving_right {
        move_right(utprl, &mut state, &vars);
    }
    if state.pitching_up {
        pitch_up(utprl, &mut state, &vars);
    }
    if state.pitching_down {
        pitch_down(utprl, &mut state, &vars);
    }
    if state.yawing_left {
        yaw_right(utprl, &mut state, &vars);
    }
    if state.yawing_right {
        yaw_left(utprl, &mut state, &vars);
    }
    /* MOVEMENT */
}
