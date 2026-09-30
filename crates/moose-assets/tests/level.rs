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

#[test]
fn loads_entity_options() {
    use moose_assets::Occluder;
    // Without options: moving, casting shadows as its own model.
    let level = assets().parse_level("two_rooms.mmp", LEVEL).unwrap();
    let a1 = &level.spawns[1];
    assert_eq!((a1.is_static, a1.occluder), (false, Occluder::Mesh));
    let row = "1.00   crate_a1";
    let with = |options: &str| {
        let src = LEVEL.replacen(row, &format!("{row} {options}"), 1);
        assets().parse_level("two_rooms.mmp", &src).unwrap().spawns[1].clone()
    };
    let e = with("static");
    assert_eq!((e.is_static, e.occluder), (true, Occluder::Mesh));
    assert_eq!(with("occluder=none").occluder, Occluder::None);
    assert_eq!(with("static occluder=lod:2").occluder, Occluder::Lod(2));
    assert_eq!(
        with("occluder=facing:12:0.5").occluder,
        Occluder::Facing { sides: 12, radius: 0.5, center: Vec3::ZERO }
    );
    assert_eq!(
        with("occluder=facing:8:0.3:0:0.5:0").occluder,
        Occluder::Facing { sides: 8, radius: 0.3, center: Vec3::new(0.0, 0.5, 0.0) }
    );
    // A proxy model is loaded like the entity's own.
    let mut a = assets();
    let src = LEVEL.replacen(row, &format!("{row} occluder=model:ball.obj"), 1);
    let level = a.parse_level("two_rooms.mmp", &src).unwrap();
    assert_eq!(level.spawns[1].occluder, Occluder::Model(a.mesh_id("ball.obj").unwrap()));
}

#[test]
fn rejects_entity_option_errors() {
    let row = "1.00   crate_a1";
    let options = |o: &str| format!("{row} {o}");
    assert_rejects(row, &options("glowing"), "unknown option 'glowing'");
    assert_rejects(row, &options("occluder=blob"), "unknown occluder 'blob'");
    assert_rejects(row, &options("occluder=facing:2:0.5"), "3 to 64 sides");
    assert_rejects(row, &options("occluder=facing:8:0"), "radius must be positive");
    assert_rejects(row, &options("occluder=facing:8:0.5:1"), "unknown occluder");
    assert_rejects(row, &options("occluder=model:missing.obj"), "occluder");
    assert_rejects("1.00   player_start", "1.00   player_start static", "spawn points take no options");
    let actor = LEVEL.replacen("   1   prop   0", "   1   actor  0", 1).replacen(row, &options("static"), 1);
    let msg = assets().parse_level("two_rooms.mmp", &actor).err().unwrap().to_string();
    assert!(msg.contains("only props can be static"), "{msg}");
}

#[test]
fn loads_light_options_sky_and_directional_lights() {
    let src = with_lights(
        "ambient 0 0 0",
        &["0  -2.0  3.0  5.0   1 1 1  6.0  radius=0.05", "0  -2.0  3.0  5.0   1 1 1  6.0  shadows=off"],
    );
    let level = assets().parse_level("two_rooms.mmp", &src).unwrap();
    assert_eq!((level.lights[0].radius, level.lights[0].shadows), (0.05, true));
    assert_eq!((level.lights[1].radius, level.lights[1].shadows), (0.0, false));
    assert!(level.directional.is_empty());
    // Directional lights: direction (normalized), color, angle, shadows.
    let src = format!("{src}\ndirectional 2\n   0   0 -2 0   1.0 0.9 0.8   0.53\n   1   1 -1 0   0.2 0.2 0.3   0   shadows=off\n");
    let level = assets().parse_level("two_rooms.mmp", &src).unwrap();
    let d = level.directional[0];
    assert_eq!((d.direction, d.color, d.angle, d.shadows), (Vec3::NEG_Y, Vec3::new(1.0, 0.9, 0.8), 0.53, true));
    assert!(!level.directional[1].shadows);
    // A sky surface (0x2): room_a's ceiling.
    let sky = LEVEL.replacen("   1   0       -1      0x0 ", "   1   0       -1      0x2 ", 1);
    assert_ne!(sky, LEVEL);
    let mut a = assets();
    let level = a.parse_level("two_rooms.mmp", &sky).unwrap();
    let mesh = a.mesh(level.geometry);
    assert_eq!(mesh.polygons.iter().filter(|p| p.flags.sky()).count(), 1);
}

