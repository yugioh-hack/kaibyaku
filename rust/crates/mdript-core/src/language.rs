use image::RgbImage;
pub(crate) fn gray(p: &image::Rgb<u8>) -> u8 {
    ((p[0] as u32 * 19595 + p[1] as u32 * 38470 + p[2] as u32 * 7471 + 32768) >> 16) as u8
}
pub fn stroke_ratio(card: &RgbImage) -> Option<f64> {
    let (w, h) = card.dimensions();
    let (l, t, r, b) = (
        (w as f64 * 0.07) as u32,
        (h as f64 * 0.755) as u32,
        (w as f64 * 0.93) as u32,
        (h as f64 * 0.895) as u32,
    );
    let (rw, rh) = ((r - l) as usize, (b - t) as usize);
    if rw < 3 || rh < 3 {
        return None;
    }
    let px: Vec<_> = (t..b)
        .flat_map(|y| (l..r).map(move |x| gray(card.get_pixel(x, y))))
        .collect();
    let mut sorted = px.clone();
    sorted.sort_unstable();
    let median = (sorted[(px.len() - 1) / 2] as f64 + sorted[px.len() / 2] as f64) / 2.;
    let ink: Vec<_> = px.iter().map(|v| (*v as f64) < median - 60.).collect();
    if (ink.iter().filter(|v| **v).count() as f64) / (px.len() as f64) < 0.005 {
        return None;
    }
    let (mut horizontal, mut vertical) = (0u64, 0u64);
    for y in 0..rh - 1 {
        for x in 0..rw - 1 {
            let i = y * rw + x;
            if ink[i] || ink[i + rw] || ink[i + 1] {
                horizontal += px[i].abs_diff(px[i + 1]) as u64;
                vertical += px[i].abs_diff(px[i + rw]) as u64;
            }
        }
    }
    (horizontal > 0).then(|| vertical as f64 / horizontal as f64)
}
pub fn frame_twin(card: &RgbImage, cid: &str) -> String {
    if !["4370", "19092"].contains(&cid) {
        return cid.into();
    }
    let (w, h) = card.dimensions();
    let (l, t, r, b) = (
        (w as f64 * 0.04) as u32,
        (h as f64 * 0.30) as u32,
        (w as f64 * 0.08) as u32,
        (h as f64 * 0.60) as u32,
    );
    // Pillow BOX reduces horizontally to 8 bits before the vertical pass.
    if r <= l || b <= t {
        return cid.into();
    }
    let kw = ((1u64 << 22) as f64 / (r - l) as f64 + 0.5) as u64;
    let kh = ((1u64 << 22) as f64 / (b - t) as f64 + 0.5) as u64;
    let (mut red, mut blue) = (1u64 << 21, 1u64 << 21);
    for y in t..b {
        let (mut rr, mut bb) = (1u64 << 21, 1u64 << 21);
        for x in l..r {
            let p = card.get_pixel(x, y);
            rr += p[0] as u64 * kw;
            bb += p[2] as u64 * kw;
        }
        red += ((rr >> 22).min(255)) * kh;
        blue += ((bb >> 22).min(255)) * kh;
    }
    let delta = ((blue >> 22).min(255) as i64) - ((red >> 22).min(255) as i64);
    if delta >= 40 {
        "4370".into()
    } else if delta <= -40 {
        "19092".into()
    } else {
        cid.into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn empty_text_is_unknown() {
        assert_eq!(stroke_ratio(&RgbImage::new(100, 150)), None);
        assert_eq!(stroke_ratio(&RgbImage::new(1, 1)), None);
    }
}
