use crate::{
    artwork_hash,
    catalog::Catalog,
    layout::{self, Resolution},
};
use image::RgbImage;
#[derive(Clone, Debug)]
pub struct Found {
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
pub fn identify(im: &RgbImage, catalog: &Catalog, resolution: &Resolution) -> Option<Found> {
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
    let current_best = best.as_ref()?;
    let mut center = detected.unwrap_or(current_best.card_box);
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
    if let Some(f) = found {
        best = better(evaluate(im, f.card_box, true, false, catalog), best);
    }
    best
}
#[cfg(test)]
mod tests {
    use super::*;
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
