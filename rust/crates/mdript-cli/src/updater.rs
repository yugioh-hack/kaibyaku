use regex::Regex;
use serde_json::{Map, Value, json};
use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::OnceLock,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

const HOST: &str = "www.db.yugioh-card.com";
pub(crate) const BASE: &str = "https://www.db.yugioh-card.com/yugiohdb/";
const SEARCH: &str = "https://www.db.yugioh-card.com/yugiohdb/card_search.action";
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const MAX_RESPONSE: u64 = 32 * 1024 * 1024;
const INTERVAL: Duration = Duration::from_millis(400);
const PAGE_SIZE: usize = 2000;
static CANCELLED: AtomicBool = AtomicBool::new(false);
static CANCEL_HANDLER: OnceLock<Result<(), String>> = OnceLock::new();

pub(crate) fn install_cancel_handler() -> Result<(), String> {
    CANCEL_HANDLER
        .get_or_init(|| {
            ctrlc::set_handler(|| CANCELLED.store(true, Ordering::SeqCst))
                .map_err(|error| error.to_string())
        })
        .clone()
}

pub(crate) fn check_cancelled() -> Result<(), String> {
    if CANCELLED.load(Ordering::SeqCst) {
        Err("更新を中断しました".into())
    } else {
        Ok(())
    }
}

fn pattern(source: &'static str) -> &'static Regex {
    static ROW: OnceLock<Regex> = OnceLock::new();
    static CID: OnceLock<Regex> = OnceLock::new();
    static NAME: OnceLock<Regex> = OnceLock::new();
    static RUBY: OnceLock<Regex> = OnceLock::new();
    match source {
        "row" => ROW.get_or_init(|| Regex::new("class=\"t_row").unwrap()),
        "cid" => CID.get_or_init(|| Regex::new("class=\"cid\"\\s+value=\"(\\d+)\"").unwrap()),
        "name" => {
            NAME.get_or_init(|| Regex::new("(?s)<span class=\"card_name\">(.*?)</span>").unwrap())
        }
        "ruby" => {
            RUBY.get_or_init(|| Regex::new("(?s)<span class=\"card_ruby\">(.*?)</span>").unwrap())
        }
        _ => unreachable!(),
    }
}

pub(crate) fn clean(value: &str) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    html_escape::decode_html_entities(&collapsed)
        .trim()
        .to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NameRow {
    pub(crate) id: String,
    name: String,
    ruby: String,
}

pub(crate) fn parse_rows(html: &str) -> Vec<NameRow> {
    pattern("row")
        .split(html)
        .skip(1)
        .filter_map(|block| {
            let id = pattern("cid").captures(block)?.get(1)?.as_str().to_owned();
            let name = clean(pattern("name").captures(block)?.get(1)?.as_str());
            let ruby = pattern("ruby")
                .captures(block)
                .and_then(|match_| match_.get(1))
                .map_or_else(String::new, |match_| clean(match_.as_str()));
            Some(NameRow { id, name, ruby })
        })
        .collect()
}

fn allowed_url(value: &str) -> Result<(), String> {
    let uri: ureq::http::Uri = value.parse().map_err(|_| "invalid URL")?;
    if uri.scheme_str() != Some("https")
        || uri.host() != Some(HOST)
        || uri.authority().is_none_or(|a| a.as_str() != HOST)
    {
        return Err("公式DB以外のURLは使用できません".into());
    }
    Ok(())
}

fn redirect_url(current: &str, location: &str) -> Result<String, String> {
    let value = if location.starts_with("https://") {
        location.to_owned()
    } else if location.starts_with('/') && !location.starts_with("//") {
        format!("https://{HOST}{location}")
    } else if !location.contains(':') && !location.starts_with('/') {
        let prefix = current.split('?').next().unwrap_or(current);
        let prefix = prefix.rsplit_once('/').map_or(BASE, |(prefix, _)| prefix);
        format!("{prefix}/{location}")
    } else {
        return Err("想定外のリダイレクト先です".into());
    };
    allowed_url(&value)?;
    Ok(value)
}

pub(crate) trait Fetch {
    fn get(&mut self, url: &str, referer: Option<&str>) -> Result<Vec<u8>, String>;
}

pub(crate) struct HttpClient {
    agent: ureq::Agent,
    last_request: Option<Instant>,
}

impl HttpClient {
    pub(crate) fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(0)
            .timeout_global(Some(Duration::from_secs(60)))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
            last_request: None,
        }
    }

    fn one(&mut self, url: &str, referer: Option<&str>) -> Result<Vec<u8>, String> {
        let mut current = url.to_owned();
        for _ in 0..=10 {
            check_cancelled()?;
            allowed_url(&current)?;
            if let Some(last) = self.last_request {
                let remaining = INTERVAL.saturating_sub(last.elapsed());
                if !remaining.is_zero() {
                    thread::sleep(remaining);
                }
            }
            self.last_request = Some(Instant::now());
            let mut request = self
                .agent
                .get(&current)
                .header("User-Agent", UA)
                .header("Accept-Language", "ja,en;q=0.8")
                .header("Accept-Encoding", "gzip");
            if let Some(referer) = referer {
                request = request.header("Referer", referer);
            }
            let mut response = request.call().map_err(|e| e.to_string())?;
            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get("Location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or("redirect without Location")?;
                current = redirect_url(&current, location)?;
                continue;
            }
            let mut bytes = Vec::new();
            response
                .body_mut()
                .as_reader()
                .take(MAX_RESPONSE + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() as u64 > MAX_RESPONSE {
                return Err("応答が大きすぎます".into());
            }
            check_cancelled()?;
            return Ok(bytes);
        }
        Err("リダイレクト回数が多すぎます".into())
    }
}

