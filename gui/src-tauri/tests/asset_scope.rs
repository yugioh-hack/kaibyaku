#[path = "../src/image_scope.rs"]
mod image_scope;
use image_scope::{allow_input_images, allow_output_images, safe_directory};
use mdript_cli::process;
use std::fs;

fn empty_scope() -> tauri::scope::fs::Scope {
    let app = tauri::test::mock_app();
    tauri::scope::fs::Scope::new(&app, &tauri::utils::config::FsScope::AllowedPaths(vec![]))
        .unwrap()
}

#[test]
fn scope_groups_safe_folders_and_keeps_unvisited_subfolders_private() {
    let root = std::env::temp_dir().join(format!("kaibyaku-scope-{}", std::process::id()));
    fs::create_dir_all(root.join("input/nested")).unwrap();
    for name in ["input/one.png", "input/private.txt", "input/nested/two.jpg"] {
        fs::write(root.join(name), b"x").unwrap();
    }
    let mut o = process::Options::new(root.join("output"), root.join("user"));
    o.recursive = false;
    let files = process::collect(vec![root.join("input")], &o).unwrap();
    assert_eq!(files, [root.join("input/one.png")]);
    let scope = empty_scope();
    allow_input_images(&scope, &files, Some(&root.join("home"))).unwrap();
    assert!(scope.is_allowed(root.join("input/one.png")));
    assert!(scope.is_allowed(root.join("input/private.txt")));
    assert!(!scope.is_allowed(root.join("input/nested/two.jpg")));
    let saved = root.join("output/detail/review/card.webp");
    fs::create_dir_all(saved.parent().unwrap()).unwrap();
    fs::write(&saved, b"x").unwrap();
    allow_output_images(&scope, &o.output, Some(&root.join("home"))).unwrap();
    assert!(scope.is_allowed(&saved));
    assert!(scope.is_allowed(root.join("output/other.webp")));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn home_ancestors_and_drive_roots_are_never_allowed_as_directories() {
    let root = std::env::temp_dir().join(format!("kaibyaku-home-scope-{}", std::process::id()));
    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    for path in [
        home.join("selected.png"),
        home.join("private.txt"),
        root.join("selected.png"),
        root.join("private.txt"),
    ] {
        fs::write(path, b"x").unwrap();
    }
    let scope = empty_scope();
    allow_input_images(
        &scope,
        &[home.join("selected.png"), root.join("selected.png")],
        Some(&home),
    )
    .unwrap();
    assert!(scope.is_allowed(home.join("selected.png")));
    assert!(scope.is_allowed(root.join("selected.png")));
    assert!(!scope.is_allowed(home.join("private.txt")));
    assert!(!scope.is_allowed(root.join("private.txt")));
    for directory in [
        &home,
        &root,
        &home.join(".."),
        root.ancestors().last().unwrap(),
    ] {
        assert!(safe_directory(directory, Some(&home)).is_none());
        assert!(allow_output_images(&scope, directory, Some(&home)).is_err());
    }
    assert!(safe_directory(&home.join("cards"), Some(&home)).is_some());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn thousands_of_images_do_not_add_thousands_of_permission_patterns() {
    let root = std::env::temp_dir().join(format!("kaibyaku-patterns-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let paths: Vec<_> = (0..5000).map(|i| root.join(format!("{i}.png"))).collect();
    let scope = empty_scope();
    allow_input_images(&scope, &paths[..1], None).unwrap();
    let patterns = scope.allowed_patterns().len();
    allow_input_images(&scope, &paths, None).unwrap();
    assert_eq!(scope.allowed_patterns().len(), patterns);
    assert!(patterns <= 4);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "大量の画像の許可を手動で計測する"]
fn measure_asset_scope_for_large_batches() {
    use std::time::Instant;
    let root = std::env::temp_dir().join(format!("kaibyaku-scope-bench-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let paths: Vec<_> = (0..5000)
        .map(|i| root.join(format!("card-{i:05}.png")))
        .collect();
    for path in &paths {
        fs::write(path, b"x").unwrap();
    }
    let scope = empty_scope();
    let started = Instant::now();
    for path in &paths {
        scope.allow_file(path).unwrap();
    }
    let registration = started.elapsed();
    let started = Instant::now();
    for path in &paths {
        assert!(scope.is_allowed(path));
    }
    let reads = started.elapsed();
    let folders = empty_scope();
    let started = Instant::now();
    folders.allow_directory(&root, true).unwrap();
    let folder_registration = started.elapsed();
    let started = Instant::now();
    for path in &paths {
        assert!(folders.is_allowed(path));
    }
    eprintln!(
        "5000件: 個別許可={registration:?}, 個別照会={reads:?}, フォルダ許可={folder_registration:?}, フォルダ照会={:?}",
        started.elapsed()
    );
    let grouped = empty_scope();
    let started = Instant::now();
    allow_input_images(&grouped, &paths, None).unwrap();
    let grouped_registration = started.elapsed();
    let started = Instant::now();
    for path in &paths {
        assert!(grouped.is_allowed(path));
    }
    eprintln!(
        "5000件: 入力フォルダ集約許可={grouped_registration:?}, 集約照会={:?}, 許可パターン数={}",
        started.elapsed(),
        grouped.allowed_patterns().len()
    );
    fs::remove_dir_all(root).unwrap();
}
