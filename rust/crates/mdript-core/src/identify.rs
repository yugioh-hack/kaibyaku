use crate::{
    artwork_hash,
    catalog::Catalog,
    layout::{self, Resolution},
};
use image::RgbImage;
/// カードが写っていた画面の種類。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Maximized,
    Detail,
}
impl Screen {
    pub fn as_str(self) -> &'static str {
        match self {
            Screen::Maximized => "maximized",
            Screen::Detail => "detail",
        }
    }
}
#[derive(Clone, Debug)]
pub struct Found {
    pub screen: Screen,
    pub card_box: [i64; 4],
    pub estimated: bool,
    pub hash: String,
    pub distance: u32,
    pub margin: u32,
    pub card_id: String,
    pub ranked: Vec<(u32, String)>,
}
impl Found {
    pub fn confirmed(&self) -> bool {
        if self.estimated {
            self.distance <= 16 && self.margin >= 6
        } else {
            (self.distance <= 16 && self.margin >= 2) || (self.distance <= 32 && self.margin >= 4)
        }
    }
    pub fn card(&self, im: &RgbImage) -> RgbImage {
        let [l, t, r, b] = self.card_box;
        image::imageops::crop_imm(im, l as u32, t as u32, (r - l) as u32, (b - t) as u32).to_image()
    }
}
fn evaluate(
    im: &RgbImage,
    b: [i64; 4],
    estimated: bool,
    coarse: bool,
    catalog: &Catalog,
) -> Option<Found> {
    let [l, t, r, bt] = b;
    if l < 0
        || t < 0
        || r <= l
        || bt <= t
        || r > im.width() as i64
        || bt > im.height() as i64
        || r - l < 40
        || bt - t < 40
        || (r - l).checked_mul(bt - t)? > 16_000_000
    {
        return None;
    }
    let hash = artwork_hash(im, b, coarse, false).ok()?;
    let ranked = catalog.rank(&hash, 3, coarse, || {
        artwork_hash(im, b, false, true)
            .ok()
            .map(|s| s[32..].to_string())
    });
    let (distance, card_id) = ranked.first()?.clone();
    let margin = ranked.get(1).map_or(99, |v| v.0 - distance);
    Some(Found {
        screen: Screen::Maximized,
        card_box: b,
        estimated,
        hash,
        distance,
        margin,
        card_id,
        ranked,
    })
}
fn better(a: Option<Found>, b: Option<Found>) -> Option<Found> {
    match (a, b) {
        (Some(a), Some(b)) => {
            if (a.distance, a.estimated, std::cmp::Reverse(a.margin))
                <= (b.distance, b.estimated, std::cmp::Reverse(b.margin))
            {
                Some(a)
            } else {
                Some(b)
            }
        }
        (a, b) => a.or(b),
    }
}
/// 登録に使う位置。拡大画面の予測位置は枠が合うときだけ使い、
/// 合わなければ詳細画面の予測位置（枠が合うとき）、それもなければ拡大画面の予測位置。
pub fn identify_trusted(im: &RgbImage, catalog: &Catalog) -> Option<Found> {
    let predicted = layout::predicted_boxes(im, &Resolution::Auto);
    let boxes = layout::trusted_boxes(im, &Resolution::Auto);
    let first = boxes
        .iter()
        .filter(|b| !predicted.contains(b) || layout::on_edges(im, **b))
        .find_map(|&b| evaluate(im, b, false, false, catalog));
    first
        .or_else(|| {
            layout::detail_boxes(im, &Resolution::Auto)
                .into_iter()
                .filter(|&b| layout::on_edges(im, b))
                .find_map(|b| {
                    evaluate(im, b, false, false, catalog).map(|f| Found {
                        screen: Screen::Detail,
                        ..f
                    })
                })
        })
        .or_else(|| {
            boxes
                .into_iter()
                .find_map(|b| evaluate(im, b, false, false, catalog))
        })
}
/// 位置の中心から 2 段階の微調整で探し、見つかった位置を詳しく評価し直す。
fn refine(im: &RgbImage, catalog: &Catalog, mut center: [i64; 4]) -> Option<Found> {
    let mut found = None;
    for (shifts, scales) in [
        (
            &[-0.03, -0.015, 0., 0.015, 0.03][..],
            &[0.94, 0.97, 1., 1.03, 1.06][..],
        ),
        (
            &[-0.008, -0.004, 0., 0.004, 0.008][..],
            &[0.99, 1., 1.01][..],
        ),
    ] {
        let mut round_best = None;
        for b in layout::nudged(center, im.dimensions(), shifts, scales) {
            let cand = evaluate(im, b, true, true, catalog);
            let done = cand
                .as_ref()
                .is_some_and(|c| c.distance <= 6 && c.margin >= 4);
            round_best = better(cand, round_best);
            if done {
                break;
            }
        }
        if let Some(c) = &round_best {
            center = c.card_box;
            let done = c.distance <= 6 && c.margin >= 4;
            found = better(round_best, found);
            if done {
                break;
            }
        }
    }
    evaluate(im, found?.card_box, true, false, catalog)
}
pub fn identify(im: &RgbImage, catalog: &Catalog, resolution: &Resolution) -> Option<Found> {
    let best = identify_maximized(im, catalog, resolution);
    if best.as_ref().is_some_and(Found::confirmed)
        || matches!(resolution, Resolution::Box(_) | Resolution::Card)
        || layout::predicted_boxes(im, resolution)
            .into_iter()
            .any(|b| layout::on_edges(im, b))
    {
        return best;
    }
    let mut detail = None;
    let boxes = layout::detail_boxes(im, resolution);
    for &b in &boxes {
        let mut cand = evaluate(im, b, false, false, catalog);
        if let Some(c) = cand.as_mut()
            && c.distance > 16
            && c.confirmed()
            && !layout::on_edges(im, b)
        {
            c.estimated = true;
        }
        detail = better(cand, detail);
        if detail.as_ref().is_some_and(Found::confirmed) {
            break;
        }
    }
    if *resolution == Resolution::Auto && !detail.as_ref().is_some_and(Found::confirmed) {
        let center = detail
            .as_ref()
            .map(|f| f.card_box)
            .or(boxes.first().copied());
        if let Some(center) = center {
            detail = better(refine(im, catalog, center), detail);
        }
    }
    let detail = detail.map(|f| Found {
        screen: Screen::Detail,
        ..f
    });
    if detail.as_ref().is_some_and(Found::confirmed) {
        return detail;
    }
    better(detail, best)
}
fn identify_maximized(im: &RgbImage, catalog: &Catalog, resolution: &Resolution) -> Option<Found> {
    let mut best = None;
    for b in layout::trusted_boxes(im, resolution) {
        let mut cand = evaluate(im, b, false, false, catalog);
        if let Some(c) = cand.as_mut()
            && c.distance > 16
            && c.confirmed()
            && b != [0, 0, im.width() as i64, im.height() as i64]
            && !layout::on_edges(im, b)
        {
            c.estimated = true;
        }
        best = better(cand, best);
        if best.as_ref().is_some_and(Found::confirmed) {
            return best;
        }
    }
    if *resolution != Resolution::Auto {
        return best;
    }
    let detected = layout::detect_card_box(im).filter(|b| {
        b.iter()
            .zip(crate::predict_card_box(im.width(), im.height()))
            .any(|(a, v)| (*a - v).abs() > 2)
    });
    if let Some(b) = detected {
        best = better(evaluate(im, b, true, false, catalog), best);
        if best.as_ref().is_some_and(Found::confirmed) {
            return best;
        }
    }
    let center = detected.unwrap_or(best.as_ref()?.card_box);
    better(refine(im, catalog, center), best)
}
#[cfg(test)]
mod tests {
    use super::*;
    /// 枠と模様のあるカードを `b` に描く。模様は位置の比で決めるので、大きさが違っても同じ絵になる。
    fn draw_card(im: &mut RgbImage, b: [i64; 4], invert: bool) {
        let [l, t, r, bt] = b;
        let (w, h) = ((r - l) as f64, (bt - t) as f64);
        for y in t..bt {
            for x in l..r {
                let (u, v) = ((x - l) as f64 / w, (y - t) as f64 / h);
                let border = u < 0.04 || u > 0.96 || v < 0.03 || v > 0.97;
                let cell =
                    ((u * 16.) as u32 * 31 + (v * 24.) as u32 * 17 + 5).wrapping_mul(2654435761);
                let mut value = (cell >> 24) as u8;
                if invert {
                    value = 255 - value;
                }
                im.put_pixel(
                    x as u32,
                    y as u32,
                    image::Rgb([if border { 250 } else { value }; 3]),
                );
            }
        }
    }
    fn catalog_for(b: [i64; 4], size: (u32, u32)) -> Catalog {
        let mut im = RgbImage::new(size.0, size.1);
        draw_card(&mut im, b, false);
        let hash = artwork_hash(&im, b, false, false).unwrap();
        Catalog::from_json(&format!(r#"{{"hashes":{{"1":["{hash}"]}}}}"#), None, None).unwrap()
    }
    #[test]
    fn detail_screen_is_found_at_the_detail_box() {
        let size = (640, 360);
        let detail = crate::predict_detail_box(size.0, size.1);
        let maximized = crate::predict_card_box(size.0, size.1);
        let catalog = catalog_for(detail, size);
        let mut im = RgbImage::new(size.0, size.1);
        draw_card(&mut im, detail, false);
        for resolution in [
            Resolution::Auto,
            Resolution::Fixed,
            Resolution::Original(1280, 720),
        ] {
            let found = identify(&im, &catalog, &resolution).unwrap();
            assert_eq!(found.screen, Screen::Detail, "{resolution:?}");
            assert_eq!(found.card_id, "1");
            assert!(found.confirmed(), "{resolution:?}");
        }
        assert!(!layout::on_edges(&im, maximized));
        let trusted = identify_trusted(&im, &catalog).unwrap();
        assert_eq!((trusted.screen, trusted.card_box), (Screen::Detail, detail));
    }
    #[test]
    fn maximized_screen_over_detail_screen_stays_maximized() {
        let size = (640, 360);
        let detail = crate::predict_detail_box(size.0, size.1);
        let maximized = crate::predict_card_box(size.0, size.1);
        let catalog = catalog_for(maximized, size);
        let mut im = RgbImage::new(size.0, size.1);
        draw_card(&mut im, detail, false);
        draw_card(&mut im, maximized, false);
        let found = identify(&im, &catalog, &Resolution::Auto).unwrap();
        assert_eq!(
            (found.screen, found.card_box),
            (Screen::Maximized, maximized)
        );
        assert!(found.confirmed());
        // 拡大画面側の絵が崩れて確定しなくても、枠が合うので背景の詳細画面は見ない
        draw_card(&mut im, maximized, true);
        for resolution in [Resolution::Auto, Resolution::Fixed] {
            let found = identify(&im, &catalog, &resolution).unwrap();
            assert_eq!(found.screen, Screen::Maximized, "{resolution:?}");
        }
        let trusted = identify_trusted(&im, &catalog).unwrap();
        assert_eq!(
            (trusted.screen, trusted.card_box),
            (Screen::Maximized, maximized)
        );
    }
    #[test]
    fn invalid_and_empty_catalog() {
        let im = RgbImage::new(100, 150);
        for b in [
            [i64::MAX, 0, i64::MIN, 150],
            [0, i64::MAX, 100, i64::MIN],
            [0, 0, i64::MAX, i64::MAX],
        ] {
            assert!(identify(&im, &Catalog::default(), &Resolution::Box(b)).is_none());
        }
        assert!(
            identify(
                &im,
                &Catalog::default(),
                &Resolution::Box([-1, 0, 100, 150])
            )
            .is_none()
        );
        assert!(identify(&im, &Catalog::default(), &Resolution::Auto).is_none());
    }
    #[test]
    fn detected_box_too_small_does_not_trigger_nudged_search() {
        let mut im = RgbImage::new(90, 60);
        for y in 2..58 {
            for x in 5..43 {
                im.put_pixel(x, y, image::Rgb([220, 220, 220]));
            }
        }
        let detected = layout::detect_card_box(&im).expect("card edge should be detected");
        assert!(detected[2] - detected[0] < 40, "{detected:?}");
        assert!(detected[0] < 20, "{detected:?}");
        let larger = layout::nudged(
            detected,
            im.dimensions(),
            &[-0.03, -0.015, 0., 0.015, 0.03],
            &[0.94, 0.97, 1., 1.03, 1.06],
        )
        .into_iter()
        .find(|b| b[2] - b[0] >= 40 && b[3] - b[1] >= 40)
        .expect("nudged search should have a valid box");
        let hash = artwork_hash(&im, larger, false, false).unwrap();
        let catalog =
            Catalog::from_json(&format!(r#"{{"hashes":{{"1":["{hash}"]}}}}"#), None, None).unwrap();
        assert!(
            layout::trusted_boxes(&im, &Resolution::Auto)
                .into_iter()
                .all(|b| evaluate(&im, b, false, false, &catalog).is_none())
        );
        assert!(evaluate(&im, detected, true, false, &catalog).is_none());
        assert!(evaluate(&im, larger, true, true, &catalog).is_some());
        assert!(identify(&im, &catalog, &Resolution::Auto).is_none());
    }
}
