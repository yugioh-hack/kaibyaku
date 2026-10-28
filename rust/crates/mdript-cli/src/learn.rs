use mdript_core::{catalog::Catalog, decode::load_rgb, identify::identify_trusted};
use serde_json::{Map, Value, json};
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

const BUNDLED: &str = include_str!("../../../../data/card_hashes.json");
static WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) fn read_object(path: &Path) -> Option<Value> {
    let value = serde_json::from_str::<Value>(&fs::read_to_string(path).ok()?).ok()?;
    value.is_object().then_some(value)
}

fn valid_hash(value: &Value) -> Option<&str> {
    value.as_str().filter(|hash| {
        [32, 48].contains(&hash.len())
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn within_distance(existing: &str, candidate: &str, min_distance: u32) -> bool {
    existing
        .get(..32)
        .and_then(|hash| u128::from_str_radix(hash, 16).ok())
        .zip(u128::from_str_radix(candidate, 16).ok())
        .is_some_and(|(a, b)| (a ^ b).count_ones() < min_distance)
}

pub(crate) fn loaded_hashes(
    user: Option<&Value>,
) -> Result<(Map<String, Value>, Vec<String>), String> {
    let base: Value = serde_json::from_str(BUNDLED).map_err(|e| e.to_string())?;
    let mut hashes = base["hashes"].as_object().cloned().unwrap_or_default();
    let mut packs = Vec::new();
    if let Some(user) = user.filter(|value| value["meta"]["format"] == 2) {
        if let Some(list) = user["meta"]["packs"].as_array() {
            packs = list
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            packs.sort();
            packs.dedup();
        }
        if let Some(extra) = user["hashes"].as_object() {
            for (cid, list) in extra {
                let entry = hashes.entry(cid.clone()).or_insert_with(|| json!([]));
                let current = entry.as_array_mut();
                if let (Some(current), Some(list)) = (current, list.as_array()) {
                    for item in list.iter().filter(|item| valid_hash(item).is_some()) {
                        if !current.contains(item) {
                            current.push(item.clone());
                        }
                    }
                }
            }
        }
    }
    Ok((hashes, packs))
}

/// 照合用ハッシュを持っているカードID（learn で登録できるカード）。
pub fn known_card_ids(user_dir: &Path) -> std::collections::HashSet<String> {
    let user = read_object(&user_dir.join("card_hashes_user.json"));
    loaded_hashes(user.as_ref())
        .map(|(map, _)| map.keys().cloned().collect())
        .unwrap_or_default()
}

/// 照合データの件数（カード種類数・ハッシュ数・カード名の件数と取得日時）。画面の表示用。
pub fn data_summary(user_dir: &Path) -> Value {
    let user = read_object(&user_dir.join("card_hashes_user.json"));
    let (cards, hashes) = loaded_hashes(user.as_ref()).map_or((0, 0), |(map, _)| {
        let hashes = map
            .values()
            .filter_map(Value::as_array)
            .map(|list| list.iter().filter(|h| valid_hash(h).is_some()).count())
            .sum::<usize>();
        (map.len(), hashes)
    });
    let names = read_object(&user_dir.join("card_names.json"));
    let name_count = names
        .as_ref()
        .and_then(|n| n["cards"].as_object())
        .map_or(0, |c| c.len());
    let fetched = names
        .as_ref()
        .and_then(|n| n["meta"]["fetched"].as_str())
        .unwrap_or("")
        .to_owned();
    let english_names = names
        .as_ref()
        .and_then(|n| n["cards"].as_object())
        .map_or(0, |cards| {
            cards
                .values()
                .filter(|card| {
                    card["e"].as_str().is_some_and(|s| !s.is_empty())
                        || (card["l"] == "en" && card["n"].as_str().is_some_and(|s| !s.is_empty()))
                })
                .count()
        });
    json!({"cards":cards,"hashes":hashes,"names":name_count,"english_names":english_names,"names_fetched":fetched})
}

pub(crate) fn save_hashes(
    path: &Path,
    hashes: &Map<String, Value>,
    packs: &[String],
) -> Result<(), String> {
    save_hashes_checked(path, hashes, packs, || Ok(()))
}

pub(crate) fn save_hashes_checked(
    path: &Path,
    hashes: &Map<String, Value>,
    packs: &[String],
    check: impl Fn() -> Result<(), String>,
) -> Result<(), String> {
    let base: Value = serde_json::from_str(BUNDLED).map_err(|e| e.to_string())?;
    let base = base["hashes"].as_object();
    let mut extra = Map::new();
    for (cid, values) in hashes {
        if let Some(values) = values.as_array() {
            let originals = base
                .and_then(|base| base.get(cid))
                .and_then(Value::as_array);
            let diff: Vec<_> = values
                .iter()
                .filter(|value| !originals.is_some_and(|values| values.contains(value)))
                .cloned()
                .collect();
            if !diff.is_empty() {
                extra.insert(cid.clone(), Value::Array(diff));
            }
        }
    }
    let data = json!({"meta":{"format":2,"cards":extra.len(),"packs":packs},"hashes":extra});
    let bytes = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
    fs::create_dir_all(path.parent().ok_or("invalid user-data path")?)
        .map_err(|e| e.to_string())?;
    let temporary = path.with_file_name("card_hashes_user.json.tmp");
    fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    if let Err(error) = check() {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(&temporary, path).map_err(|e| e.to_string())
}

/// 画像1枚のハッシュをカードIDの照合用に追加する（Python 版 cli.learn と同じ判定）。
pub fn learn_image(cid: &str, image: &Path, user_dir: &Path) -> Result<Value, String> {
    let user_dir = crate::paths::validate_user_data(user_dir)?;
    learn(cid, image, &user_dir)
}

fn learn(cid: &str, image: &Path, user_dir: &Path) -> Result<Value, String> {
    let guard = WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let user_path = user_dir.join("card_hashes_user.json");
    let user = read_object(&user_path);
    let names = fs::read_to_string(user_dir.join("card_names.json")).ok();
    let catalog = Catalog::bundled(
        user.as_ref().map(Value::to_string).as_deref(),
        names.as_deref(),
    )?;
    let (mut hashes, packs) = loaded_hashes(user.as_ref())?;
    if !hashes.contains_key(cid) {
        return Err(format!("カードID {cid} は照合用データにありません"));
    }
    let image = load_rgb(image)?;
    let found = identify_trusted(&image, &catalog).ok_or(
        "カードの位置を決められませんでした（カードの拡大画面を全画面で撮った画像を使ってください）",
    )?;
    let current = format!(
        "{}（{}）  現在の照合: {}（{}）距離 {}",
        catalog.display_name(cid, false),
        cid,
        catalog.display_name(&found.card_id, false),
        found.card_id,
        found.distance
    );
    let message = if found.card_id == cid && found.confirmed() {
        "すでに確定できるので、追加しません"
    } else {
        let present = hashes
            .get(cid)
            .and_then(Value::as_array)
            .is_some_and(|list| {
                list.iter()
                    .filter_map(valid_hash)
                    .any(|known| within_distance(known, &found.hash, 1))
            });
        if present {
            "近いハッシュがすでにあるので、追加しません"
        } else {
            hashes
                .get_mut(cid)
                .and_then(Value::as_array_mut)
                .ok_or("invalid hashes for card")?
                .push(json!(found.hash));
            save_hashes(&user_path, &hashes, &packs)?;
            "照合用ハッシュを追加しました"
        }
    };
    drop(guard);
    Ok(json!({"current":current,"message":message}))
}

pub fn run(args: Vec<OsString>) -> Result<(), String> {
    if args.len() == 1 && args[0] == "--help" {
        println!("mdript-cli learn CID IMAGE --user-data ABS_DIR");
        return Ok(());
    }
    if args.len() != 4 || args[2] != "--user-data" {
        return Err("usage: mdript-cli learn CID IMAGE --user-data ABS_DIR".into());
    }
    let cid = args[0].to_str().ok_or("CID must be Unicode")?;
    let image = PathBuf::from(&args[1]);
    let user_dir = PathBuf::from(&args[3]);
    let user_dir = crate::paths::validate_user_data(&user_dir)?;
    println!("{}", learn(cid, &image, &user_dir)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};
    use std::thread;

    #[test]
    fn concurrent_learns_keep_the_user_file_valid() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local-results")
            .join(format!("learn-concurrent-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let image_path = root.join("input.png");
        let image = RgbImage::from_fn(400, 600, |x, y| {
            Rgb([
                (x * 7 + y * 3) as u8,
                (x * y / 17) as u8,
                ((x * 11) ^ (y * 5)) as u8,
            ])
        });
        image.save(&image_path).unwrap();
        let user_data = root.join("isolated-user-data");
        let outputs: Vec<_> = thread::scope(|scope| {
            let tasks: Vec<_> = (0..2)
                .map(|_| scope.spawn(|| learn("4138", &image_path, &user_data)))
                .collect();
            tasks.into_iter().map(|task| task.join().unwrap()).collect()
        });
        assert!(outputs.iter().all(Result::is_ok), "{outputs:?}");
        let saved = read_object(&user_data.join("card_hashes_user.json")).unwrap();
        assert_eq!(saved["meta"]["format"], 2);
        assert_eq!(saved["meta"]["cards"], 1);
        assert_eq!(saved["hashes"]["4138"].as_array().unwrap().len(), 1);
        assert!(!user_data.join("card_hashes_user.json.tmp").exists());
    }

    #[test]
    fn distance_threshold_is_strict() {
        let zero = "00000000000000000000000000000000";
        let one = "00000000000000000000000000000001";
        assert!(within_distance(zero, zero, 1));
        assert!(!within_distance(zero, one, 1));
        assert!(within_distance(zero, one, 2));
        assert!(within_distance(&format!("{zero}{zero}"), one, 2));
    }

    #[test]
    fn interrupted_save_keeps_previous_file_and_removes_temporary() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local-results")
            .join(format!("learn-interrupt-{}", std::process::id()))
            .join("isolated-user-data");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("card_hashes_user.json");
        fs::write(&path, b"previous").unwrap();
        let (hashes, packs) = loaded_hashes(None).unwrap();
        let result = save_hashes_checked(&path, &hashes, &packs, || Err("cancelled".into()));
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), b"previous");
        assert!(!root.join("card_hashes_user.json.tmp").exists());
    }
}
