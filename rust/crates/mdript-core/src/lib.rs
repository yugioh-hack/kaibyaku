const PRECISION_BITS: i32 = 32 - 8 - 2;
const ART_BOXES: [(f64, f64, f64, f64); 2] = [(0.18, 0.22, 0.82, 0.38), (0.18, 0.50, 0.82, 0.62)];

struct Gray {
    w: usize,
    h: usize,
    px: Vec<u8>,
}

fn lanczos(x: f64) -> f64 {
    fn sinc(x: f64) -> f64 {
        if x == 0.0 {
            1.0
        } else {
            let x = x * std::f64::consts::PI;
            x.sin() / x
        }
    }
    if (-3.0..3.0).contains(&x) {
        sinc(x) * sinc(x / 3.0)
    } else {
        0.0
    }
}

// Pillow Resample.c の precompute_coeffs + normalize_coeffs_8bpc
fn coeffs(
    in_size: usize,
    in0: f64,
    in1: f64,
    out: usize,
) -> (usize, Vec<(usize, usize)>, Vec<i32>) {
    let scale = (in1 - in0) / out as f64;
    let fscale = scale.max(1.0);
    let support = 3.0 * fscale;
    let ksize = support.ceil() as usize * 2 + 1;
    let mut bounds = Vec::with_capacity(out);
    let mut kk = vec![0i32; out * ksize];
    for xx in 0..out {
        let center = in0 + (xx as f64 + 0.5) * scale;
        let ss = 1.0 / fscale;
        let xmin = ((center - support + 0.5) as i64).max(0) as usize;
        let xmax = ((center + support + 0.5) as i64).min(in_size as i64) as usize - xmin;
        let mut k: Vec<f64> = (0..xmax)
            .map(|x| lanczos((x as f64 + xmin as f64 - center + 0.5) * ss))
            .collect();
        let ww: f64 = k.iter().sum();
        if ww != 0.0 {
            for v in k.iter_mut() {
                *v /= ww;
            }
        }
        for (x, v) in k.iter().enumerate() {
            let f = v * (1i64 << PRECISION_BITS) as f64;
            kk[xx * ksize + x] = if *v < 0.0 {
                (-0.5 + f) as i32
            } else {
                (0.5 + f) as i32
            };
        }
        bounds.push((xmin, xmax));
    }
    (ksize, bounds, kk)
}

fn clip8(v: i64) -> u8 {
    if v >= (1i64 << PRECISION_BITS << 8) {
        255
    } else if v <= 0 {
        0
    } else {
        (v >> PRECISION_BITS) as u8
    }
}

// Image.resize((ow, oh), LANCZOS) と同じ結果（横→縦、縦に必要な行だけ横を計算）
fn resize_pillow(src: &Gray, ow: usize, oh: usize) -> Gray {
    let (kh, bh, kkh) = coeffs(src.w, 0.0, src.w as f64, ow);
    let (kv, mut bv, kkv) = coeffs(src.h, 0.0, src.h as f64, oh);
    let y0 = bv[0].0;
    let y1 = bv[oh - 1].0 + bv[oh - 1].1;
    for b in bv.iter_mut() {
        b.0 -= y0;
    }
    let th = y1 - y0;
    let mut tmp = vec![0u8; ow * th];
    for y in 0..th {
        let row = &src.px[(y + y0) * src.w..(y + y0 + 1) * src.w];
        for xx in 0..ow {
            let (xmin, xmax) = bh[xx];
            let k = &kkh[xx * kh..];
            let mut ss: i64 = 1 << (PRECISION_BITS - 1);
            for x in 0..xmax {
                ss += row[x + xmin] as i64 * k[x] as i64;
            }
            tmp[y * ow + xx] = clip8(ss);
        }
    }
    let mut out = vec![0u8; ow * oh];
    for yy in 0..oh {
        let (ymin, ymax) = bv[yy];
        let k = &kkv[yy * kv..];
        for xx in 0..ow {
            let mut ss: i64 = 1 << (PRECISION_BITS - 1);
            for y in 0..ymax {
                ss += tmp[(y + ymin) * ow + xx] as i64 * k[y] as i64;
            }
            out[yy * ow + xx] = clip8(ss);
        }
    }
    Gray {
        w: ow,
        h: oh,
        px: out,
    }
}

