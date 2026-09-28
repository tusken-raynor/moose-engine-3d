use moose_assets::glam::Vec3;
use moose_assets::{Assets, AttribData, EntityKind, StorageFormat};

const LEVEL: &str = include_str!("../../../assets/levels/two_rooms.mmp");

fn assets() -> Assets {
    Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"))
}

#[test]
fn loads_two_rooms() {
    let mut assets = assets();
    let level = assets.load_level("two_rooms.mmp").unwrap();
    assert_eq!(level.name, "Two Rooms");

    // Crate loaded once and shared, plus the level geometry.
    assert_eq!(assets.meshes().len(), 2);
    let crate_id = assets.mesh_id("crate.obj").unwrap();

    let geo = assets.mesh(level.geometry);
    assert_eq!(geo.positions.len(), 28);
    // The vertex table carries over one-to-one, so file indices are mesh indices.
    assert_eq!(geo.positions[27], Vec3::new(-4.0, 4.0, -12.0));
    assert_eq!(geo.polygons.len(), 20); // 24 surfaces minus 4 portals
    // room: floor 6 + ceiling 6 + 3 walls x 4 + pillars 5 + 5 + lintel 4 = 38; hallway 4 x 4 = 16
    assert_eq!(geo.vertex_positions.len(), 38 + 16 + 38);
    let color = geo.attrib("color").unwrap();
    assert_eq!((color.count, color.format()), (3, StorageFormat::U8));
    assert_eq!(color.vertex_count(), geo.vertex_positions.len());
    let AttribData::U8(values) = &color.data else {
        unreachable!()
    };
    assert_eq!(&values[..3], &[102, 76, 56]); // floor of room_a, values row 0

    // Sectors own contiguous polygon and portal ranges.
    let names: Vec<_> = level.sectors.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["room_a", "hallway", "room_b"]);
    let ranges: Vec<_> = level
        .sectors
        .iter()
        .map(|s| (s.polygons.clone(), s.portals.clone()))
        .collect();
    assert_eq!(ranges, [(0..8, 0..1), (8..12, 1..3), (12..20, 3..4)]);
    for (si, s) in level.sectors.iter().enumerate() {
        for p in s.polygons.clone() {
            let plane = geo.polygons[p as usize].plane;
            assert!(
                plane.distance(s.center) > 0.0,
                "polygon {p} of sector {si} faces outward"
            );
        }
    }

    // Portals pair up across sectors and share the doorway's positions.
    assert_eq!(level.portals.len(), 4);
    for (i, p) in level.portals.iter().enumerate() {
        let m = &level.portals[p.mirror as usize];
        assert_eq!(m.mirror as usize, i);
        assert_eq!((p.target, m.target), (m.sector, p.sector));
        let mut reversed = m.positions.clone();
        reversed.reverse();
        let k = reversed.iter().position(|&v| v == p.positions[0]).unwrap();
        reversed.rotate_left(k);
        assert_eq!(reversed, p.positions);
        assert!(p.flags.render_through() && p.flags.passable());
        assert!(p.plane.distance(level.sectors[p.sector as usize].center) > 0.0);
    }
    assert_eq!((level.portals[0].sector, level.portals[0].target), (0, 1));
    let doorway = Vec3::new(-1.0, 3.0, 0.0);
    assert!(
        level.portals[0]
            .positions
            .iter()
            .any(|&i| geo.positions[i as usize] == doorway)
    );

    // Spawns.
    assert_eq!(level.spawns.len(), 7);
    let start = &level.spawns[0];
    assert_eq!(
        (start.kind, start.mesh, start.sector),
        (EntityKind::Spawn, None, 0)
    );
    assert!(
        level.spawns[1..]
            .iter()
            .all(|s| s.kind == EntityKind::Prop && s.mesh == Some(crate_id))
    );
    // Yaw 0 faces -Z; positive yaw turns counter-clockwise seen from above (toward -X).
    let a3 = level.spawns.iter().find(|s| s.name == "crate_a3").unwrap();
    let forward = a3.rotation * Vec3::NEG_Z;
    let expected = Vec3::new(-30f32.to_radians().sin(), 0.0, -30f32.to_radians().cos());
    assert!((forward - expected).length() < 1e-5, "{forward}");
}

