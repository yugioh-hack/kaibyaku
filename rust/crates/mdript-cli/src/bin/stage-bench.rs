use serde_json::{Value, json};
use std::{fs, hint::black_box, time::Instant};
fn measure<T>(mut f: impl FnMut() -> T) -> (Vec<f64>, T) {
    black_box(f());
    let mut times = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        let v = f();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        black_box(v);
    }
    (times, f())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = std::env::args().nth(1).ok_or("manifest required")?;
    let cases: Value = serde_json::from_slice(&fs::read(manifest)?)?;
    let mut rows = Vec::new();
    for c in cases.as_array().ok_or("array required")? {
        let original = fs::read(c["source"].as_str().ok_or("source")?)?;
        let png = fs::read(c["png"].as_str().ok_or("png")?)?;
        let rgb = image::load_from_memory(&png)?.to_rgb8();
        let rgba = image::open(c["rgba"].as_str().ok_or("rgba")?)?.to_rgba8();
        let b = mdript_core::predict_card_box(rgb.width(), rgb.height());
        let (decode, _) = measure(|| {
            image::load_from_memory(black_box(&original))
                .unwrap()
                .to_rgb8()
        });
        let (png_decode, _) =
            measure(|| image::load_from_memory(black_box(&png)).unwrap().to_rgb8());
        let (hash, h) =
            measure(|| mdript_core::artwork_hash(black_box(&rgb), b, false, false).unwrap());
        let mut cfg = webp::WebPConfig::new().map_err(|_| "config")?;
        cfg.quality = 90.0;
        cfg.method = 4;
        let (encode, encoded) = measure(|| {
            webp::Encoder::from_rgba(black_box(rgba.as_raw()), rgba.width(), rgba.height())
                .encode_advanced(&cfg)
                .unwrap()
        });
        fs::write(c["rust_output"].as_str().ok_or("output")?, &*encoded)?;
        rows.push(json!({"id":c["id"],"decode_rgb":decode,"png_decode_rgb":png_decode,"phash_128":hash,"webp_encode":encode,"hash":h,"bytes":encoded.len()}));
    }
    println!("{}", serde_json::to_string_pretty(&rows)?);
    Ok(())
}
