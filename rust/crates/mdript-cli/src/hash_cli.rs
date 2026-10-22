use serde_json::json;
use std::{env, path::PathBuf};
pub fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let Some(path) = args.next() else {
        return Err(
            "Usage: mdript-cli IMAGE [--card | --box L T R B] [--coarse | --center]".into(),
        );
    };
    if path == "--help" {
        println!(
            "mdript-cli IMAGE [--card | --box L T R B] [--coarse | --center]\nHash compatibility mode. Use mdript-cli identify --help for identification and saving."
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
    let rgb = mdript_core::decode::load_rgb(&path)?;
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
