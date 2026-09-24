use std::f32::consts::{FRAC_PI_2, PI};

use crate::{interpolate::interpolate_f32, utils::unit_mod_f32};

const TAC: usize = 16;
const FTAC: f32 = TAC as f32;
const TABLE: [f32; TAC] = [
    0.0, 0.098017, 0.19509, 0.290285, 0.382683, 0.471397, 0.55557, 0.634393, 0.707107, 0.77301,
    0.83147, 0.881921, 0.92388, 0.95694, 0.980785, 0.995185,
];
const PI2: f32 = PI * 2.0;

fn gata(idx: usize) -> f32 {
    if idx == TAC {
        return 1.0;
    }
    return TABLE[idx];
}

fn pre_gata(uidx: usize) -> f32 {
    let idx = uidx % TAC;
    if uidx < TAC || uidx == TAC * 4 {
        return gata(idx);
    } else if uidx >= TAC && uidx < TAC * 2 {
        return gata(TAC - idx);
    } else if uidx >= TAC * 2 && uidx < TAC * 3 {
        return 0.0 - gata(idx);
    } else {
        return 0.0 - gata(TAC - idx);
    }
}

pub fn sin(num: f32) -> f32 {
    let unum = num.rem_euclid(PI2);
    let uidx_n = unum / PI2 * FTAC * 4.0;
    let uidx = uidx_n as usize;
    let uidx2 = uidx + 1;
    let edn = unit_mod_f32(uidx_n);
    return interpolate_f32(pre_gata(uidx), pre_gata(uidx2), edn);
    // return pre_gata(uidx);
    // return num.sin();
}

pub fn cos(num: f32) -> f32 {
    return sin(num + FRAC_PI_2);
    // return num.cos();
}
