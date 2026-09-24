use std::f32::consts::PI;

use crate::{
    interpolate::{interpolate_f32, interpolate_u16, interpolate_vec_s, derive_alpha_f32},
    matrix::{apply_transformation_to_point, create_matrix_4x4},
    utils::mod_from_range_usize, vertex::ProjectedVertex,
};

pub struct Camera {
    matrix: [f32; 16],
    width: u32,
    height: u32,
}

impl Camera {
    pub fn new(width: u32, height: u32, mut fov: f32, znear: f32, zfar: f32) -> Self {
        fov = if fov >= 180.0 { 179.9 } else { fov };
        let aspect_ratio = height as f32 / width as f32;
        let radians = fov as f32 / 180.0 * PI;
        let w = znear / (1.0 / (radians / 2.0).tan()) * 2.0;
        let h = w * aspect_ratio;
        let mut matrix = create_matrix_4x4();
        matrix[0] = 2.0 * znear / w;
        matrix[5] = 2.0 * znear / h;
        matrix[10] = 1.0 / (zfar - znear);
        matrix[11] = -znear / (zfar - znear);
        matrix[14] = 1.0;
        matrix[15] = 0.0;
        Self {
            matrix,
            width,
            height,
        }
    }
    pub fn update(&mut self, width: u32, height: u32, mut fov: f32, znear: f32, zfar: f32) {
        fov = if fov >= 180.0 { 179.9 } else { fov };
        let aspect_ratio = height as f32 / width as f32;
        let radians = fov as f32 / 180.0 * PI;
        let w = znear / (1.0 / (radians / 2.0).tan()) * 2.0;
        let h = w * aspect_ratio;
        self.matrix[0] = 2.0 * znear / w;
        self.matrix[5] = 2.0 * znear / h;
        self.matrix[10] = 1.0 / (zfar - znear);
        self.matrix[11] = -znear / (zfar - znear);
        self.matrix[14] = 1.0;
        self.matrix[15] = 0.0;
        self.width = width;
        self.height = height;
    }

