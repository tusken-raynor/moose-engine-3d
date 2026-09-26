use moose_assets::{Assets, Texture};

#[test]
fn the_test_floor_texture_loads() {
    let mut assets = Assets::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets"));
    let id = assets.load_texture("test_floor.png").unwrap();
    assert_eq!(
        assets.load_texture("test_floor.png").unwrap(),
        id,
        "loaded once"
    );
    let t = assets.texture(id);
    let base = t.base();
    assert_eq!(
        (base.width, base.height, base.width_log2, base.height_log2),
        (64, 64, 6, 6)
    );
    // Roughness in alpha: 128 (shiny) to 255 (rough); the grout border is fully rough.
    let alphas = base.texels.iter().map(|&c| c >> 24);
    assert!(alphas.clone().all(|a| (128..=255).contains(&a)));
    assert_eq!(t.texel(0, 10) >> 24, 255);
    assert!(alphas.clone().any(|a| a < 140), "some of the slab is shiny");
    // Wrapping.
    assert_eq!(t.texel(64 + 5, 128 + 7), t.texel(5, 7));
}

#[test]
fn textures_must_tile() {
    assert!(Texture::new("t", 64, 48, vec![0; 64 * 48]).is_err());
    assert!(Texture::new("t", 64, 64, vec![0; 10]).is_err());
    let t = Texture::new("t", 4, 2, (0..8).collect()).unwrap();
    assert_eq!(t.texel(5, 3), 5);
}

#[test]
fn mip_chains_halve_down_to_one_texel() {
    let t = Texture::new("t", 64, 64, vec![0; 64 * 64]).unwrap();
    let sizes: Vec<(u32, u32)> = t.levels.iter().map(|l| (l.width, l.height)).collect();
    assert_eq!(
        sizes,
        [(64, 64), (32, 32), (16, 16), (8, 8), (4, 4), (2, 2), (1, 1)]
    );
    // Not square: the short side stops at 1 while the long side keeps halving.
    let t = Texture::new("t", 8, 2, vec![0; 16]).unwrap();
    let sizes: Vec<(u32, u32)> = t.levels.iter().map(|l| (l.width, l.height)).collect();
    assert_eq!(sizes, [(8, 2), (4, 1), (2, 1), (1, 1)]);
}

#[test]
fn mip_levels_average_every_channel() {
    // A 2x2 texture whose 1x1 level is the rounded average of each channel.
    let t = Texture::new(
        "t",
        2,
        2,
        vec![0x80_00_10_FF, 0xFF_00_20_00, 0x80_FF_30_00, 0xFF_FF_41_01],
    )
    .unwrap();
    assert_eq!(t.levels[1].texels, [0xC0_80_28_40]);
    // 2x1 averages along the long side only.
    let t = Texture::new("t", 2, 1, vec![0x00_00_00_00, 0x02_04_06_09]).unwrap();
    assert_eq!(t.levels[1].texels, [0x01_02_03_05]);
}

#[test]
fn cube_maps_stack_six_faces_and_halve_each_on_its_own() {
    // Face f is all f * 0x10; each level keeps the faces apart.
    let faces: [Vec<u32>; 6] = std::array::from_fn(|f| vec![f as u32 * 0x10; 16]);
    let t = Texture::cube("c", 4, faces).unwrap();
    let sizes: Vec<(u32, u32)> = t.levels.iter().map(|l| (l.width, l.height)).collect();
    assert_eq!(sizes, [(4, 24), (2, 12), (1, 6)]);
    for level in &t.levels {
        let face_texels = (level.width * level.width) as usize;
        for (i, &texel) in level.texels.iter().enumerate() {
            assert_eq!(texel, (i / face_texels) as u32 * 0x10);
        }
    }
    assert!(Texture::cube("c", 3, std::array::from_fn(|_| vec![0; 9])).is_err());
}
