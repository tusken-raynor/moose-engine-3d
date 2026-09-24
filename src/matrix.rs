use crate::trig::{cos, sin};
#[derive(Debug, Clone)]
pub struct Transformation {
    matrices: Vec<[f32; 16]>,
    compound_matrix: [f32; 16],
    origin: [f32; 3],
    built: bool,
}

impl Transformation {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self {
            matrices: Vec::new(),
            origin: [x, y, z],
            compound_matrix: create_matrix_4x4(),
            built: false,
        }
    }
    pub fn get_origin(&self) -> [f32; 3] {
        return self.origin.clone();
    }
    pub fn set_origin(&mut self, origin: [f32; 3]) {
        self.origin = origin;
    }
    pub fn set_origin_x(&mut self, val: f32) {
        self.origin[0] = val;
    }
    pub fn set_origin_y(&mut self, val: f32) {
        self.origin[1] = val;
    }
    pub fn set_origin_z(&mut self, val: f32) {
        self.origin[2] = val;
    }
    pub fn translate(&mut self, x: f32, y: f32, z: f32) {
        let mut matrix = create_matrix_4x4();
        matrix[3] = x;
        matrix[7] = y;
        matrix[11] = z;
        self.matrices.insert(0, matrix);
    }
    pub fn scale(&mut self, x: f32, y: f32, z: f32) {
        let mut matrix = create_matrix_4x4();
        matrix[0] = x;
        matrix[5] = y;
        matrix[10] = z;
        self.matrices.insert(0, matrix);
    }
    pub fn rotate_x(&mut self, angle: f32) {
        let mut matrix = create_matrix_4x4();
        matrix[5] = cos(angle);
        matrix[6] = -sin(angle);
        matrix[9] = sin(angle);
        matrix[10] = cos(angle);
        self.matrices.insert(0, matrix);
    }
    pub fn rotate_y(&mut self, angle: f32) {
        let mut matrix = create_matrix_4x4();
        matrix[0] = cos(angle);
        matrix[2] = sin(angle);
        matrix[8] = -sin(angle);
        matrix[10] = cos(angle);
        self.matrices.insert(0, matrix);
    }
    pub fn rotate_z(&mut self, angle: f32) {
        let mut matrix = create_matrix_4x4();
        matrix[0] = cos(angle);
        matrix[1] = -sin(angle);
        matrix[4] = sin(angle);
        matrix[5] = cos(angle);
        self.matrices.insert(0, matrix);
    }
    pub fn rotate(&mut self, x: f32, y: f32, z: f32) {
        self.rotate_x(x);
        self.rotate_y(y);
        self.rotate_z(z);
    }
    pub fn build_matrix(&mut self) {
        // If the transformation origin isn't at zero, adjust the origin to be
        // at zero before the other transformations are multiplied in, then move
        // it back to the original origin afterwards
        let mut matrices = self.matrices.to_vec();
        if self.origin[0] != 0.0 || self.origin[1] != 0.0 || self.origin[2] != 0.0 {
            // let mut matrix_s = create_matrix_4x4();
            // matrix_s[3] = -self.origin[0];
            // matrix_s[7] = -self.origin[1];
            // matrix_s[11] = -self.origin[2];
            // matrices.push(matrix_s);
            let mut matrix_f = create_matrix_4x4();
            matrix_f[3] = self.origin[0];
            matrix_f[7] = self.origin[1];
            matrix_f[11] = self.origin[2];
            matrices.insert(0, matrix_f)
        }
        let len = matrices.len();
        let mut matrix = if len > 0 {
            matrices[0].clone()
        } else {
            create_matrix_4x4()
        };
        let mut i: usize = 1;
        while i < len {
            matrix = multiply_matrices_4x4(matrix, matrices[i]);
            i += 1;
        }
        self.compound_matrix = matrix;
        self.built = true;
    }
    pub fn transform_point(&self, point: [f32; 3]) -> [f32; 3] {
        if self.built {
            let t_point = apply_transformation_to_point(self.compound_matrix, point);
            return [t_point[0], t_point[1], t_point[2]];
        }
        // Scale the coordinates to screen space
        return point;
    }
    pub fn reset(&mut self) {
        self.matrices.clear();
        self.compound_matrix = create_matrix_4x4();
        self.built = false;
    }
}

