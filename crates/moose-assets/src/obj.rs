use std::path::Path;

use glam::Vec3;

use crate::error::LoadError;
use crate::mesh::{AttribData, Mesh, MeshBuilder, PolyFlags, StorageFormat};
use crate::text::tokenize;

/// Parses a Wavefront OBJ file into a [`Mesh`]. Faces stay as authored n-gons
/// (never triangulated) and must be planar and convex, counter-clockwise from the front.
///
/// Supported statements:
/// - `v x y z [r g b]`: position, with the common vertex-color extension (0-1 floats).
///   Colors are stored as the attribute `color`, u8 x 3.
/// - `vt u v [w]`: stored as `uv`, f32 x 2.
/// - `vn x y z`: stored as `normal`, f32 x 3.
/// - `f` with `v`, `v/vt`, `v//vn` or `v/vt/vn` references (1-based, or negative
///   for relative). Every face must use the same form.
/// - `o`, `g`, `s`, `mtllib`, `usemtl` are accepted and ignored for now.
pub fn parse_obj(path: &Path, name: &str, src: &str) -> Result<Mesh, LoadError> {
    let err = |line: usize, msg: String| LoadError::new(path, Some(line), msg);
    let lines = tokenize(src).map_err(|(l, m)| err(l, m))?;

    let mut positions: Vec<Vec3> = Vec::new();
    let mut colors: Vec<[u8; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    // Per face: (line, [position, uv, normal] per vertex; uv/normal are usize::MAX when absent).
    let mut faces: Vec<(usize, Vec<[usize; 3]>)> = Vec::new();
    let mut colored: Option<bool> = None;
    let mut face_form: Option<(bool, bool)> = None;

    for line in &lines {
        let t = &line.tokens;
        let numbers = || -> Result<Vec<f32>, LoadError> {
            t[1..]
                .iter()
                .map(|s| {
                    s.parse::<f32>()
                        .ok()
                        .filter(|x| x.is_finite())
                        .ok_or_else(|| err(line.no, format!("'{s}' is not a number")))
                })
                .collect()
        };
        match t[0].as_str() {
            "v" => {
                let v = numbers()?;
                let has_color = match v.len() {
                    3 => false,
                    6 => true,
                    n => {
                        return Err(err(
                            line.no,
                            format!("'v' needs 3 values, or 6 with a color; found {n}"),
                        ));
                    }
                };
                if *colored.get_or_insert(has_color) != has_color {
                    return Err(err(
                        line.no,
                        "some vertices have colors and others don't".into(),
                    ));
                }
                positions.push(Vec3::new(v[0], v[1], v[2]));
                if has_color {
                    let mut c = [0u8; 3];
                    for (k, &x) in v[3..].iter().enumerate() {
                        if !(0.0..=1.0).contains(&x) {
                            return Err(err(
                                line.no,
                                format!("color component {x} is outside 0-1"),
                            ));
                        }
                        c[k] = (x * 255.0).round() as u8;
                    }
                    colors.push(c);
                }
            }
            "vt" => {
                let v = numbers()?;
                if !(2..=3).contains(&v.len()) {
                    return Err(err(
                        line.no,
                        format!("'vt' needs 2 values; found {}", v.len()),
                    ));
                }
                uvs.push([v[0], v[1]]);
            }
            "vn" => {
                let v = numbers()?;
                if v.len() != 3 {
                    return Err(err(
                        line.no,
                        format!("'vn' needs 3 values; found {}", v.len()),
                    ));
                }
                normals.push([v[0], v[1], v[2]]);
            }
            "f" => {
                if t.len() < 4 {
                    return Err(err(line.no, "a face needs at least 3 vertices".into()));
                }
                let resolve = |s: &str, count: usize, what: &str| -> Result<usize, LoadError> {
                    let i: i64 = s
                        .parse()
                        .map_err(|_| err(line.no, format!("'{s}' is not a valid {what} index")))?;
                    let resolved = if i > 0 { i - 1 } else { count as i64 + i };
                    if i == 0 || resolved < 0 || resolved >= count as i64 {
                        return Err(err(
                            line.no,
                            format!("{what} index {i} is out of range ({count} defined so far)"),
                        ));
                    }
                    Ok(resolved as usize)
                };
                let mut refs = Vec::with_capacity(t.len() - 1);
                for token in &t[1..] {
                    let parts: Vec<&str> = token.split('/').collect();
                    if parts.len() > 3 {
                        return Err(err(
                            line.no,
                            format!("'{token}' is not a valid face vertex"),
                        ));
                    }
                    let optional = |k: usize, count: usize, what: &str| match parts.get(k) {
                        None | Some(&"") => Ok(None),
                        Some(s) => resolve(s, count, what).map(Some),
                    };
                    let v = resolve(parts[0], positions.len(), "position")?;
                    let vt = optional(1, uvs.len(), "texture coordinate")?;
                    let vn = optional(2, normals.len(), "normal")?;
                    let form = (vt.is_some(), vn.is_some());
                    if *face_form.get_or_insert(form) != form {
                        return Err(err(
                            line.no,
                            "every face vertex must use the same form (v, v/vt, v//vn or v/vt/vn)"
                                .into(),
                        ));
                    }
                    refs.push([v, vt.unwrap_or(usize::MAX), vn.unwrap_or(usize::MAX)]);
                }
                faces.push((line.no, refs));
            }
            "o" | "g" | "s" | "mtllib" | "usemtl" => {}
            other => return Err(err(line.no, format!("unsupported statement '{other}'"))),
        }
    }

    let Some((has_uv, has_normal)) = face_form else {
        return Err(LoadError::new(path, None, "file has no faces"));
    };
    let has_color = colored == Some(true);
    let mut decls = Vec::new();
    if has_color {
        decls.push(("color".to_string(), 3, StorageFormat::U8));
    }
    if has_uv {
        decls.push(("uv".to_string(), 2, StorageFormat::F32));
    }
    if has_normal {
        decls.push(("normal".to_string(), 3, StorageFormat::F32));
    }

    // Every used `v` line becomes one position, in file order, even if it repeats another's coordinates.
    let mut builder = MeshBuilder::new(name, positions, decls);
    for (line, refs) in &faces {
        let indices: Vec<u32> = refs.iter().map(|r| r[0] as u32).collect();
        builder
            .push_polygon(&indices, PolyFlags::default())
            .map_err(|m| err(*line, format!("face: {m}")))?;
        for r in refs {
            let mut k = 0;
            if has_color {
                if let AttribData::U8(d) = builder.attrib_data(k) {
                    d.extend_from_slice(&colors[r[0]]);
                }
                k += 1;
            }
            if has_uv {
                if let AttribData::F32(d) = builder.attrib_data(k) {
                    d.extend_from_slice(&uvs[r[1]]);
                }
                k += 1;
            }
            if has_normal && let AttribData::F32(d) = builder.attrib_data(k) {
                d.extend_from_slice(&normals[r[2]]);
            }
        }
    }
    Ok(builder.finish(&mut []))
}