impl Fetch for HttpClient {
    fn get(&mut self, url: &str, referer: Option<&str>) -> Result<Vec<u8>, String> {
        let mut last = String::new();
        for attempt in 0..3 {
            check_cancelled()?;
            match self.one(url, referer) {
                Ok(bytes) => return Ok(bytes),
                Err(error) => last = error,
            }
            if attempt < 2 {
                thread::sleep(Duration::from_millis(1500 * (attempt + 1)));
            }
        }
        Err(last)
    }
}

fn fetch_name_list(fetch: &mut impl Fetch, locale: &str) -> Result<Vec<NameRow>, String> {
    let mut all = Vec::new();
    for page in 1..=60 {
        let url = format!(
            "{SEARCH}?ope=1&sess=1&rp={PAGE_SIZE}&page={page}&sort=1&keyword=&stype=1&othercon=2&request_locale={locale}"
        );
        let raw = fetch.get(&url, None)?;
        let html = String::from_utf8_lossy(&raw);
        let rows = parse_rows(&html);
        let count = rows.len();
        all.extend(rows);
        if count < PAGE_SIZE {
            break;
        }
    }
    Ok(all)
}

fn fetch_card_names(fetch: &mut impl Fetch) -> Result<Map<String, Value>, String> {
    let ja = fetch_name_list(fetch, "ja")?;
    if ja.is_empty() {
        return Err("カード名を1件も取得できませんでした。公式データベースがメンテナンス中の可能性があります。".into());
    }
    let mut cards = Map::new();
    for row in ja {
        cards.insert(row.id, json!({"n":row.name,"r":row.ruby}));
    }
    if let Ok(en) = fetch_name_list(fetch, "en") {
        for row in en {
            if let Some(card) = cards.get_mut(&row.id) {
                if !row.name.is_empty() {
                    card["e"] = json!(row.name);
                }
            } else {
                cards.insert(row.id, json!({"n":row.name,"r":"","l":"en"}));
            }
        }
    }
    Ok(cards)
}

