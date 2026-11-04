use crate::paths::{is_within, resolved};
use image::{ImageEncoder, RgbImage, RgbaImage};
use mdript_core::{
    catalog::Catalog,
    identify::{Screen, identify},
    language::{frame_twin, stroke_ratio},
    layout::Resolution,
};
use serde_json::{Value, json};
use std::{
    any::Any,
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{
        Condvar, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};
use unicode_normalization::UnicodeNormalization;
const SKIPPED: &str = "同じイラストがすでにあるのでスキップ";
#[derive(Clone)]
pub struct Options {
    pub output: PathBuf,
    pub user_data: PathBuf,
    pub format: String,
    pub quality: f32,
    pub rounded: bool,
    pub recursive: bool,
    pub duplicates: bool,
    pub review: bool,
    pub not_card: bool,
    pub english: String,
    pub split: String,
    pub detail: String,
    pub resolution: Resolution,
    pub dry: bool,
    pub csv: bool,
    pub workers: usize,
}
impl Options {
    /// Python 版 Options と同じ既定値。
    pub fn new(output: PathBuf, user_data: PathBuf) -> Self {
        Self {
            output,
            user_data,
            format: "webp".into(),
            quality: 90.,
            rounded: true,
            recursive: true,
            duplicates: true,
            review: true,
            not_card: true,
            english: "name".into(),
            split: "none".into(),
            detail: "folder".into(),
            resolution: Resolution::Auto,
            dry: false,
            csv: true,
            workers: thread::available_parallelism()
                .map_or(4, |n| n.get())
                .clamp(2, 8),
        }
    }
    /// 出力先とユーザーデータを正規化し、互いに重ならず、本番の APPDATA/mdript とも重ならないことを確かめる。
    pub fn validate_paths(&mut self) -> Result<(), String> {
        if !self.output.is_absolute() || !self.user_data.is_absolute() {
            return Err("output and user-data must be absolute paths".into());
        }
        self.output = resolved(&self.output)?;
        self.user_data = resolved(&self.user_data)?;
        if self.output.parent().is_none()
            || std::env::var_os("USERPROFILE").is_some_and(|home| {
                resolved(&PathBuf::from(home)).is_ok_and(|home| home == self.output)
            })
        {
            return Err("ここには保存できません。専用のフォルダを選んでください".into());
        }
        if self.output == self.user_data
            || self.output.starts_with(&self.user_data)
            || self.user_data.starts_with(&self.output)
        {
            return Err("ここには保存できません。専用のフォルダを選んでください".into());
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let live = resolved(&PathBuf::from(appdata).join("mdript"))?;
            if is_within(&self.output, &live) || is_within(&self.user_data, &live) {
                return Err(
                    "旧アプリのデータと重なる場所は使えません。専用のフォルダを選んでください"
                        .into(),
                );
            }
        }
        Ok(())
    }
}
fn take_value(args: &[OsString], i: &mut usize) -> Result<OsString, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or("option requires a value".into())
}
fn parse(args: Vec<OsString>) -> Result<(Options, Vec<PathBuf>), String> {
    let mut output = None;
    let mut user = None;
    let mut paths = vec![];
    let mut o = Options::new(PathBuf::new(), PathBuf::new());
    let mut i = 0;
    let mut positional = false;
    let mut card = false;
    let mut explicit = None;
    while i < args.len() {
        let arg = args[i].to_str().ok_or("option is not valid Unicode")?;
        if positional {
            paths.push(PathBuf::from(&args[i]));
            i += 1;
            continue;
        }
        match arg {
            "--" => positional = true,
            "-o" | "--output" => output = Some(PathBuf::from(take_value(&args, &mut i)?)),
            "--user-data" => user = Some(PathBuf::from(take_value(&args, &mut i)?)),
            "--format" => {
                o.format = take_value(&args, &mut i)?.to_string_lossy().into_owned();
                if !["png", "webp", "both"].contains(&o.format.as_str()) {
                    return Err("--format must be png, webp or both".into());
                }
            }
            "--quality" => {
                o.quality = take_value(&args, &mut i)?
                    .to_string_lossy()
                    .parse()
                    .map_err(|_| "quality must be an integer 1..100")?;
                if o.quality.fract() != 0. || !(1.0..=100.).contains(&o.quality) {
                    return Err("quality must be an integer 1..100".into());
                }
            }
            "--english" => {
                o.english = take_value(&args, &mut i)?.to_string_lossy().into_owned();
                if !["name", "ja", "off"].contains(&o.english.as_str()) {
                    return Err("invalid --english".into());
                }
            }
            "--split" => {
                o.split = take_value(&args, &mut i)?.to_string_lossy().into_owned();
                if !["none", "en", "both"].contains(&o.split.as_str()) {
                    return Err("invalid --split".into());
                }
            }
            "--detail" => {
                o.detail = take_value(&args, &mut i)?.to_string_lossy().into_owned();
                if !["folder", "suffix"].contains(&o.detail.as_str()) {
                    return Err("invalid --detail".into());
                }
            }
            "--resolution" => {
                let text = take_value(&args, &mut i)?.to_string_lossy().into_owned();
                o.resolution = match text.as_str() {
                    "auto" => Resolution::Auto,
                    "fixed" => Resolution::Fixed,
                    _ => {
                        let parts: Vec<_> = text.split(['x', 'X']).collect();
                        if parts.len() != 2 {
                            return Err("invalid resolution".into());
                        }
                        let w: u32 = parts[0].parse().map_err(|_| "invalid resolution")?;
                        let h: u32 = parts[1].parse().map_err(|_| "invalid resolution")?;
                        if w == 0 || h == 0 || w > 16384 || h > 16384 {
                            return Err("resolution must be 1..16384".into());
                        }
                        Resolution::Original(w, h)
                    }
                };
            }
            "--card" => card = true,
            "--box" => {
                let mut b = [0; 4];
                for v in &mut b {
                    *v = take_value(&args, &mut i)?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "box requires four integers")?;
                }
                explicit = Some(b);
            }
            "--no-rounded" => o.rounded = false,
            "--no-recursive" => o.recursive = false,
            "--keep-duplicates" => o.duplicates = false,
            "--no-review" => o.review = false,
            "--no-not-card" => o.not_card = false,
            "--dry-run" => o.dry = true,
            "--no-csv" => o.csv = false,
            "--workers" => {
                o.workers = take_value(&args, &mut i)?
                    .to_string_lossy()
                    .parse()
                    .map_err(|_| "--workers must be 1..256")?;
                if !(1..=256).contains(&o.workers) {
                    return Err("--workers must be 1..256".into());
                }
            }
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ => paths.push(PathBuf::from(&args[i])),
        }
        i += 1;
    }
    if card && explicit.is_some() {
        return Err("--card and --box are mutually exclusive".into());
    }
    if card {
        o.resolution = Resolution::Card;
    }
    if let Some(b) = explicit {
        o.resolution = Resolution::Box(b);
    }
    o.output = output.ok_or("identify requires --output ABSOLUTE_DIRECTORY")?;
    o.user_data = user.ok_or("identify requires --user-data ABSOLUTE_DIRECTORY")?;
    o.validate_paths()?;
    if paths.is_empty() {
        return Err("identify requires at least one input".into());
    }
    Ok((o, paths))
}
fn display_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(local) = text.strip_prefix(r"\\?\")
        && local.as_bytes().get(1) == Some(&b':')
    {
        local.to_owned()
    } else {
        text.into_owned()
    }
}
pub(crate) fn load_image(path: &Path) -> Result<RgbImage, String> {
    mdript_core::decode::load_rgb(path)
}

