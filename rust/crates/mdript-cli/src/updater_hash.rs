use crate::{
    learn::{loaded_hashes, read_object, save_hashes_checked},
    updater::{
        BASE, Fetch, HttpClient, check_cancelled, clean, install_cancel_handler, parse_rows,
    },
};
use mdript_core::{artwork_hash, decode::load_rgb_bytes};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    io::Cursor,
    path::PathBuf,
};

const MAX_IMAGE_PIXELS: u64 = 4000 * 4000;
const SPECIAL: [&str; 5] = ["5224", "5271", "5272", "5273", "5274"];

fn re(source: &str) -> Regex {
    Regex::new(source).expect("static updater expression")
}

fn capture<'a>(expression: &Regex, text: &'a str) -> Option<&'a str> {
    expression
        .captures(text)?
        .get(1)
        .map(|value| value.as_str())
}

#[derive(Clone, Debug)]
struct Pack {
    pid: String,
    name: String,
}

fn parse_pack_list(html: &str) -> Vec<Pack> {
    let pid = re(r"pid=(\d+)");
    let name = re(r"(?s)<p>(.*?)</p>");
    let mut seen = HashSet::new();
    html.split("<div class=\"t_row")
        .skip(1)
        .filter_map(|block| {
            let pid = capture(&pid, block)?.to_owned();
            seen.insert(pid.clone()).then(|| Pack {
                pid,
                name: capture(&name, block).map_or_else(String::new, clean),
            })
        })
        .collect()
}

fn fetch_pack_list(fetch: &mut impl Fetch) -> Result<Vec<Pack>, String> {
    let raw = fetch.get(&format!("{BASE}card_list.action?request_locale=ja"), None)?;
    Ok(parse_pack_list(&String::from_utf8_lossy(&raw)))
}

fn fetch_pack_cards(fetch: &mut impl Fetch, pid: &str) -> Result<Vec<String>, String> {
    if !pid.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("invalid pack id".into());
    }
    let url = format!("{BASE}card_search.action?ope=1&sess=1&pid={pid}&rp=99999&request_locale=ja");
    let raw = fetch.get(&url, None)?;
    Ok(parse_rows(&String::from_utf8_lossy(&raw))
        .into_iter()
        .map(|row| row.id)
        .collect())
}

