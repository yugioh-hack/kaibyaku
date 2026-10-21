use serde_json::json;
use std::{env, path::PathBuf, process::ExitCode};
fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let Some(path) = args.next() else {
        return Err(
            "Usage: mdript-cli IMAGE [--card | --box L T R B] [--coarse | --center]".into(),
        );
    };
    if path == "--help" {
        println!(
            "mdript-cli IMAGE [--card | --box L T R B] [--coarse | --center]\nRead-only hash prototype; JSON output. No detection, identification or saving yet."
        );
        return Ok(());
    }
    let path = PathBuf::from(path);
    let mut card = false;
    let mut explicit = None;
    let mut coarse = false;
    let mut center = false;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--card") => card = true,
            Some("--coarse") => coarse = true,
            Some("--center") => center = true,
            Some("--box") => {
                let mut b = [0i64; 4];
                for v in &mut b {
                    *v = args
                        .next()
                        .and_then(|s| s.to_str().and_then(|s| s.parse().ok()))
                        .ok_or("--box requires four integers")?;
                }
                explicit = Some(b);
            }
            _ => return Err(format!("unknown option: {}", arg.to_string_lossy())),
        }
    }
    if card && explicit.is_some() {
        return Err("--card and --box are mutually exclusive".into());
    }
    if coarse && center {
        return Err("--coarse and --center are mutually exclusive".into());
    }
    let mut reader = image::ImageReader::open(&path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(512 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|e| e.to_string())?;
    if u64::from(decoded.width()) * u64::from(decoded.height()) > 64_000_000 {
        return Err("image exceeds 64 million pixels".into());
    }
    let rgb = decoded.to_rgb8();
    let b = explicit.unwrap_or_else(|| {
        if card {
            [0, 0, rgb.width() as i64, rgb.height() as i64]
        } else {
            mdript_core::predict_card_box(rgb.width(), rgb.height())
        }
    });
    let hash = mdript_core::artwork_hash(&rgb, b, coarse, center)?;
    println!(
        "{}",
        json!({"source":path,"box":b,"hash":hash,"mode":if coarse {"coarse"} else if center {"center"} else {"normal"}})
    );
    Ok(())
}
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", json!({"error":e}));
            ExitCode::FAILURE
        }
    }
}
