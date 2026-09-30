//! The model tables (`ModelDoc`) the mesh editor changes: read, written back, edited.

use std::path::Path;

use moose_assets::glam::Vec3;
use moose_assets::{Assets, ModelDoc};

const WALKER: &str = include_str!("../../../assets/models/walker.mmdl");

fn assets() -> Assets {
    Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"))
}

/// The walker's tables, and an asset store with it loaded, to load edits into.
fn walker() -> (ModelDoc, Assets) {
    let mut assets = assets();
    assets.load_mesh("walker.mmdl").unwrap();
    (ModelDoc::parse(Path::new("walker.mmdl"), WALKER).unwrap(), assets)
}

fn load(assets: &mut Assets, doc: &ModelDoc) -> moose_assets::Mesh {
    assets.set_model_text("walker.mmdl", &doc.to_text()).unwrap();
    assets.mesh(assets.mesh_id("walker.mmdl").unwrap()).clone()
}

#[test]
fn a_model_written_back_loads_the_same() {
    let (doc, mut assets) = walker();
    let before = assets.mesh(assets.mesh_id("walker.mmdl").unwrap()).clone();
    assert_eq!(load(&mut assets, &doc), before);
    let again = ModelDoc::parse(Path::new("walker.mmdl"), &doc.to_text()).unwrap();
    assert_eq!(again, doc);
}

#[test]
fn proxies_come_off_and_go_back_on_per_bone() {
    let (mut doc, mut assets) = walker();
    let proxies = |doc: &ModelDoc| doc.polygons.iter().filter(|p| p.proxy()).count();
    assert_eq!(proxies(&doc), 42, "a box per bone");
    // The left leg's (bone 3) proxy off: its six faces, and its eight positions.
    let positions = doc.positions.len();
    assert_eq!(doc.remove_proxies(Some(3)), 6);
    assert_eq!(doc.positions.len(), positions - 8);
    let mesh = load(&mut assets, &doc);
    assert_eq!(mesh.polygons.iter().filter(|p| p.flags.proxy()).count(), 36);
    // A new box around the leg's drawn polygons: the leg's box itself.
    let first = doc.add_proxy_box(Some(3)).unwrap();
    assert_eq!(doc.polygon_bone(first), Some(3));
    let mesh = load(&mut assets, &doc);
    let skin = mesh.skin.as_ref().unwrap();
    let corners: Vec<Vec3> = mesh.polygons[first..first + 6]
        .iter()
        .flat_map(|p| mesh.polygon_points(p))
        .collect();
    assert!(corners.iter().all(|p| (-0.24..=-0.06).contains(&p.x) && (0.0..=0.88).contains(&p.y)));
    let bones: Vec<u16> = mesh.polygons[first..first + 6]
        .iter()
        .flat_map(|p| mesh.vertex_positions[p.vertices()].iter().map(|&i| skin.position_bones[i as usize]))
        .collect();
    assert!(bones.iter().all(|&b| b == 3));
    // All proxies off: the model loads without them.
    doc.remove_proxies(None);
    assert!(!load(&mut assets, &doc).polygons.iter().any(|p| p.flags.proxy()));
}

#[test]
fn a_polygon_takes_a_color_of_its_own() {
    let (mut doc, mut assets) = walker();
    // A new row for the first face; the others keep theirs.
    let others: Vec<_> = doc.polygons[1..].to_vec();
    let rows = doc.attributes[0].values.len();
    doc.set_polygon_color(0, Vec3::new(1.0, 0.0, 0.0)).unwrap();
    assert_eq!(doc.polygon_color(0), Some(Vec3::new(1.0, 0.0, 0.0)));
    assert_eq!(doc.attributes[0].values.len(), rows + 1);
    assert_eq!(doc.polygons[1..], others[..]);
    load(&mut assets, &doc);
    // A polygon's removal leaves the model loading.
    let n = doc.polygons.len();
    doc.remove_polygon(0);
    assert_eq!(load(&mut assets, &doc).polygons.len(), n - 1);
}