fn extension(p: &Path) -> bool {
    p.extension().and_then(|v| v.to_str()).is_some_and(|e| {
        ["png", "jpg", "jpeg", "webp", "bmp"].contains(&e.to_ascii_lowercase().as_str())
    })
}
fn visit(path: &Path, o: &Options, out: &mut Vec<PathBuf>) {
    let Ok(actual) = resolved(path) else {
        return;
    };
    if actual == o.output {
        return;
    }
    let Ok(entries) = fs::read_dir(path) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    let mut dirs = vec![];
    for e in entries {
        let Ok(ft) = e.file_type() else {
            continue;
        };
        if ft.is_dir() {
            if o.recursive {
                dirs.push(e.path());
            }
        } else if extension(&e.path()) {
            out.push(e.path());
        }
    }
    for d in dirs {
        visit(&d, o, out);
    }
}

/// 今回処理する画像だけを集め、出力先と対象外の拡張子を除く。
pub fn collect(paths: Vec<PathBuf>, o: &Options) -> Result<Vec<PathBuf>, String> {
    let mut out = vec![];
    for path in paths {
        let path = std::path::absolute(path).map_err(|e| e.to_string())?;
        if path.is_dir() {
            visit(&path, o, &mut out);
        } else if extension(&path) {
            out.push(path);
        }
    }
    let mut seen = HashSet::new();
    out.retain(|p| {
        !resolved(p).is_ok_and(|p| p.starts_with(&o.output))
            && seen.insert(p.to_string_lossy().to_lowercase())
    });
    Ok(out)
}
fn safe_filename(name: &str) -> String {
    let invalid = "\\/:*?\"<>|";
    let replacements = ['￥', '／', '：', '＊', '？', '”', '＜', '＞', '｜'];
    let name: String = name
        .nfc()
        .filter(|c| *c >= ' ')
        .map(|c| {
            invalid
                .chars()
                .position(|v| v == c)
                .map_or(c, |i| replacements[i])
        })
        .take(150)
        .collect();
    let name = name.trim().trim_end_matches([' ', '.']);
    if name.is_empty() {
        return "unknown".into();
    }
    let first = name.split('.').next().unwrap_or("").to_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&first.as_str())
        || ((first.starts_with("COM") || first.starts_with("LPT"))
            && first.len() == 4
            && ('1'..='9').contains(&first.chars().last().unwrap()));
    if reserved {
        format!("_{name}")
    } else {
        name.into()
    }
}
fn rounded(card: &RgbImage, enabled: bool) -> RgbaImage {
    let (w, h) = card.dimensions();
    let mut rgba = RgbaImage::from_fn(w, h, |x, y| {
        let p = card.get_pixel(x, y);
        image::Rgba([p[0], p[1], p[2], 255])
    });
    if !enabled {
        return rgba;
    }
    let radius = ((w as f64 * 22. / 894.).round_ties_even() as u32)
        .max(2)
        .min(w / 2)
        .min(h / 2);
    // Pillow Draw.c quarter ellipse traversal (Pillow license in third-party).
    let a = 2 * radius as i64;
    let (mut cx, mut cy) = (a, 0i64);
    let mut last_y = -1;
    let delta = |x: i64, y: i64| (a * a * y * y + a * a * x * x - a * a * a * a).abs();
    loop {
        if cy != last_y {
            let y = ((a - cy) / 2) as u32;
            let start = ((a - cx) / 2) as u32;
            for x in 0..start {
                for (xx, yy) in [
                    (x, y),
                    (w - 1 - x, y),
                    (x, h - 1 - y),
                    (w - 1 - x, h - 1 - y),
                ] {
                    rgba.get_pixel_mut(xx, yy)[3] = 0;
                }
            }
            last_y = cy;
        }
        if cx == 0 && cy == a {
            break;
        }
        let (mut nx, mut ny) = (cx, cy + 2);
        let mut ndelta = delta(nx, ny);
        if nx > 1 {
            let d = delta(cx - 2, cy + 2);
            if ndelta > d {
                nx = cx - 2;
                ny = cy + 2;
                ndelta = d;
            }
            if ndelta > delta(cx - 2, cy) {
                nx = cx - 2;
                ny = cy;
            }
        }
        cx = nx;
        cy = ny;
    }
    rgba
}
fn encode(
    card: &RgbImage,
    o: &Options,
    rounded: bool,
) -> Result<Vec<(&'static str, Vec<u8>)>, String> {
    let rgba = rounded_image(card, rounded);
    let (w, h) = card.dimensions();
    let mut out = vec![];
    if o.format != "png" {
        let mut cfg = webp::WebPConfig::new().map_err(|_| "WebP configuration failure")?;
        cfg.quality = o.quality;
        cfg.method = 4;
        let bytes = webp::Encoder::from_rgba(rgba.as_raw(), w, h)
            .encode_advanced(&cfg)
            .map_err(|e| format!("WebP encoding: {e:?}"))?;
        out.push(("webp", bytes.to_vec()));
    }
    if o.format != "webp" {
        let mut bytes = vec![];
        let encoder = image::codecs::png::PngEncoder::new_with_quality(
            &mut bytes,
            image::codecs::png::CompressionType::Default,
            image::codecs::png::FilterType::Adaptive,
        );
        if rounded {
            encoder.write_image(rgba.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        } else {
            encoder.write_image(card.as_raw(), w, h, image::ExtendedColorType::Rgb8)
        }
        .map_err(|e| e.to_string())?;
        out.push(("png", bytes));
    }
    Ok(out)
}
fn rounded_image(card: &RgbImage, enabled: bool) -> RgbaImage {
    rounded(card, enabled)
}
#[derive(Default)]
struct Saver {
    hashes: HashMap<PathBuf, String>,
    pending: HashSet<PathBuf>,
}
struct Reservation {
    paths: Vec<PathBuf>,
    owned: Vec<(PathBuf, File)>,
    hash: String,
}
enum Choice {
    Reserved(Reservation),
    /// 同じイラストの既存ファイル
    Skipped(PathBuf),
    Wait,
}
impl Reservation {
    fn write(&mut self, card: &RgbImage, o: &Options, round: bool) -> Result<(), String> {
        #[cfg(test)]
        if std::env::var_os("MDRIPT_TEST_PANIC_WRITE").is_some() {
            static PANICKED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !PANICKED.swap(true, Ordering::SeqCst) {
                panic!("injected write panic");
            }
        }
        let bytes = encode(card, o, round)?;
        for ((_, file), (_, bytes)) in self.owned.iter_mut().zip(bytes) {
            file.write_all(&bytes).map_err(|e| e.to_string())?;
            file.flush().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}
impl Saver {
    fn reserve(
        &mut self,
        target: &Path,
        name: &str,
        hash: Option<&str>,
        o: &Options,
    ) -> Result<Choice, String> {
        if !resolved(target)?.starts_with(&o.output) {
            return Err("保存先が指定した出力先フォルダの外を指しています".into());
        }
        fs::create_dir_all(target).map_err(|e| e.to_string())?;
        let extensions = match o.format.as_str() {
            "png" => vec!["png"],
            "both" => vec!["webp", "png"],
            _ => vec!["webp"],
        };
        let base = safe_filename(name);
        for index in 0u64.. {
            let stem = if index == 0 {
                base.clone()
            } else {
                format!("{base}_alt{index}")
            };
            let paths: Vec<_> = extensions
                .iter()
                .map(|e| target.join(format!("{stem}.{e}")))
                .collect();
            if paths.iter().any(|p| self.pending.contains(p)) {
                return Ok(Choice::Wait);
            }
            let taken: Vec<_> = paths.iter().filter(|p| p.exists()).collect();
            if taken.is_empty() {
                let mut owned: Vec<(PathBuf, File)> = vec![];
                let mut retry = false;
                for path in &paths {
                    match OpenOptions::new().write(true).create_new(true).open(path) {
                        Ok(file) => owned.push((path.clone(), file)),
                        Err(e) => {
                            retry = e.kind() == std::io::ErrorKind::AlreadyExists;
                            for (p, file) in owned.drain(..) {
                                drop(file);
                                let _ = fs::remove_file(p);
                            }
                            if !retry {
                                return Err(e.to_string());
                            }
                            break;
                        }
                    }
                }
                if retry {
                    continue;
                }
                self.pending.extend(paths.iter().cloned());
                return Ok(Choice::Reserved(Reservation {
                    paths,
                    owned,
                    hash: hash.unwrap_or("").into(),
                }));
            }
            if o.duplicates
                && let Some(hash) = hash
            {
                for p in taken {
                    let other = self.hashes.entry(p.clone()).or_insert_with(|| {
                        load_image(p)
                            .ok()
                            .and_then(|im| {
                                mdript_core::artwork_hash(
                                    &im,
                                    [0, 0, im.width() as i64, im.height() as i64],
                                    false,
                                    false,
                                )
                                .ok()
                            })
                            .unwrap_or_default()
                    });
                    if let (Ok(a), Ok(b)) = (
                        u128::from_str_radix(other, 16),
                        u128::from_str_radix(hash, 16),
                    ) && (a ^ b).count_ones() <= 10
                    {
                        return Ok(Choice::Skipped(p.clone()));
                    }
                }
            }
        }
        unreachable!()
    }
    fn complete(&mut self, reservation: Reservation, success: bool) {
        let Reservation { paths, owned, hash } = reservation;
        drop(owned);
        if success {
            for path in &paths {
                self.hashes.insert(path.clone(), hash.clone());
            }
        } else {
            for path in &paths {
                let _ = fs::remove_file(path);
            }
        }
        for path in paths {
            self.pending.remove(&path);
        }
    }
    #[cfg(test)]
    fn save(
        &mut self,
        card: &RgbImage,
        target: &Path,
        name: &str,
        hash: Option<&str>,
        o: &Options,
        round: bool,
    ) -> Result<(String, String), String> {
        if o.dry {
            return Ok((String::new(), String::new()));
        }
        let mut reservation = match self.reserve(target, name, hash, o)? {
            Choice::Reserved(r) => r,
            Choice::Skipped(_) => return Ok((String::new(), SKIPPED.into())),
            Choice::Wait => return Err("unexpected pending reservation".into()),
        };
        let path = display_path(&reservation.paths[0]);
        let written = reservation.write(card, o, round);
        self.complete(reservation, written.is_ok());
        written.map(|()| (path, String::new()))
    }
}
struct SaveTask {
    card: RgbImage,
    target: PathBuf,
    name: String,
    hash: Option<String>,
    round: bool,
    propagate_skip: bool,
}
struct Prepared {
    result: Value,
    save: Option<SaveTask>,
}
impl Prepared {
    fn complete(result: Value) -> Self {
        Self { result, save: None }
    }
}
fn prepare(path: &Path, im: RgbImage, catalog: &Catalog, o: &Options) -> Result<Prepared, String> {
    #[cfg(test)]
    if std::env::var_os("MDRIPT_TEST_PANIC_PREPARE")
        .is_some_and(|name| path.file_name() == Some(name.as_os_str()))
    {
        panic!("injected prepare panic");
    }
    let mut result = json!({"source":path,"size":[im.width(),im.height()],"status":"カードなし","card_id":"","name":"","distance":999,"margin":0,"saved_path":"","note":"","english":false,"screen":null,"box":null,"origin":null,"hash":null,"ranked":[],"stroke_ratio":null});
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let Some(found) = identify(&im, catalog, &o.resolution) else {
        result["note"] = json!("照合できる画像がありません");
        return Ok(Prepared::complete(result));
    };
    result["distance"] = json!(found.distance);
    result["margin"] = json!(found.margin);
    result["box"] = json!(found.card_box);
    result["origin"] = json!(if found.estimated {
        "estimated"
    } else {
        "trusted"
    });
    result["hash"] = json!(found.hash);
    result["ranked"] = json!(found.ranked);
    let card = found.card(&im);
    if found.distance > 36 {
        result["note"] = json!("カードの拡大画面か詳細画面ではないようです");
        if o.not_card {
            return Ok(Prepared {
                result,
                save: Some(SaveTask {
                    card,
                    target: o.output.join("not_card"),
                    name: stem.into_owned(),
                    hash: Some(found.hash),
                    round: o.rounded,
                    propagate_skip: true,
                }),
            });
        } else {
            result["margin"] = json!(0);
        }
        return Ok(Prepared::complete(result));
    }
    result["screen"] = json!(found.screen.as_str());
    let cid = frame_twin(&card, &found.card_id);
    let japanese = catalog.display_name(&cid, false);
    let mut name = japanese.clone();
    let mut note = String::new();
    let ratio = stroke_ratio(&card);
    let english = o.english != "off" && ratio.is_some_and(|v| v <= 0.75);
    result["stroke_ratio"] = json!(ratio);
    if english {
        note = "英語版".into();
        if o.english == "name" {
            if catalog.has_english_name(&cid) {
                name = catalog.display_name(&cid, true);
            } else {
                note = "英語版（英語名が未取得のため日本語名で保存）".into();
            }
        }
        if o.split == "none" && name == japanese {
            name.push_str("_en");
        }
    }
    let detail = found.screen == Screen::Detail;
    if detail && o.detail == "suffix" {
        name.push_str("_detail");
    }
    let confirmed = found.confirmed();
    result["status"] = json!(if confirmed { "確定" } else { "要確認" });
    result["card_id"] = json!(cid);
    result["name"] = json!(name);
    result["english"] = json!(english);
    if !confirmed && !o.review {
        result["note"] = json!("要確認のため保存しませんでした");
        return Ok(Prepared::complete(result));
    }
    let root = if detail && o.detail == "folder" {
        o.output.join("detail")
    } else {
        o.output.clone()
    };
    let mut target = if confirmed { root } else { root.join("review") };
    if o.english != "off" {
        if english && ["en", "both"].contains(&o.split.as_str()) {
            target.push("en");
        } else if !english && o.split == "both" {
            target.push("ja");
        }
    }
    result["note"] = json!(note);
    Ok(Prepared {
        result,
        save: Some(SaveTask {
            card,
            target,
            name,
            hash: Some(found.hash),
            round: o.rounded,
            propagate_skip: true,
        }),
    })
}
#[derive(Default)]
struct SequenceState {
    next_turn: usize,
    saver: Saver,
}
#[derive(Default)]
struct Sequencer {
    state: Mutex<SequenceState>,
    changed: Condvar,
}
fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
fn wait_recover<'a, T>(changed: &Condvar, state: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    changed
        .wait(state)
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    let detail = if let Some(message) = payload.downcast_ref::<String>() {
        message.as_str()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        message
    } else {
        "non-string panic payload"
    };
    format!("panic: {detail}")
}
struct TurnGuard<'a> {
    sequencer: &'a Sequencer,
    turn: usize,
    advanced: bool,
}
impl TurnGuard<'_> {
    fn advance(&mut self, state: &mut SequenceState) {
        if !self.advanced && state.next_turn == self.turn {
            state.next_turn += 1;
            self.advanced = true;
            self.sequencer.changed.notify_all();
        }
    }
}
impl Drop for TurnGuard<'_> {
    fn drop(&mut self) {
        if self.advanced {
            return;
        }
        let mut state = lock_recover(&self.sequencer.state);
        while state.next_turn < self.turn {
            state = wait_recover(&self.sequencer.changed, state);
        }
        self.advance(&mut state);
    }
}
struct PendingGuard<'a> {
    sequencer: &'a Sequencer,
    reservation: Option<Reservation>,
    success: bool,
}
impl PendingGuard<'_> {
    fn write(&mut self, card: &RgbImage, o: &Options, round: bool) -> Result<(), String> {
        self.reservation
            .as_mut()
            .expect("reservation remains owned until completion")
            .write(card, o, round)
    }
}
impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if let Some(reservation) = self.reservation.take() {
            let mut state = lock_recover(&self.sequencer.state);
            state.saver.complete(reservation, self.success);
            self.sequencer.changed.notify_all();
        }
    }
}
impl Sequencer {
    fn finish(
        &self,
        turn: usize,
        prepared: Result<Prepared, String>,
        o: &Options,
    ) -> Result<Value, String> {
        let mut turn_guard = TurnGuard {
            sequencer: self,
            turn,
            advanced: false,
        };
        let mut state = lock_recover(&self.state);
        while state.next_turn < turn {
            state = wait_recover(&self.changed, state);
        }
        #[cfg(test)]
        if turn == 0 && std::env::var_os("MDRIPT_TEST_PANIC_WITH_LOCK").is_some() {
            panic!("injected mutex poison panic");
        }
        let Prepared { mut result, save } = prepared?;
        let Some(task) = save else {
            return Ok(result);
        };
        if o.dry {
            return Ok(result);
        }
        let reservation = loop {
            match state
                .saver
                .reserve(&task.target, &task.name, task.hash.as_deref(), o)
            {
                Ok(Choice::Wait) => state = wait_recover(&self.changed, state),
                Ok(Choice::Skipped(existing)) => {
                    if task.propagate_skip {
                        result["note"] = json!(SKIPPED);
                    }
                    // 保存しなかった代わりに、同じイラストの既存ファイルを示す（画面のサムネイル用。CSV には出さない）
                    result["existing_path"] = json!(display_path(&existing));
                    return Ok(result);
                }
                Ok(Choice::Reserved(reservation)) => break reservation,
                Err(error) => return Err(error),
            }
        };
        turn_guard.advance(&mut state);
        drop(state);

        let saved_path = display_path(&reservation.paths[0]);
        let saved_paths: Vec<String> = reservation
            .paths
            .iter()
            .map(|path| display_path(path))
            .collect();
        let mut pending = PendingGuard {
            sequencer: self,
            reservation: Some(reservation),
            success: false,
        };
        let written = catch_unwind(AssertUnwindSafe(|| {
            pending.write(&task.card, o, task.round)
        }))
        .unwrap_or_else(|payload| Err(panic_message(payload)));
        pending.success = written.is_ok();
        drop(pending);
        written?;
        result["saved_path"] = json!(saved_path);
        // この結果が実際に作ったファイルすべて（both 形式なら 2 つ）。登録後の掃除はこれだけを消す
        result["saved_paths"] = json!(saved_paths);
        Ok(result)
    }
}
fn error_result(path: &Path, error: String) -> Value {
    failure_result(path, format!("画像を処理できませんでした（{error}）"))
}

