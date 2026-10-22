use crate::{Gray, language::gray, predict_card_box};
use image::RgbImage;
pub const ASPECT: f64 = 894. / 1302.;
fn round(v: f64) -> i64 {
    v.round_ties_even() as i64
}
fn peaks(profile: &[f64]) -> Vec<usize> {
    let max = profile.iter().copied().fold(0., f64::max);
    let mut idx: Vec<_> = (0..profile.len()).collect();
    idx.sort_by(|&a, &b| profile[b].total_cmp(&profile[a]).then(b.cmp(&a)));
    let mut out = vec![];
    for i in idx {
        if profile[i] < max * 0.25 {
            break;
        }
        if out.iter().all(|&j| i.abs_diff(j) >= 6) {
            out.push(i);
        }
        if out.len() == 8 {
            break;
        }
    }
    out.sort_unstable();
    out
}
pub fn detect_card_box(im: &RgbImage) -> Option<[i64; 4]> {
    let (w, h) = im.dimensions();
    if w < 16 || h < 16 {
        return None;
    }
    let scale = 1f64.min(640. / w as f64).min(640. / h as f64);
    let (sw, sh) = (
        round(w as f64 * scale).max(2) as usize,
        round(h as f64 * scale).max(2) as usize,
    );
    let g = Gray {
        w: w as usize,
        h: h as usize,
        px: im.pixels().map(gray).collect(),
    };
    let small = crate::resize_bilinear(&g, sw, sh);
    let mut gx = vec![0.; sw - 1];
    let mut gy = vec![0.; sh - 1];
    for y in 0..sh {
        for (x, v) in gx.iter_mut().enumerate() {
            *v += small.px[y * sw + x].abs_diff(small.px[y * sw + x + 1]) as f64;
        }
    }
    for (y, v) in gy.iter_mut().enumerate() {
        for x in 0..sw {
            *v += small.px[y * sw + x].abs_diff(small.px[(y + 1) * sw + x]) as f64;
        }
    }
    if gx.iter().all(|v| *v == 0.) || gy.iter().all(|v| *v == 0.) {
        return None;
    }
    let (cols, rows) = (peaks(&gx), peaks(&gy));
    let mut best = None;
    for (i, &l) in cols.iter().enumerate() {
        for &r in &cols[i + 1..] {
            let bw = (r - l) as f64;
            if bw < (sw as f64) * 0.1 {
                continue;
            }
            for (j, &t) in rows.iter().enumerate() {
                for &b in &rows[j + 1..] {
                    let bh = (b - t) as f64;
                    if bh < (sh as f64) * 0.3
                        || bh > (sh as f64) * 0.99
                        || (bw / bh - ASPECT).abs() > 0.08
                    {
                        continue;
                    }
                    let score = (gx[l] + gx[r]) / bh + (gy[t] + gy[b]) / bw;
                    if best.is_none_or(|(s, _)| score > s) {
                        best = Some((
                            score,
                            [
                                round(l as f64 * w as f64 / sw as f64),
                                round(t as f64 * h as f64 / sh as f64),
                                round((r + 1) as f64 * w as f64 / sw as f64),
                                round((b + 1) as f64 * h as f64 / sh as f64),
                            ],
                        ));
                    }
                }
            }
        }
    }
    best.map(|v| v.1)
}
pub fn on_edges(im: &RgbImage, [l, t, r, b]: [i64; 4]) -> bool {
    let (w, h) = (r - l, b - t);
    if w < 16 || h < 16 {
        return false;
    }
    let reach = round(h as f64 * 0.06).max(3);
    let near = round(h as f64 * 0.004).max(2);
    let mut sides = 0;
    for (vertical, edge) in [(true, l), (true, r), (false, t), (false, b)] {
        let limit = if vertical { im.width() } else { im.height() } as i64;
        let (start, stop) = ((edge - 1 - reach).max(0), (edge + reach + 1).min(limit));
        if stop - start < 2 {
            continue;
        }
        let mut profile = vec![0u64; (stop - start - 1) as usize];
        for (i, v) in profile.iter_mut().enumerate() {
            let pos = start + i as i64;
            let (inner0, inner1) = if vertical {
                (t + h / 10, b - h / 10)
            } else {
                (l + w / 10, r - w / 10)
            };
            for j in inner0..inner1 {
                let (x, y) = if vertical { (pos, j) } else { (j, pos) };
                let (x2, y2) = if vertical { (x + 1, y) } else { (x, y + 1) };
                if x < 0 || y < 0 || x2 >= im.width() as i64 || y2 >= im.height() as i64 {
                    continue;
                }
                *v += gray(im.get_pixel(x as u32, y as u32))
                    .abs_diff(gray(im.get_pixel(x2 as u32, y2 as u32)))
                    as u64;
            }
        }
        let pos = edge - 1 - start;
        let a = (pos - near).max(0) as usize;
        let z = (pos + near + 1).max(0).min(profile.len() as i64) as usize;
        let max = *profile.iter().max().unwrap_or(&0);
        if a < z && max > 0 && (*profile[a..z].iter().max().unwrap() as f64) >= max as f64 * 0.3 {
            sides += 1;
        }
    }
    sides >= 3
}
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    Auto,
    Fixed,
    Original(u32, u32),
    Box([i64; 4]),
    Card,
}
pub fn trusted_boxes(im: &RgbImage, resolution: &Resolution) -> Vec<[i64; 4]> {
    let (w, h) = im.dimensions();
    let predicted = predict_card_box(w, h);
    match resolution {
        Resolution::Box(b) => vec![*b],
        Resolution::Card => vec![[0, 0, w as i64, h as i64]],
        Resolution::Original(ow, oh) => {
            let b = predict_card_box(*ow, *oh);
            vec![
                [
                    (b[0] as f64 * w as f64 / (*ow as f64)) as i64,
                    (b[1] as f64 * h as f64 / (*oh as f64)) as i64,
                    (b[2] as f64 * w as f64 / (*ow as f64)) as i64,
                    (b[3] as f64 * h as f64 / (*oh as f64)) as i64,
                ],
                predicted,
            ]
        }
        _ => {
            let mut out = vec![];
            if (w as f64 / h as f64 - ASPECT).abs() <= 0.08 {
                out.push([0, 0, w as i64, h as i64]);
            }
            out.push(predicted);
            out
        }
    }
}
pub fn nudged(b: [i64; 4], size: (u32, u32), shifts: &[f64], scales: &[f64]) -> Vec<[i64; 4]> {
    let [l, t, r, b] = b;
    let (w, h) = ((r - l) as f64, (b - t) as f64);
    let (cx, cy) = ((l + r) as f64 / 2., (t + b) as f64 / 2.);
    let mut out = vec![];
    let mut seen = std::collections::HashSet::new();
    for &scale in scales {
        for &dx in shifts {
            for &dy in shifts {
                let (nx, ny) = (cx + w * dx, cy + h * dy);
                let candidate = [
                    round(nx - w * scale / 2.),
                    round(ny - h * scale / 2.),
                    round(nx + w * scale / 2.),
                    round(ny + h * scale / 2.),
                ];
                if seen.insert(candidate)
                    && candidate[0] >= 0
                    && candidate[1] >= 0
                    && candidate[2] <= size.0 as i64
                    && candidate[3] <= size.1 as i64
                {
                    out.push(candidate);
                }
            }
        }
    }
    out
}