#[test]
fn rejects_light_option_and_directional_errors() {
    let reject = |src: String, expected: &str| {
        let msg = match assets().parse_level("two_rooms.mmp", &src) {
            Ok(_) => panic!("loaded despite {expected:?}"),
            Err(e) => e.to_string(),
        };
        assert!(msg.contains(expected), "expected '{expected}' in: {msg}");
    };
    let light = |o: &str| with_lights("ambient 1 1 1", &[&format!("0  0.0 2.0 4.0  1 1 1  5  {o}")]);
    reject(light("radius=-1"), "must be 0 or more");
    reject(light("shadows=maybe"), "unknown option");
    reject(light("glow=1"), "unknown option");
    let directional = |row: &str| format!("{}\ndirectional 1\n   0   {row}\n", with_lights("ambient 1 1 1", &[]));
    reject(directional("0 0 0  1 1 1  0.5"), "direction cannot be zero");
    reject(directional("0 -1 0  1 -1 1  0.5"), "color cannot be negative");
    reject(directional("0 -1 0  1 1 1  60"), "angle must be from 0 to 45");
    reject(directional("0 -1 0  1 1 1"), "needs 7 fields");
    reject(directional("0 -1 0  1 1 1  0.5  bright=on"), "unknown option");
    reject(
        LEVEL.replacen("   1   0       -1      0x0 ", "   1   0       -1      0x3 ", 1),
        "both reflective and sky",
    );
}

#[test]
fn loads_and_checks_moving_lights() {
    let src = with_lights("ambient 0 0 0", &["1  0.0 2.0 -6.0  1 1 1  5  oscillate=0:0:4:5"]);
    let l = assets().parse_level("two_rooms.mmp", &src).unwrap().lights[0];
    let m = l.motion.unwrap();
    assert_eq!((m.offset, m.period), (Vec3::new(0.0, 0.0, 4.0), 5.0));
    assert!(!l.is_static, "a moving light isn't baked");
    // A sine through its position: a quarter period in, at the offset's end.
    assert!((l.at_time(1.25) - Vec3::new(0.0, 2.0, -2.0)).length() < 1e-4);
    assert!((l.at_time(3.75) - Vec3::new(0.0, 2.0, -10.0)).length() < 1e-4);
    let reject = |row: &str, expected: &str| {
        let msg = assets()
            .parse_level("two_rooms.mmp", &with_lights("ambient 0 0 0", &[row]))
            .err()
            .unwrap()
            .to_string();
        assert!(msg.contains(expected), "expected '{expected}' in: {msg}");
    };
    reject("1  0.0 2.0 -6.0  1 1 1  5  oscillate=0:0:4", "oscillate is DX:DY:DZ:PERIOD");
    reject("1  0.0 2.0 -6.0  1 1 1  5  oscillate=0:0:4:0", "period must be positive");
    reject("1  0.0 2.0 -6.0  1 1 1  5  oscillate=0:0:40:5", "swings outside the level");
}

#[test]
fn levels_written_back_load_the_same() {
    use moose_assets::LevelDoc;
    for file in ["two_rooms.mmp", "shiny_rooms.mmp", "sunny_rooms.mmp", "mirror_rooms.mmp"] {
        let path = format!("{}/../../assets/levels/{file}", env!("CARGO_MANIFEST_DIR"));
        let src = std::fs::read_to_string(&path).unwrap();
        let mut assets = assets();
        let level = assets.parse_level(file, &src).unwrap();
        let doc = LevelDoc::parse(std::path::Path::new(&path), &src).unwrap();
        let written = doc.to_text();
        // Reading what was written gives the same document, and the same level.
        let again = LevelDoc::parse(std::path::Path::new(&path), &written).unwrap();
        assert_eq!(again.surfaces, doc.surfaces, "{file}");
        assert_eq!(again.vertices, doc.vertices, "{file}");
        let level_again = assets.parse_level(file, &written).unwrap();
        assert_eq!(level_again.sectors, level.sectors, "{file}");
        assert_eq!(level_again.portals, level.portals, "{file}");
        assert_eq!(level_again.lights, level.lights, "{file}");
        assert_eq!(level_again.directional, level.directional, "{file}");
        assert_eq!(level_again.ambient, level.ambient, "{file}");
        let (g0, g1) = (assets.mesh(level.geometry), assets.mesh(level_again.geometry));
        assert_eq!(g0.positions, g1.positions, "{file}");
        assert_eq!(g0.polygons.len(), g1.polygons.len(), "{file}");
        for (a, b) in level.spawns.iter().zip(&level_again.spawns) {
            assert_eq!((a.kind, a.sector, a.mesh, &a.name), (b.kind, b.sector, b.mesh, &b.name));
            assert!(a.position.distance(b.position) < 1e-4, "{} moved", a.name);
            // (angle_between takes an acos, imprecise near 0.)
            assert!(a.rotation.angle_between(b.rotation) < 1e-3, "{} turned", a.name);
            assert_eq!((a.scale, a.is_static, a.occluder), (b.scale, b.is_static, b.occluder));
        }
    }
}

/// two_rooms as editable tables, and a check that tables load as a level.
fn doc(file: &str) -> moose_assets::LevelDoc {
    let path = format!("{}/../../assets/levels/{file}", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&path).unwrap();
    moose_assets::LevelDoc::parse(std::path::Path::new(&path), &src).unwrap()
}

fn loads(doc: &moose_assets::LevelDoc) -> moose_assets::Level {
    let mut assets = assets();
    match assets.parse_level("edited.mmp", &doc.to_text()) {
        Ok(level) => level,
        Err(e) => panic!("{e}\n{}", doc.to_text()),
    }
}

