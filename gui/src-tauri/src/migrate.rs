//! 旧アプリ（Python 版）のユーザーデータを、新アプリの保存先へ一度だけコピーする。
//!
//! 旧アプリのフォルダ（%APPDATA%\mdript）は読むだけで、変更も削除もしない。旧アプリへはそのまま戻れる。
//! 新アプリ側にすでにあるファイルは上書きしない。何度実行しても結果は同じ。
use serde_json::{Value, json};
use std::{fs, path::Path};

const FILES: [&str; 3] = ["config.json", "card_names.json", "card_hashes_user.json"];
const MARKER: &str = "migration.json";
const BACKUP_DIR: &str = "legacy-backup";

#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// 新アプリの保存先へコピーしたファイル
    pub copied: Vec<String>,
    /// JSON として読めず、取り込まなかったファイル（原本は legacy-backup に残す）
    pub corrupt: Vec<String>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.copied.is_empty() && self.corrupt.is_empty()
    }

    pub fn to_json(&self) -> Value {
        json!({"copied": self.copied, "corrupt": self.corrupt})
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}

/// 取り込む前の確認。JSON のオブジェクトで、カード名ならさらに `cards` があること。
fn is_valid(name: &str, bytes: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    match name {
        "card_names.json" => value["cards"].is_object(),
        _ => value.is_object(),
    }
}

/// `legacy` の対象ファイルを `target` へコピーする。完了したら `migration.json` を残し、以後は何もしない。
/// 途中で止まったときは印がないので、次回の起動で残りだけをやり直す。
pub fn migrate(
    legacy: &Path,
    target: &Path,
    filter_config: impl Fn(&Value) -> Value,
) -> Result<Report, String> {
    let mut report = Report::default();
    if target.join(MARKER).exists() || !legacy.is_dir() {
        return Ok(report);
    }
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    for name in FILES {
        let source = legacy.join(name);
        let destination = target.join(name);
        // 読めないもの（無い・権限なし）は取り込まない。新アプリのデータは上書きしない
        let Ok(bytes) = fs::read(&source) else {
            continue;
        };
        if destination.exists() {
            continue;
        }
        // 原本をそのまま控えておく（壊れたファイルも）
        let backup = target.join(BACKUP_DIR);
        fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
        write_atomic(&backup.join(name), &bytes)?;
        if !is_valid(name, &bytes) {
            report.corrupt.push(name.into());
            continue;
        }
        if name == "config.json" {
            let config = filter_config(&serde_json::from_slice(&bytes).map_err(|e| e.to_string())?);
            write_atomic(
                &destination,
                &serde_json::to_vec(&config).map_err(|e| e.to_string())?,
            )?;
        } else {
            write_atomic(&destination, &bytes)?;
        }
        report.copied.push(name.into());
    }
    let marker = json!({
        "version": 1,
        "from": legacy.to_string_lossy(),
        "copied": report.copied,
        "corrupt": report.corrupt,
    });
    write_atomic(
        &target.join(MARKER),
        &serde_json::to_vec_pretty(&marker).map_err(|e| e.to_string())?,
    )?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mdript-migrate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn keep(value: &Value) -> Value {
        value.clone()
    }
    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (
                    e.file_name().to_string_lossy().into_owned(),
                    fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn copies_valid_files_and_leaves_legacy_untouched() {
        let root = temp("copy");
        let (legacy, target) = (root.join("mdript"), root.join("mdript-v2"));
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("config.json"), br#"{"output_dir":"C:\\out"}"#).unwrap();
        fs::write(
            legacy.join("card_names.json"),
            br#"{"cards":{"1":{"n":"a"}}}"#,
        )
        .unwrap();
        fs::write(legacy.join("card_hashes_user.json"), br#"{"1":["00"]}"#).unwrap();
        let before = snapshot(&legacy);
        let report = migrate(&legacy, &target, keep).unwrap();
        assert_eq!(report.copied.len(), 3);
        assert!(report.corrupt.is_empty());
        assert_eq!(snapshot(&legacy), before);
        assert_eq!(
            fs::read(target.join("card_hashes_user.json")).unwrap(),
            br#"{"1":["00"]}"#
        );
        assert!(target.join("legacy-backup/card_names.json").exists());
        assert!(target.join(MARKER).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_files_are_backed_up_but_not_imported() {
        let root = temp("corrupt");
        let (legacy, target) = (root.join("mdript"), root.join("mdript-v2"));
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("card_names.json"), b"{broken").unwrap();
        fs::write(legacy.join("card_hashes_user.json"), b"[1,2]").unwrap();
        fs::write(legacy.join("config.json"), br#"{"rounded":false}"#).unwrap();
        let report = migrate(&legacy, &target, keep).unwrap();
        assert_eq!(report.corrupt, ["card_names.json", "card_hashes_user.json"]);
        assert_eq!(report.copied, ["config.json"]);
        assert!(!target.join("card_names.json").exists());
        assert_eq!(
            fs::read(target.join("legacy-backup/card_names.json")).unwrap(),
            b"{broken"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rerun_does_nothing_and_never_overwrites_new_data() {
        let root = temp("rerun");
        let (legacy, target) = (root.join("mdript"), root.join("mdript-v2"));
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(legacy.join("card_names.json"), br#"{"cards":{"1":{}}}"#).unwrap();
        fs::write(legacy.join("card_hashes_user.json"), br#"{"1":["00"]}"#).unwrap();
        // 新アプリ側にすでにあるデータは残す
        fs::write(target.join("card_hashes_user.json"), br#"{"2":["ff"]}"#).unwrap();
        let first = migrate(&legacy, &target, keep).unwrap();
        assert_eq!(first.copied, ["card_names.json"]);
        assert_eq!(
            fs::read(target.join("card_hashes_user.json")).unwrap(),
            br#"{"2":["ff"]}"#
        );
        // 旧データが後から変わっても、完了後は取り込まない
        fs::write(legacy.join("config.json"), br#"{"rounded":false}"#).unwrap();
        let second = migrate(&legacy, &target, keep).unwrap();
        assert!(second.is_empty());
        assert!(!target.join("config.json").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_run_resumes_with_the_remaining_files() {
        let root = temp("resume");
        let (legacy, target) = (root.join("mdript"), root.join("mdript-v2"));
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(legacy.join("config.json"), br#"{"rounded":false}"#).unwrap();
        fs::write(legacy.join("card_names.json"), br#"{"cards":{"1":{}}}"#).unwrap();
        // 前回は config.json まで進んで止まった（印はない）
        fs::write(target.join("config.json"), br#"{"rounded":false}"#).unwrap();
        fs::write(target.join("card_names.json.tmp"), b"half").unwrap();
        let report = migrate(&legacy, &target, keep).unwrap();
        assert_eq!(report.copied, ["card_names.json"]);
        assert!(target.join(MARKER).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_legacy_folder_is_not_an_error_and_leaves_no_marker() {
        let root = temp("none");
        let report = migrate(&root.join("mdript"), &root.join("mdript-v2"), keep).unwrap();
        assert!(report.is_empty());
        assert!(!root.join("mdript-v2").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
