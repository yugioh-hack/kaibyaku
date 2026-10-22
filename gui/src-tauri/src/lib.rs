//! 画面（Vue）から呼ぶコマンド。処理そのものは mdript-cli のライブラリをそのまま使う。
//!
//! 試作版のユーザーデータは %APPDATA%\mdript-prototype に置き、現行版の %APPDATA%\mdript には触れない。
use mdript_cli::{learn, process, updater, updater_hash};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
use tauri::{AppHandle, Emitter, State};

const USER_DIR_NAME: &str = "mdript-prototype";
const CANCELLED: &str = "更新を中断しました";

#[derive(Default)]
struct Busy {
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
}

/// 処理中の印。処理が終わる（panic を含む）と必ず外れる。
struct RunningGuard<'a>(&'a AtomicBool);
impl Drop for RunningGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

fn user_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("APPDATA").ok_or("APPDATA が見つかりません")?;
    let dir = PathBuf::from(base).join(USER_DIR_NAME);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn default_output() -> String {
    std::env::var_os("USERPROFILE")
        .map(|home| PathBuf::from(home).join("Pictures").join("mdript"))
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Python 版 catalog.load_config と同じく、既知の項目を正しい型のときだけ残す。
fn filter_config(value: &Value) -> Map<String, Value> {
    const STRINGS: [&str; 5] = [
        "output_dir",
        "resolution",
        "save_format",
        "english",
        "split",
    ];
    const BOOLS: [&str; 6] = [
        "rounded",
        "recursive",
        "skip_duplicate",
        "save_review",
        "save_not_card",
        "asked_names",
    ];
    let Some(object) = value.as_object() else {
        return Map::new();
    };
    object
        .iter()
        .filter(|(key, value)| {
            (STRINGS.contains(&key.as_str()) && value.is_string())
                || (BOOLS.contains(&key.as_str()) && value.is_boolean())
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn read_config(dir: &Path) -> Map<String, Value> {
    let value: Value = fs::read_to_string(dir.join("config.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    filter_config(&value)
}

fn write_config(dir: &Path, config: &Map<String, Value>) -> Result<(), String> {
    let path = dir.join("config.json");
    let temporary = dir.join("config.json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(config).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

#[tauri::command]
fn get_state() -> Result<Value, String> {
    let dir = user_dir()?;
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "user_data": dir.to_string_lossy(),
        "default_output": default_output(),
        "config": read_config(&dir),
        "data": learn::data_summary(&dir),
    }))
}

#[tauri::command]
fn save_config(config: Map<String, Value>) -> Result<(), String> {
    let dir = user_dir()?;
    let mut merged = read_config(&dir);
    merged.extend(filter_config(&Value::Object(config)));
    write_config(&dir, &merged)
}

#[derive(Deserialize)]
struct IdentifyArgs {
    output: String,
    save_format: String,
    rounded: bool,
    recursive: bool,
    skip_duplicate: bool,
    save_review: bool,
    save_not_card: bool,
    english: String,
    split: String,
    /// "auto" または "1920x1080" のような元の解像度
    resolution: String,
}

fn options(args: &IdentifyArgs, user_data: PathBuf) -> Result<process::Options, String> {
    let mut o = process::Options::new(PathBuf::from(args.output.trim()), user_data);
    if !["webp", "png", "both"].contains(&args.save_format.as_str())
        || !["name", "ja", "off"].contains(&args.english.as_str())
        || !["none", "en", "both"].contains(&args.split.as_str())
    {
        return Err("設定の値が正しくありません".into());
    }
    o.format = args.save_format.clone();
    o.rounded = args.rounded;
    o.recursive = args.recursive;
    o.duplicates = args.skip_duplicate;
    o.review = args.save_review;
    o.not_card = args.save_not_card;
    o.english = args.english.clone();
    o.split = args.split.clone();
    o.resolution = match args.resolution.split_once('x') {
        Some((w, h)) => {
            let w: u32 = w.parse().map_err(|_| "解像度の値が正しくありません")?;
            let h: u32 = h.parse().map_err(|_| "解像度の値が正しくありません")?;
            mdript_core::layout::Resolution::Original(w, h)
        }
        None => mdript_core::layout::Resolution::Auto,
    };
    if o.output.as_os_str().is_empty() {
        return Err("出力先フォルダを指定してください。".into());
    }
    if !o.output.is_absolute() {
        return Err("出力先フォルダは C:\\ から始まる場所を指定してください。".into());
    }
    o.validate_paths()?;
    Ok(o)
}

fn start(busy: &Busy) -> Result<(), String> {
    if busy.running.swap(true, Ordering::SeqCst) {
        return Err("いま処理中です。終わるまでお待ちください。".into());
    }
    busy.cancel.store(false, Ordering::SeqCst);
    updater::reset_cancel();
    Ok(())
}

#[tauri::command]
fn start_identify(
    app: AppHandle,
    busy: State<'_, Arc<Busy>>,
    paths: Vec<String>,
    args: IdentifyArgs,
) -> Result<(), String> {
    let o = options(&args, user_dir()?)?;
    start(&busy)?;
    let busy = Arc::clone(&busy);
    thread::spawn(move || {
        let _guard = RunningGuard(&busy.running);
        let inputs = paths.into_iter().map(PathBuf::from).collect();
        let on_result = |done: usize, total: usize, result: &Value| {
            let _ = app.emit(
                "identify-progress",
                json!({"done":done,"total":total,"result":result}),
            );
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            process::identify_paths(&o, inputs, &busy.cancel, &on_result)
        }))
        .unwrap_or_else(|_| Err("処理中に予期しないエラーが発生しました".into()));
        let payload = match outcome {
            Ok(mut summary) => {
                summary["results"] = Value::Null;
                summary
            }
            Err(error) => json!({"error": error}),
        };
        let _ = app.emit("identify-done", payload);
    });
    Ok(())
}

fn update_message(result: &Value) -> Vec<String> {
    let number = |key: &str| result[key].as_u64().unwrap_or(0);
    let mut message = Vec::new();
    if number("seeded") > 0 {
        message.push(format!(
            "収録シリーズ {} 個を記録しました（同梱ハッシュでカバー済み）",
            number("seeded")
        ));
    }
    if number("new_cards") > 0 || number("refreshed") > 0 {
        let names: Vec<_> = result["pack_names"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .take(3)
            .collect();
        let packs = if names.is_empty() {
            format!("{} 個", number("packs"))
        } else {
            names.join("、")
        };
        let failed = result["failed"].as_array().map_or(0, Vec::len);
        message.push(format!(
            "新しいカード {} 件 / 再録カードの確認 {} 件 / イラスト {} 枚を追加（{packs}）{}",
            number("new_cards"),
            number("refreshed"),
            number("new_hashes"),
            if failed > 0 {
                format!(" / 取得できなかったもの {failed} 件")
            } else {
                String::new()
            }
        ));
    } else if number("seeded") == 0 {
        message.push("新しいカードセットはありませんでした".into());
    }
    if result["pack_list_failed"] == true {
        message.push(
            "収録シリーズの一覧を取得できなかったため、再録カードの確認は省略しました".into(),
        );
    }
    if result["skipped_packs"]
        .as_array()
        .is_some_and(|packs| !packs.is_empty())
    {
        message.push("収録数が多いシリーズがあったため、再録カードの確認は省略しました".into());
    }
    message
}

/// Python 版と同じく、カード名を取得してから（hashes なら続けて）新しいカードセットを取り込む。
#[tauri::command]
fn start_update(app: AppHandle, busy: State<'_, Arc<Busy>>, hashes: bool) -> Result<(), String> {
    let dir = user_dir()?;
    start(&busy)?;
    let busy = Arc::clone(&busy);
    thread::spawn(move || {
        let _guard = RunningGuard(&busy.running);
        let progress_app = app.clone();
        updater::set_progress(Some(Box::new(move |message: &str| {
            let _ = progress_app.emit("update-progress", message);
        })));
        let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<String, String> {
            let count = updater::update_names(&dir)?;
            let mut message = vec![format!("カード名 {count} 件を取得しました")];
            if hashes {
                let result = updater_hash::update_new_sets_in(&dir, 4000)?;
                message.extend(update_message(&result));
            }
            Ok(message.join(" / "))
        }))
        .unwrap_or_else(|_| Err("更新中に予期しないエラーが発生しました".into()));
        updater::set_progress(None);
        let payload = match outcome {
            Ok(message) => json!({"message": message}),
            Err(error) if error == CANCELLED => json!({"message": "中止しました"}),
            Err(error) => json!({"error": error}),
        };
        let _ = app.emit("update-done", payload);
    });
    Ok(())
}

#[tauri::command]
fn stop(busy: State<'_, Arc<Busy>>) {
    busy.cancel.store(true, Ordering::SeqCst);
    updater::request_cancel();
}

#[tauri::command]
fn data_summary() -> Result<Value, String> {
    Ok(learn::data_summary(&user_dir()?))
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    let path = PathBuf::from(path.trim());
    if !path.is_absolute() {
        return Err("出力先フォルダは C:\\ から始まる場所を指定してください。".into());
    }
    fs::create_dir_all(&path).map_err(|e| format!("出力先を開けませんでした。\n{e}"))?;
    std::process::Command::new("explorer.exe")
        .arg(&path)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("出力先を開けませんでした。\n{e}"))
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(Busy::default()))
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_config,
            start_identify,
            start_update,
            stop,
            data_summary,
            open_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_keeps_only_known_typed_keys() {
        let dir = std::env::temp_dir().join(format!("mdript-gui-config-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.json"),
            r#"{"output_dir":"C:\\out","rounded":"yes","recursive":false,"unknown":1}"#,
        )
        .unwrap();
        let config = read_config(&dir);
        assert_eq!(config.len(), 2);
        assert_eq!(config["output_dir"], "C:\\out");
        assert_eq!(config["recursive"], false);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn update_message_matches_python_wording() {
        let seeded = json!({"seeded":3,"new_cards":0,"refreshed":0,"new_hashes":0,"packs":0,
            "pack_names":[],"failed":[],"pack_list_failed":false,"skipped_packs":[]});
        assert_eq!(
            update_message(&seeded),
            vec!["収録シリーズ 3 個を記録しました（同梱ハッシュでカバー済み）"]
        );
        let none = json!({"seeded":0,"new_cards":0,"refreshed":0,"new_hashes":0,"packs":0,
            "pack_names":[],"failed":[],"pack_list_failed":false,"skipped_packs":[]});
        assert_eq!(
            update_message(&none),
            vec!["新しいカードセットはありませんでした"]
        );
    }
}
