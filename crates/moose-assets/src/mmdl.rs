//! Parser for `.mmdl` models (see `Model Format Spec.md`): polygons (convex n-gons, shadow
//! proxies among them), their attributes, and optionally a skeleton and animations.

use std::path::Path;

use glam::{Quat, Vec3};

use crate::error::LoadError;
use crate::mesh::{AttribData, Mesh, MeshBuilder, PolyFlags, StorageFormat};
use crate::mmp::Cursor;
use crate::skin::{Animation, Bone, Pose, Skin};
use crate::text::tokenize;

pub(crate) fn parse_mmdl(path: &Path, name: &str, src: &str) -> Result<Mesh, LoadError> {
    let lines = tokenize(src).map_err(|(l, m)| LoadError::new(path, Some(l), m))?;
    let mut c = Cursor { path, lines, pos: 0 };
    let (line, args) = c.header("MOOSEMODEL", 1)?;
    if args[0] != "1" {
        return Err(c.err(line, format!("unsupported format version '{}'", args[0])));
    }
    c.header("name", 1)?;

    // ---- Positions, each with its bone ('-' in a model without bones)
    let section = c.section("positions", false, Some(4))?;
    let positions_line = section.line;
    let mut positions = Vec::with_capacity(section.rows.len());
    let mut bone_of = Vec::with_capacity(section.rows.len());
    for r in &section.rows {
        positions.push(Vec3::new(c.float(r, 0)?, c.float(r, 1)?, c.float(r, 2)?));
        bone_of.push(match r.tokens[3].as_str() {
            "-" => None,
            _ => Some(c.parse::<u16>(r, 3, "bone index")?),
        });
    }

    // ---- Attributes and their values
    let section = c.section("attributes", false, Some(3))?;
    let mut decls: Vec<(String, u8, StorageFormat)> = Vec::new();
    for r in &section.rows {
        let format = StorageFormat::parse(&r.tokens[1])
            .ok_or_else(|| c.err(r.no, format!("unknown storage format '{}'", r.tokens[1])))?;
        let count: u8 = c.parse(r, 2, "component count")?;
        if count == 0 || decls.iter().any(|d| d.0 == r.tokens[0]) {
            return Err(c.err(r.no, format!("attribute '{}': bad or repeated", r.tokens[0])));
        }
        decls.push((r.tokens[0].clone(), count, format));
    }
    let mut tables = Vec::new();
    for (attribute, count, format) in &decls {
        let section = c.section("values", true, Some(*count as usize))?;
        if section.name.as_deref() != Some(attribute.as_str()) {
            return Err(c.err(section.line, format!("expected values for '{attribute}'")));
        }
        let mut data = AttribData::new(*format);
        for r in &section.rows {
            for t in &r.tokens {
                data.push_token(t).map_err(|m| c.err(r.no, m))?;
            }
        }
        tables.push(data);
    }

    // ---- Polygons
    let section = c.section("polygons", false, None)?;
    let mut builder = MeshBuilder::new(name, positions.clone(), decls.iter().cloned());
    for r in &section.rows {
        if r.tokens.len() < 2 {
            return Err(c.err(r.no, "polygon row needs flags and a vertex count"));
        }
        let flags = c.hex(r, 0)?;
        if flags & !PolyFlags::PROXY != 0 {
            return Err(c.err(r.no, format!("unknown polygon flags {flags:#x} (models have 0x100, proxy)")));
        }
        let n: usize = c.parse(r, 1, "vertex count")?;
        if n < 3 || r.tokens.len() != 2 + n {
            return Err(c.err(r.no, format!("polygon needs {n} (at least 3) vertices listed")));
        }
        let mut indices = Vec::with_capacity(n);
        let mut rows = Vec::with_capacity(n);
        for t in &r.tokens[2..] {
            let mut parts = t.split(':');
            let p: usize = parts
                .next()
                .and_then(|s| s.parse().ok())
                .filter(|&p| p < positions.len())
                .ok_or_else(|| c.err(r.no, format!("'{t}': no such position")))?;
            let refs: Vec<usize> = parts
                .map(|s| s.parse().map_err(|_| c.err(r.no, format!("'{t}': bad values row"))))
                .collect::<Result<_, _>>()?;
            if refs.len() != decls.len() {
                return Err(c.err(r.no, format!("'{t}' needs one values row per attribute ({})", decls.len())));
            }
            indices.push(p as u32);
            rows.push(refs);
        }
        builder
            .push_polygon(&indices, PolyFlags(flags))
            .map_err(|m| c.err(r.no, m))?;
        for refs in &rows {
            for (k, &row) in refs.iter().enumerate() {
                let count = decls[k].1 as usize;
                if (row + 1) * count > tables[k].len() {
                    return Err(c.err(r.no, format!("values {} has no row {row}", decls[k].0)));
                }
                builder.attrib_data(k).extend_from(&tables[k], row * count..(row + 1) * count);
            }
        }
    }
    let (mut mesh, remap) = builder.finish_remapped(&mut []);

    // ---- Optional: bones, then animations with their poses
    let at = |c: &Cursor, keyword: &str| c.lines.get(c.pos).is_some_and(|l| l.tokens[0] == keyword);
    let mut bones = Vec::new();
    if at(&c, "bones") {
        let section = c.section("bones", false, Some(9))?;
        for (i, r) in section.rows.iter().enumerate() {
            let parent = match r.tokens[1].as_str() {
                "-" => None,
                _ => {
                    let p: u16 = c.parse(r, 1, "parent bone")?;
                    if p as usize >= i {
                        return Err(c.err(r.no, "a bone's parent must come before it"));
                    }
                    Some(p)
                }
            };
            bones.push(Bone {
                name: r.tokens[0].clone(),
                parent,
                rest: pose(&c, r, 2)?,
            });
        }
    }
    let mut animations = Vec::new();
    if at(&c, "animations") {
        if bones.is_empty() {
            return Err(c.err(c.lines[c.pos].no, "animations need bones"));
        }
        let section = c.section("animations", false, Some(4))?;
        for r in &section.rows {
            let fps: f32 = c.float(r, 1)?;
            let frames: usize = c.parse(r, 2, "frame count")?;
            if fps <= 0.0 || frames == 0 {
                return Err(c.err(r.no, "an animation needs a positive rate and a frame"));
            }
            animations.push(Animation {
                name: r.tokens[0].clone(),
                fps,
                frames,
                looping: match r.tokens[3].as_str() {
                    "loop" => true,
                    "once" => false,
                    t => return Err(c.err(r.no, format!("'{t}' is not loop or once"))),
                },
                poses: Vec::new(),
            });
        }
        for animation in &mut animations {
            let section = c.section("poses", true, Some(9))?;
            if section.name.as_deref() != Some(animation.name.as_str()) {
                return Err(c.err(section.line, format!("expected poses for '{}'", animation.name)));
            }
            if section.rows.len() != animation.frames * bones.len() {
                return Err(c.err(section.line, "poses need a row per frame per bone"));
            }
            for (k, r) in section.rows.iter().enumerate() {
                let (frame, bone): (usize, usize) = (c.parse(r, 0, "frame")?, c.parse(r, 1, "bone")?);
                if (frame, bone) != (k / bones.len(), k % bones.len()) {
                    return Err(c.err(r.no, "poses go frame by frame, bone by bone"));
                }
                animation.poses.push(pose(&c, r, 2)?);
            }
        }
    }
    if let Some(extra) = c.lines.get(c.pos) {
        return Err(c.err(extra.no, format!("unexpected '{}' after the last section", extra.tokens[0])));
    }
    if !bones.is_empty() {
        // Each kept position's bone.
        let mut position_bones = vec![0u16; mesh.positions.len()];
        for (old, &new) in remap.iter().enumerate() {
            if new == u32::MAX {
                continue;
            }
            let bone = bone_of[old].ok_or_else(|| {
                c.err(positions_line, format!("position {old} has no bone in a model with bones"))
            })?;
            if bone as usize >= bones.len() {
                return Err(c.err(positions_line, format!("position {old}: no bone {bone}")));
            }
            position_bones[new as usize] = bone;
        }
        mesh.skin = Some(Skin::new(bones, position_bones, animations));
    }
    Ok(mesh)
}

/// A pose from row columns `i..i + 7`: x y z, then a rotation quaternion qx qy qz qw
/// (made unit length).
fn pose(c: &Cursor, r: &crate::text::Line, i: usize) -> Result<Pose, LoadError> {
    let q = Quat::from_xyzw(c.float(r, i + 3)?, c.float(r, i + 4)?, c.float(r, i + 5)?, c.float(r, i + 6)?);
    if q.length_squared() < 1e-12 {
        return Err(c.err(r.no, "a rotation can't be zero"));
    }
    Ok(Pose {
        translation: Vec3::new(c.float(r, i)?, c.float(r, i + 1)?, c.float(r, i + 2)?),
        rotation: q.normalize(),
    })
}