fn failure_result(path: &Path, error: String) -> Value {
    json!({"source":path,"status":"エラー","card_id":"","name":"","distance":999,"margin":0,"saved_path":"","note":error,"english":false})
}
pub fn run(args: Vec<OsString>) -> Result<(), String> {
    if args.len() == 1 && args[0] == "--help" {
        println!(
            "mdript-cli identify PATH... --output ABS_DIR --user-data ABS_DIR [--format webp|png|both] [--quality 1..100] [--resolution auto|fixed|WxH] [--card|--box L T R B] [--english name|ja|off] [--split none|en|both] [--detail folder|suffix] [--workers 1..256] [--dry-run] [--no-csv] [--keep-duplicates] [--no-rounded] [--no-recursive] [--no-review] [--no-not-card]\nUser data is optional read-only copied card_names.json/card_hashes_user.json. No implicit APPDATA access. JSON summary on stdout; errors exit 1."
        );
        return Ok(());
    }
    let (o, paths) = parse(args)?;
    let summary = identify_paths(&o, paths, &AtomicBool::new(false), &|_, _, _| {})?;
    println!("{summary}");
    if let Some(csv) = summary["csv_error"].as_object() {
        eprintln!(
            "warning: could not write the log file {}: {}",
            csv["path"].as_str().unwrap_or(""),
            csv["message"].as_str().unwrap_or("")
        );
    }
    let error = summary["error"].as_u64().unwrap_or(0);
    if error > 0 {
        return Err(format!("{error} input(s) failed"));
    }
    Ok(())
}

