use crate::{
    interpolate::{interpolate_f32, interpolate_vec_s},
    threedee::get_face_normal,
};

pub struct ClipPlane {
    x: f32,
    y: f32,
    z: f32,
    normal_x: f32,
    normal_y: f32,
    normal_z: f32,
    dot: f32,
}

impl ClipPlane {
    // 0: x, 1: y, 2: z, 3: normal x, 4: normal y, 5: normal z, 6: u, 7: v, 8: x world, 9: y world, 10: z world
    pub fn clip_face(&self, points: Vec<[f32; 11]>, debug: bool) -> Vec<[f32; 11]> {
        // Loop through the points and determine if the face is completely
        // on the right side, on the wrong side, or clipping the plane
        let mut cull = true;
        let mut clip = false;
        let mut set_s0 = false;
        let mut set_s1 = false;
        let mut set_s2 = false;
        let mut set_s3 = false;
        let mut set_sf1 = false;
        let mut set_sf3 = false;
        let mut val_s0: usize = 0;
        let mut val_s1: usize = 0;
        let mut val_s2: usize = 0;
        let mut val_s3: usize = 0;
        let mut clip_buffer = vec![];
        let len = points.len();
        let mut i = 0;
        while i < len {
            let point = points[i];
            let dot = self.normal_x * (self.x - point[8])
                + self.normal_y * (self.y - point[9])
                + self.normal_z * (self.z - point[10]);

            if dot <= 0.0 {
                cull = false;
                // We are on the right side
                clip_buffer.push(true);
                if !clip {
                    if !set_s3 || (set_s2 && !set_sf3) {
                        val_s3 = i;
                        set_s3 = true;
                        if i != 0 {
                            set_sf3 = true;
                        }
                    }
                    if !set_sf1 {
                        val_s0 = i;
                        set_s0 = true;
                    }
                }
            } else {
                // We are on the wrong side
                clip_buffer.push(false);
                if !clip {
                    if !set_s1 || (set_s0 && !set_sf1) {
                        val_s1 = i;
                        set_s1 = true;
                        if i != 0 {
                            set_sf1 = true;
                        }
                    }
                    if !set_sf3 {
                        val_s2 = i;
                        set_s2 = true;
                    }
                }
            }

            // Set clip to true, and we don't have to calculate clippings anymore
            if set_s0 && set_sf1 && set_s2 && set_sf3 {
                clip = true;
            }
            i += 1;
        }
        // If all the non-final index sets are true, then let's clip
        if set_s0 && set_s1 && set_s2 && set_s3 {
            clip = true;
        }

        if debug {
            println!(
                "{:?} -- {} :: {} :: {} :: {} -- {}",
                clip_buffer, set_s0, set_s1, set_s2, set_s3, clip
            );
        }

        if cull {
            // Discard the face
            return Vec::new();
        } else if clip {
            let mut clipped_points = vec![];
            // Clip the face
            i = 0;
            let mut clip_alt = false;
            while i < len {
                let keep = clip_buffer[i];
                if i == val_s1 || i == val_s2 {
                    let do_primary = i == val_s1 && !clip_alt;
                    let point1 = if do_primary {
                        points[val_s0]
                    } else {
                        points[val_s2]
                    };
                    let point2 = if do_primary {
                        points[val_s1]
                    } else {
                        points[val_s3]
                    };
                    // Generate the clip point thing
                    let [wx, wy, wz, alpha] = get_line_plane_intersection(
                        self.dot,
                        self.normal_x,
                        self.normal_y,
                        self.normal_z,
                        point1[8],
                        point1[9],
                        point1[10],
                        point2[8],
                        point2[9],
                        point2[10],
                    );
                    let [normal_x, normal_y, normal_z] = interpolate_vec_s(
                        point1[3], point1[4], point1[5], point2[3], point2[4], point2[5], alpha,
                    );
                    let u = interpolate_f32(point1[6], point2[6], alpha);
                    let v = interpolate_f32(point1[7], point2[7], alpha);
                    let x = interpolate_f32(point1[0], point2[0], alpha);
                    let y = interpolate_f32(point1[1], point2[1], alpha);
                    let z = interpolate_f32(point1[2], point2[2], alpha);
                    clipped_points.push([x, y, z, normal_x, normal_y, normal_z, u, v, wx, wy, wz]);
                } else if keep {
                    clipped_points.push(points[i]);
                }

                // If the the index for both segment parts are the same, go through the loop again
                // but alternate to the other condition
                if val_s1 == val_s2 && val_s1 == i && !clip_alt {
                    clip_alt = true;
                } else {
                    i += 1;
                }
            }
            return clipped_points;
        }

        // Just return the face
        return points;
    }
}

impl ClipPlane {
    pub fn from_triangle(points: [[f32; 3]; 3]) -> ClipPlane {
        let [x, y, z] = points[0].clone();
        let [normal_x, normal_y, normal_z] = get_face_normal(&points.to_vec());
        return ClipPlane {
            x,
            y,
            z,
            normal_x,
            normal_y,
            normal_z,
            dot: normal_x * x + normal_y * y + normal_z * z,
        };
    }
}

// Returns the x,y,z coordinates as well as the scalar between the two original points of the line.
fn get_line_plane_intersection(
    dot: f32,
    nx: f32,
    ny: f32,
    nz: f32,
    x1: f32,
    y1: f32,
    z1: f32,
    x2: f32,
    y2: f32,
    z2: f32,
) -> [f32; 4] {
    let x_diff = x2 - x1;
    let y_diff = y2 - y1;
    let z_diff = z2 - z1;

    let t = (dot - nx * x1 - ny * y1 - nz * z1) / (nx * x_diff + ny * y_diff + nz * z_diff);

    return [x1 + t * x_diff, y1 + t * y_diff, z1 + t * z_diff, t];
}
