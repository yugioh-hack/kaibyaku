use mdript_cli::{is_within, resolved};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

/// ドライブ直下、ホーム自身とその親は、まとめて読める範囲にしない。
pub(crate) fn safe_directory(path: &Path, home: Option<&Path>) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let path = resolved(path).ok()?;
    path.parent()?;
    if let Some(home) = home {
        let home = resolved(home).ok()?;
        if is_within(&home, &path) {
            return None;
        }
    }
    Some(path)
}

pub(crate) fn allow_input_images(
    scope: &tauri::scope::fs::Scope,
    paths: &[PathBuf],
    home: Option<&Path>,
) -> Result<(), String> {
    let mut directories = HashSet::new();
    let mut parents = HashMap::new();
    // 同じ入力フォルダの許可は1回にまとめる。別のサブフォルダまでは許可しない。
    for path in paths {
        // ファイル自身がシンボリックリンクなら、リンク先のフォルダまで広げず1枚だけ許可する。
        let linked = std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
        let directory = if linked {
            None
        } else {
            path.parent().and_then(|parent| {
                parents
                    .entry(parent.to_path_buf())
                    .or_insert_with(|| safe_directory(parent, home))
                    .clone()
            })
        };
        let allowed = if let Some(directory) = directory {
            if !directories.insert(directory.clone()) {
                continue;
            }
            scope.allow_directory(directory, false)
        } else {
            scope.allow_file(path)
        };
        allowed.map_err(|e| format!("画像を表示できませんでした（{e}）"))?;
    }
    Ok(())
}

pub(crate) fn allow_output_images(
    scope: &tauri::scope::fs::Scope,
    output: &Path,
    home: Option<&Path>,
) -> Result<(), String> {
    let directory = safe_directory(output, home)
        .ok_or("ここには保存できません。専用のフォルダを選んでください")?;
    scope
        .allow_directory(directory, true)
        .map_err(|e| format!("画像を表示できませんでした（{e}）"))
}