/// 入力を集めて判定・保存し、件数と結果一覧の JSON を返す。
/// `on_result(完了数, 全体数, 結果)` は画像1枚ごとに呼ばれる（完了順）。
/// `cancel` が立った後の画像は処理せず、結果にも含めない（Python 版の should_stop と同じ）。
pub fn identify_paths(
    o: &Options,
    inputs: Vec<PathBuf>,
    cancel: &AtomicBool,
    on_result: &(dyn Fn(usize, usize, &Value) + Sync),
) -> Result<Value, String> {
    identify_files(o, collect(inputs, o)?, cancel, on_result)
}

/// 収集済みの画像一覧をそのまま処理する。フォルダの走査・再収集は行わない。
pub fn identify_files(
    o: &Options,
    paths: Vec<PathBuf>,
    cancel: &AtomicBool,
    on_result: &(dyn Fn(usize, usize, &Value) + Sync),
) -> Result<Value, String> {
    let started = Instant::now();
    let user = fs::read_to_string(o.user_data.join("card_hashes_user.json")).ok();
    let names = fs::read_to_string(o.user_data.join("card_names.json")).ok();
    let catalog = Catalog::bundled(user.as_deref(), names.as_deref())?;
    let sequencer = Sequencer::default();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let slots = Mutex::new(vec![None; paths.len()]);
    thread::scope(|scope| {
        for _ in 0..o.workers.min(paths.len()) {
            let paths = &paths;
            let catalog = &catalog;
            let sequencer = &sequencer;
            let next = &next;
            let done = &done;
            let slots = &slots;
            scope.spawn(move || {
                loop {
                    let turn = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(turn) else { break };
                    if cancel.load(Ordering::SeqCst) {
                        // 順番だけ進め、結果には含めない
                        let skipped = Ok(Prepared::complete(Value::Null));
                        let _ =
                            catch_unwind(AssertUnwindSafe(|| sequencer.finish(turn, skipped, o)));
                        lock_recover(slots)[turn] = Some(Value::Null);
                        continue;
                    }
                    let prepared = catch_unwind(AssertUnwindSafe(|| match load_image(path) {
                        Ok(im) => prepare(path, im, catalog, o),
                        // 読み込みの説明は復号側で付ける。保存・判定の失敗とは別の経路にする。
                        Err(error) => Ok(Prepared::complete(failure_result(path, error))),
                    }))
                    .unwrap_or_else(|payload| Err(panic_message(payload)));
                    let result =
                        catch_unwind(AssertUnwindSafe(|| sequencer.finish(turn, prepared, o)))
                            .unwrap_or_else(|payload| Err(panic_message(payload)))
                            .unwrap_or_else(|error| error_result(path, error));
                    let count = done.fetch_add(1, Ordering::SeqCst) + 1;
                    let _ =
                        catch_unwind(AssertUnwindSafe(|| on_result(count, paths.len(), &result)));
                    lock_recover(slots)[turn] = Some(result);
                }
            });
        }
    });
    let results: Vec<Value> = slots
        .into_inner()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .into_iter()
        .enumerate()
        .map(|(turn, result)| {
            result.unwrap_or_else(|| {
                error_result(
                    &paths[turn],
                    "画像の処理が結果を返す前に終了しました".into(),
                )
            })
        })
        .filter(|result| !result.is_null())
        .collect();
    let (mut confirmed, mut review, mut not_card, mut error, mut skipped) = (0, 0, 0, 0, 0);
    for r in &results {
        match r["status"].as_str() {
            Some("確定") => confirmed += 1,
            Some("要確認") => review += 1,
            Some("カードなし") => not_card += 1,
            _ => error += 1,
        }
        if r["note"] == SKIPPED {
            skipped += 1;
        }
    }
    // ログを書けなくても画像の結果は残す。件数には混ぜず、別に知らせる
    let csv_error = if o.csv && !o.dry && !results.is_empty() {
        crate::csv_log::write_csv(&o.output, &results).err().map(
            |e| json!({"path":display_path(&o.output.join("log.csv")),"message":e.to_string()}),
        )
    } else {
        None
    };
    Ok(
        json!({"confirmed":confirmed,"review":review,"not_card":not_card,"error":error,"skipped":skipped,"seconds":started.elapsed().as_secs_f64(),"total":paths.len(),"cancelled":cancel.load(Ordering::SeqCst),"results":results,"catalog_hashes":catalog.hash_count(),"csv_error":csv_error}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        sync::atomic::{AtomicU64, Ordering},
        time::Duration,
    };
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "mdript-rust-test-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Self(fs::canonicalize(p).unwrap())
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn options(dir: &Temp) -> Options {
        Options {
            output: dir.0.join("output"),
            user_data: dir.0.join("user"),
            format: "png".into(),
            quality: 90.,
            rounded: true,
            recursive: true,
            duplicates: false,
            review: true,
            not_card: true,
            english: "name".into(),
            split: "none".into(),
            detail: "folder".into(),
            resolution: Resolution::Auto,
            dry: false,
            csv: true,
            workers: 1,
        }
    }
    #[test]
    fn panic_test_child() {
        let Some(root) = std::env::var_os("MDRIPT_TEST_PANIC_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        let args = vec![
            OsString::from("identify"),
            root.join("input").into_os_string(),
            OsString::from("--output"),
            root.join("output").into_os_string(),
            OsString::from("--user-data"),
            root.join("user").into_os_string(),
            OsString::from("--card"),
            OsString::from("--format"),
            OsString::from("png"),
            OsString::from("--no-rounded"),
            OsString::from("--keep-duplicates"),
            OsString::from("--workers"),
            OsString::from("8"),
        ];
        assert!(run(args).is_err());
    }
    #[test]
    fn panics_release_turns_and_files_before_timeout() {
        for scenario in ["prepare", "write", "poison"] {
            let root = Temp::new();
            let input = root.0.join("input");
            let first = input.join("a");
            let second = input.join("b");
            fs::create_dir_all(&first).unwrap();
            fs::create_dir_all(&second).unwrap();
            let first_name = if scenario == "write" {
                "same.png"
            } else {
                "bad.png"
            };
            let second_name = if scenario == "write" {
                "same.png"
            } else {
                "good.png"
            };
            RgbImage::from_pixel(60, 80, image::Rgb([10, 20, 30]))
                .save(first.join(first_name))
                .unwrap();
            RgbImage::from_pixel(60, 80, image::Rgb([40, 50, 60]))
                .save(second.join(second_name))
                .unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args(["--exact", "process::tests::panic_test_child", "--nocapture"])
                .env("MDRIPT_TEST_PANIC_ROOT", &root.0)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            match scenario {
                "prepare" => {
                    command.env("MDRIPT_TEST_PANIC_PREPARE", first_name);
                }
                "write" => {
                    command.env("MDRIPT_TEST_PANIC_WRITE", "1");
                }
                "poison" => {
                    command.env("MDRIPT_TEST_PANIC_WITH_LOCK", "1");
                }
                _ => unreachable!(),
            }
            let mut child = command.spawn().unwrap();
            let started = Instant::now();
            loop {
                if child.try_wait().unwrap().is_some() {
                    break;
                }
                if started.elapsed() > Duration::from_secs(10) {
                    child.kill().unwrap();
                    let _ = child.wait();
                    panic!("{scenario}: child did not finish within 10 seconds");
                }
                thread::sleep(Duration::from_millis(20));
            }
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{scenario}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8(output.stdout).unwrap();
            let summary: Value = stdout
                .lines()
                .filter_map(|line| {
                    line.find('{')
                        .and_then(|start| serde_json::from_str(&line[start..]).ok())
                })
                .find(|value: &Value| value.get("results").is_some())
                .expect("child printed result JSON");
            assert_eq!(summary["error"], 1, "{scenario}");
            let rows = summary["results"].as_array().unwrap();
            assert_eq!(rows.len(), 2);
            assert_eq!(rows[0]["status"], "エラー", "{scenario}");
            assert!(rows[0]["note"].as_str().unwrap().contains("panic:"));
            assert_ne!(rows[1]["status"], "エラー", "{scenario}");
            let saved = PathBuf::from(rows[1]["saved_path"].as_str().unwrap());
            assert!(saved.is_file(), "{scenario}: later image was not saved");
            assert_eq!(saved.file_name().unwrap(), second_name, "{scenario}");
            let files: Vec<_> = fs::read_dir(saved.parent().unwrap())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
                .collect();
            assert_eq!(files.len(), 1, "{scenario}: failed reservation left a file");
        }
    }
    #[test]
    fn vanished_directory_does_not_stop_collecting_other_images() {
        let temp = Temp::new();
        let o = options(&temp);
        let vanished = temp.0.join("vanished");
        fs::create_dir(&vanished).unwrap();
        fs::remove_dir(&vanished).unwrap();
        let remaining = temp.0.join("remaining");
        fs::create_dir(&remaining).unwrap();
        let image = remaining.join("card.png");
        fs::write(&image, b"fixture").unwrap();
        fs::create_dir(&o.output).unwrap();
        fs::write(o.output.join("previous.png"), b"ignored").unwrap();

        let mut found = Vec::new();
        visit(&vanished, &o, &mut found);
        visit(&remaining, &o, &mut found);
        assert_eq!(found, vec![image.clone()]);
        assert!(
            collect(vec![temp.0.join("missing")], &o)
                .unwrap()
                .is_empty()
        );
        assert_eq!(collect(vec![temp.0.clone()], &o).unwrap(), vec![image]);
    }
    #[test]
    fn safe_names_and_unicode() {
        assert_eq!(safe_filename(" ../CON.txt. "), "..／CON.txt");
        assert_eq!(safe_filename("CON.txt"), "_CON.txt");
        assert_eq!(safe_filename("\u{FA19}"), "神");
        assert_eq!(safe_filename("\n . "), "unknown");
        assert_eq!(safe_filename("a/b\\c"), "a／b￥c");
    }
    #[test]
    fn display_path_keeps_drive_and_unc_paths_without_device_prefix() {
        assert_eq!(
            display_path(Path::new(r"\\?\C:\cards\one.webp")),
            r"C:\cards\one.webp"
        );
        assert_eq!(
            display_path(Path::new(r"\\?\UNC\host\share\one.webp")),
            r"\\host\share\one.webp"
        );
        assert_eq!(
            display_path(Path::new(r"C:\cards\one.webp")),
            r"C:\cards\one.webp"
        );
    }
    #[test]
    fn overlapping_and_root_outputs_explain_how_to_choose_a_folder() {
        let temp = Temp::new();
        let mut o = options(&temp);
        o.output = temp.0.clone();
        assert_eq!(
            o.validate_paths().unwrap_err(),
            "ここには保存できません。専用のフォルダを選んでください"
        );
        o.output = temp.0.ancestors().last().unwrap().to_path_buf();
        assert_eq!(
            o.validate_paths().unwrap_err(),
            "ここには保存できません。専用のフォルダを選んでください"
        );
    }

    #[test]
    fn options_reject_unsafe_or_unknown() {
        let temp = Temp::new();
        let o = options(&temp);
        let args = vec![
            OsString::from("image.png"),
            OsString::from("--output"),
            o.output.into_os_string(),
            OsString::from("--user-data"),
            o.user_data.into_os_string(),
        ];
        assert!(parse(args.clone()).is_ok());
        let mut bad = args.clone();
        bad.push("--wat".into());
        assert!(parse(bad).is_err());
        let mut bad = args.clone();
        bad.extend([OsString::from("--quality"), OsString::from("NaN")]);
        assert!(parse(bad).is_err());
        let mut one = args.clone();
        one.extend([OsString::from("--workers"), OsString::from("1")]);
        assert_eq!(parse(one).unwrap().0.workers, 1);
        let mut bad = args.clone();
        bad.extend([OsString::from("--workers"), OsString::from("0")]);
        assert!(parse(bad).is_err());
        assert!(parse(vec!["image.png".into()]).is_err());
    }
    #[test]
    fn cross_instance_reservations_do_not_overwrite() {
        let temp = Temp::new();
        let o = options(&temp);
        let card = RgbImage::from_pixel(40, 60, image::Rgb([32, 64, 96]));
        let mut joins = vec![];
        for _ in 0..2 {
            let o = o.clone();
            let card = card.clone();
            joins.push(std::thread::spawn(move || {
                Saver::default()
                    .save(&card, &o.output, "same", None, &o, false)
                    .unwrap()
                    .0
            }));
        }
        let paths: Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
        assert_ne!(paths[0], paths[1]);
        assert_eq!(fs::read_dir(&o.output).unwrap().count(), 2);
        for p in paths {
            assert_eq!(load_image(Path::new(&p)).unwrap(), card);
        }
    }
    #[test]
    fn both_failure_removes_only_own_reservations() {
        let temp = Temp::new();
        let mut o = options(&temp);
        o.format = "both".into();
        o.quality = -1.;
        fs::create_dir_all(&o.output).unwrap();
        let existing = o.output.join("keep.webp");
        fs::write(&existing, b"existing").unwrap();
        let err =
            Saver::default().save(&RgbImage::new(40, 60), &o.output, "broken", None, &o, true);
        assert!(err.is_err());
        assert_eq!(fs::read(&existing).unwrap(), b"existing");
        assert_eq!(fs::read_dir(&o.output).unwrap().count(), 1);
    }
    #[test]
    fn failed_pending_save_releases_base_name_for_next_turn() {
        let temp = Temp::new();
        let mut bad = options(&temp);
        bad.format = "both".into();
        bad.quality = -1.;
        let mut good = bad.clone();
        good.quality = 90.;
        let sequencer = Sequencer::default();
        let card = RgbImage::from_pixel(40, 60, image::Rgb([12, 34, 56]));
        let make_task = |card: RgbImage| Prepared {
            result: json!({"status":"要確認","saved_path":"","note":""}),
            save: Some(SaveTask {
                card,
                target: good.output.clone(),
                name: "same".into(),
                hash: None,
                round: true,
                propagate_skip: true,
            }),
        };
        std::thread::scope(|scope| {
            let first = scope.spawn(|| sequencer.finish(0, Ok(make_task(card.clone())), &bad));
            let second = scope.spawn(|| sequencer.finish(1, Ok(make_task(card.clone())), &good));
            assert!(first.join().unwrap().is_err());
            let result = second.join().unwrap().unwrap();
            assert!(
                result["saved_path"]
                    .as_str()
                    .unwrap()
                    .ends_with("same.webp")
            );
        });
        assert!(good.output.join("same.webp").exists());
        assert!(good.output.join("same.png").exists());
        assert_eq!(fs::read_dir(&good.output).unwrap().count(), 2);
    }
    #[test]
    fn duplicates_are_checked_across_instances() {
        let temp = Temp::new();
        let mut o = options(&temp);
        o.duplicates = true;
        let card = RgbImage::new(40, 60);
        let hash = "0".repeat(32);
        let first = Saver::default()
            .save(&card, &o.output, "card", Some(&hash), &o, false)
            .unwrap();
        let second = Saver::default()
            .save(&card, &o.output, "card", Some(&hash), &o, false)
            .unwrap();
        assert!(!first.0.is_empty());
        assert_eq!(second, (String::new(), SKIPPED.into()));
    }
    #[test]
    fn dry_run_has_no_filesystem_side_effects() {
        let temp = Temp::new();
        let mut o = options(&temp);
        o.dry = true;
        Saver::default()
            .save(&RgbImage::new(40, 60), &o.output, "x", None, &o, true)
            .unwrap();
        assert!(!o.output.exists());
        assert!(!o.user_data.exists());
    }
    #[test]
    fn unsafe_target_is_rejected_before_directory_creation() {
        let temp = Temp::new();
        let o = options(&temp);
        let outside = temp.0.join("outside/new");
        assert!(
            Saver::default()
                .save(&RgbImage::new(40, 60), &outside, "x", None, &o, true)
                .is_err()
        );
        assert!(!outside.exists());
    }
    fn plain_input(temp: &Temp) -> PathBuf {
        let input = temp.0.join("input");
        fs::create_dir_all(&input).unwrap();
        RgbImage::from_pixel(60, 80, image::Rgb([10, 20, 30]))
            .save(input.join("shot.png"))
            .unwrap();
        input
    }
    fn run_one(o: &Options, input: PathBuf) -> Value {
        identify_paths(o, vec![input], &AtomicBool::new(false), &|_, _, _| {}).unwrap()
    }
    #[test]
    fn mixed_languages_use_localized_names_in_webp_folders() {
        let temp = Temp::new();
        let input = temp.0.join("input");
        fs::create_dir_all(&input).unwrap();
        let mut card = RgbImage::from_fn(200, 300, |x, y| {
            image::Rgb([
                ((x * 17 + y * 3) % 256) as u8,
                ((x * 7 + y * 19) % 256) as u8,
                ((x * 11 + y * 13) % 256) as u8,
            ])
        });
        let hash = mdript_core::artwork_hash(&card, [0, 0, 200, 300], false, false).unwrap();
        for (language, vertical) in [("ja", false), ("en", true)] {
            for y in 226..269 {
                for x in 14..186 {
                    let ink = if vertical { x % 8 < 2 } else { y % 8 < 2 };
                    card.put_pixel(x, y, image::Rgb([if ink { 0 } else { 240 }; 3]));
                }
            }
            card.save(input.join(format!("{language}.png"))).unwrap();
        }
        let mut o = options(&temp);
        o.resolution = Resolution::Card;
        o.format = "webp".into();
        o.split = "both".into();
        o.workers = 2;
        fs::create_dir_all(&o.user_data).unwrap();
        fs::write(
            o.user_data.join("card_hashes_user.json"),
            json!({"meta":{"format":2},"hashes":{"99999":[hash]}}).to_string(),
        )
        .unwrap();
        fs::write(
            o.user_data.join("card_names.json"),
            json!({"cards":{"99999":{"n":"テストカード","e":"Test Card"}}}).to_string(),
        )
        .unwrap();
        let summary = run_one(&o, input);
        assert_eq!(summary["confirmed"], 2, "{summary}");
        assert_eq!(summary["skipped"], 0);
        assert!(o.output.join("ja/テストカード.webp").is_file());
        assert!(o.output.join("en/Test Card.webp").is_file());
    }

    #[test]
    fn collected_files_are_processed_without_collecting_again() {
        let temp = Temp::new();
        let input = plain_input(&temp);
        let mut o = options(&temp);
        o.dry = true;
        let files = collect(vec![input.clone()], &o).unwrap();
        fs::copy(input.join("shot.png"), input.join("added.png")).unwrap();
        let summary = identify_files(&o, files, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
        assert_eq!(summary["total"], 1);
        let summary =
            identify_files(&o, vec![input], &AtomicBool::new(false), &|_, _, _| {}).unwrap();
        assert_eq!(summary["error"], 1);
    }

    #[test]
    fn error_wrapping_depends_on_the_stage_not_the_wording() {
        let path = Path::new("image.png");
        for wording in ["画像を読めません", "読み込みに失敗しました", "別の文言"]
        {
            assert_eq!(failure_result(path, wording.into())["note"], wording);
            assert_eq!(
                error_result(path, wording.into())["note"],
                format!("画像を処理できませんでした（{wording}）")
            );
        }
    }

    #[test]
    fn unreadable_image_result_has_a_short_japanese_explanation() {
        let temp = Temp::new();
        let source = temp.0.join("broken.png");
        fs::write(&source, b"broken").unwrap();
        let summary = run_one(&options(&temp), source);
        assert_eq!(summary["error"], 1);
        assert!(
            summary["results"][0]["note"]
                .as_str()
                .unwrap()
                .starts_with("画像を読み込めませんでした（")
        );
        assert_eq!(summary["results"][0]["saved_path"], "");
    }

    #[test]
    fn image_without_card_frame_is_not_copied() {
        let temp = Temp::new();
        let input = plain_input(&temp);
        RgbImage::new(30, 40).save(input.join("shot.png")).unwrap();
        let o = options(&temp);
        let summary = run_one(&o, input.clone());
        let result = &summary["results"][0];
        assert_eq!(result["note"], "照合できる画像がありません");
        assert_eq!(result["saved_path"], "");
        assert_eq!(
            Path::new(result["source"].as_str().unwrap()),
            input.join("shot.png")
        );
        assert!(!o.output.join("not_card").exists());
    }

    #[test]
    fn unmatched_card_frame_saves_only_the_crop() {
        let temp = Temp::new();
        let source = temp.0.join("private-photo.png");
        RgbImage::from_pixel(100, 150, image::Rgb([10, 20, 30]))
            .save(&source)
            .unwrap();
        let mut o = options(&temp);
        o.resolution = Resolution::Box([10, 10, 70, 90]);
        let summary = run_one(&o, source);
        let result = &summary["results"][0];
        assert!(result["distance"].as_u64().unwrap() > 36);
        assert_eq!(result["status"], "カードなし");
        let saved = Path::new(result["saved_path"].as_str().unwrap());
        assert_eq!(
            fs::canonicalize(saved.parent().unwrap()).unwrap(),
            fs::canonicalize(o.output.join("not_card")).unwrap()
        );
        assert_eq!(load_image(saved).unwrap().dimensions(), (60, 80));
    }

    #[test]
    fn results_list_every_file_they_created() {
        let temp = Temp::new();
        let input = plain_input(&temp);
        for (format, count) in [("webp", 1), ("png", 1), ("both", 2)] {
            let mut o = options(&temp);
            o.resolution = Resolution::Card;
            o.output = temp.0.join(format!("output-{format}"));
            o.format = format.into();
            let summary = run_one(&o, input.clone());
            let result = &summary["results"][0];
            let saved: Vec<&str> = result["saved_paths"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect();
            assert_eq!(saved.len(), count, "{format}");
            assert_eq!(result["saved_path"], saved[0], "{format}");
            assert!(saved.iter().all(|path| Path::new(path).is_file()));
            assert!(summary["csv_error"].is_null());
        }
    }
    #[test]
    fn cancel_returns_only_after_started_images_are_written_and_logged() {
        let temp = Temp::new();
        let input = temp.0.join("input");
        fs::create_dir_all(&input).unwrap();
        for i in 0..6u8 {
            RgbImage::from_pixel(60, 80, image::Rgb([i * 20, 20, 30]))
                .save(input.join(format!("shot{i}.png")))
                .unwrap();
        }
        for format in ["webp", "both"] {
            let mut o = options(&temp);
            o.resolution = Resolution::Card;
            o.output = temp.0.join(format!("cancel-{format}"));
            o.format = format.into();
            o.workers = 2;
            let cancel = AtomicBool::new(false);
            let summary = identify_paths(&o, vec![input.clone()], &cancel, &|done, _, _| {
                if done == 1 {
                    cancel.store(true, Ordering::SeqCst);
                }
            })
            .unwrap();
            assert_eq!(summary["cancelled"], true);
            let results = summary["results"].as_array().unwrap();
            assert!(!results.is_empty() && results.len() < 6, "{format}");
            let expected: usize = results
                .iter()
                .map(|r| r["saved_paths"].as_array().map_or(0, Vec::len))
                .sum();
            let mut on_disk = 0;
            for entry in fs::read_dir(o.output.join("not_card")).unwrap() {
                let entry = entry.unwrap();
                assert!(
                    entry.metadata().unwrap().len() > 0,
                    "empty reservation left"
                );
                on_disk += 1;
            }
            assert_eq!(on_disk, expected, "{format}");
            let log = fs::read_to_string(o.output.join("log.csv")).unwrap();
            assert_eq!(log.lines().count(), results.len() + 1, "{format}");
        }
    }
    #[test]
    fn csv_failure_is_reported_without_counting_as_image_error() {
        let temp = Temp::new();
        let input = plain_input(&temp);
        let mut o = options(&temp);
        o.resolution = Resolution::Card;
        fs::create_dir_all(o.output.join("log.csv")).unwrap();
        let summary = run_one(&o, input);
        assert_eq!(summary["error"], 0);
        assert_eq!(summary["total"], 1);
        let saved = summary["results"][0]["saved_path"].as_str().unwrap();
        assert!(Path::new(saved).is_file());
        let csv = &summary["csv_error"];
        assert!(csv["path"].as_str().unwrap().ends_with("log.csv"));
        assert!(!csv["message"].as_str().unwrap().is_empty());
    }
    #[cfg(windows)]
    #[test]
    fn locked_log_file_is_reported() {
        use std::os::windows::fs::OpenOptionsExt;
        let temp = Temp::new();
        let input = plain_input(&temp);
        let mut o = options(&temp);
        o.resolution = Resolution::Card;
        fs::create_dir_all(&o.output).unwrap();
        let log = o.output.join("log.csv");
        fs::write(&log, b"").unwrap();
        // 他のアプリ（Excel など）が開いているのと同じく、共有なしで開いておく
        let _lock = OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&log)
            .unwrap();
        let summary = run_one(&o, input);
        assert_eq!(summary["error"], 0);
        assert!(summary["csv_error"]["path"].is_string());
        assert!(Path::new(summary["results"][0]["saved_path"].as_str().unwrap()).is_file());
    }
    const SIZE: (u32, u32) = (640, 360);
    /// 枠と模様のあるカードを `b` に描く。`english` なら名前欄を縦縞（英語版）にする。
    fn draw_card(im: &mut RgbImage, b: [i64; 4], seed: u32, english: bool) {
        let [l, t, r, bt] = b;
        let (w, h) = ((r - l) as f64, (bt - t) as f64);
        for y in t..bt {
            for x in l..r {
                let (u, v) = ((x - l) as f64 / w, (y - t) as f64 / h);
                let border = u < 0.04 || u > 0.96 || v < 0.03 || v > 0.97;
                let mut cell = ((u * 16.) as u32 * 31 + (v * 24.) as u32 * 17) ^ (seed << 12);
                cell = cell.wrapping_mul(2654435761);
                cell = (cell ^ (cell >> 15)).wrapping_mul(2246822519);
                let mut value = (cell >> 24) as u8;
                if (0.76..0.89).contains(&v) && (0.08..0.92).contains(&u) {
                    let n = (if english { x - l } else { y - t }) % 8;
                    value = if n < 2 { 0 } else { 240 };
                }
                let value = if border { 250 } else { value };
                im.put_pixel(x as u32, y as u32, image::Rgb([value; 3]));
            }
        }
    }
    fn card_hash(seed: u32, b: [i64; 4]) -> String {
        let mut im = RgbImage::new(SIZE.0, SIZE.1);
        draw_card(&mut im, b, seed, false);
        mdript_core::artwork_hash(&im, b, false, false).unwrap()
    }
    /// 見た目が違う 2 枚のカード（9001、9002）の照合データと名前を用意する。
    /// 9002 のハッシュは 34 ビット崩してあり、確定しない（要確認）距離になる。
    fn detail_options(temp: &Temp) -> Options {
        let o = options(temp);
        let detail = mdript_core::predict_detail_box(SIZE.0, SIZE.1);
        let h1 = card_hash(5, detail);
        let h2 = u128::from_str_radix(&card_hash(77, detail), 16).unwrap() ^ ((1u128 << 34) - 1);
        fs::create_dir_all(&o.user_data).unwrap();
        fs::write(
            o.user_data.join("card_hashes_user.json"),
            json!({"meta":{"format":2},"hashes":{"9001":[h1],"9002":[format!("{h2:032x}")]}})
                .to_string(),
        )
        .unwrap();
        fs::write(
            o.user_data.join("card_names.json"),
            json!({"cards":{"9001":{"n":"テストカード","e":"Test Card"},"9002":{"n":"確認カード","e":"Check Card"}}}).to_string(),
        )
        .unwrap();
        o
    }
    fn screenshot(
        input: &Path,
        file: &str,
        box_of: fn(u32, u32) -> [i64; 4],
        seed: u32,
        english: bool,
    ) {
        let mut im = RgbImage::new(SIZE.0, SIZE.1);
        draw_card(&mut im, box_of(SIZE.0, SIZE.1), seed, english);
        im.save(input.join(file)).unwrap();
    }
    fn names_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .map(|d| {
                d.filter_map(Result::ok)
                    .filter(|e| e.path().is_file())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
    fn detail_input(temp: &Temp) -> PathBuf {
        let input = temp.0.join("input");
        fs::create_dir_all(&input).unwrap();
        let detail = mdript_core::predict_detail_box;
        screenshot(&input, "a_ja.png", detail, 5, false);
        screenshot(&input, "b_en.png", detail, 5, true);
        screenshot(&input, "c_review.png", detail, 77, false);
        RgbImage::from_pixel(SIZE.0, SIZE.1, image::Rgb([10, 20, 30]))
            .save(input.join("d_none.png"))
            .unwrap();
        input
    }
    #[test]
    fn unmatched_detail_saves_only_its_detected_frame() {
        let temp = Temp::new();
        let source = temp.0.join("detail.png");
        let mut im = RgbImage::new(SIZE.0, SIZE.1);
        draw_card(
            &mut im,
            mdript_core::predict_detail_box(SIZE.0, SIZE.1),
            5,
            false,
        );
        im.save(&source).unwrap();
        let mut o = options(&temp);
        o.resolution = Resolution::Fixed;
        let detail_hash = u128::from_str_radix(
            &mdript_core::artwork_hash(
                &im,
                mdript_core::predict_detail_box(SIZE.0, SIZE.1),
                false,
                false,
            )
            .unwrap(),
            16,
        )
        .unwrap();
        let (catalog, found) = (0..128)
            .find_map(|shift| {
                let hash = detail_hash ^ ((1u128 << 40) - 1).rotate_left(shift);
                let catalog = Catalog::from_json(
                    &json!({"hashes":{"9001":[format!("{hash:032x}")]}}).to_string(),
                    None,
                    None,
                )
                .unwrap();
                let found = identify(&im, &catalog, &o.resolution)?;
                (found.screen == Screen::Detail && found.distance > 36).then_some((catalog, found))
            })
            .expect("詳細画面として検出される不一致画像を用意する");
        let prepared = prepare(&source, im.clone(), &catalog, &o).unwrap();
        let crop_size = found.card(&im).dimensions();
        let result = Sequencer::default().finish(0, Ok(prepared), &o).unwrap();
        let saved = Path::new(result["saved_path"].as_str().unwrap());
        assert_eq!(load_image(saved).unwrap().dimensions(), crop_size);
        assert_ne!(crop_size, im.dimensions());
        assert_eq!(saved.parent().unwrap().file_name().unwrap(), "not_card");
    }

    #[test]
    fn detail_folder_option_keeps_the_usual_layout_under_detail() {
        let temp = Temp::new();
        let input = detail_input(&temp);
        let mut o = detail_options(&temp);
        o.split = "en".into();
        let summary = run_one(&o, input);
        let screens: Vec<_> = summary["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["screen"].clone())
            .collect();
        assert_eq!(
            screens,
            [
                json!("detail"),
                json!("detail"),
                json!("detail"),
                Value::Null
            ],
            "{summary}"
        );
        assert_eq!(names_in(&o.output.join("detail")), ["テストカード.png"]);
        assert_eq!(names_in(&o.output.join("detail/en")), ["Test Card.png"]);
        assert_eq!(
            names_in(&o.output.join("detail/review")),
            ["確認カード.png"]
        );
        assert_eq!(names_in(&o.output.join("not_card")), ["d_none.png"]);
        assert_eq!(names_in(&o.output), ["log.csv"]);
    }
    #[test]
    fn detail_suffix_option_goes_after_the_language_suffix() {
        let temp = Temp::new();
        let input = detail_input(&temp);
        let mut o = detail_options(&temp);
        o.detail = "suffix".into();
        o.english = "ja".into();
        let summary = run_one(&o, input);
        assert_eq!(summary["confirmed"], 2, "{summary}");
        assert_eq!(
            names_in(&o.output),
            [
                "log.csv",
                "テストカード_detail.png",
                "テストカード_en_detail.png"
            ]
        );
        assert_eq!(
            names_in(&o.output.join("review")),
            ["確認カード_detail.png"]
        );
        assert_eq!(names_in(&o.output.join("not_card")), ["d_none.png"]);
        assert!(!o.output.join("detail").exists());
    }
    #[test]
    fn maximized_and_detail_of_the_same_card_do_not_skip_each_other() {
        for detail in ["folder", "suffix"] {
            let temp = Temp::new();
            let input = temp.0.join("input");
            fs::create_dir_all(&input).unwrap();
            screenshot(&input, "a_max.png", mdript_core::predict_card_box, 5, false);
            screenshot(
                &input,
                "b_detail.png",
                mdript_core::predict_detail_box,
                5,
                false,
            );
            let mut o = detail_options(&temp);
            o.duplicates = true;
            o.detail = detail.into();
            // 拡大画面の位置のハッシュも同じカードとして登録する
            let h = card_hash(5, mdript_core::predict_card_box(SIZE.0, SIZE.1));
            let path = o.user_data.join("card_hashes_user.json");
            let mut user: Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            user["hashes"]["9001"]
                .as_array_mut()
                .unwrap()
                .push(json!(h));
            fs::write(&path, user.to_string()).unwrap();
            let summary = run_one(&o, input);
            assert_eq!(summary["confirmed"], 2, "{detail}: {summary}");
            assert_eq!(summary["skipped"], 0, "{detail}: {summary}");
            let screens: Vec<_> = summary["results"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["screen"].as_str().unwrap().to_owned())
                .collect();
            assert_eq!(screens, ["maximized", "detail"], "{detail}");
            if detail == "folder" {
                assert_eq!(names_in(&o.output), ["log.csv", "テストカード.png"]);
                assert_eq!(names_in(&o.output.join("detail")), ["テストカード.png"]);
            } else {
                assert_eq!(
                    names_in(&o.output),
                    ["log.csv", "テストカード.png", "テストカード_detail.png"]
                );
            }
        }
    }
    #[test]
    fn detail_option_is_validated() {
        let temp = Temp::new();
        let o = options(&temp);
        let args = |value: &str| {
            vec![
                OsString::from("image.png"),
                OsString::from("--output"),
                o.output.clone().into_os_string(),
                OsString::from("--user-data"),
                o.user_data.clone().into_os_string(),
                OsString::from("--detail"),
                OsString::from(value),
            ]
        };
        assert_eq!(parse(args("suffix")).unwrap().0.detail, "suffix");
        assert_eq!(parse(args("folder")).unwrap().0.detail, "folder");
        assert!(parse(args("off")).is_err());
    }
    #[test]
    fn rounded_mask_matches_pillow_fixture() {
        let im = rounded(&RgbImage::new(894, 1302), true);
        let expected = [
            18, 15, 12, 11, 9, 8, 7, 6, 5, 4, 4, 3, 2, 2, 2, 1, 1, 1, 0, 0, 0, 0,
        ];
        for (y, &start) in expected.iter().enumerate() {
            assert_eq!(
                (0..22).find(|&x| im.get_pixel(x, y as u32)[3] > 0),
                Some(start)
            );
        }
    }
}