pub fn create_matrix_4x4() -> [f32; 16] {
    return [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
}

pub fn multiply_matrices_1x3_3x3(m1: [f32; 3], m2: [f32; 9]) -> [f32; 3] {
    return [
        // First row
        m1[0] * m2[0] + m1[1] * m2[3] + m1[2] * m2[6], // Column 1
        m1[0] * m2[1] + m1[1] * m2[4] + m1[2] * m2[7], // Column 2
        m1[0] * m2[2] + m1[1] * m2[5] + m1[2] * m2[8], // Column 3
    ];
}

pub fn multiply_matrices_1x3_3x1(m1: [f32; 3], m2: [f32; 3]) -> [f32; 1] {
    return [
        // First row
        m1[0] * m2[0] + m1[1] * m2[1] + m1[2] * m2[2], // Column 1
    ];
}

pub fn multiply_matrices_3x3(m1: [f32; 9], m2: [f32; 9]) -> [f32; 9] {
    return [
        // First row
        m1[0] * m2[0] + m1[1] * m2[3] + m1[2] * m2[6], // Column 1
        m1[0] * m2[1] + m1[1] * m2[4] + m1[2] * m2[7], // Column 2
        m1[0] * m2[2] + m1[1] * m2[5] + m1[2] * m2[8], // Column 3
        // Second row
        m1[3] * m2[0] + m1[4] * m2[3] + m1[5] * m2[6], // Column 1
        m1[3] * m2[1] + m1[4] * m2[4] + m1[5] * m2[7], // Column 2
        m1[3] * m2[2] + m1[4] * m2[5] + m1[5] * m2[8], // Column 3
        // Thrid row
        m1[6] * m2[0] + m1[7] * m2[3] + m1[8] * m2[6], // Column 1
        m1[6] * m2[1] + m1[7] * m2[4] + m1[8] * m2[7], // Column 2
        m1[6] * m2[2] + m1[7] * m2[5] + m1[8] * m2[8], // Column 3
    ];
}

pub fn add_matrices_3x3(m1: [f32; 9], m2: [f32; 9]) -> [f32; 9] {
    return [
        // First row
        m1[0] + m2[0], // Column 1
        m1[1] + m2[1], // Column 2
        m1[2] + m2[2], // Column 3
        // Second row
        m1[3] + m2[3], // Column 1
        m1[4] + m2[4], // Column 2
        m1[5] + m2[5], // Column 3
        // Thrid row
        m1[6] + m2[6], // Column 1
        m1[7] + m2[7], // Column 2
        m1[8] + m2[8], // Column 3
    ];
}

pub fn multiply_matrices_4x4(m1: [f32; 16], m2: [f32; 16]) -> [f32; 16] {
    return [
        // First row
        m1[0] * m2[0] + m1[1] * m2[4] + m1[2] * m2[8] + m1[3] * m2[12], // Column 1
        m1[0] * m2[1] + m1[1] * m2[5] + m1[2] * m2[9] + m1[3] * m2[13], // Column 2
        m1[0] * m2[2] + m1[1] * m2[6] + m1[2] * m2[10] + m1[3] * m2[14], // Column 3
        m1[0] * m2[3] + m1[1] * m2[7] + m1[2] * m2[11] + m1[3] * m2[15], // Column 4
        // Second row
        m1[4] * m2[0] + m1[5] * m2[4] + m1[6] * m2[8] + m1[7] * m2[12], // Column 1
        m1[4] * m2[1] + m1[5] * m2[5] + m1[6] * m2[9] + m1[7] * m2[13], // Column 2
        m1[4] * m2[2] + m1[5] * m2[6] + m1[6] * m2[10] + m1[7] * m2[14], // Column 3
        m1[4] * m2[3] + m1[5] * m2[7] + m1[6] * m2[11] + m1[7] * m2[15], // Column 4
        // Thrid row
        m1[8] * m2[0] + m1[9] * m2[4] + m1[10] * m2[8] + m1[11] * m2[12], // Column 1
        m1[8] * m2[1] + m1[9] * m2[5] + m1[10] * m2[9] + m1[11] * m2[13], // Column 2
        m1[8] * m2[2] + m1[9] * m2[6] + m1[10] * m2[10] + m1[11] * m2[14], // Column 3
        m1[8] * m2[3] + m1[9] * m2[7] + m1[10] * m2[11] + m1[11] * m2[15], // Column 4
        // Fourth row
        m1[12] * m2[0] + m1[13] * m2[4] + m1[14] * m2[8] + m1[15] * m2[12], // Column 1
        m1[12] * m2[1] + m1[13] * m2[5] + m1[14] * m2[9] + m1[15] * m2[13], // Column 2
        m1[12] * m2[2] + m1[13] * m2[6] + m1[14] * m2[10] + m1[15] * m2[14], // Column 3
        m1[12] * m2[3] + m1[13] * m2[7] + m1[14] * m2[11] + m1[15] * m2[15], // Column 4
    ];
}

pub fn apply_transformation_to_point(matrix: [f32; 16], point: [f32; 3]) -> [f32; 4] {
    return [
        matrix[0] * point[0] + matrix[1] * point[1] + matrix[2] * point[2] + matrix[3],
        matrix[4] * point[0] + matrix[5] * point[1] + matrix[6] * point[2] + matrix[7],
        matrix[8] * point[0] + matrix[9] * point[1] + matrix[10] * point[2] + matrix[11],
        matrix[12] * point[0] + matrix[13] * point[1] + matrix[14] * point[2] + matrix[15],
    ];
}
