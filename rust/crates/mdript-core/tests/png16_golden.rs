use std::path::Path;

// 100x150 PNG fixtures were generated as 16-bit PNG with Pillow 12.2.
// Expected hashes are artwork_phash_hex(Image.open(path).convert("RGB")).
#[test]
fn sixteen_bit_png_matches_pillow_rgb_conversion() {
    let cases = [
        ("0_low.png", "80aaff5555d5a682ff5511aa55aa7d00"),
        ("0_full.png", "c155d5ffaaa280aaed92e01f6de0921e"),
        ("4_low.png", "00000000000000000000000000000000"),
        ("4_full.png", "80aaaa7faa81b2cfeeff557529790001"),
        ("2_low.png", "00000000000000000000000000000000"),
        ("2_full.png", "80fffe45aa0183d5e7ff55aa2b502221"),
        ("6_low.png", "00000000000000000000000000000000"),
        ("6_full.png", "80fffe45aa0183d5e7ff55aa2b502221"),
    ];
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for (name, expected) in cases {
        let rgb = mdript_core::decode::load_rgb(&fixtures.join(name)).unwrap();
        assert_eq!(rgb.dimensions(), (100, 150), "{name}");
        let actual = mdript_core::artwork_hash(&rgb, [0, 0, 100, 150], false, false).unwrap();
        assert_eq!(actual, expected, "{name}");
    }
}

#[test]
fn grayscale_16bit_clamps_instead_of_scaling() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let low = mdript_core::decode::load_rgb(&fixtures.join("0_low.png")).unwrap();
    let full = mdript_core::decode::load_rgb(&fixtures.join("0_full.png")).unwrap();
    assert_eq!(low.get_pixel(1, 0).0, [17, 17, 17]);
    assert_eq!(full.get_pixel(1, 0).0, [255, 255, 255]);
}
