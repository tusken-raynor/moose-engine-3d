//! Skeletal animation, Half-Life style: a model's positions each belong to one bone, and
//! each frame the bones' poses move them. See `Model Format Spec.md`.
//!
//! Per bone per frame: the animation's keyframes are blended (position lerp, rotation
//! slerp), each bone is put in place after its parent (`world = parent_world × local`),
//! and multiplied by the inverse of its rest pose (`skin = world × inverse_rest`). Per
//! position, one matrix multiply: `p' = skin[bone] × p`. Nothing is blended per position.

use glam::{Affine3A, Quat, Vec3};

/// A bone's place relative to its parent (or the model, for a root).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub translation: Vec3,
    pub rotation: Quat,
}

impl Pose {
    pub fn matrix(self) -> Affine3A {
        Affine3A::from_rotation_translation(self.rotation, self.translation)
    }

    /// `t` of the way from `self` to `to`.
    pub fn blend(self, to: Pose, t: f32) -> Pose {
        Pose {
            translation: self.translation.lerp(to.translation, t),
            rotation: self.rotation.slerp(to.rotation, t),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub name: String,
    /// Its parent, which comes before it; `None` for a root.
    pub parent: Option<u16>,
    /// Its rest pose, where the model's positions are as written (the bind pose).
    pub rest: Pose,
}

/// A clip: every bone's pose at every frame, `frames` of them `1 / fps` s apart.
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub name: String,
    pub fps: f32,
    pub frames: usize,
    /// It runs on from its last frame back into its first.
    pub looping: bool,
    /// Frame by frame, bone by bone: `poses[frame * bones + bone]`.
    pub poses: Vec<Pose>,
}

impl Animation {
    /// How long it lasts, in seconds (a looping one, back to its start).
    pub fn duration(&self) -> f32 {
        let frames = if self.looping { self.frames } else { self.frames.saturating_sub(1) };
        frames as f32 / self.fps
    }
}

/// A model's skeleton and its animations.
#[derive(Clone, Debug, PartialEq)]
pub struct Skin {
    /// Parents before children.
    pub bones: Vec<Bone>,
    /// Per position of the model's mesh, its bone.
    pub position_bones: Vec<u16>,
    pub animations: Vec<Animation>,
    /// Per bone, the inverse of its rest pose in model space.
    pub inverse_rest: Vec<Affine3A>,
}

impl Skin {
    /// A skin over `bones`, with the inverses of their rest poses worked out.
    pub fn new(bones: Vec<Bone>, position_bones: Vec<u16>, animations: Vec<Animation>) -> Skin {
        let rest: Vec<Pose> = bones.iter().map(|b| b.rest).collect();
        let world = world_matrices(&bones, &rest);
        Skin {
            inverse_rest: world.iter().map(|m| m.inverse()).collect(),
            bones,
            position_bones,
            animations,
        }
    }

    pub fn animation(&self, name: &str) -> Option<usize> {
        self.animations.iter().position(|a| a.name == name)
    }

    /// Each bone's skinning matrix `time` seconds into animation `animation` (clamped to
    /// its ends, or wrapped if it loops): model-space rest positions to posed ones.
    pub fn matrices(&self, animation: usize, time: f32) -> Vec<Affine3A> {
        let a = &self.animations[animation];
        let n = self.bones.len();
        let frame = (time * a.fps).max(0.0);
        let (f0, f1, t) = if a.looping {
            let f = frame % a.frames as f32;
            let f0 = f.floor() as usize % a.frames;
            (f0, (f0 + 1) % a.frames, f.fract())
        } else {
            let last = a.frames.saturating_sub(1);
            let f = frame.min(last as f32);
            let f0 = f.floor() as usize;
            (f0, (f0 + 1).min(last), f.fract())
        };
        let poses: Vec<Pose> = (0..n)
            .map(|b| a.poses[f0 * n + b].blend(a.poses[f1 * n + b], t))
            .collect();
        world_matrices(&self.bones, &poses)
            .iter()
            .zip(&self.inverse_rest)
            .map(|(world, inverse)| *world * *inverse)
            .collect()
    }

    /// The rest positions `rest`, posed by skinning `matrices`, into `out`.
    pub fn pose_positions(&self, rest: &[Vec3], matrices: &[Affine3A], out: &mut [Vec3]) {
        for ((p, &bone), out) in rest.iter().zip(&self.position_bones).zip(out) {
            *out = matrices[bone as usize].transform_point3(*p);
        }
    }
}

/// Each bone's model-space matrix for local poses `poses` (parents come first).
fn world_matrices(bones: &[Bone], poses: &[Pose]) -> Vec<Affine3A> {
    let mut world: Vec<Affine3A> = Vec::with_capacity(bones.len());
    for (bone, pose) in bones.iter().zip(poses) {
        let local = pose.matrix();
        world.push(match bone.parent {
            Some(p) => world[p as usize] * local,
            None => local,
        });
    }
    world
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arm: a shoulder at the origin, an elbow 1 m out along x, bending about z.
    fn arm(bend: f32) -> Skin {
        let rest = |x: f32| Pose { translation: Vec3::new(x, 0.0, 0.0), rotation: Quat::IDENTITY };
        let bones = vec![
            Bone { name: "shoulder".into(), parent: None, rest: rest(0.0) },
            Bone { name: "elbow".into(), parent: Some(0), rest: rest(1.0) },
        ];
        let bent = Pose {
            translation: Vec3::X,
            rotation: Quat::from_rotation_z(bend),
        };
        let animation = Animation {
            name: "bend".into(),
            fps: 1.0,
            frames: 2,
            looping: false,
            poses: vec![rest(0.0), rest(1.0), rest(0.0), bent],
        };
        Skin::new(bones, vec![0, 1], vec![animation])
    }

    #[test]
    fn rest_pose_leaves_positions_where_they_are() {
        let skin = arm(1.0);
        let matrices = skin.matrices(0, 0.0);
        let rest = [Vec3::new(0.5, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)];
        let mut out = [Vec3::ZERO; 2];
        skin.pose_positions(&rest, &matrices, &mut out);
        assert!(out[0].distance(rest[0]) < 1e-6 && out[1].distance(rest[1]) < 1e-6);
    }

    #[test]
    fn a_bent_elbow_swings_its_positions_about_it() {
        let skin = arm(std::f32::consts::FRAC_PI_2);
        // The forearm's end (2, 0, 0) at the end of the clip: a quarter turn about the
        // elbow (1, 0, 0) puts it at (1, 1, 0); the upper arm stays.
        let matrices = skin.matrices(0, 1.0);
        let mut out = [Vec3::ZERO; 2];
        skin.pose_positions(&[Vec3::new(0.5, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)], &matrices, &mut out);
        assert!(out[0].distance(Vec3::new(0.5, 0.0, 0.0)) < 1e-5);
        assert!(out[1].distance(Vec3::new(1.0, 1.0, 0.0)) < 1e-5, "{}", out[1]);
        // Halfway: an eighth turn (slerp), and past the end it holds.
        let half = skin.matrices(0, 0.5)[1].transform_point3(Vec3::new(2.0, 0.0, 0.0));
        let s = std::f32::consts::FRAC_1_SQRT_2;
        assert!(half.distance(Vec3::new(1.0 + s, s, 0.0)) < 1e-5, "{half}");
        let late = skin.matrices(0, 9.0)[1].transform_point3(Vec3::new(2.0, 0.0, 0.0));
        assert!(late.distance(Vec3::new(1.0, 1.0, 0.0)) < 1e-5);
    }
}