fn save_names(user_dir: &Path, cards: &Map<String, Value>, fetched: &str) -> Result<(), String> {
    fs::create_dir_all(user_dir).map_err(|e| e.to_string())?;
    let path = user_dir.join("card_names.json");
    let temporary = user_dir.join("card_names.json.tmp");
    let data = json!({"meta":{"format":1,"fetched":fetched,"count":cards.len()},"cards":cards});
    fs::write(
        &temporary,
        serde_json::to_vec(&data).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if let Err(error) = check_cancelled() {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    fs::rename(temporary, path).map_err(|e| e.to_string())
}

pub fn run_names(args: Vec<OsString>) -> Result<(), String> {
    if args.len() == 1 && args[0] == "--help" {
        println!("mdript-cli update-names --user-data ABS_DIR");
        return Ok(());
    }
    if args.len() != 2 || args[0] != "--user-data" {
        return Err("usage: mdript-cli update-names --user-data ABS_DIR".into());
    }
    let user_dir = PathBuf::from(&args[1]);
    let user_dir = crate::paths::validate_user_data(&user_dir)?;
    install_cancel_handler()?;
    let cards = fetch_card_names(&mut HttpClient::new())?;
    check_cancelled()?;
    let fetched = chrono::Local::now().format("%Y-%m-%d %H:%M").to_string();
    save_names(&user_dir, &cards, &fetched)?;
    println!(
        "{}",
        json!({"message":format!("カード名 {} 件を保存しました",cards.len()),"count":cards.len()})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct Fixture(HashMap<String, Vec<u8>>);
    impl Fetch for Fixture {
        fn get(&mut self, url: &str, _: Option<&str>) -> Result<Vec<u8>, String> {
            let locale = if url.contains("request_locale=en") {
                "en"
            } else {
                "ja"
            };
            self.0.get(locale).cloned().ok_or("fixture missing".into())
        }
    }

    #[test]
    fn names_merge_japanese_english_and_english_only() {
        let mut fixture = Fixture(HashMap::from([
            (
                "ja".into(),
                include_bytes!("../tests/fixtures/names-ja.html").to_vec(),
            ),
            (
                "en".into(),
                include_bytes!("../tests/fixtures/names-en.html").to_vec(),
            ),
        ]));
        let cards = fetch_card_names(&mut fixture).unwrap();
        assert_eq!(cards.len(), 3);
        assert_eq!(
            cards["5224"],
            json!({"n":"ウィジャ盤","r":"","e":"Destiny Board"})
        );
        assert_eq!(
            cards["5271"],
            json!({"n":"死のメッセージ「E」","r":"シノメッセージ","e":"Spirit Message E"})
        );
        assert_eq!(cards["99999"], json!({"n":"English only","r":"","l":"en"}));
    }

    #[test]
    fn redirect_host_and_scheme_are_rejected() {
        let base = format!("{BASE}card_search.action");
        assert!(redirect_url(&base, "https://evil.example/path").is_err());
        assert!(redirect_url(&base, "http://www.db.yugioh-card.com/path").is_err());
        assert!(redirect_url(&base, "//evil.example/path").is_err());
        assert!(redirect_url(&base, "/yugiohdb/card_search.action").is_ok());
    }

    #[test]
    fn names_are_atomically_replaced_with_python_format() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local-results")
            .join(format!("names-save-{}", std::process::id()));
        let user_dir = root.join("isolated-user-data");
        let mut fixture = Fixture(HashMap::from([
            (
                "ja".into(),
                include_bytes!("../tests/fixtures/names-ja.html").to_vec(),
            ),
            (
                "en".into(),
                include_bytes!("../tests/fixtures/names-en.html").to_vec(),
            ),
        ]));
        let cards = fetch_card_names(&mut fixture).unwrap();
        save_names(&user_dir, &cards, "2026-10-22 12:34").unwrap();
        save_names(&user_dir, &cards, "2026-10-22 12:35").unwrap();
        let saved: Value =
            serde_json::from_str(&fs::read_to_string(user_dir.join("card_names.json")).unwrap())
                .unwrap();
        assert_eq!(
            saved["meta"],
            json!({"format":1,"fetched":"2026-10-22 12:35","count":3})
        );
        assert_eq!(saved["cards"], Value::Object(cards));
        assert!(!user_dir.join("card_names.json.tmp").exists());
    }
}
