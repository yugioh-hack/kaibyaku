use serde_json::Value;
use std::collections::{HashMap, HashSet};

const SPECIAL: [&str; 5] = ["5224", "5271", "5272", "5273", "5274"];
const BUNDLED: &str = include_str!("../../../../data/card_hashes.json");
#[derive(Clone, Debug)]
struct Row {
    cid: String,
    upper: u64,
    lower: u64,
    center: Option<u64>,
}
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    rows: Vec<Row>,
    pub names: HashMap<String, HashMap<String, String>>,
}

fn valid_hash(v: &Value) -> Option<&str> {
    v.as_str().filter(|s| {
        [32, 48].contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}
impl Catalog {
    pub fn bundled(user_hashes: Option<&str>, names: Option<&str>) -> Result<Self, String> {
        Self::from_json(BUNDLED, user_hashes, names)
    }
    pub fn from_json(
        bundled: &str,
        user: Option<&str>,
        names: Option<&str>,
    ) -> Result<Self, String> {
        let base: Value = serde_json::from_str(bundled).map_err(|e| e.to_string())?;
        let mut hashes: serde_json::Map<String, Value> = base
            .get("hashes")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if let Some(u) = user
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .filter(|u| u["meta"]["format"] == 2)
            && let Some(extra) = u["hashes"].as_object()
        {
            for (cid, hs) in extra {
                let current = hashes
                    .entry(cid.clone())
                    .or_insert_with(|| Value::Array(vec![]));
                if !current.is_array() {
                    *current = Value::Array(vec![]);
                }
                if let Some(hs) = hs.as_array() {
                    let cur = current.as_array_mut().unwrap();
                    for h in hs.iter().filter(|h| valid_hash(h).is_some()) {
                        if !cur.contains(h) {
                            cur.push(h.clone());
                        }
                    }
                }
            }
        }
        let mut out = Self::default();
        for (cid, hs) in hashes {
            if let Some(hs) = hs.as_array() {
                for h in hs.iter().filter_map(valid_hash) {
                    out.rows.push(Row {
                        upper: u64::from_str_radix(&h[..16], 16).unwrap(),
                        lower: u64::from_str_radix(&h[16..32], 16).unwrap(),
                        center: if h.len() == 48 && SPECIAL.contains(&cid.as_str()) {
                            u64::from_str_radix(&h[32..], 16).ok()
                        } else {
                            None
                        },
                        cid: cid.clone(),
                    });
                }
            }
        }
        if let Some(n) = names.and_then(|s| serde_json::from_str::<Value>(s).ok())
            && let Some(cards) = n["cards"].as_object()
        {
            for (cid, info) in cards {
                if let Some(fields) = info.as_object() {
                    out.names.insert(
                        cid.clone(),
                        fields
                            .iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect(),
                    );
                }
            }
        }
        Ok(out)
    }
    pub fn hash_count(&self) -> usize {
        self.rows.len()
    }
    fn plain_name(&self, cid: &str, english: bool) -> String {
        let info = self.names.get(cid);
        if english && let Some(e) = info.and_then(|n| n.get("e")).filter(|s| !s.is_empty()) {
            return e.clone();
        }
        info.and_then(|n| n.get("n"))
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| format!("cid_{cid}"))
    }
    pub fn display_name(&self, cid: &str, english: bool) -> String {
        let mut name = self.plain_name(cid, english);
        if ["4370", "19092"].contains(&cid) {
            let other = if cid == "4370" { "19092" } else { "4370" };
            if name == self.plain_name(other, english) {
                name.push_str(if cid == "4370" {
                    "（儀式）"
                } else {
                    "（通常）"
                });
            }
        }
        name
    }
    pub fn has_english_name(&self, cid: &str) -> bool {
        self.names.get(cid).is_some_and(|n| {
            n.get("e").is_some_and(|s| !s.is_empty()) || n.get("l").is_some_and(|s| s == "en")
        })
    }
    fn key(&self, cid: &str) -> String {
        if ["4370", "19092"].contains(&cid) {
            "twin".into()
        } else if let Some(n) = self
            .names
            .get(cid)
            .and_then(|n| n.get("n"))
            .filter(|s| !s.is_empty())
        {
            format!("name:{n}")
        } else {
            format!("cid:{cid}")
        }
    }
    pub fn rank(
        &self,
        hash: &str,
        top: usize,
        coarse: bool,
        center: impl FnOnce() -> Option<String>,
    ) -> Vec<(u32, String)> {
        let needed = if coarse { 16 } else { 32 };
        if hash.len() != needed || !hash.is_ascii() || top == 0 {
            return vec![];
        }
        let Ok(upper) = u64::from_str_radix(&hash[..16], 16) else {
            return vec![];
        };
        let lower = if coarse {
            0
        } else {
            let Ok(v) = u64::from_str_radix(&hash[16..32], 16) else {
                return vec![];
            };
            v
        };
        let mut rows: Vec<(u32, usize)> = self
            .rows
            .iter()
            .enumerate()
            .map(|(i, r)| {
                (
                    (r.upper ^ upper).count_ones()
                        + if coarse {
                            0
                        } else {
                            (r.lower ^ lower).count_ones()
                        },
                    i,
                )
            })
            .collect();
        rows.sort_by_key(|r| *r);
        if rows.is_empty() {
            return vec![];
        }
        let threshold = rows[(rows.len().min(256)) - 1].0;
        rows.retain(|r| r.0 <= threshold);
        if !coarse
            && SPECIAL.contains(&self.rows[rows[0].1].cid.as_str())
            && let Some(mid) = center().and_then(|s| u64::from_str_radix(&s, 16).ok())
        {
            let family = rows[0].0;
            let mut totals: Vec<(String, u32)> = vec![];
            let mut others = vec![];
            let mut seen = HashSet::new();
            for &(d, i) in &rows {
                let r = &self.rows[i];
                if SPECIAL.contains(&r.cid.as_str()) {
                    let total = d + r.center.map_or(32, |v| (v ^ mid).count_ones());
                    if let Some(v) = totals.iter_mut().find(|v| v.0 == r.cid) {
                        v.1 = v.1.min(total);
                    } else {
                        totals.push((r.cid.clone(), total));
                    }
                } else if seen.insert(&r.cid) {
                    others.push((d, r.cid.clone()));
                }
            }
            totals.sort_by_key(|v| v.1);
            let best = totals[0].1;
            let mut merged: Vec<_> = totals
                .into_iter()
                .map(|(cid, t)| (family + t - best, cid))
                .chain(others)
                .collect();
            merged.sort_by_key(|r| r.0);
            merged.truncate(top);
            return merged;
        }
        let mut seen = HashSet::new();
        rows.into_iter()
            .filter_map(|(d, i)| {
                let r = &self.rows[i];
                seen.insert(self.key(&r.cid)).then(|| (d, r.cid.clone()))
            })
            .take(top)
            .collect()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_and_grouped() {
        let c=Catalog::from_json(r#"{"hashes":{"9":["00000000000000000000000000000000"],"1":["00000000000000000000000000000000"],"2":["00000000000000000000000000000001"]}}"#,None,Some(r#"{"cards":{"9":{"n":"same"},"1":{"n":"same"}}}"#)).unwrap();
        assert_eq!(
            c.rank(&"0".repeat(32), 3, false, || None),
            vec![(0, "9".into()), (1, "2".into())]
        );
    }
    #[test]
    fn special_center_keeps_family_distance() {
        let c=Catalog::from_json(r#"{"hashes":{"5224":["00000000000000000000000000000000ffffffffffffffff"],"5271":["000000000000000000000000000000000000000000000000"]}}"#,None,None).unwrap();
        assert_eq!(
            c.rank(&"0".repeat(32), 2, false, || Some("0".repeat(16))),
            vec![(0, "5271".into()), (64, "5224".into())]
        );
    }
    #[test]
    fn malformed_types_do_not_panic() {
        for s in ["{}", "[]", r#"{"hashes":{"1":[5,"xx"],"2":4}}"#] {
            assert_eq!(
                Catalog::from_json(s, Some("[]"), Some(r#"{"cards":{"3":4}}"#))
                    .unwrap()
                    .hash_count(),
                0
            );
        }
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    #[test]
    fn partition_boundary_retains_all_ties_in_input_order() {
        let mut hashes = serde_json::Map::new();
        for cid in (1..=300).rev() {
            hashes.insert(
                cid.to_string(),
                serde_json::json!(["00000000000000000000000000000000"]),
            );
        }
        let c = Catalog::from_json(
            &serde_json::json!({"hashes":hashes}).to_string(),
            None,
            None,
        )
        .unwrap();
        let ranked = c.rank(&"0".repeat(32), 300, false, || None);
        assert_eq!(ranked.len(), 300);
        assert_eq!(ranked[0].1, "300");
        assert_eq!(ranked[299].1, "1");
    }
    #[test]
    fn user_hash_merges_with_existing_card_before_next_card() {
        let c=Catalog::from_json(r#"{"hashes":{"9":["00000000000000000000000000000001"],"1":["00000000000000000000000000000000"]}}"#,Some(r#"{"meta":{"format":2},"hashes":{"9":["00000000000000000000000000000000"],"2":["00000000000000000000000000000000"]}}"#),None).unwrap();
        assert_eq!(
            c.rank(&"0".repeat(32), 3, false, || None),
            vec![(0, "9".into()), (0, "1".into()), (0, "2".into())]
        );
    }
}
