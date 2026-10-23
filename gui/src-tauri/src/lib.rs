//! 画面（Vue）から呼ぶコマンド。処理そのものは mdript-cli のライブラリをそのまま使う。
//!
//! ユーザーデータは %APPDATA%\mdript-v2 に置く。旧アプリの %APPDATA%\mdript は、初回起動時に
//! 読み取ってコピーするだけで、変更しない（migrate.rs）。
mod migrate;

use mdript_cli::{learn, process, updater, updater_hash};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::{
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};
use tauri::{AppHandle, Emitter, Manager, State};

const USER_DIR_NAME: &str = "mdript-v2";
const LEGACY_DIR_NAME: &str = "mdript";
const CANCELLED: &str = "更新を中断しました";

#[derive(Default)]
struct BusyState {
    running: bool,
    closing: bool,
}

/// 処理中かどうかと、中止の合図。終了するときは処理が終わるまで待てる。
#[derive(Default)]
struct Busy {
    state: Mutex<BusyState>,
    idle: Condvar,
    cancel: Arc<AtomicBool>,
}

/// 処理中の印。処理が終わる（panic を含む）と必ず外れ、終了待ちを起こす。
struct RunningGuard(Arc<Busy>);
impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.lock().running = false;
        self.0.idle.notify_all();
    }
}

impl Busy {
    fn lock(&self) -> MutexGuard<'_, BusyState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 処理を始める印を付ける。処理中・終了中なら始めない。
    fn begin(busy: &Arc<Busy>) -> Result<RunningGuard, String> {
        let mut state = busy.lock();
        if state.closing {
            return Err("終了しています。".into());
        }
        if state.running {
            return Err("いま処理中です。終わるまでお待ちください。".into());
        }
        state.running = true;
        busy.cancel.store(false, Ordering::SeqCst);
        updater::reset_cancel();
        Ok(RunningGuard(Arc::clone(busy)))
    }

