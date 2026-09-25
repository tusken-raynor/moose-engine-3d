//! Loads a level and prints what ended up in memory.
//!
//! cargo run -p moose-assets --example inspect [level.mmp]

use moose_assets::Assets;

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "two_rooms.mmp".into());
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let level = match assets.load_level(&name) {
        Ok(level) => level,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    println!("level \"{}\"", level.name);
    for mesh in assets.meshes() {
        let attribs: Vec<String> = mesh
            .attribs
            .iter()
            .map(|a| format!("{}: {} x{}", a.name, a.format().name(), a.count))
            .collect();
        println!(
            "  mesh {:<14} {:>3} positions  {:>3} polygon vertices  {:>3} polygons  [{}]",
            mesh.name,
            mesh.positions.len(),
            mesh.vertex_positions.len(),
            mesh.polygons.len(),
            attribs.join(", ")
        );
    }
    for (i, s) in level.sectors.iter().enumerate() {
        println!(
            "  sector {i} {:<8} polygons {:?}  portals {:?}  bounds {} .. {}",
            s.name, s.polygons, s.portals, s.bounds.min, s.bounds.max
        );
    }
    for (i, p) in level.portals.iter().enumerate() {
        println!(
            "  portal {i}: sector {} -> {} (mirror {}), {} points",
            p.sector,
            p.target,
            p.mirror,
            p.positions.len()
        );
    }
    for s in &level.spawns {
        let mesh = s
            .mesh
            .map_or("-".to_string(), |id| assets.mesh(id).name.clone());
        println!(
            "  {:?} {:<16} sector {}  at {}  model {}",
            s.kind, s.name, s.sector, s.position, mesh
        );
    }
}