fn fetch_card_page(
    fetch: &mut impl Fetch,
    cid: &str,
    locale: &str,
) -> Result<(Vec<image::RgbImage>, Vec<String>), String> {
    if !cid.bytes().all(|byte| byte.is_ascii_digit()) || !["ja", "en"].contains(&locale) {
        return Err("invalid card request".into());
    }
    let page = format!("{BASE}card_search.action?ope=2&cid={cid}&request_locale={locale}");
    let raw = fetch.get(&page, None)?;
    let html = String::from_utf8_lossy(&raw);
    let image_src = re(r#"(get_image\.action\?[^"'\s>]+)"#);
    let ciid = re(r"ciid=(\d+)");
    let pid = re(r"pid=(\d+)");
    let mut seen = HashSet::new();
    let mut images = Vec::new();
    for hit in image_src.captures_iter(&html) {
        let src = html_escape::decode_html_entities(&hit[1]).to_string();
        if !src.contains("type=1") {
            continue;
        }
        let key = capture(&ciid, &src).unwrap_or(&src);
        if !seen.insert(key.to_owned()) {
            continue;
        }
        let raw = fetch.get(&format!("{BASE}{src}"), Some(&page))?;
        let dimensions = image::ImageReader::new(Cursor::new(raw.as_slice()))
            .with_guessed_format()
            .ok()
            .and_then(|reader| reader.into_dimensions().ok());
        if dimensions.is_none_or(|(w, h)| u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS) {
            continue;
        }
        if let Ok(image) = load_rgb_bytes(&raw)
            && u64::from(image.width()) * u64::from(image.height()) <= MAX_IMAGE_PIXELS
        {
            images.push(image);
        }
    }
    let packs = if locale == "ja" {
        let mut packs: Vec<_> = pid
            .captures_iter(&html)
            .filter_map(|hit| hit.get(1).map(|value| value.as_str().to_owned()))
            .collect();
        packs.sort();
        packs.dedup();
        packs
    } else {
        Vec::new()
    };
    Ok((images, packs))
}

#[derive(Default)]
struct HashProgress {
    done: usize,
    added: usize,
    failed: Vec<String>,
    packs: HashSet<String>,
}

fn update_hashes(
    fetch: &mut impl Fetch,
    hashes: &mut Map<String, Value>,
    cids: &[String],
    english_only: &HashSet<String>,
    check: &impl Fn() -> Result<(), String>,
) -> Result<HashProgress, String> {
    let mut progress = HashProgress::default();
    for cid in cids {
        check()?;
        let page = if english_only.contains(cid) {
            fetch_card_page(fetch, cid, "en")
        } else {
            match fetch_card_page(fetch, cid, "ja") {
                Ok((images, packs)) if images.is_empty() => {
                    fetch_card_page(fetch, cid, "en").map(|(english, _)| (english, packs))
                }
                other => other,
            }
        };
        check()?;
        let Ok((images, packs)) = page else {
            progress.failed.push(cid.clone());
            continue;
        };
        progress.packs.extend(packs);
        let card_hashes: Result<Vec<_>, _> = images
            .into_iter()
            .map(|image| {
                let bounds = [0, 0, image.width() as i64, image.height() as i64];
                artwork_hash(&image, bounds, false, SPECIAL.contains(&cid.as_str()))
            })
            .collect();
        let Ok(card_hashes) = card_hashes else {
            progress.failed.push(cid.clone());
            continue;
        };
        if card_hashes.is_empty() {
            progress.failed.push(cid.clone());
            continue;
        }
        let entry = hashes.entry(cid.clone()).or_insert_with(|| json!([]));
        if !entry.is_array() {
            *entry = json!([]);
        }
        let current = entry.as_array_mut().expect("array just created");
        for hash in card_hashes {
            let value = json!(hash);
            if !current.contains(&value) {
                current.push(value);
                progress.added += 1;
            }
        }
        progress.done += 1;
    }
    Ok(progress)
}

fn update_new_sets(
    fetch: &mut impl Fetch,
    hashes: &mut Map<String, Value>,
    known_packs: &mut Vec<String>,
    cards: &Map<String, Value>,
    max_cards: usize,
    check: &impl Fn() -> Result<(), String>,
) -> Result<Value, String> {
    check()?;
    let mut result = json!({"new_cards":0,"new_hashes":0,"packs":0,"refreshed":0,
        "failed":[],"skipped_packs":[],"seeded":0,"pack_names":[],"pack_list_failed":false});
    let all_packs = fetch_pack_list(fetch).unwrap_or_default();
    check()?;
    let already_known: HashSet<_> = known_packs.iter().cloned().collect();
    let mut new_packs = if all_packs.is_empty() {
        result["pack_list_failed"] = json!(true);
        Vec::new()
    } else if already_known.is_empty() {
        *known_packs = all_packs.iter().map(|pack| pack.pid.clone()).collect();
        result["seeded"] = json!(all_packs.len());
        Vec::new()
    } else {
        all_packs
            .iter()
            .filter(|pack| !already_known.contains(&pack.pid))
            .cloned()
            .collect()
    };
    let missing: Vec<_> = cards
        .keys()
        .filter(|cid| !hashes.contains_key(*cid))
        .cloned()
        .collect();
    let english_only: HashSet<_> = cards
        .iter()
        .filter(|(_, card)| card["l"] == "en")
        .map(|(cid, _)| cid.clone())
        .collect();
    // First-run seeding may have changed known_packs above.
    let known: HashSet<_> = known_packs.iter().cloned().collect();
    if !missing.is_empty() {
        let progress = update_hashes(fetch, hashes, &missing, &english_only, check)?;
        result["new_cards"] = json!(progress.done);
        result["new_hashes"] = json!(progress.added);
        result["failed"] = json!(progress.failed);
        if !all_packs.is_empty() {
            let listed: HashSet<_> = new_packs
                .iter()
                .map(|pack: &Pack| pack.pid.clone())
                .collect();
            let mut discovered: Vec<_> = progress
                .packs
                .difference(&known)
                .filter(|pid| !listed.contains(*pid))
                .cloned()
                .collect();
            discovered.sort();
            new_packs.extend(discovered.into_iter().map(|pid| Pack {
                pid,
                name: String::new(),
            }));
        }
    }
    let missing_set: HashSet<_> = missing.into_iter().collect();
    let mut todo = Vec::new();
    let mut seen_cards = HashSet::new();
    let mut pack_members = HashMap::new();
    for pack in new_packs {
        check()?;
        let Ok(members) = fetch_pack_cards(fetch, &pack.pid) else {
            continue;
        };
        check()?;
        if members.is_empty() {
            continue;
        }
        if !pack.name.is_empty() {
            result["pack_names"]
                .as_array_mut()
                .expect("array")
                .push(json!(pack.name));
        }
        for cid in &members {
            if !missing_set.contains(cid) && seen_cards.insert(cid.clone()) {
                todo.push(cid.clone());
            }
        }
        pack_members.insert(pack.pid, members.into_iter().collect::<HashSet<_>>());
    }
    result["packs"] = json!(pack_members.len());
    if todo.len() > max_cards {
        let mut skipped: Vec<_> = pack_members.keys().cloned().collect();
        skipped.sort();
        result["skipped_packs"] = json!(skipped);
    } else {
        if !todo.is_empty() {
            let progress = update_hashes(fetch, hashes, &todo, &HashSet::new(), check)?;
            result["refreshed"] = json!(progress.done);
            result["new_hashes"] =
                json!(result["new_hashes"].as_u64().unwrap_or(0) + progress.added as u64);
            result["failed"]
                .as_array_mut()
                .expect("array")
                .extend(progress.failed.into_iter().map(|cid| json!(cid)));
        }
        let failed: HashSet<_> = result["failed"]
            .as_array()
            .expect("array")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for (pid, members) in pack_members {
            if members.iter().all(|cid| !failed.contains(cid.as_str())) {
                known_packs.push(pid);
            }
        }
        known_packs.sort();
        known_packs.dedup();
    }
    check()?;
    Ok(result)
}

pub fn run(args: Vec<OsString>) -> Result<(), String> {
    if args.len() == 1 && args[0] == "--help" {
        println!(
            "mdript-cli update-hashes --user-data ABS_DIR [--max-cards N] [--verify-pack PID]"
        );
        return Ok(());
    }
    let mut user_dir = None;
    let mut max_cards = 4000;
    let mut verify_pack = None;
    let mut i = 0;
    while i < args.len() {
        let key = args[i].to_str().ok_or("invalid option")?;
        i += 1;
        let value = args.get(i).ok_or("option requires a value")?;
        match key {
            "--user-data" => user_dir = Some(PathBuf::from(value)),
            "--max-cards" => {
                max_cards = value
                    .to_string_lossy()
                    .parse()
                    .map_err(|_| "invalid max-cards")?
            }
            "--verify-pack" => verify_pack = Some(value.to_string_lossy().into_owned()),
            _ => return Err(format!("unknown option: {key}")),
        }
        i += 1;
    }
    let user_dir = user_dir.ok_or("--user-data ABS_DIR is required")?;
    let user_dir = crate::paths::validate_user_data(&user_dir)?;
    let names = read_object(&user_dir.join("card_names.json"))
        .ok_or("card_names.json is required; run update-names first")?;
    let cards = names["cards"]
        .as_object()
        .ok_or("card_names.json has no cards")?;
    let user_file = user_dir.join("card_hashes_user.json");
    let user = read_object(&user_file);
    let (mut hashes, mut packs) = loaded_hashes(user.as_ref())?;
    install_cancel_handler()?;
    let mut fetch = HttpClient::new();
    let result = if let Some(pid) = verify_pack {
        let members = fetch_pack_cards(&mut fetch, &pid)?;
        let english_only = HashSet::new();
        let progress = update_hashes(
            &mut fetch,
            &mut hashes,
            &members,
            &english_only,
            &check_cancelled,
        )?;
        json!({"pack":pid,"members":members.len(),"cards":progress.done,
            "new_hashes":progress.added,"failed":progress.failed})
    } else {
        update_new_sets(
            &mut fetch,
            &mut hashes,
            &mut packs,
            cards,
            max_cards,
            &check_cancelled,
        )?
    };
    check_cancelled()?;
    save_hashes_checked(&user_file, &hashes, &packs, check_cancelled)?;
    println!("{result}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
    use std::{cell::Cell, fs, io::Cursor};

    struct Fixture {
        pages: HashMap<String, Vec<u8>>,
    }
    impl Fetch for Fixture {
        fn get(&mut self, url: &str, _: Option<&str>) -> Result<Vec<u8>, String> {
            let key = if url.contains("card_list.action") {
                "list"
            } else if url.contains("pid=") {
                "pack"
            } else if url.contains("get_image.action") {
                "image"
            } else {
                "card"
            };
            self.pages.get(key).cloned().ok_or("fixture missing".into())
        }
    }
    fn fixture() -> Fixture {
        let image = RgbImage::from_fn(100, 150, |x, y| {
            Rgb([(x * 7 + y) as u8, (x * y) as u8, (y * 3) as u8])
        });
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        Fixture {
            pages: HashMap::from([
                (
                    "list".into(),
                    include_bytes!("../tests/fixtures/packs.html").to_vec(),
                ),
                (
                    "pack".into(),
                    include_bytes!("../tests/fixtures/pack-cards.html").to_vec(),
                ),
                (
                    "card".into(),
                    include_bytes!("../tests/fixtures/card-page.html").to_vec(),
                ),
                ("image".into(), bytes.into_inner()),
            ]),
        }
    }
    #[test]
    fn one_new_pack_adds_hash_and_pack_after_success() {
        let mut hashes =
            Map::from_iter([("4138".into(), json!(["00000000000000000000000000000000"]))]);
        let mut packs = vec!["10".into()];
        let cards = Map::from_iter([("4138".into(), json!({"n":"sample","r":""}))]);
        let result = update_new_sets(
            &mut fixture(),
            &mut hashes,
            &mut packs,
            &cards,
            4000,
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(result["packs"], 1);
        assert_eq!(result["refreshed"], 1);
        assert_eq!(result["new_hashes"], 1);
        assert!(packs.contains(&"11".to_owned()));
        assert_eq!(hashes["4138"].as_array().unwrap().len(), 2);
    }
    #[test]
    fn first_run_seeds_packs_without_refetching_old_cards() {
        let mut hashes =
            Map::from_iter([("4138".into(), json!(["00000000000000000000000000000000"]))]);
        let mut packs = Vec::new();
        let cards = Map::from_iter([("4138".into(), json!({"n":"sample","r":""}))]);
        let result = update_new_sets(
            &mut fixture(),
            &mut hashes,
            &mut packs,
            &cards,
            4000,
            &|| Ok(()),
        )
        .unwrap();
        assert_eq!(result["seeded"], 1);
        assert_eq!(result["refreshed"], 0);
        assert_eq!(packs, vec!["11"]);
    }
    #[test]
    fn update_branches_match_python_golden_summaries() {
        let golden: Value = serde_json::from_str(include_str!(
            "../tests/fixtures/update-new-sets-golden.json"
        ))
        .unwrap();
        for case in [
            "initial_seed_with_missing_card",
            "new_card_in_known_pack",
            "new_pack_reprint",
            "pack_list_failure",
            "max_cards_exceeded",
        ] {
            let mut fetch = fixture();
            let mut hashes =
                Map::from_iter([("4138".into(), json!(["00000000000000000000000000000000"]))]);
            let mut packs =
                if case == "initial_seed_with_missing_card" || case == "pack_list_failure" {
                    Vec::new()
                } else if case == "new_card_in_known_pack" {
                    vec!["11".into()]
                } else {
                    vec!["10".into()]
                };
            let mut cards = Map::from_iter([("4138".into(), json!({"n":"Sample"}))]);
            if matches!(
                case,
                "initial_seed_with_missing_card" | "new_card_in_known_pack" | "pack_list_failure"
            ) {
                cards.insert("99999".into(), json!({"n":"New"}));
            }
            if case == "new_pack_reprint" {
                cards.insert("99999".into(), json!({"n":"Reprint"}));
                hashes.insert("99999".into(), json!(["00000000000000000000000000000000"]));
                let row = include_str!("../tests/fixtures/pack-cards.html");
                fetch.pages.insert(
                    "pack".into(),
                    format!("{row}{}", row.replace("4138", "99999")).into_bytes(),
                );
            }
            if case == "pack_list_failure" {
                fetch.pages.remove("list");
            }
            let max_cards = if case == "max_cards_exceeded" {
                0
            } else {
                4000
            };
            let result = update_new_sets(
                &mut fetch,
                &mut hashes,
                &mut packs,
                &cards,
                max_cards,
                &|| Ok(()),
            )
            .unwrap();
            assert_eq!(result, golden[case], "{case}");
        }
    }
    #[test]
    fn card_page_uses_central_hash_image_and_deduplicates_ciid() {
        let (images, packs) = fetch_card_page(&mut fixture(), "4138", "ja").unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(packs, vec!["11"]);
    }

    #[test]
    fn oversized_official_image_is_rejected_before_decoding() {
        let mut fixture = fixture();
        let image = RgbImage::new(4001, 4000);
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        fixture.pages.insert("image".into(), bytes.into_inner());
        let (images, _) = fetch_card_page(&mut fixture, "4138", "ja").unwrap();
        assert!(images.is_empty());
    }

    #[test]
    fn wide_official_image_within_pixel_limit_is_accepted() {
        let mut fixture = fixture();
        let image = RgbImage::new(5000, 100);
        let mut bytes = Cursor::new(Vec::new());
        DynamicImage::ImageRgb8(image)
            .write_to(&mut bytes, ImageFormat::Png)
            .unwrap();
        fixture.pages.insert("image".into(), bytes.into_inner());
        let (images, _) = fetch_card_page(&mut fixture, "4138", "ja").unwrap();
        assert_eq!(images.len(), 1);
    }

    #[test]
    fn cancellation_stops_remaining_cards_and_preserves_user_file() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local-results")
            .join(format!("hash-cancel-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("card_hashes_user.json");
        let original = b"{\"unchanged\":true}";
        fs::write(&path, original).unwrap();
        let mut hashes = Map::new();
        let mut packs = vec!["11".into()];
        let cards = Map::from_iter([
            ("4138".into(), json!({"n":"First"})),
            ("99999".into(), json!({"n":"Second"})),
        ]);
        let calls = Cell::new(0);
        let check = || {
            calls.set(calls.get() + 1);
            if calls.get() >= 5 {
                Err("更新を中断しました".into())
            } else {
                Ok(())
            }
        };
        let result = update_new_sets(
            &mut fixture(),
            &mut hashes,
            &mut packs,
            &cards,
            4000,
            &check,
        )
        .and_then(|summary| {
            save_hashes_checked(&path, &hashes, &packs, check)?;
            Ok(summary)
        });
        assert_eq!(result.unwrap_err(), "更新を中断しました");
        assert_eq!(calls.get(), 5);
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}