    pub fn rect_in_view(
        &self,
        rect: [[f32; 3]; 8],
        calc_occlusion: bool,
        occlusion_rect: &mut (usize, usize, usize, usize, u32),
    ) -> bool {
        let mut cull_near = true;
        let mut cull_far = true;
        let mut cull_left = true;
        let mut cull_right = true;
        let mut cull_top = true;
        let mut cull_bottom = true;
        let mut t_points = vec![];
        for corner in rect {
            let t_point = apply_transformation_to_point(self.matrix, corner);
            // Attempt to find the rect within the frustrum
            if t_point[2] >= 0.0 {
                cull_near = false;
            }
            if t_point[2] <= 1.0 {
                cull_far = false;
            }
            if t_point[0] >= -t_point[3] {
                cull_left = false;
            }
            if t_point[0] <= t_point[3] {
                cull_right = false;
            }
            if t_point[1] >= -t_point[3] {
                cull_bottom = false;
            }
            if t_point[1] <= t_point[3] {
                cull_top = false;
            }
            if calc_occlusion {
                t_points.push(t_point);
            }
        }
        if cull_far || cull_near || cull_left || cull_right || cull_top || cull_bottom {
            return false;
        }
        if calc_occlusion {
            let w = self.width as usize;
            let h = self.height as usize;
            // Construct an occlusion rectangle
            let mut bottom: usize = 0;
            let mut top = h;
            let mut left = w;
            let mut right: usize = 0;
            let mut depth = u32::MAX as i64;
            for point in t_points {
                let x = ((1.0 - point[0] / point[3]) / 2.0 * w as f32) as isize;
                let y = ((1.0 - point[1] / point[3]) / 2.0 * h as f32) as isize;
                let z = (point[2] as f64 * u32::MAX as f64) as i64;
                // OPT you are maxing and mining number each time whenever they are smaller
                // when you could just do it once at the end
                if x < left as isize {
                    left = x.max(0) as usize;
                }
                if x > right as isize {
                    right = x.min(w as isize - 1) as usize;
                }
                if y < top as isize {
                    top = y.max(0) as usize;
                }
                if y > bottom as isize {
                    bottom = y.min(h as isize - 1) as usize;
                }
                if z < depth {
                    depth = z;
                }
            }
            if depth > 0 && left < right && top < bottom {
                occlusion_rect.0 = left;
                occlusion_rect.1 = right;
                occlusion_rect.2 = bottom;
                occlusion_rect.3 = top;
                occlusion_rect.4 = depth as u32;
            }
        }
        return true;
    }
    pub fn project_face(
        &self,
        // 0: x, 1: y, 2: z, 3: normal x, 4: normal y, 5: normal z, 6: u, 7: v, 8: x world, 9: y world, 10: z world
        points: Vec<[f32; 11]>,
        y_bounds: &mut (usize, usize),
        mipmap_distances: &Vec<f32>,
    ) -> Vec<ProjectedVertex> {
        // 0: x screen, 1: y screen, 2: z depth, 3: w,
        // 4: normal x, 5: normal y, 6: normal z,
        // 7: u, 8: v,
        // 9: x world, 10: y world, 11: z world
        let mut t_points: Vec<[f32; 13]> = vec![];
        // Track if any points of the face fall inside the frustrum
        let mut cull_near = true;
        let mut cull_far = true;
        let mut cull_left = true;
        let mut cull_right = true;
        let mut cull_top = true;
        let mut cull_bottom = true;
        let mut clip_z = false;
        let mut clip_left = false;
        let mut clip_right = false;
        let mut clip_top = false;
        let mut clip_bottom = false;
        for point in points {
            let t_point =
                apply_transformation_to_point(self.matrix, [point[0], point[1], point[2]]);
            // Attempt to find the face within the frustrum
            if t_point[2] >= 0.0 {
                cull_near = false;
            } else {
                clip_z = true;
            }
            if t_point[2] <= 1.0 {
                cull_far = false;
            }
            if t_point[0] >= -t_point[3] {
                cull_left = false;
            } else {
                clip_left = true;
            }
            if t_point[0] <= t_point[3] {
                cull_right = false;
            } else {
                clip_right = true;
            }
            if t_point[1] >= -t_point[3] {
                cull_bottom = false;
            } else {
                clip_bottom = true;
            }
            if t_point[1] <= t_point[3] {
                cull_top = false;
            } else {
                clip_top = true;
            }
            // Calculate the mipmap level index (with floating point component)
            let mut mm_idx = 0.0f32;
            let len = mipmap_distances.len();
            if len > 1 {
              let mut i0 = 0;
              let mut i1 = 1;
              while i1 < len {
                let l0 = mipmap_distances[i0];
                let l1 = mipmap_distances[i1];
                if point[2] >= l0 && point[2] < l1 {
                  mm_idx = i0 as f32;
                  mm_idx += derive_alpha_f32(l0, l1, point[2]);
                } else if point[2] >= l1 {
                  // OPT: maybe move the check outside the loop to reduce number of checks
                  mm_idx = (len - 1) as f32;
                  break;
                }
                i0 = i1;
                i1 += 1;
              }
            }
            t_points.push([
                t_point[0], // x
                t_point[1], // y
                t_point[2], // z
                t_point[3], // w
                point[3],   // normal x
                point[4],   // normal y
                point[5],   // normal z
                point[6],   // u
                point[7],   // v
                point[8],   // x world
                point[9],   // y world
                point[10],  // z world
                mm_idx,     // mipmap level
            ]);
        }
        if cull_near || cull_far || cull_left || cull_right || cull_top || cull_bottom {
            // If we get in here, that means all the points of the face are outside the frustrum
            return vec![];
        }
        // We wont be clipping along the far plane, as
        // it would be more expensive to clip on that
        // plane than what little savings we might get
        if clip_z {
            // 0: x screen, 1: y screen, 2: z depth, 3: w,
            // 4: normal x, 5: normal y, 6: normal z,
            // 7: u, 8: v,
            // 9: x world, 10: y world, 11: z world
            let mut c_points: Vec<[f32; 13]> = vec![];
            let mut intersections = 0;
            let len = t_points.len();
            let mut i = 0;
            while i < len {
                let this_point = t_points[i];
                let next_point = t_points[mod_from_range_usize(i + 1, len)];
                if this_point[2] >= 0.0 && next_point[2] < 0.0 {
                    let alpha = this_point[2] / (this_point[2] - next_point[2]);
                    let x = interpolate_f32(this_point[0], next_point[0], alpha);
                    let y = interpolate_f32(this_point[1], next_point[1], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);

                    let new_point: [f32; 13] = [x, y, 0.0, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(this_point);
                    c_points.push(new_point);
                    intersections += 1;
                    if intersections > 1 {
                        break;
                    }
                } else if this_point[2] < 0.0 && next_point[2] >= 0.0 {
                    let alpha = this_point[2] / (this_point[2] - next_point[2]);
                    let x = interpolate_f32(this_point[0], next_point[0], alpha);
                    let y = interpolate_f32(this_point[1], next_point[1], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);

                    let new_point: [f32; 13] = [x, y, 0.0, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(new_point);
                    intersections += 1;
                } else if this_point[2] >= 0.0 {
                    c_points.push(this_point);
                }
                i += 1;
            }
            t_points = c_points;
        }
        // OPT Along each edge of the frustrum, we have some identical code which can be consolidated
        if clip_left {
            // 0: x screen, 1: y screen, 2: z depth, 3: w,
            // 4: normal x, 5: normal y, 6: normal z,
            // 7: u, 8: v,
            // 9: x world, 10: y world, 11: z world
            let mut c_points: Vec<[f32; 13]> = vec![];
            let mut intersections = 0;
            let len = t_points.len();
            let mut i = 0;
            while i < len {
                let this_point = t_points[i];
                let next_point = t_points[mod_from_range_usize(i + 1, len)];
                if this_point[0] >= -this_point[3] && next_point[0] < -next_point[3] {
                    let alpha = (this_point[0] + this_point[3])
                        / ((this_point[0] + this_point[3]) - (next_point[0] + next_point[3]));
                    let y = interpolate_f32(this_point[1], next_point[1], alpha);
                    let z = interpolate_f32(this_point[2], next_point[2], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);

                    let new_point: [f32; 13] = [-w, y, z, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(this_point);
                    c_points.push(new_point);
                    intersections += 1;
                    if intersections > 1 {
                        break;
                    }
                } else if this_point[0] < -this_point[3] && next_point[0] >= -next_point[3] {
                    let alpha = (this_point[0] + this_point[3])
                        / ((this_point[0] + this_point[3]) - (next_point[0] + next_point[3]));
                    let y = interpolate_f32(this_point[1], next_point[1], alpha);
                    let z = interpolate_f32(this_point[2], next_point[2], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);

                    let new_point: [f32; 13] = [-w, y, z, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(new_point);
                    intersections += 1;
                } else if this_point[0] >= -this_point[3] {
                    c_points.push(this_point);
                }
                i += 1;
            }
            t_points = c_points;
        }
        if clip_right {
            // 0: x screen, 1: y screen, 2: z depth, 3: w,
            // 4: normal x, 5: normal y, 6: normal z,
            // 7: u, 8: v,
            // 9: x world, 10: y world, 11: z world
            let mut c_points: Vec<[f32; 13]> = vec![];
            let mut intersections = 0;
            let len = t_points.len();
            let mut i = 0;
            while i < len {
                let this_point = t_points[i];
                let next_point = t_points[mod_from_range_usize(i + 1, len)];
                if this_point[0] <= this_point[3] && next_point[0] > next_point[3] {
                    let alpha = (this_point[0] - this_point[3])
                        / ((this_point[0] - this_point[3]) - (next_point[0] - next_point[3]));
                    let y = interpolate_f32(this_point[1], next_point[1], alpha);
                    let z = interpolate_f32(this_point[2], next_point[2], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);

                    let new_point: [f32; 13] = [w, y, z, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(this_point);
                    c_points.push(new_point);
                    intersections += 1;
                    if intersections > 1 {
                        break;
                    }
                } else if this_point[0] > this_point[3] && next_point[0] <= next_point[3] {
                    let alpha = (this_point[0] - this_point[3])
                        / ((this_point[0] - this_point[3]) - (next_point[0] - next_point[3]));
                    let y = interpolate_f32(this_point[1], next_point[1], alpha);
                    let z = interpolate_f32(this_point[2], next_point[2], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);

                    let new_point: [f32; 13] = [w, y, z, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(new_point);
                    intersections += 1;
                } else if this_point[0] <= this_point[3] {
                    c_points.push(this_point);
                }
                i += 1;
            }
            t_points = c_points;
        }
        if clip_bottom {
            // 0: x screen, 1: y screen, 2: z depth, 3: w,
            // 4: normal x, 5: normal y, 6: normal z,
            // 7: u, 8: v,
            // 9: x world, 10: y world, 11: z world
            let mut c_points: Vec<[f32; 13]> = vec![];
            let mut intersections = 0;
            let len = t_points.len();
            let mut i = 0;
            while i < len {
                let this_point = t_points[i];
                let next_point = t_points[mod_from_range_usize(i + 1, len)];
                if this_point[1] >= -this_point[3] && next_point[1] < -next_point[3] {
                    let alpha = (this_point[1] + this_point[3])
                        / ((this_point[1] + this_point[3]) - (next_point[1] + next_point[3]));
                    let x = interpolate_f32(this_point[0], next_point[0], alpha);
                    let z = interpolate_f32(this_point[2], next_point[2], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);

                    let new_point: [f32; 13] = [x, -w, z, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(this_point);
                    c_points.push(new_point);
                    intersections += 1;
                    if intersections > 1 {
                        break;
                    }
                } else if this_point[1] < -this_point[3] && next_point[1] >= -next_point[3] {
                    let alpha = (this_point[1] - -this_point[3])
                        / ((this_point[1] + this_point[3]) - (next_point[1] + next_point[3]));
                    let x = interpolate_f32(this_point[0], next_point[0], alpha);
                    let z = interpolate_f32(this_point[2], next_point[2], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);

                    let new_point: [f32; 13] = [x, -w, z, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(new_point);
                    intersections += 1;
                } else if this_point[1] >= -this_point[3] {
                    c_points.push(this_point);
                }
                i += 1;
            }
            t_points = c_points;
        }
        if clip_top {
            // 0: x screen, 1: y screen, 2: z depth, 3: w,
            // 4: normal x, 5: normal y, 6: normal z,
            // 7: u, 8: v,
            // 9: x world, 10: y world, 11: z world
            let mut c_points: Vec<[f32; 13]> = vec![];
            let mut intersections = 0;
            let len = t_points.len();
            let mut i = 0;
            while i < len {
                let this_point = t_points[i];
                let next_point = t_points[mod_from_range_usize(i + 1, len)];
                if this_point[1] <= this_point[3] && next_point[1] > next_point[3] {
                    let alpha = (this_point[1] - this_point[3])
                        / ((this_point[1] - this_point[3]) - (next_point[1] - next_point[3]));
                    let x = interpolate_f32(this_point[0], next_point[0], alpha);
                    let z = interpolate_f32(this_point[2], next_point[2], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);
                    
                    let new_point: [f32; 13] = [x, w, z, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(this_point);
                    c_points.push(new_point);
                    intersections += 1;
                    if intersections > 1 {
                        break;
                    }
                } else if this_point[1] > this_point[3] && next_point[1] <= next_point[3] {
                    let alpha = (this_point[1] - this_point[3])
                        / ((this_point[1] - this_point[3]) - (next_point[1] - next_point[3]));
                    let x = interpolate_f32(this_point[0], next_point[0], alpha);
                    let z = interpolate_f32(this_point[2], next_point[2], alpha);
                    let w = interpolate_f32(this_point[3], next_point[3], alpha);
                    let wx = interpolate_f32(this_point[9], next_point[9], alpha);
                    let wy = interpolate_f32(this_point[10], next_point[10], alpha);
                    let wz = interpolate_f32(this_point[11], next_point[11], alpha);
                    let u = interpolate_f32(this_point[7], next_point[7], alpha);
                    let v = interpolate_f32(this_point[8], next_point[8], alpha);
                    let n = interpolate_vec_s(
                        this_point[4],
                        this_point[5],
                        this_point[6],
                        next_point[4],
                        next_point[5],
                        next_point[6],
                        alpha,
                    );
                    let mm = interpolate_f32(this_point[12], next_point[12], alpha);
                    
                    let new_point: [f32; 13] = [x, w, z, w, n[0], n[1], n[2], u, v, wx, wy, wz, mm];

                    c_points.push(new_point);
                    intersections += 1;
                } else if this_point[1] <= this_point[3] {
                    c_points.push(this_point);
                }
                i += 1;
            }
            t_points = c_points;
        }

        let mut p_points: Vec<ProjectedVertex> = vec![];
        for t_point in t_points {
            let p_point = ProjectedVertex {
                // Move the x,y coordinates into screen space
                x: ((1.0 - t_point[0] / t_point[3]) / 2.0 * self.width as f32).round() as usize,
                y: ((1.0 - t_point[1] / t_point[3]) / 2.0 * self.height as f32).round() as usize,
                // Convert the z coordinate to a u32 for the depth buffer
                z: (t_point[2] as f64 * 4294967295.0) as u32,
                // Save reciprocal of w for perspective correction
                w: 1.0 / t_point[3],
                nx: t_point[4],
                ny: t_point[5],
                nz: t_point[6],
                u: (t_point[7] * 65536.0) as i32, // We are reformatting the uv to mimic fixed point format
                v: (t_point[8] * 65536.0) as i32, // 1bit sign, 15bit integer, 16bit decimal
                wx: t_point[9],
                wy: t_point[10],
                wz: t_point[11],
                mm: (t_point[12] * 256.0) as u16, // Convert mipmap level to integer, where alpha component now ranges from 0-255
            };

            // Save the mins and maxs of the y coordinates
            let y_val = p_point.y;
            if y_val < y_bounds.0 {
                y_bounds.0 = y_val;
            }
            if y_val > y_bounds.1 {
                y_bounds.1 = y_val;
            }

            p_points.push(p_point);
        }
        // Scale the x,y coordinates to screen space and include the frustrum normalized coords
        return p_points;
    }

    pub fn project_line(
        &self,
        // 0: x, 1: y, 2: z
        line: [[f32; 3]; 2],
    ) -> [isize; 4] {
        let point1 = apply_transformation_to_point(self.matrix, line[0]);
        let point2 = apply_transformation_to_point(self.matrix, line[1]);

        return [
            ((1.0 - point1[0] / point1[3]) / 2.0 * self.width as f32) as isize,
            ((1.0 - point1[1] / point1[3]) / 2.0 * self.height as f32) as isize,
            ((1.0 - point2[0] / point2[3]) / 2.0 * self.width as f32) as isize,
            ((1.0 - point2[1] / point2[3]) / 2.0 * self.height as f32) as isize,
        ];
    }
}