#[test]
fn crate_mesh() {
    let mut assets = assets();
    let id = assets.load_mesh("crate.obj").unwrap();
    let m = assets.mesh(id);
    assert_eq!(
        m.positions.len(),
        24,
        "every OBJ 'v' line stays its own position"
    );
    assert_eq!(m.vertex_positions.len(), 24);
    assert_eq!(m.polygons.len(), 6);
    assert!(m.polygons.iter().all(|p| p.vertex_count == 4));
    assert_eq!(m.attribs.len(), 2);
    assert_eq!(m.attrib("color").unwrap().data.len(), 72);
    // Each face shows the whole texture: its uvs are the four corners of 0-1.
    let moose_assets::AttribData::F32(uv) = &m.attrib("uv").unwrap().data else {
        panic!("uvs are f32");
    };
    assert_eq!(uv.len(), 24 * 2);
    for face in uv.chunks_exact(4 * 2) {
        let mut corners: Vec<[u32; 2]> =
            face.chunks_exact(2).map(|t| [t[0] as u32, t[1] as u32]).collect();
        corners.sort();
        assert_eq!(corners, [[0, 0], [0, 1], [1, 0], [1, 1]]);
    }
    assert_eq!(
        (m.bounds.min, m.bounds.max),
        (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5))
    );
    let center = Vec3::new(0.0, 0.5, 0.0);
    assert!(
        m.polygons.iter().all(|p| p.plane.distance(center) < 0.0),
        "crate faces point outward"
    );
}

/// Applies `edit` (old -> new) to the level, which must change it, and returns the load error.
fn broken(old: &str, new: &str) -> String {
    assert!(LEVEL.contains(old), "edit does not apply: {old}");
    let src = LEVEL.replacen(old, new, 1);
    match assets().parse_level("two_rooms.mmp", &src) {
        Ok(_) => panic!("loaded despite edit {old:?} -> {new:?}"),
        Err(e) => e.to_string(),
    }
}

#[track_caller]
fn assert_rejects(old: &str, new: &str, expected: &str) {
    let msg = broken(old, new);
    assert!(msg.contains(expected), "expected '{expected}' in: {msg}");
}

#[test]
fn rejects_geometry_errors() {
    // Floor listed clockwise from inside.
    assert_rejects(
        "0:0 1:0 2:0 3:0 4:0 5:0",
        "5:0 4:0 3:0 2:0 1:0 0:0",
        "does not face into sector 'room_a'",
    );
    // Floor missing the collinear doorway corner: T-junction.
    assert_rejects(
        "6       0:0 1:0 2:0 3:0 4:0 5:0",
        "5       0:0 1:0 2:0 4:0 5:0",
        "not closed",
    );
    // Lintel cut down to 2 vertices.
    assert_rejects(
        "0x0    4       12:6 13:6 8:2 7:2",
        "0x0    2       12:6 13:6",
        "polygon has 2 vertices, needs at least 3",
    );
    // Far wall pushed out of plane.
    assert_rejects(
        "   0       -4.00     0.00     8.00",
        "   0       -4.00     0.00     8.50",
        "not planar",
    );
}

/// A closed box whose roof is a V-shaped valley. Every face is planar and convex,
/// but the sector is not. Surface windings generated to face inward.
const VALLEY: &str = "MOOSEMAP 1
name \"Valley\"
vertices 12
0 0 0 0
1 1 0 0
2 2 0 0
3 2 2 0
4 1 1.5 0
5 0 2 0
6 0 0 1
7 1 0 1
8 2 0 1
9 2 2 1
10 1 1.5 1
11 0 2 1
attributes 1
0 color u8 3
values color 1
0 200 200 200
sectors 1
0 valley 0 9
surfaces 9
0 0 -1 0x0 6 6:0 7:0 8:0 2:0 1:0 0:0
1 0 -1 0x0 4 5:0 11:0 6:0 0:0
2 0 -1 0x0 4 2:0 8:0 9:0 3:0
3 0 -1 0x0 4 0:0 1:0 4:0 5:0
4 0 -1 0x0 4 1:0 2:0 3:0 4:0
5 0 -1 0x0 4 11:0 10:0 7:0 6:0
6 0 -1 0x0 4 10:0 9:0 8:0 7:0
7 0 -1 0x0 4 5:0 4:0 10:0 11:0
8 0 -1 0x0 4 4:0 3:0 9:0 10:0
adjoins 0
entities 0
";

