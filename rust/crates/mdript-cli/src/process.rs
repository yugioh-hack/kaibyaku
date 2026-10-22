use crate::paths::{is_within, resolved};
use image::{ImageEncoder, RgbImage, RgbaImage};
use mdript_core::{
    catalog::Catalog,
    identify::identify,
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
        if self.output == self.user_data
            || self.output.starts_with(&self.user_data)
            || self.user_data.starts_with(&self.output)
        {
            return Err("output and user-data must be separate directories".into());
        }
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let live = resolved(&PathBuf::from(appdata).join("mdript"))?;
            if is_within(&self.output, &live) || is_within(&self.user_data, &live) {
                return Err("prototype paths must be isolated from live APPDATA/mdript".into());
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

fn collect(paths: Vec<PathBuf>, o: &Options) -> Result<Vec<PathBuf>, String> {
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
    Skipped,
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
            return Err("output directory resolves outside isolated output root".into());
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
                        return Ok(Choice::Skipped);
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
            Choice::Skipped => return Ok((String::new(), SKIPPED.into())),
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
fn prepare(path: &Path, catalog: &Catalog, o: &Options) -> Result<Prepared, String> {
    #[cfg(test)]
    if std::env::var_os("MDRIPT_TEST_PANIC_PREPARE")
        .is_some_and(|name| path.file_name() == Some(name.as_os_str()))
    {
        panic!("injected prepare panic");
    }
    let im = load_image(path)?;
    let mut result = json!({"source":path,"status":"カードなし","card_id":"","name":"","distance":999,"margin":0,"saved_path":"","note":"","english":false,"box":null,"origin":null,"hash":null,"ranked":[],"stroke_ratio":null});
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let Some(found) = identify(&im, catalog, &o.resolution) else {
        result["note"] = json!("照合できる画像がありません");
        if o.not_card {
            return Ok(Prepared {
                result,
                save: Some(SaveTask {
                    card: im,
                    target: o.output.join("not_card"),
                    name: stem.into_owned(),
                    hash: None,
                    round: false,
                    propagate_skip: false,
                }),
            });
        }
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
        result["note"] = json!("カードの拡大画面ではないようです");
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
    let confirmed = found.confirmed();
    result["status"] = json!(if confirmed { "確定" } else { "要確認" });
    result["card_id"] = json!(cid);
    result["name"] = json!(name);
    result["english"] = json!(english);
    if !confirmed && !o.review {
        result["note"] = json!("要確認のため保存しませんでした");
        return Ok(Prepared::complete(result));
    }
    let mut target = if confirmed {
        o.output.clone()
    } else {
        o.output.join("review")
    };
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
                Ok(Choice::Skipped) => {
                    if task.propagate_skip {
                        result["note"] = json!(SKIPPED);
                    }
                    return Ok(result);
                }
                Ok(Choice::Reserved(reservation)) => break reservation,
                Err(error) => return Err(error),
            }
        };
        turn_guard.advance(&mut state);
        drop(state);

        let saved_path = display_path(&reservation.paths[0]);
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
        Ok(result)
    }
}
fn error_result(path: &Path, error: String) -> Value {
    json!({"source":path,"status":"エラー","card_id":"","name":"","distance":999,"margin":0,"saved_path":"","note":error,"english":false})
}
pub fn run(args: Vec<OsString>) -> Result<(), String> {
    if args.len() == 1 && args[0] == "--help" {
        println!(
            "mdript-cli identify PATH... --output ABS_DIR --user-data ABS_DIR [--format webp|png|both] [--quality 1..100] [--resolution auto|fixed|WxH] [--card|--box L T R B] [--english name|ja|off] [--split none|en|both] [--workers 1..256] [--dry-run] [--no-csv] [--keep-duplicates] [--no-rounded] [--no-recursive] [--no-review] [--no-not-card]\nUser data is optional read-only copied card_names.json/card_hashes_user.json. No implicit APPDATA access. JSON summary on stdout; errors exit 1."
        );
        return Ok(());
    }
    let (o, paths) = parse(args)?;
    let summary = identify_paths(&o, paths, &AtomicBool::new(false), &|_, _, _| {})?;
    println!("{summary}");
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
    let paths = collect(inputs, o)?;
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
                    let prepared = catch_unwind(AssertUnwindSafe(|| prepare(path, catalog, o)))
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
                    "worker ended before producing a result".into(),
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
    if o.csv && !o.dry && !results.is_empty() {
        let _ = crate::csv_log::write_csv(&o.output, &results);
    }
    Ok(
        json!({"confirmed":confirmed,"review":review,"not_card":not_card,"error":error,"skipped":skipped,"seconds":started.elapsed().as_secs_f64(),"total":paths.len(),"cancelled":cancel.load(Ordering::SeqCst),"results":results,"catalog_hashes":catalog.hash_count()}),
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
            RgbImage::from_pixel(30, 40, image::Rgb([10, 20, 30]))
                .save(first.join(first_name))
                .unwrap();
            RgbImage::from_pixel(30, 40, image::Rgb([40, 50, 60]))
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
