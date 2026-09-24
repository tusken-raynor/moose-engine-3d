use crate::{
    colors::{from_rgb, greyscale},
    matrix::{multiply_matrices_1x3_3x1, multiply_matrices_1x3_3x3},
    threedee::{normalize_vec, point_pythag_soft},
};

pub struct DirectionalLight {
    pub dir: [f32; 3],
    pub intensity: [u8; 3],
    pub intensity_grey: u8,
}

impl DirectionalLight {
    pub fn new(direction: [f32; 3], intensity: [u8; 3]) -> DirectionalLight {
        Self {
            dir: normalize_vec(direction),
            intensity,
            intensity_grey: greyscale(intensity[0], intensity[1], intensity[2]),
        }
    }
}

pub struct PointLight {
    pub pos: [f32; 3],
    pub intensity: [u8; 3],
    pub intensity_grey: u8,
    pub range: f32,
}

impl PointLight {
    pub fn new(position: [f32; 3], intensity: [u8; 3], range: f32) -> PointLight {
        Self {
            pos: position,
            intensity,
            intensity_grey: greyscale(intensity[0], intensity[1], intensity[2]),
            range,
        }
    }
}

pub fn get_vector_light(
    vec: [f32; 3],
    pos: [f32; 3],
    color: [u8; 3],
    ambient: &mut [f32; 3],
    direction_lights: &Vec<DirectionalLight>,
    point_lights: &Vec<PointLight>,
    coloredlighting: bool,
) -> u32 {
    // OPT all the light operations can be compounded into a single matrix
    // Can they though? It more likely will work for directional lights, but it might
    // not for point light sources
    // EDIT: Ruvecing into issues with compounding light matrices create negative light
    // OPT Maybe conversion from ndc to u8 can be coloruced
    // OPT Calculations seem to be made for back facing triangles, do not that
    if coloredlighting {
        let mut i = 0;
        let len = direction_lights.len();

        while i < len {
            let light = &direction_lights[i];
            let matrix: [f32; 9] = [
                -light.dir[0] * (light.intensity[0] as f32 / 255.0),
                -light.dir[0] * (light.intensity[1] as f32 / 255.0),
                -light.dir[0] * (light.intensity[2] as f32 / 255.0),
                -light.dir[1] * (light.intensity[0] as f32 / 255.0),
                -light.dir[1] * (light.intensity[1] as f32 / 255.0),
                -light.dir[1] * (light.intensity[2] as f32 / 255.0),
                -light.dir[2] * (light.intensity[0] as f32 / 255.0),
                -light.dir[2] * (light.intensity[1] as f32 / 255.0),
                -light.dir[2] * (light.intensity[2] as f32 / 255.0),
            ];
            // light_matrix = add_matrices_3x3(matrix, light_matrix);
            let vals = multiply_matrices_1x3_3x3(vec, matrix);
            ambient[0] += vals[0].max(0.0);
            ambient[1] += vals[1].max(0.0);
            ambient[2] += vals[2].max(0.0);
            i += 1;
        }
        i = 0;
        while i < point_lights.len() {
            let light = &point_lights[i];
            let mut dist = point_pythag_soft(light.pos, pos);
            if dist < light.range * light.range {
                dist = dist.sqrt();
                let mut alpha: f32 = 1.0 - dist / light.range;
                alpha = (alpha * alpha).clamp(0.0, 1.1);
                let nvec: [f32; 3] = normalize_vec([
                    light.pos[0] - pos[0],
                    light.pos[1] - pos[1],
                    light.pos[2] - pos[2],
                ]);
                let matrix = [
                    nvec[0] * (light.intensity[0] as f32 / 255.0),
                    nvec[0] * (light.intensity[1] as f32 / 255.0),
                    nvec[0] * (light.intensity[2] as f32 / 255.0),
                    nvec[1] * (light.intensity[0] as f32 / 255.0),
                    nvec[1] * (light.intensity[1] as f32 / 255.0),
                    nvec[1] * (light.intensity[2] as f32 / 255.0),
                    nvec[2] * (light.intensity[0] as f32 / 255.0),
                    nvec[2] * (light.intensity[1] as f32 / 255.0),
                    nvec[2] * (light.intensity[2] as f32 / 255.0),
                ];
                // light_matrix = add_matrices_3x3(matrix, light_matrix);
                let vals = multiply_matrices_1x3_3x3(vec, matrix);
                ambient[0] += vals[0].max(0.0) * alpha;
                ambient[1] += vals[1].max(0.0) * alpha;
                ambient[2] += vals[2].max(0.0) * alpha;
            }
            i += 1;
        }

        return from_rgb(
            (ambient[0].clamp(0.0, 1.0) * color[0] as f32) as u8,
            (ambient[1].clamp(0.0, 1.0) * color[1] as f32) as u8,
            (ambient[2].clamp(0.0, 1.0) * color[2] as f32) as u8,
        );
    } else {
        let mut i = 0;
        let len = direction_lights.len();

        while i < len {
            let light = &direction_lights[i];
            let matrix: [f32; 3] = [
                -light.dir[0] * (light.intensity_grey as f32 / 255.0),
                -light.dir[1] * (light.intensity_grey as f32 / 255.0),
                -light.dir[2] * (light.intensity_grey as f32 / 255.0),
            ];
            // light_matrix = add_matrices_3x3(matrix, light_matrix);
            let vals = multiply_matrices_1x3_3x1(vec, matrix);
            ambient[0] += vals[0].max(0.0);
            ambient[1] += vals[0].max(0.0);
            ambient[2] += vals[0].max(0.0);
            i += 1;
        }
        i = 0;
        while i < point_lights.len() {
            let light = &point_lights[i];
            let mut dist = point_pythag_soft(light.pos, pos);
            if dist < light.range * light.range {
                dist = dist.sqrt();
                let mut alpha = 1.0 - dist / light.range;
                alpha = alpha * alpha;
                let nvec: [f32; 3] = normalize_vec([
                    light.pos[0] - pos[0],
                    light.pos[1] - pos[1],
                    light.pos[2] - pos[2],
                ]);
                let matrix = [
                    nvec[0] * (light.intensity_grey as f32 / 255.0),
                    nvec[1] * (light.intensity_grey as f32 / 255.0),
                    nvec[2] * (light.intensity_grey as f32 / 255.0),
                ];
                // light_matrix = add_matrices_3x3(matrix, light_matrix);
                let vals = multiply_matrices_1x3_3x1(vec, matrix);
                ambient[0] += vals[0].max(0.0) * alpha;
                ambient[1] += vals[0].max(0.0) * alpha;
                ambient[2] += vals[0].max(0.0) * alpha;
            }
            i += 1;
        }

        return from_rgb(
            (ambient[0].clamp(0.0, 1.0) * color[0] as f32) as u8,
            (ambient[1].clamp(0.0, 1.0) * color[1] as f32) as u8,
            (ambient[2].clamp(0.0, 1.0) * color[2] as f32) as u8,
        );
    }
}