// phash.phash_hex: 32x32 の DCT の左上 8x8 を中央値で二値化
fn phash(g: &Gray) -> String {
    let n = 32usize;
    let m: Vec<f64> = (0..8)
        .flat_map(|k| {
            (0..n).map(move |i| {
                2.0 * (std::f64::consts::PI * k as f64 * (2 * i + 1) as f64 / (2 * n) as f64).cos()
            })
        })
        .collect();
    let mut t = vec![0f64; 8 * n];
    for k in 0..8 {
        for j in 0..n {
            t[k * n + j] = (0..n).map(|i| m[k * n + i] * g.px[i * n + j] as f64).sum();
        }
    }
    let mut low = [0f64; 64];
    for k in 0..8 {
        for l in 0..8 {
            low[k * 8 + l] = (0..n).map(|j| t[k * n + j] * m[l * n + j]).sum();
        }
    }
    let mut s = low;
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = (s[31] + s[32]) / 2.0;
    let mut v: u64 = 0;
    for x in low {
        v = (v << 1) | (x > med) as u64;
    }
    format!("{v:016x}")
}

/// Python round uses ties-to-even, including half-pixel coordinates.
pub fn predict_card_box(w: u32, h: u32) -> [i64; 4] {
    let scale = h as f64 / 1440.0;
    let cw = 894.0 * scale;
    let left = w as f64 / 2.0 - cw / 2.0;
    let top = 69.0 * scale;
    [left, top, left + cw, top + 1302.0 * scale].map(|v| v.round_ties_even() as i64)
}

/// Validates before cropping or allocating. Never writes user data.
pub fn artwork_hash(
    rgb: &image::RgbImage,
    b: [i64; 4],
    coarse: bool,
    center: bool,
) -> Result<String, String> {
    let [l, t, r, bt] = b;
    if l < 0 || t < 0 || r <= l || bt <= t || r > rgb.width() as i64 || bt > rgb.height() as i64 {
        return Err("card box is outside the image or empty".into());
    }
    let (cw, ch) = ((r - l) as u64, (bt - t) as u64);
    if cw * ch > 16_000_000 {
        return Err("card exceeds 16 million pixels".into());
    }
    let mut boxes = if coarse {
        vec![ART_BOXES[0]]
    } else {
        ART_BOXES.to_vec()
    };
    if center && !coarse {
        boxes.push((0.20, 0.40, 0.50, 0.50));
    }
    let mut result = String::new();
    for (x, y, z, v) in boxes {
        let (x0, y0, x1, y1) = (
            (cw as f64 * x) as u32,
            (ch as f64 * y) as u32,
            (cw as f64 * z) as u32,
            (ch as f64 * v) as u32,
        );
        if x1 <= x0 || y1 <= y0 {
            return Err("hash region is empty".into());
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        let mut px = Vec::with_capacity(w * h);
        for yy in y0..y1 {
            for xx in x0..x1 {
                let p = rgb.get_pixel(l as u32 + xx, t as u32 + yy).0;
                px.push(
                    ((p[0] as u32 * 19595 + p[1] as u32 * 38470 + p[2] as u32 * 7471 + 0x8000)
                        >> 16) as u8,
                );
            }
        }
        result.push_str(&phash(&resize_pillow(&Gray { w, h, px }, 32, 32)));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_box() {
        assert_eq!(predict_card_box(2560, 1440), [833, 69, 1727, 1371]);
    }
    #[test]
    fn unsafe_boxes_are_errors() {
        let im = image::RgbImage::new(40, 100);
        for b in [
            [-1, 0, 20, 20],
            [0, 0, 41, 20],
            [20, 0, 10, 20],
            [0, 0, 0, 0],
            [0, 0, 1, 1],
        ] {
            assert!(artwork_hash(&im, b, false, false).is_err());
        }
    }
    #[test]
    fn modes_and_black_image() {
        let im = image::RgbImage::new(100, 150);
        let b = [0, 0, 100, 150];
        assert_eq!(artwork_hash(&im, b, false, false).unwrap(), "0".repeat(32));
        assert_eq!(artwork_hash(&im, b, true, true).unwrap(), "0".repeat(16));
        assert_eq!(artwork_hash(&im, b, false, true).unwrap(), "0".repeat(48));
    }
}
