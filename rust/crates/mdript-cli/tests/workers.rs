use serde_json::Value;
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn files(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files(root, &path, out);
        } else if path.extension().is_some_and(|v| v == "png") {
            out.insert(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                fs::read(path).unwrap(),
            );
        }
    }
}

fn run(
    input: &Path,
    output: &Path,
    user: &Path,
    workers: usize,
) -> (Value, BTreeMap<String, Vec<u8>>, String) {
    let response = Command::new(env!("CARGO_BIN_EXE_mdript-cli"))
        .arg("identify")
        .arg(input)
        .arg("--output")
        .arg(output)
        .arg("--user-data")
        .arg(user)
        .args(["--card", "--format", "png", "--no-rounded", "--workers"])
        .arg(workers.to_string())
        .output()
        .unwrap();
    assert!(
        response.status.success(),
        "{}",
        String::from_utf8_lossy(&response.stderr)
    );
    let mut summary: Value = serde_json::from_slice(&response.stdout).unwrap();
    summary.as_object_mut().unwrap().remove("seconds");
    for row in summary["results"].as_array_mut().unwrap() {
        let saved = row["saved_path"].as_str().unwrap();
        if !saved.is_empty() {
            let relative = Path::new(saved).strip_prefix(output).unwrap();
            row["saved_path"] = Value::String(relative.to_string_lossy().into_owned());
        }
    }
    let mut images = BTreeMap::new();
    files(output, output, &mut images);
    let csv = fs::read_to_string(output.join("log.csv")).unwrap();
    let normalized = csv
        .lines()
        .map(|line| {
            let line = line.replace(&output.to_string_lossy().to_string(), "<output>");
            if line.starts_with("日時,") {
                line
            } else {
                format!("<time>,{}", line.split_once(',').unwrap().1)
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (summary, images, normalized)
}

#[test]
fn one_and_eight_workers_produce_same_order_names_and_files() {
    let base = std::env::temp_dir().join(format!(
        "mdript-worker-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let input = base.join("input");
    fs::create_dir_all(&input).unwrap();
    for (i, dir) in ["a", "b", "c"].iter().enumerate() {
        let folder = input.join(dir);
        fs::create_dir(&folder).unwrap();
        image::RgbImage::from_pixel(30, 40, image::Rgb([i as u8 * 50, 70, 90]))
            .save(folder.join("same.png"))
            .unwrap();
    }
    let image = image::RgbImage::from_fn(100, 150, |x, y| {
        image::Rgb([
            ((x * 17 + y * 3) % 256) as u8,
            ((x * 7 + y * 19) % 256) as u8,
            ((x * 11 + y * 13) % 256) as u8,
        ])
    });
    for dir in ["d", "e"] {
        let folder = input.join(dir);
        fs::create_dir(&folder).unwrap();
        image.save(folder.join("valid.png")).unwrap();
    }
    let user = base.join("empty-user-data");
    let baseline = run(&input, &base.join("output-one"), &user, 1);
    assert!(baseline.0["skipped"].as_u64().unwrap() >= 1);
    assert!(baseline.1.keys().any(|name| name.contains("_alt")));
    for i in 0..3 {
        let parallel = run(&input, &base.join(format!("output-eight-{i}")), &user, 8);
        assert_eq!(parallel.0, baseline.0);
        assert_eq!(parallel.1, baseline.1);
        assert_eq!(parallel.2, baseline.2);
    }
    fs::remove_dir_all(base).unwrap();
}