#[test]
fn coincident_vertices_stay_split_and_unused_ones_drop() {
    // Append a vertex nothing references, plus a copy of vertex 0 that the floor
    // uses in place of vertex 0. The copy is not merged back into vertex 0.
    let src = LEVEL
        .replacen("vertices 28", "vertices 30", 1)
        .replacen(
            "   27      -4.00     4.00   -12.00",
            "   27      -4.00     4.00   -12.00\n   28 9 9 9\n   29 -4 0 8",
            1,
        )
        .replacen("0:0 1:0 2:0 3:0 4:0 5:0", "29:0 1:0 2:0 3:0 4:0 5:0", 1);
    let mut assets = assets();
    let level = assets.parse_level("two_rooms.mmp", &src);
    // Vertex 29 splits the floor's corner from the walls' corner 0, so the room has a crack.
    let err = level.unwrap_err().to_string();
    assert!(err.contains("not closed"), "{err}");

    // Without the split, the unreferenced vertex 28 is dropped.
    let src = LEVEL.replacen("vertices 28", "vertices 29", 1).replacen(
        "   27      -4.00     4.00   -12.00",
        "   27      -4.00     4.00   -12.00\n   28 9 9 9",
        1,
    );
    let level = assets.parse_level("two_rooms.mmp", &src).unwrap();
    let geo = assets.mesh(level.geometry);
    assert_eq!(geo.positions.len(), 28);
    assert!(!geo.positions.contains(&Vec3::splat(9.0)));
}

#[test]
fn rejects_non_convex_sector() {
    let err = assets()
        .parse_level("valley.mmp", VALLEY)
        .unwrap_err()
        .to_string();
    assert!(err.contains("sector 'valley' is not convex"), "{err}");
    // Control: the same room with a flat roof (valley raised to y = 2) loads.
    let flat = VALLEY
        .replace("4 1 1.5 0", "4 1 2 0")
        .replace("10 1 1.5 1", "10 1 2 1");
    assets().parse_level("flat.mmp", &flat).unwrap();
}

#[test]
fn rejects_portal_errors() {
    assert_rejects(
        "   3   23       2       0x3",
        "   3   23       1       0x3",
        "does not point back",
    );
    assert_rejects(
        "4 3 13 12 ",
        "4:0 3 13 12 ",
        "portal surfaces list vertex indices only",
    );
    assert_rejects(
        "   0   8        1       0x3",
        "   0   8        1       0x7",
        "unknown flags",
    );
    // Mirror surface with the same vertices in the same (not reversed) order faces the wrong way.
    assert_rejects(
        "12 13 3 4 ",
        "4 3 13 12 ",
        "surface 13 does not face into sector 'hallway'",
    );
}

#[test]
fn rejects_attribute_errors() {
    assert_rejects(
        "   0     102   76   56",
        "   0     300   76   56",
        "'300' is not a valid u8 value",
    );
    assert_rejects(
        "   0     102   76   56",
        "   0     102   76",
        "needs 3 fields",
    );
    assert_rejects(
        "0:0 1:0 2:0",
        "0:0:1 1:0 2:0",
        "references 2 attribute rows, 1 attributes are declared",
    );
    assert_rejects("0:0 1:0 2:0", "0:99 1:0 2:0", "color row 99 does not exist");
    assert_rejects(
        "values color 18",
        "values colour 18",
        "expected values for attribute 'color', found 'colour'",
    );
    assert_rejects(
        "   0   color   u8      3",
        "   0   color   u9      3",
        "unknown storage format 'u9'",
    );
}

#[test]
fn rejects_structure_errors() {
    assert_rejects("MOOSEMAP 1", "MOOSEMAP 2", "unsupported format version");
    assert_rejects("vertices 28", "vertices 29", "expected 28");
    assert_rejects(
        "   1   hallway   9",
        "   1   hallway   10",
        "ranges must be contiguous",
    );
    assert_rejects(
        "0x0    6       0:0",
        "0x4    6       0:0",
        "unknown surface flags 0x4",
    );
    assert_rejects(
        "0x0    4       4 3 13 12",
        "0x1    4       4 3 13 12",
        "portal surfaces take no surface flags",
    );
    assert_rejects(
        "prop   1       crate.obj  0.30    0.00",
        "prop   1       crate.obj  3.00    0.00",
        "outside sector 'hallway'",
    );
    assert_rejects(
        "crate.obj  -2.50   0.00",
        "missing.obj  -2.50   0.00",
        "entity 'crate_a1'",
    );
    assert_rejects(
        "spawn  0       -",
        "spawn  0       crate.obj",
        "spawn points take no model",
    );
    assert_rejects("crate_b2", "crate_b1", "used twice");
}

