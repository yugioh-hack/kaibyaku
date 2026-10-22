use serde_json::Value;
use std::{fs, path::Path, process::Command};

fn run(input: &Path, output: &Path, user: &Path, flags: &[&str]) -> Value {
    let response = Command::new(env!("CARGO_BIN_EXE_mdript-cli"))
        .arg("identify")
        .arg(input)
        .arg("--output")
        .arg(output)
        .arg("--user-data")
        .arg(user)
        .arg("--card")
        .args(flags)
        .output()
        .unwrap();
    assert!(
        response.status.success(),
        "{}",
        String::from_utf8_lossy(&response.stderr)
    );
    serde_json::from_slice(&response.stdout).unwrap()
}

#[test]
fn default_csv_is_written_but_no_csv_and_dry_run_leave_no_log() {
    let base = std::env::temp_dir().join(format!(
        "mdript-cli-csv-integration-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&base).unwrap();
    let input = base.join("input.png");
    image::RgbImage::from_fn(100, 150, |x, y| {
        image::Rgb([((x * 7 + y * 3) % 256) as u8, 60, 120])
    })
    .save(&input)
    .unwrap();
    let user = base.join("empty-user-data");

    let normal = base.join("normal");
    let result = run(&input, &normal, &user, &[]);
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    let saved_path = result["results"][0]["saved_path"].as_str().unwrap();
    assert!(!saved_path.contains(r"\\?\"));
    assert!(Path::new(saved_path).exists());
    let bytes = fs::read(normal.join("log.csv")).unwrap();
    assert!(bytes.starts_with(b"\xef\xbb\xbf"));
    let csv = String::from_utf8(bytes[3..].to_vec()).unwrap();
    assert!(csv.starts_with(
        "日時,状態,カード名,カードID,距離,2位との差,出力ファイル,備考,入力ファイル\r\n"
    ));
    assert_eq!(csv.matches("\r\n").count(), 2);
    assert!(!csv.contains(r"\\?\"));
    assert!(csv.contains(saved_path));

    let disabled = base.join("disabled");
    let result = run(&input, &disabled, &user, &["--no-csv"]);
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert!(!disabled.join("log.csv").exists());

    let dry = base.join("dry");
    let result = run(&input, &dry, &user, &["--dry-run"]);
    assert_eq!(result["results"].as_array().unwrap().len(), 1);
    assert!(!dry.exists());
    fs::remove_dir_all(base).unwrap();
}