    fn request_stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        updater::request_cancel();
    }

    /// 以降の処理開始を止め、いまの処理に中止を伝えて、終わる（保存・記録を済ませる）まで待つ。
    fn shutdown(&self) {
        let mut state = self.lock();
        state.closing = true;
        self.request_stop();
        while state.running {
            state = self.idle.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
}

fn user_dir() -> Result<PathBuf, String> {
    let base = std::env::var_os("APPDATA").ok_or("APPDATA が見つかりません")?;
    let dir = PathBuf::from(base).join(USER_DIR_NAME);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // ジャンクション等で現行版の %APPDATA%\mdript を指していたら使わない
    mdript_cli::validate_user_data(&dir)
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
    // 旧アプリのデータの取り込みは一度だけ。失敗しても起動は止めない
    let legacy = std::env::var_os("APPDATA").map(|base| PathBuf::from(base).join(LEGACY_DIR_NAME));
    let migration = match legacy {
        Some(legacy) => migrate::migrate(&legacy, &dir, |v| Value::Object(filter_config(v)))
            .map(|report| {
                if report.is_empty() {
                    Value::Null
                } else {
                    report.to_json()
                }
            })
            .unwrap_or_else(|error| json!({"error": error})),
        None => Value::Null,
    };
    Ok(json!({
        "migration": migration,
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

#[tauri::command]
fn start_identify(
    app: AppHandle,
    busy: State<'_, Arc<Busy>>,
    paths: Vec<String>,
    args: IdentifyArgs,
) -> Result<(), String> {
    let o = options(&args, user_dir()?)?;
    let guard = Busy::begin(&busy)?;
    // サムネイルと確認パネル用に、出力先と、今回入力した画像だけを画面から読めるようにする
    let scope = app.asset_protocol_scope();
    let _ = scope.allow_directory(PathBuf::from(args.output.trim()), true);
    for path in paths.iter().map(PathBuf::from) {
        let _ = if path.is_dir() {
            scope.allow_directory(&path, true)
        } else {
            scope.allow_file(&path)
        };
    }
    let busy = Arc::clone(&busy);
    thread::spawn(move || {
        // イベントを送った後に外れる（終了待ちは、結果の通知まで済んでから解ける）
        let _guard = guard;
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

/// 画像1枚だけを判定し直す（登録の後に使う）。画面の一覧はこの結果で置き換える。
#[tauri::command(async)]
fn identify_one(
    busy: State<'_, Arc<Busy>>,
    path: String,
    args: IdentifyArgs,
) -> Result<Value, String> {
    let mut o = options(&args, user_dir()?)?;
    o.workers = 1;
    let _guard = Busy::begin(&busy)?;
    let summary =
        process::identify_paths(&o, vec![PathBuf::from(path)], &busy.cancel, &|_, _, _| {})?;
    let Some(result) = summary["results"].get(0).cloned() else {
        return Err(if summary["cancelled"] == true {
            "中止しました".into()
        } else {
            "画像が見つかりませんでした".into()
        });
    };
    Ok(json!({"result": result, "csv_error": summary["csv_error"]}))
}

fn catalog(dir: &Path) -> Result<mdript_core::catalog::Catalog, String> {
    let user = fs::read_to_string(dir.join("card_hashes_user.json")).ok();
    let names = fs::read_to_string(dir.join("card_names.json")).ok();
    mdript_core::catalog::Catalog::bundled(user.as_deref(), names.as_deref())
}

/// カードIDの表示名（候補の一覧用）。
#[tauri::command(async)]
fn describe_cards(cids: Vec<String>) -> Result<Vec<Value>, String> {
    let catalog = catalog(&user_dir()?)?;
    Ok(cids
        .iter()
        .map(|cid| json!({"cid": cid, "name": catalog.display_name(cid, false)}))
        .collect())
}

/// カード名（日本語名・英語名・ルビ）で検索する。照合用ハッシュを持つカードだけを最大30件返す。
#[tauri::command(async)]
fn search_cards(query: String) -> Result<Vec<Value>, String> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Ok(vec![]);
    }
    let dir = user_dir()?;
    let known = learn::known_card_ids(&dir);
    let catalog = catalog(&dir)?;
    let mut hits: Vec<_> = catalog
        .names
        .iter()
        .filter(|(cid, _)| known.contains(*cid))
        .filter(|(cid, fields)| {
            cid.as_str() == query
                || ["n", "e", "r"].iter().any(|key| {
                    fields
                        .get(*key)
                        .is_some_and(|value| value.to_lowercase().contains(&query))
                })
        })
        .map(|(cid, _)| (cid.clone(), catalog.display_name(cid, false)))
        .collect();
    hits.sort_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| a.1.cmp(&b.1)));
    Ok(hits
        .into_iter()
        .take(30)
        .map(|(cid, name)| json!({"cid": cid, "name": name}))
        .collect())
}

/// 画像1枚のハッシュを、選んだカードの照合用として追加する（Python 版の --learn と同じ判定）。
/// reprocess が true なら、判定し直せば確定する見込みがある。
#[tauri::command(async)]
fn learn_card(busy: State<'_, Arc<Busy>>, cid: String, source: String) -> Result<Value, String> {
    // 照合データの書き込み中に終了しないよう、処理中として扱う
    let _guard = Busy::begin(&busy)?;
    let result = learn::learn_image(&cid, Path::new(&source), &user_dir()?)?;
    let message = result["message"].as_str().unwrap_or_default().to_owned();
    let reprocess = message == "照合用ハッシュを追加しました"
        || message == "すでに確定できるので、追加しません";
    Ok(json!({"message": message, "reprocess": reprocess}))
}

/// 登録して確定した後に、その結果が review/ か not_card/ に作った古いコピーを消す。
/// 渡されたファイルだけが対象（同じ名前の別形式は、別の入力から作られたものかもしれないので触れない）。
/// 出力先の中の review・not_card フォルダにある webp/png だけを受け付け、1つでも違えば何も消さない。
#[tauri::command(async)]
fn remove_review_copy(paths: Vec<String>, output: String) -> Result<(), String> {
    let output = PathBuf::from(output.trim());
    if !output.is_absolute() {
        return Err("削除できない場所です".into());
    }
    let output = fs::canonicalize(&output).map_err(|e| e.to_string())?;
    let mut targets = Vec::new();
    for path in paths {
        let file = PathBuf::from(&path);
        if !file.is_absolute() {
            return Err("削除できない場所です".into());
        }
        // 既に無いものは掃除済みとして扱う
        let Ok(file) = fs::canonicalize(&file) else {
            continue;
        };
        let in_review = file
            .parent()
            .and_then(|parent| parent.strip_prefix(&output).ok())
            .and_then(|relative| relative.components().next())
            .is_some_and(|first| {
                let first = first.as_os_str();
                first == "review" || first == "not_card"
            });
        let extension = file
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        if !in_review || !matches!(extension.as_deref(), Some("webp" | "png")) || !file.is_file() {
            return Err("削除できない場所です".into());
        }
        targets.push(file);
    }
    for file in targets {
        fs::remove_file(&file).map_err(|e| e.to_string())?;
    }
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
    let guard = Busy::begin(&busy)?;
    thread::spawn(move || {
        let _guard = guard;
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
    busy.request_stop();
}

/// ウィンドウを閉じる前に呼ぶ。処理が止まり、保存と記録が済むまで戻らない。
#[tauri::command(async)]
fn shutdown(busy: State<'_, Arc<Busy>>) {
    busy.shutdown();
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
        .setup(|app| {
            // アプリ名の右に版番号を出す
            if let Some(window) = app.get_webview_window("main") {
                let title = format!(
                    "開闢 v{} - MDカード画像ツール",
                    env!("CARGO_PKG_VERSION")
                );
                let _ = window.set_title(&title);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_config,
            start_identify,
            start_update,
            stop,
            data_summary,
            open_folder,
            identify_one,
            describe_cards,
            search_cards,
            learn_card,
            remove_review_copy,
            shutdown
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
    fn identify_args_map_to_cli_options() {
        let root = std::env::temp_dir().join(format!("mdript-gui-args-{}", std::process::id()));
        let args = |output: &str, resolution: &str, format: &str| IdentifyArgs {
            output: output.into(),
            save_format: format.into(),
            rounded: false,
            recursive: false,
            skip_duplicate: false,
            save_review: false,
            save_not_card: true,
            english: "ja".into(),
            split: "both".into(),
            resolution: resolution.into(),
        };
        let out = root.join("out").to_string_lossy().into_owned();
        let o = options(&args(&out, "1920x1080", "png"), root.join("user")).unwrap();
        assert_eq!(o.format, "png");
        assert!(!o.rounded && !o.recursive && !o.duplicates && !o.review && o.not_card);
        assert_eq!((o.english.as_str(), o.split.as_str()), ("ja", "both"));
        assert_eq!(
            o.resolution,
            mdript_core::layout::Resolution::Original(1920, 1080)
        );
        let auto = options(&args(&out, "auto", "webp"), root.join("user")).unwrap();
        assert_eq!(auto.resolution, mdript_core::layout::Resolution::Auto);
        assert!(options(&args("", "auto", "webp"), root.join("user")).is_err());
        assert!(options(&args("relative", "auto", "webp"), root.join("user")).is_err());
        assert!(options(&args(&out, "auto", "gif"), root.join("user")).is_err());
        assert!(options(&args(&out, "axb", "webp"), root.join("user")).is_err());
    }

    fn remove(paths: &[PathBuf], out: &str) -> Result<(), String> {
        remove_review_copy(
            paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            out.to_owned(),
        )
    }

    #[test]
    fn remove_review_copy_only_touches_review_and_not_card() {
        let root = std::env::temp_dir().join(format!("mdript-gui-remove-{}", std::process::id()));
        let review = root.join("review");
        let not_card = root.join("not_card");
        let en = root.join("en");
        for dir in [&review, &not_card, &en] {
            fs::create_dir_all(dir).unwrap();
        }
        let out = root.to_string_lossy().into_owned();
        for path in [
            review.join("a.webp"),
            review.join("keep.webp"),
            not_card.join("b.webp"),
            en.join("c.webp"),
            root.join("d.webp"),
            review.join("e.txt"),
            review.join("Mr. X.webp"),
            review.join("Mr.webp"),
        ] {
            fs::write(path, b"x").unwrap();
        }
        remove(&[review.join("a.webp")], &out).unwrap();
        assert!(!review.join("a.webp").exists() && review.join("keep.webp").exists());
        remove(&[review.join("Mr. X.webp")], &out).unwrap();
        assert!(review.join("Mr.webp").exists());
        remove(&[not_card.join("b.webp")], &out).unwrap();
        assert!(!not_card.join("b.webp").exists());
        assert!(remove(&[en.join("c.webp")], &out).is_err());
        assert!(remove(&[root.join("d.webp")], &out).is_err());
        assert!(remove(&[review.join("e.txt")], &out).is_err());
        assert!(remove(&[review.join("..").join("d.webp")], &out).is_err());
        assert!(en.join("c.webp").exists() && root.join("d.webp").exists());
        assert!(remove(&[PathBuf::from("relative.webp")], &out).is_err());
        // 1つでも対象外なら、ほかも消さない
        assert!(remove(&[review.join("keep.webp"), root.join("d.webp")], &out).is_err());
        assert!(review.join("keep.webp").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remove_review_copy_keeps_same_name_files_from_other_inputs() {
        let root = std::env::temp_dir().join(format!("mdript-gui-same-{}", std::process::id()));
        let not_card = root.join("not_card");
        let review = root.join("review");
        fs::create_dir_all(&not_card).unwrap();
        fs::create_dir_all(&review).unwrap();
        let out = root.to_string_lossy().into_owned();
        // 入力 A は webp、別フォルダの同名入力 B は png で保存した
        for path in [
            not_card.join("shot.webp"),
            not_card.join("shot.png"),
            not_card.join("shot_alt1.webp"),
            review.join("pair.webp"),
            review.join("pair.png"),
            review.join("pair_alt1.webp"),
            review.join("pair_alt1.png"),
        ] {
            fs::write(path, b"x").unwrap();
        }
        remove(&[not_card.join("shot.png")], &out).unwrap();
        assert!(not_card.join("shot.webp").exists());
        assert!(!not_card.join("shot.png").exists());
        assert!(not_card.join("shot_alt1.webp").exists());
        // both 形式で作った2つだけを消し、別の実行の同名ファイルは残す
        remove(
            &[review.join("pair_alt1.webp"), review.join("pair_alt1.png")],
            &out,
        )
        .unwrap();
        assert!(!review.join("pair_alt1.webp").exists() && !review.join("pair_alt1.png").exists());
        assert!(review.join("pair.webp").exists() && review.join("pair.png").exists());
        // 既に無いファイルは問題にしない
        remove(&[not_card.join("shot.png")], &out).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shutdown_waits_for_running_work_and_refuses_new_work() {
        let busy = Arc::new(Busy::default());
        let guard = Busy::begin(&busy).unwrap();
        assert!(Busy::begin(&busy).is_err());
        let finished = Arc::new(AtomicBool::new(false));
        let worker = {
            let (busy, finished) = (Arc::clone(&busy), Arc::clone(&finished));
            thread::spawn(move || {
                let _guard = guard;
                while !busy.cancel.load(Ordering::SeqCst) {
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                // 中止を受けた後の書き込みに時間がかかる
                thread::sleep(std::time::Duration::from_millis(150));
                finished.store(true, Ordering::SeqCst);
            })
        };
        busy.shutdown();
        assert!(
            finished.load(Ordering::SeqCst),
            "shutdown returned before the work ended"
        );
        worker.join().unwrap();
        assert!(Busy::begin(&busy).is_err());
    }

    #[test]
    fn shutdown_is_released_when_the_work_panics() {
        let busy = Arc::new(Busy::default());
        let guard = Busy::begin(&busy).unwrap();
        let worker = thread::spawn(move || {
            let _guard = guard;
            thread::sleep(std::time::Duration::from_millis(50));
            panic!("injected panic");
        });
        busy.shutdown();
        assert!(worker.join().is_err());
    }

    #[test]
    fn shutdown_without_work_returns_at_once() {
        Busy::default().shutdown();
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
