use std::f32::consts::PI;

use crate::{
    matrix::Transformation,
    threedee::{normalize_vec, Model},
};

const PI2: f32 = PI * 2.0;
#[derive(Debug, Clone)]
// This struct is TEMPORARY
pub struct Entity {
    pub archetype: usize,
    sector: i32,
    position: [f32; 3],
    orientation: [f32; 3],
    scale: [f32; 3],
    transformation: Transformation,
    rotation: Transformation,
    occludee: bool,
    pub lightmode: i8,
}

impl Entity {
    pub fn new(
        archetype: usize,
        position: [f32; 3],
        orientation: [f32; 3],
        scale: [f32; 3],
        lightmode: i8,
        occludee: bool,
    ) -> Self {
        let transformation = Transformation::new(position[0], position[1], position[2]);
        // OPT Maybe create a rotation specific transformation struct to reduce overhead
        let rotation = Transformation::new(0.0, 0.0, 0.0);
        let mut this = Self {
            archetype,
            sector: -1,
            position,
            orientation,
            scale,
            transformation,
            rotation,
            lightmode,
            occludee,
        };
        this.build_transform_matrix();
        return this;
    }
    pub fn get_pos(&self) -> [f32; 3] {
        return self.position.clone();
    }
    pub fn set_pos(&mut self, x: f32, y: f32, z: f32) {
        let pos = [x, y, z];
        self.position = pos;
        self.transformation.set_origin(pos);
        self.build_transform_matrix();
    }
    pub fn get_pos_x(&self) -> f32 {
        return self.position[0];
    }
    pub fn set_pos_x(&mut self, x: f32) {
        self.position[0] = x;
        self.transformation.set_origin_x(x);
        self.build_transform_matrix();
    }
    pub fn get_pos_y(&self) -> f32 {
        return self.position[1];
    }
    pub fn set_pos_y(&mut self, y: f32) {
        self.position[1] = y;
        self.transformation.set_origin_y(y);
        self.build_transform_matrix();
    }
    pub fn get_pos_z(&self) -> f32 {
        return self.position[2];
    }
    pub fn set_pos_z(&mut self, z: f32) {
        self.position[2] = z;
        self.transformation.set_origin_z(z);
        self.build_transform_matrix();
    }

    pub fn get_scale(&self) -> [f32; 3] {
        return self.scale.clone();
    }
    pub fn set_scale(&mut self, x: f32, y: f32, z: f32) {
        self.scale = [x, y, z];
        self.build_transform_matrix();
        self.build_rotation_matrix();
    }
    pub fn set_scale_x(&mut self, x: f32) {
        self.scale[0] = x;
        self.build_transform_matrix();
        self.build_rotation_matrix();
    }
    pub fn set_scale_y(&mut self, y: f32) {
        self.scale[1] = y;
        self.build_transform_matrix();
        self.build_rotation_matrix();
    }
    pub fn set_scale_z(&mut self, z: f32) {
        self.scale[2] = z;
        self.build_transform_matrix();
        self.build_rotation_matrix();
    }

    pub fn get_orientation(&self) -> [f32; 3] {
        return self.position.clone();
    }
    pub fn set_orientation(&mut self, x: f32, y: f32, z: f32) {
        self.orientation = [x.rem_euclid(PI2), y.rem_euclid(PI2), z.rem_euclid(PI2)];
        self.build_transform_matrix();
        self.build_rotation_matrix();
    }
    pub fn set_orientation_x(&mut self, x: f32) {
        self.orientation[0] = x.rem_euclid(PI2);
        self.build_transform_matrix();
        self.build_rotation_matrix();
    }
    pub fn set_orientation_y(&mut self, y: f32) {
        self.orientation[1] = y.rem_euclid(PI2);
        self.build_transform_matrix();
        self.build_rotation_matrix();
    }
    pub fn set_orientation_z(&mut self, z: f32) {
        self.orientation[2] = z.rem_euclid(PI2);
        self.build_transform_matrix();
        self.build_rotation_matrix();
    }

    pub fn transform_point(&self, point: [f32; 3]) -> [f32; 3] {
        return self.transformation.transform_point(point);
    }
    pub fn transform_vector(&self, vec: [f32; 3]) -> [f32; 3] {
        return normalize_vec(self.rotation.transform_point(vec));
    }
    pub fn is_occludee(&self) -> bool {
        return self.occludee;
    }
    fn build_transform_matrix(&mut self) {
        self.transformation.reset();
        // Translations are done automatically
        if self.scale[0] != 1.0 || self.scale[1] != 1.0 || self.scale[2] != 1.0 {
            self.transformation
                .scale(self.scale[0], self.scale[1], self.scale[2]);
        }
        if self.orientation[0] != 0.0 || self.orientation[1] != 0.0 || self.orientation[2] != 0.0 {
            self.transformation.rotate(
                self.orientation[0],
                self.orientation[1],
                self.orientation[2],
            );
        }
        self.transformation.build_matrix();
    }
    fn build_rotation_matrix(&mut self) {
        self.rotation.reset();
        if self.scale[0] != 1.0 || self.scale[1] != 1.0 || self.scale[2] != 1.0 {
            self.transformation
                .scale(self.scale[0], self.scale[1], self.scale[2]);
        }
        if self.orientation[0] != 0.0 || self.orientation[1] != 0.0 || self.orientation[2] != 0.0 {
            self.rotation.rotate(
                self.orientation[0],
                self.orientation[1],
                self.orientation[2],
            );
        }
        self.rotation.build_matrix();
    }
}
