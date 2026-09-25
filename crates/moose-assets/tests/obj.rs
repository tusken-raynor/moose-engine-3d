use std::path::Path;

use moose_assets::glam::Vec3;
use moose_assets::{AttribData, Mesh, parse_obj};

fn parse(src: &str) -> Result<Mesh, String> {
    parse_obj(Path::new("test.obj"), "test.obj", src).map_err(|e| e.to_string())
}

#[test]
fn keeps_ngons_and_shares_positions() {
    // A pentagon and a quad sharing an edge, both facing +Z.
    let m = parse(
        "v 0 0 0\nv 2 0 0\nv 3 1 0\nv 1 2 0\nv -1 1 0\nv 2 -2 0\nv 0 -2 0\n\
         f 1 2 3 4 5\nf 7 6 2 1\n",
    )
    .unwrap();
    assert_eq!(m.polygons.len(), 2);
    assert_eq!(
        (m.polygons[0].vertex_count, m.polygons[1].vertex_count),
        (5, 4)
    );
    assert_eq!(m.positions.len(), 7);
    assert_eq!(m.vertex_positions, [0, 1, 2, 3, 4, 6, 5, 1, 0]); // file order
    assert!(m.attribs.is_empty());
    assert!((m.polygons[0].plane.normal - Vec3::Z).length() < 1e-6);
}

#[test]
fn attributes_follow_polygon_vertices() {
    let m = parse(
        "v 0 0 0 1 0 0\nv 1 0 0 0 1 0\nv 0 1 0 0 0 1\n\
         vt 0 0\nvt 1 0\nvt 0 1\nvn 0 0 1\n\
         f 1/1/1 2/2/1 3/3/1\n",
    )
    .unwrap();
    let names: Vec<_> = m.attribs.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["color", "uv", "normal"]);
    assert_eq!(
        m.attrib("color").unwrap().data,
        AttribData::U8(vec![255, 0, 0, 0, 255, 0, 0, 0, 255])
    );
    assert_eq!(
        m.attrib("uv").unwrap().data,
        AttribData::F32(vec![0., 0., 1., 0., 0., 1.])
    );
    assert_eq!(m.attrib("normal").unwrap().vertex_count(), 3);
}

#[test]
fn coincident_vertices_stay_separate() {
    // Two triangles meeting at the same coordinates through different 'v' lines.
    let m = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 0 0\nv 1 1 0\nf 1 2 3\nf 4 5 3\n").unwrap();
    assert_eq!(m.positions.len(), 5);
    assert_eq!(m.vertex_positions, [0, 1, 2, 3, 4, 2]);
    assert_eq!(m.positions[1], m.positions[3]);
}

#[test]
fn unused_positions_are_dropped() {
    // 'v' 1 and 4 are never used; the rest keep their order and indices are remapped.
    let m = parse("v 9 9 9\nv 0 0 0\nv 1 0 0\nv 8 8 8\nv 0 1 0\nf 2 3 5\n").unwrap();
    assert_eq!(m.positions, [Vec3::ZERO, Vec3::X, Vec3::Y]);
    assert_eq!(m.vertex_positions, [0, 1, 2]);
}

#[test]
fn negative_indices_are_relative() {
    let m = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n").unwrap();
    assert_eq!(m.vertex_positions, [0, 1, 2]);
}

#[test]
fn rejects_bad_input() {
    let err = |src: &str, expected: &str| {
        let e = parse(src).unwrap_err();
        assert!(e.contains(expected), "expected '{expected}' in: {e}");
    };
    let tri = "v 0 0 0\nv 1 0 0\nv 0 1 0\n";
    err("v 0 0 0 1 0 0\nv 1 0 0\n", "some vertices have colors");
    err("v 0 0 0 1.5 0 0\n", "outside 0-1");
    err(
        &format!("{tri}f 1 2 4\n"),
        "position index 4 is out of range",
    );
    err(
        &format!("{tri}f 1 2 0\n"),
        "position index 0 is out of range",
    );
    err(&format!("{tri}vt 0 0\nf 1/1 2/1 3\n"), "same form");
    err(&format!("{tri}f 1 2\n"), "at least 3");
    err(&format!("{tri}l 1 2\n"), "unsupported statement 'l'");
    err(tri, "no faces");
    err(
        "v 0 0 0\nv 2 0 0\nv 1 0.5 0\nv 2 2 0\nv 0 2 0\nf 1 2 3 4 5\n",
        "reflex",
    );
    err(
        "v 0 0 0\nv 1 0 0\nv 1 1 1\nv 0 1 0\nf 1 2 3 4\n",
        "not planar",
    );
    // Errors carry the line number.
    err(&format!("{tri}\n\nf 1 2 9\n"), "test.obj:6:");
}