#[test]
fn errors_point_at_the_line() {
    let src = LEVEL.replacen("   0     102   76   56", "   0     300   76   56", 1);
    let line = src.lines().position(|l| l.contains("300")).unwrap() + 1;
    let err = assets().parse_level("two_rooms.mmp", &src).unwrap_err();
    assert_eq!(err.line, Some(line));
}

/// two_rooms.mmp with an ambient line and `lights` rows appended.
fn with_lights(ambient: &str, rows: &[&str]) -> String {
    let mut src = format!("{LEVEL}\n{ambient}\n\nlights {}\n", rows.len());
    for (i, r) in rows.iter().enumerate() {
        src += &format!("   {i}   {r}\n");
    }
    src
}

#[test]
fn loads_ambient_and_lights() {
    // Without them: full ambient light, no lights.
    let level = assets().parse_level("two_rooms.mmp", LEVEL).unwrap();
    assert_eq!((level.ambient, level.lights.len()), (Vec3::ONE, 0));
    let src = with_lights(
        "ambient 0.1 0.2 0.3",
        &["0  -2.0  3.0  5.0   1.5 1.0 0.5  6.0", "2  1.0  1.0  -15.0   0.2 0.4 1.2  3.5"],
    );
    let level = assets().parse_level("two_rooms.mmp", &src).unwrap();
    assert_eq!(level.ambient, Vec3::new(0.1, 0.2, 0.3));
    assert_eq!(level.lights.len(), 2);
    let l = level.lights[1];
    assert_eq!((l.sector, l.position, l.range), (2, Vec3::new(1.0, 1.0, -15.0), 3.5));
    assert_eq!(l.color, Vec3::new(0.2, 0.4, 1.2));
    assert!(l.is_point() && l.cone() == (0.0, 1.0));
    // A spot light: its direction (normalized) and cone half-angles, in degrees.
    let src = with_lights(
        "ambient 0 0 0",
        &["0  2.0  3.5  7.0   2 2 2  12   0 -2 0  10 20"],
    );
    let l = assets().parse_level("two_rooms.mmp", &src).unwrap().lights[0];
    assert!(!l.is_point());
    assert_eq!(l.direction, Vec3::NEG_Y);
    assert!((l.cos_inner - 10f32.to_radians().cos()).abs() < 1e-6);
    assert!((l.cos_outer - 20f32.to_radians().cos()).abs() < 1e-6);
}

#[test]
fn rejects_light_errors() {
    let reject = |ambient: &str, row: &str, expected: &str| {
        let src = with_lights(ambient, &[row]);
        let msg = match assets().parse_level("two_rooms.mmp", &src) {
            Ok(_) => panic!("loaded despite light {row:?}"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains(expected), "expected '{expected}' in: {msg}");
    };
    let fine = "0  0.0 2.0 4.0  1 1 1  5";
    reject("ambient 1 -1 1", fine, "ambient light cannot be negative");
    reject("ambient 1 1 1", "7  0.0 2.0 4.0  1 1 1  5", "sector 7 does not exist");
    reject("ambient 1 1 1", "0  0.0 2.0 4.0  1 -1 1  5", "color cannot be negative");
    reject("ambient 1 1 1", "0  0.0 2.0 4.0  1 1 1  0", "range must be positive");
    reject("ambient 1 1 1", "0  0.0 2.0 -6.0  1 1 1  5", "outside sector 'room_a'");
    reject("ambient 1 1 1", "0  0.0 2.0 4.0  1 1 1", "needs 8 fields");
    reject("ambient 1 1 1", "0  0.0 2.0 4.0  1 1 1  5  0 0 0  10 20", "direction cannot be zero");
    reject("ambient 1 1 1", "0  0.0 2.0 4.0  1 1 1  5  0 -1 0  30 20", "cone angles");
}