/// The solid surface of `sector` facing `normal`.
fn wall(doc: &moose_assets::LevelDoc, sector: usize, normal: Vec3) -> usize {
    doc.sector_surfaces(sector)
        .find(|&i| doc.surfaces[i].adjoin.is_none() && doc.surface_normal(i).dot(normal) > 0.99)
        .unwrap()
}

#[test]
fn extruding_a_wall_makes_a_new_sector_behind_it() {
    let mut d = doc("two_rooms.mmp");
    let before = loads(&d);
    // room_a's back wall, at z = 8, faces into the room (-Z).
    let back = wall(&d, 0, Vec3::NEG_Z);
    let (sector, far) = d.extrude(back, 2.0).unwrap();
    let level = loads(&d);
    assert_eq!(level.sectors.len(), before.sectors.len() + 1);
    assert_eq!(level.portals.len(), before.portals.len() + 2);
    // The new sector reaches 2 m past the wall; its far end faces back at the room.
    assert!((level.sectors[sector].bounds.max.z - 10.0).abs() < 1e-4);
    assert!(d.surface_normal(far).dot(Vec3::NEG_Z) > 0.99);
    // And again, from the far end: a corridor.
    d.extrude(far, 3.0).unwrap();
    assert_eq!(loads(&d).sectors.len(), before.sectors.len() + 2);
}

#[test]
fn pushing_a_wall_stretches_the_room() {
    let mut d = doc("two_rooms.mmp");
    let back = wall(&d, 0, Vec3::NEG_Z);
    d.push_surface(back, -1.5);
    let level = loads(&d);
    assert!((level.sectors[0].bounds.max.z - 9.5).abs() < 1e-4);
}

#[test]
fn cleaving_a_room_splits_its_doorway_on_both_sides() {
    let mut d = doc("two_rooms.mmp");
    let before = loads(&d);
    // room_a down the middle (x = 0): the plane crosses its doorway to the hallway,
    // whose side of the opening is split too.
    let new = d.cleave(0, Vec3::X, Vec3::ZERO).unwrap();
    let level = loads(&d);
    assert_eq!(level.sectors.len(), before.sectors.len() + 1);
    // The cut's opening pair, and the doorway split into two pairs.
    assert_eq!(level.portals.len(), before.portals.len() + 4);
    assert!(level.sectors[0].bounds.min.x.abs() < 1e-4, "the front part is x >= 0");
    assert!(level.sectors[new].bounds.max.x.abs() < 1e-4, "the part behind is x <= 0");
    // A cut that misses: refused, nothing changed.
    let same = d.clone();
    assert!(d.cleave(0, Vec3::X, Vec3::new(100.0, 0.0, 0.0)).is_err());
    assert_eq!(d, same);
}

#[test]
fn a_new_room_joins_the_level_through_a_wall() {
    let mut d = doc("two_rooms.mmp");
    let before = loads(&d);
    // Beside room_a (x from -4 to 4, y 0 to 4, z 0 to 8), sharing its east wall.
    let room = d.add_box(Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 4.0, 8.0), "annex").unwrap();
    loads(&d);
    let west = wall(&d, room, Vec3::X);
    let east = d.adjoin(west).unwrap();
    assert_eq!(d.surfaces[east].sector, 0);
    let level = loads(&d);
    assert_eq!(level.portals.len(), before.portals.len() + 2);
    // Walled up again, then gone.
    d.unadjoin(west).unwrap();
    assert_eq!(loads(&d).portals.len(), before.portals.len());
    d.delete_sector(room).unwrap();
    assert_eq!(loads(&d).sectors.len(), before.sectors.len());
}

#[test]
fn a_sector_with_things_in_it_stays() {
    let mut d = doc("two_rooms.mmp");
    assert!(d.delete_sector(0).is_err(), "the spawn point is in room_a");
}

#[test]
fn cleaving_keeps_attributes_and_splits_every_opening_it_crosses() {
    // shiny_rooms has colors and uvs: corners made on edges get values between.
    let mut d = doc("shiny_rooms.mmp");
    let before = loads(&d);
    // The hallway (sector 1, x from -1 to 1, z from -12 to 0) lengthwise at x = 0.25: the
    // cut crosses both its doorways.
    let hallway = d.sectors.iter().position(|s| s.name == "hallway").unwrap();
    let new = d.cleave(hallway, Vec3::X, Vec3::new(0.25, 0.0, 0.0)).unwrap();
    let level = loads(&d);
    assert_eq!(level.sectors.len(), before.sectors.len() + 1);
    // Its own opening pair, and each doorway's pair split in two.
    assert_eq!(level.portals.len(), before.portals.len() + 2 + 2 + 2);
    assert!((level.sectors[new].bounds.max.x - 0.25).abs() < 1e-4);
    // And room_b across the middle, with its lights and the ball in it.
    let room_b = d.sectors.iter().position(|s| s.name == "room_b").unwrap();
    d.cleave(room_b, Vec3::Z, Vec3::new(0.0, 0.0, -16.5)).unwrap();
    loads(&d);
}
