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
        "0x8    6       0:0",
        "unknown surface flags 0x8",
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
fn loads_templates_and_shadow_kinds() {
    use moose_assets::ShadowKind;
    // Entity 1 placed by a template, its options the template's unless it sets its own.
    let templated = |own: &str| {
        let src = LEVEL
            .replacen("crate.obj", "box", 1)
            .replacen("1.00   crate_a1", &format!("1.00   crate_a1 {own}"), 1)
            .replacen("entities ", "templates 1\n   0   box  crate.obj  static shadow=hard\n\nentities ", 1);
        let mut a = assets();
        let level = a.parse_level("two_rooms.mmp", &src).unwrap();
        (level.spawns[1].clone(), a.mesh_id("crate.obj").unwrap())
    };
    let (e, crate_mesh) = templated("");
    assert_eq!((e.mesh, e.is_static, e.shadow), (Some(crate_mesh), true, ShadowKind::Hard));
    let (e, _) = templated("shadow=blurred static=off");
    assert_eq!((e.is_static, e.shadow), (false, ShadowKind::Blurred));
    // Without a template or the option: soft.
    let level = assets().parse_level("two_rooms.mmp", LEVEL).unwrap();
    assert_eq!(level.spawns[1].shadow, ShadowKind::Soft);
    let row = "1.00   crate_a1";
    assert_rejects(row, &format!("{row} shadow=fuzzy"), "unknown shadow 'fuzzy'");
    let twice = LEVEL.replacen(
        "entities ",
        "templates 2\n   0   box  crate.obj\n   1   box  crate.obj\n\nentities ",
        1,
    );
    let msg = assets().parse_level("two_rooms.mmp", &twice).err().unwrap().to_string();
    assert!(msg.contains("template name 'box' is used twice"), "{msg}");
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
    reject(
        LEVEL.replacen("   1   0       -1      0x0 ", "   1   0       -1      0x6 ", 1),
        "a hidden surface takes no other flags",
    );
    // Hidden on its own loads, and bounds the sector like any surface.
    let src = LEVEL.replacen("   1   0       -1      0x0 ", "   1   0       -1      0x4 ", 1);
    let mut a = assets();
    let level = a.parse_level("two_rooms.mmp", &src).unwrap();
    let geometry = a.mesh(level.geometry);
    assert!(geometry.polygons[1].flags.hidden());
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
    for file in ["two_rooms.mmp", "shiny_rooms.mmp", "sunny_rooms.mmp", "mirror_rooms.mmp", "walker_rooms.mmp"] {
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
            assert_eq!((a.shadow, &a.animation), (b.shadow, &b.animation), "{}", a.name);
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

#[test]
fn the_walker_loads_with_its_skeleton_and_walks() {
    let mut assets = assets();
    let id = assets.load_mesh("walker.mmdl").unwrap();
    let mesh = assets.mesh(id);
    let skin = mesh.skin.as_ref().expect("a skeleton");
    assert_eq!(skin.bones.len(), 7);
    assert_eq!(skin.position_bones.len(), mesh.positions.len());
    let proxies = mesh.polygons.iter().filter(|p| p.flags.proxy()).count();
    let drawn = mesh.polygons.len() - proxies;
    assert_eq!((drawn, proxies), (42, 42));
    let walk = skin.animation("walk").unwrap();
    assert!(skin.animation("idle").is_some());
    // A quarter of the way through the walk (0.25 s of 1 s), the left leg swings 0.5 rad
    // about x at its hip (-0.15, 0.9, 0): its foot moves along -z... or +z, and the right
    // foot the other way.
    let hip = Vec3::new(-0.15, 0.9, 0.0);
    let foot_l = mesh.positions.iter().zip(&skin.position_bones).position(|(p, &b)| b == 3 && p.y == 0.0).unwrap();
    let foot_r = mesh.positions.iter().zip(&skin.position_bones).position(|(p, &b)| b == 4 && p.y == 0.0).unwrap();
    let matrices = skin.matrices(walk, 0.25);
    let mut posed = vec![Vec3::ZERO; mesh.positions.len()];
    skin.pose_positions(&mesh.positions, &matrices, &mut posed);
    let (l, r) = (posed[foot_l], posed[foot_r]);
    // The foot stays its distance from the hip (which bobs with the hips: the leg's
    // matrix takes it where it is now), and the two feet part.
    let hip_now = matrices[3].transform_point3(hip);
    assert!((hip_now.y - (hip.y - 0.025)).abs() < 1e-4, "the hips bob down: {hip_now}");
    assert!((l.distance(hip_now) - mesh.positions[foot_l].distance(hip)).abs() < 1e-4);
    assert!((l.z - mesh.positions[foot_l].z).abs() > 0.3, "{l}");
    assert!((l.z - mesh.positions[foot_l].z) * (r.z - mesh.positions[foot_r].z) < 0.0);
    // And the level with it walking in it loads.
    let level = assets.load_level("walker_rooms.mmp").unwrap();
    let walker = level.spawns.iter().find(|s| s.name == "walker").unwrap();
    assert_eq!(walker.animation.as_deref(), Some("walk"));
}

#[test]
fn animations_must_be_the_models() {
    let mut assets = assets();
    let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/levels/walker_rooms.mmp")).unwrap();
    let bad = src.replace("anim=walk", "anim=dance");
    let err = assets.parse_level("bad.mmp", &bad).unwrap_err().to_string();
    assert!(err.contains("no animation 'dance'"), "{err}");
    let bad = src.replace("anim=walk", "anim=walk static");
    assert!(assets.parse_level("bad.mmp", &bad).is_err());
}

#[test]
fn loads_meta_values_and_light_names_and_writes_them_back() {
    // The level's own, a sector's, a surface's, a template's (one entity overriding it) and
    // a named light.
    let src = LEVEL
        .replacen("name \"Two Rooms\"", "name \"Two Rooms\" $fog=10", 1)
        .replacen("   1   hallway   9              6", "   1   hallway   9              6   $wet=200", 1)
        .replacen("material=vertex_color  # far wall", "material=vertex_color $shine=5  # far wall", 1)
        .replacen("crate.obj  -2.50   0.00    2.00", "box  -2.50   0.00    2.00", 1)
        .replacen("crate.obj  -2.50   1.00    2.00", "box  -2.50   1.00    2.00", 1)
        .replacen("1.00   crate_a2_stacked", "1.00   crate_a2_stacked $team=1,2,3", 1)
        .replacen("entities ", "templates 1\n   0   box  crate.obj  $team=204,51,51\n\nentities ", 1);
    let mut a = assets();
    let level = a.parse_level("two_rooms.mmp", &src).unwrap();
    let key = |name: &str| level.meta_keys.id(name).unwrap();
    assert_eq!(level.meta.get(key("fog")), Some(&[10.0][..]));
    assert_eq!(level.sectors[1].meta.get(key("wet")), Some(&[200.0][..]));
    assert_eq!(level.sectors[0].meta.get(key("wet")), None);
    // The far wall is room_a's third solid surface.
    assert_eq!(level.faces[2].get(key("shine")), Some(&[5.0][..]));
    assert_eq!(level.faces.len(), 20);
    assert_eq!(level.spawns[1].meta.get(key("team")), Some(&[204.0, 51.0, 51.0][..]));
    assert_eq!(level.spawns[2].meta.get(key("team")), Some(&[1.0, 2.0, 3.0][..]));
    assert!(level.light_names.is_empty());
    // The editor's tables keep them as written.
    let path = std::path::Path::new("two_rooms.mmp");
    let doc = moose_assets::LevelDoc::parse(path, &src).unwrap();
    assert_eq!(doc.name_options, ["$fog=10"]);
    assert_eq!(doc.sectors[1].options, ["$wet=200"]);
    let again = a.parse_level("two_rooms.mmp", &doc.to_text()).unwrap();
    assert_eq!((again.meta, again.sectors[1].meta.clone(), again.faces), (level.meta, level.sectors[1].meta.clone(), level.faces));
    // Bad ones are refused where they are.
    let bad = src.replacen("$wet=200", "$wet=soaked", 1);
    let msg = assets().parse_level("two_rooms.mmp", &bad).err().unwrap().to_string();
    assert!(msg.contains("sector 1: $wet: 'soaked' is not a number"), "{msg}");
    let bad = src.replacen("$wet=200", "wet=200", 1);
    let msg = assets().parse_level("two_rooms.mmp", &bad).err().unwrap().to_string();
    assert!(msg.contains("unknown option 'wet=200' (meta values are $KEY=VALUE)"), "{msg}");
}

#[test]
fn lights_take_names() {
    let src = include_str!("../../../assets/levels/shiny_rooms.mmp");
    let named = src.replacen("6.00   radius=0.05", "6.00   radius=0.05 name=lamp", 1);
    let level = assets().parse_level("shiny_rooms.mmp", &named).unwrap();
    assert_eq!(level.light_names[0].as_deref(), Some("lamp"));
    assert_eq!(level.light_names.len(), level.lights.len());
    let twice = named.replacen("5.00   radius=0.05", "5.00   radius=0.05 name=lamp", 1);
    let msg = assets().parse_level("shiny_rooms.mmp", &twice).err().unwrap().to_string();
    assert!(msg.contains("another light is named 'lamp'"), "{msg}");
}

#[test]
fn stitching_continues_a_surfaces_material_and_texture_across_the_edge() {
    use moose_assets::LevelDoc;
    let path = std::path::Path::new("shiny_rooms.mmp");
    let src = include_str!("../../../assets/levels/shiny_rooms.mmp");
    let mut doc = LevelDoc::parse(path, src).unwrap();
    let uv = doc.attributes.iter().position(|a| a.name == "uv").unwrap();
    let uvs = |doc: &LevelDoc, surface: usize| -> Vec<(moose_assets::glam::Vec3, [f32; 2])> {
        doc.surfaces[surface]
            .corners
            .iter()
            .map(|(v, rows)| {
                let t = &doc.attributes[uv].values[rows[uv]];
                (doc.vertices[*v], [t[0].parse().unwrap(), t[1].parse().unwrap()])
            })
            .collect()
    };
    // The far wall (2) from room_a's floor (0): the floor's material, and its texture
    // folded up the wall.
    let (floor, wall) = (0, 2);
    assert!(doc.stitch(wall, floor, false).unwrap());
    assert!(doc.surfaces[wall].options.contains(&"material=metal_floor_shiny".to_string()));
    let (on_floor, on_wall) = (uvs(&doc, floor), uvs(&doc, wall));
    // On the edge they share, the same coordinates.
    let mut shared = 0;
    for (p, t) in &on_wall {
        if let Some((_, f)) = on_floor.iter().find(|(q, _)| q.distance(*p) < 1e-4) {
            assert!((t[0] - f[0]).abs() < 1e-4 && (t[1] - f[1]).abs() < 1e-4, "{t:?} vs {f:?} at {p}");
            shared += 1;
        }
    }
    assert_eq!(shared, 2);
    // Up the wall, not stretched: the floor's half a tile a meter (2 m a tile), every way.
    for (p, t) in &on_wall {
        for (q, u) in &on_wall {
            let (d, du) = (p.distance(*q), ((t[0] - u[0]).powi(2) + (t[1] - u[1]).powi(2)).sqrt());
            assert!((du - d * 0.5).abs() < 1e-3, "{d} m apart, {du} apart in uv");
        }
    }
    // Mirrored: the same on the edge, and a point h up the wall takes the floor's texture
    // h back from the edge (the floor's u = x / 2, v = z / 2; the wall is at z = 8): v =
    // (8 - h) / 2, where continued it would be (8 + h) / 2.
    assert!(doc.stitch(wall, floor, true).unwrap());
    for (p, t) in uvs(&doc, wall) {
        assert!((t[0] - p.x / 2.0).abs() < 1e-3 && (t[1] - (8.0 - p.y) / 2.0).abs() < 1e-3, "{p}: {t:?}");
    }
    assert!(doc.stitch(wall, floor, false).unwrap());
    for (p, t) in uvs(&doc, wall) {
        assert!((t[1] - (8.0 + p.y) / 2.0).abs() < 1e-3, "{p}: {t:?}");
    }
    // A coplanar surface (the hallway's floor, 9) continues the room's as it is: the
    // levels are textured flat already, so nothing moves.
    let before = uvs(&doc, 9);
    assert!(doc.stitch(9, floor, false).unwrap());
    for ((_, a), (_, b)) in before.iter().zip(uvs(&doc, 9)) {
        assert!((a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4, "{a:?} became {b:?}");
    }
    // The level loads with them.
    assets().parse_level("shiny_rooms.mmp", &doc.to_text()).unwrap();
    // What can't be stitched.
    assert!(doc.stitch(floor, floor, false).unwrap_err().contains("another surface"));
    let ceiling = 1;
    assert!(doc.stitch(ceiling, floor, false).unwrap_err().contains("don't share an edge"));
    let portal = doc.surfaces.iter().position(|s| s.adjoin.is_some()).unwrap();
    assert!(doc.stitch(portal, floor, false).unwrap_err().contains("openings"));
}

#[test]
fn cutting_a_surface_splits_only_it_and_its_neighbors_take_the_new_corners() {
    use moose_assets::glam::Vec3;
    use moose_assets::LevelDoc;
    let path = std::path::Path::new("shiny_rooms.mmp");
    let src = include_str!("../../../assets/levels/shiny_rooms.mmp");
    let mut doc = LevelDoc::parse(path, src).unwrap();
    let (surfaces, sectors, adjoins) = (doc.surfaces.len(), doc.sectors.len(), doc.adjoins.len());
    let corners = |doc: &LevelDoc, i: usize| doc.surfaces[i].corners.len();
    let (floor, ceiling) = (corners(&doc, 0), corners(&doc, 1));
    // room_a's far wall (2, at z = 8, x from -4 to 4), down the middle.
    let behind = doc.cut_surface(2, Vec3::X, Vec3::ZERO).unwrap();
    assert_eq!(behind, 3);
    assert_eq!((doc.surfaces.len(), doc.sectors.len(), doc.adjoins.len()), (surfaces + 1, sectors, adjoins));
    assert_eq!(doc.sectors[0].surface_count, 10);
    for half in [2, 3] {
        assert_eq!(corners(&doc, half), 4);
        assert_eq!(doc.surfaces[half].options, ["material=brick"]);
    }
    // The floor and ceiling share the cut edges' ends: each takes the new corner.
    assert_eq!((corners(&doc, 0), corners(&doc, 1)), (floor + 1, ceiling + 1));
    // Texture coordinates run on: the bottom middle (0, 0, 8) is halfway between its ends.
    let uv = doc.attributes.iter().position(|a| a.name == "uv").unwrap();
    let at = |doc: &LevelDoc, surface: usize, p: Vec3| {
        let (_, rows) = doc.surfaces[surface].corners.iter().find(|(v, _)| doc.vertices[*v].distance(p) < 1e-4).unwrap();
        let t = &doc.attributes[uv].values[rows[uv]];
        [t[0].parse::<f32>().unwrap(), t[1].parse::<f32>().unwrap()]
    };
    let (a, b, m) = (at(&doc, 3, Vec3::new(-4.0, 0.0, 8.0)), at(&doc, 2, Vec3::new(4.0, 0.0, 8.0)), at(&doc, 2, Vec3::new(0.0, 0.0, 8.0)));
    assert!((m[0] - (a[0] + b[0]) / 2.0).abs() < 1e-4 && (m[1] - (a[1] + b[1]) / 2.0).abs() < 1e-4, "{a:?} {m:?} {b:?}");
    assets().parse_level("shiny_rooms.mmp", &doc.to_text()).unwrap();
    // The lintel over the doorway (now 8: x from -1 to 1, y from 3 to 4), down the middle:
    // its bottom edge is the opening's top, so the opening, its mirror in the hallway and
    // the hallway's ceiling all take the new corner, and the level still loads.
    let lintel = doc.surfaces.iter().position(|s| {
        let p: Vec<Vec3> = s.corners.iter().map(|(v, _)| doc.vertices[*v]).collect();
        s.adjoin.is_none() && p.iter().all(|q| q.z.abs() < 1e-4 && q.y >= 3.0 - 1e-4 && q.x.abs() <= 1.0 + 1e-4)
    });
    let lintel = lintel.unwrap();
    let openings: Vec<usize> = doc.adjoins[..2].iter().map(|a| corners(&doc, a.surface)).collect();
    doc.cut_surface(lintel, Vec3::X, Vec3::ZERO).unwrap();
    let after: Vec<usize> = doc.adjoins[..2].iter().map(|a| corners(&doc, a.surface)).collect();
    assert_eq!(after, openings.iter().map(|n| n + 1).collect::<Vec<_>>());
    assets().parse_level("shiny_rooms.mmp", &doc.to_text()).unwrap();
    // What can't be cut.
    assert!(doc.cut_surface(0, Vec3::X, Vec3::new(100.0, 0.0, 0.0)).unwrap_err().contains("doesn't cross"));
    let portal = doc.adjoins[0].surface;
    assert!(doc.cut_surface(portal, Vec3::X, Vec3::ZERO).unwrap_err().contains("opening"));
}

#[test]
fn merging_faces_takes_one_plane_touching_and_convex() {
    use moose_assets::glam::Vec3;
    use moose_assets::LevelDoc;
    let path = std::path::Path::new("shiny_rooms.mmp");
    let src = include_str!("../../../assets/levels/shiny_rooms.mmp");
    let mut doc = LevelDoc::parse(path, src).unwrap();
    let surfaces = doc.surfaces.len();
    let uv = doc.attributes.iter().position(|a| a.name == "uv").unwrap();
    // room_a's far wall cut down the middle, then merged back: one surface again, with the
    // cut's two corners kept (the floor and ceiling use them now), its material, and
    // coordinates that run on (each corner's the flat mapping's, as before).
    let behind = doc.cut_surface(2, Vec3::X, Vec3::ZERO).unwrap();
    assert_eq!(doc.merge_surfaces(2, behind).unwrap(), 2);
    assert_eq!(doc.surfaces.len(), surfaces);
    assert_eq!(doc.surfaces[2].corners.len(), 6);
    assert_eq!(doc.surfaces[2].options, ["material=brick"]);
    for (v, rows) in &doc.surfaces[2].corners {
        let p = doc.vertices[*v];
        let t = &doc.attributes[uv].values[rows[uv]];
        let (u, w) = (t[0].parse::<f32>().unwrap(), t[1].parse::<f32>().unwrap());
        assert!((u - p.x / 2.0).abs() < 1e-3 && (w + p.y / 2.0).abs() < 1e-3, "{p}: {u}, {w}");
    }
    assets().parse_level("shiny_rooms.mmp", &doc.to_text()).unwrap();
    // room_b's floor in quarters (x = 0, z = -16): two side by side merge; a third would
    // make an L.
    // The piece of room_b's floor whose middle is nearest (x, z).
    let floor_of = |doc: &LevelDoc, x: f32, z: f32| {
        let middle = |s: &moose_assets::SurfaceDoc| {
            s.corners.iter().map(|(v, _)| doc.vertices[*v]).sum::<Vec3>() / s.corners.len() as f32
        };
        (0..doc.surfaces.len())
            .filter(|&i| {
                let s = &doc.surfaces[i];
                s.adjoin.is_none() && s.sector == 2 && s.corners.iter().all(|(v, _)| doc.vertices[*v].y.abs() < 1e-4)
            })
            .min_by(|&i, &j| {
                let d = |k: usize| middle(&doc.surfaces[k]).distance(Vec3::new(x, 0.0, z));
                d(i).total_cmp(&d(j))
            })
    };
    let floor = floor_of(&doc, 1.0, -14.0).unwrap();
    doc.cut_surface(floor, Vec3::X, Vec3::ZERO).unwrap();
    for x in [1.0, -1.0] {
        let half = floor_of(&doc, x, -14.0).unwrap();
        doc.cut_surface(half, Vec3::Z, Vec3::new(0.0, 0.0, -16.0)).unwrap();
    }
    let (a, b, c) = (floor_of(&doc, 1.0, -14.0).unwrap(), floor_of(&doc, -1.0, -14.0).unwrap(), floor_of(&doc, 1.0, -18.0).unwrap());
    // Corner to corner, they meet at a point only.
    let diagonal = floor_of(&doc, -1.0, -18.0).unwrap();
    assert!(doc.merge_surfaces(a, diagonal).unwrap_err().contains("don't share an edge"));
    let half = doc.merge_surfaces(a, b).unwrap();
    let c = if b < c { c - 1 } else { c };
    assert!(doc.merge_surfaces(half, c).unwrap_err().contains("convex"));
    assets().parse_level("shiny_rooms.mmp", &doc.to_text()).unwrap();
    // Not one plane (the floor and a wall, the left and right walls); in other sectors (the
    // hallway's floor); itself.
    assert!(doc.merge_surfaces(0, 2).unwrap_err().contains("one plane"));
    let hallway_floor = doc.surfaces.iter().position(|s| s.sector == 1 && s.adjoin.is_none()).unwrap();
    assert!(doc.merge_surfaces(0, hallway_floor).unwrap_err().contains("different sectors"));
    let (left, right) = (3, 4);
    assert!(doc.merge_surfaces(left, right).unwrap_err().contains("one plane"));
    assert!(doc.merge_surfaces(2, 2).unwrap_err().contains("another surface"));
}

/// Sector `s`'s surfaces as what they look like: each one's corners (where they are, and
/// their texture coordinates), whether it is an opening, and its options, in a set.
fn looks(doc: &moose_assets::LevelDoc, s: usize) -> Vec<String> {
    let uv = doc.attributes.iter().position(|a| a.name == "uv");
    let mut out: Vec<String> = doc
        .sector_surfaces(s)
        .map(|i| {
            let t = &doc.surfaces[i];
            let mut corners: Vec<String> = t
                .corners
                .iter()
                .map(|(v, rows)| {
                    let p = doc.vertices[*v];
                    let uv = uv.and_then(|k| Some(doc.attributes[k].values[*rows.get(k)?].iter().map(|x| format!("{:.3}", x.parse::<f32>().unwrap())).collect::<Vec<_>>()));
                    format!("({:.3},{:.3},{:.3}){uv:?}", p.x, p.y, p.z)
                })
                .collect();
            corners.sort();
            format!("{} {:?} {:?} {}", t.adjoin.is_some(), t.options, t.flags, corners.join(" "))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn a_cleaved_sector_merged_back_is_as_it_was() {
    // shiny_rooms' hallway lengthwise (the cut crosses both doorways, splitting them and
    // room_a's and room_b's walls around them), then merged back: the same surfaces,
    // textures and openings, on both sides of the doorways.
    let original = doc("shiny_rooms.mmp");
    let mut d = original.clone();
    let hallway = d.sectors.iter().position(|s| s.name == "hallway").unwrap();
    let new = d.cleave(hallway, Vec3::X, Vec3::new(0.25, 0.0, 0.0)).unwrap();
    assert_eq!(d.merge_sectors(hallway, new).unwrap(), hallway);
    let level = loads(&d);
    assert_eq!(level.sectors.len(), original.sectors.len());
    assert_eq!(level.portals.len(), loads(&original).portals.len());
    for s in 0..original.sectors.len() {
        assert_eq!(looks(&d, s), looks(&original, s), "sector {s}");
    }
    // room_a across the middle, merged the other way round (room_a into its new part,
    // after the others): its crates go with it, and the sectors after it move down.
    let mut d = original.clone();
    let new = d.cleave(0, Vec3::X, Vec3::ZERO).unwrap();
    assert!(d.entities.iter().any(|e| e.sector == new));
    let merged = d.merge_sectors(new, 0).unwrap();
    assert_eq!(merged, original.sectors.len() - 1);
    assert_eq!(d.sectors[merged].name, "room_a_2");
    loads(&d);
    assert_eq!(looks(&d, merged), looks(&original, 0));
    for (e, was) in d.entities.iter().zip(&original.entities) {
        assert_eq!(e.sector, if was.sector == 0 { merged } else { was.sector - 1 }, "{}", e.name);
    }
}

#[test]
fn merging_sectors_takes_an_opening_between_them_and_a_convex_union() {
    let mut d = doc("two_rooms.mmp");
    let same = d.clone();
    assert!(d.merge_sectors(0, 2).unwrap_err().contains("don't open onto each other"));
    // room_a and the hallway meet at a doorway in room_a's wall: the wall would be inside.
    assert!(d.merge_sectors(0, 1).unwrap_err().contains("convex"));
    assert!(d.merge_sectors(1, 1).unwrap_err().contains("another sector"));
    assert_eq!(d, same);
    // A room built beside room_a, opened onto it through a whole wall: one room.
    let room = d.add_box(Vec3::new(4.0, 0.0, 0.0), Vec3::new(8.0, 4.0, 8.0), "annex").unwrap();
    d.adjoin(wall(&d, room, Vec3::X)).unwrap();
    d.merge_sectors(0, room).unwrap();
    let level = loads(&d);
    assert_eq!(level.sectors.len(), same.sectors.len());
    assert!((level.sectors[0].bounds.max.x - 8.0).abs() < 1e-4);
}

#[test]
fn every_merge_of_sectors_a_level_allows_loads() {
    // Each pair of sectors with an opening between them, in every level: merged, or
    // refused, never a level the loader refuses.
    let dir = format!("{}/../../assets/levels", env!("CARGO_MANIFEST_DIR"));
    let mut merged = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "mmp") {
            continue;
        }
        let file = path.file_name().unwrap().to_str().unwrap();
        let original = doc(file);
        let mut pairs: Vec<(usize, usize)> = original
            .adjoins
            .iter()
            .map(|a| (original.surfaces[a.surface].sector, original.surfaces[original.adjoins[a.mirror].surface].sector))
            .filter(|&(a, b)| a < b)
            .collect();
        pairs.sort_unstable();
        pairs.dedup();
        for (a, b) in pairs {
            let mut d = original.clone();
            if d.merge_sectors(a, b).is_ok() {
                let mut assets = assets();
                if let Err(e) = assets.parse_level("edited.mmp", &d.to_text()) {
                    panic!("{file}: merging sectors {a} and {b}: {e}");
                }
                merged += 1;
            }
        }
    }
    // (21 across the levels when this was written.)
    assert!(merged > 0);
}

#[test]
fn exclusion_lists_name_lights_and_become_masks() {
    use moose_assets::{FLASHLIGHT_ID, TemplateDoc};
    // sunny_rooms: one light (bit 0) and the sun (bit 1), both named.
    let mut d = doc("sunny_rooms.mmp");
    d.lights[0].options.push("name=lamp".into());
    d.directional[0].options.push("name=sun".into());
    // A surface of room_a keeps off the lamp and the flashlight, the hallway (sector 1)
    // the sun; a template keeps off the lamp, and an entity placing it says none.
    let surface = d.sector_surfaces(0).find(|&i| d.surfaces[i].adjoin.is_none()).unwrap();
    d.surfaces[surface].options.push("exclude_lights=lamp,flashlight".into());
    d.sectors[1].options.push("exclude_lights=sun".into());
    d.templates.push(TemplateDoc { name: "boxed".into(), model: "crate.obj".into(), options: vec!["exclude_lights=lamp".into()] });
    let props: Vec<usize> = (0..d.entities.len()).filter(|&e| d.entities[e].kind == moose_assets::EntityKind::Prop).collect();
    assert!(props.len() >= 2);
    d.entities[props[0]].model = Some("boxed".into());
    d.entities[props[1]].model = Some("boxed".into());
    d.entities[props[1]].options.push("exclude_lights=none".into());
    let level = loads(&d);
    // Per polygon (solid surfaces in order), its own and its sector's.
    let polygon = (0..surface).filter(|&i| d.surfaces[i].adjoin.is_none()).count();
    assert_eq!(level.face_excluded_lights[polygon], 1 | 1 << FLASHLIGHT_ID);
    let hallway = &level.sectors[1];
    assert_eq!(hallway.excluded_lights, 1 << 1);
    assert!(hallway.polygons.clone().all(|p| level.face_excluded_lights[p as usize] == 1 << 1));
    let spawn = |e: usize| level.spawns.iter().find(|s| s.name == d.entities[e].name).unwrap().excluded_lights;
    assert_eq!((spawn(props[0]), spawn(props[1])), (1, 0));
    // A light a list names must be there; a light can't take the flashlight's name.
    let mut bad = d.clone();
    bad.sectors[1].options = vec!["exclude_lights=lantern".into()];
    let mut assets = assets();
    let err = assets.parse_level("edited.mmp", &bad.to_text()).unwrap_err().to_string();
    assert!(err.contains("no light is named 'lantern'"), "{err}");
    let mut bad = d.clone();
    bad.lights[0].options = vec!["name=flashlight".into()];
    assert!(assets.parse_level("edited.mmp", &bad.to_text()).is_err());
    // Written back, it loads the same.
    let again = moose_assets::LevelDoc::parse(std::path::Path::new("x.mmp"), &d.to_text()).unwrap();
    assert_eq!(loads(&again).face_excluded_lights, level.face_excluded_lights);
}
