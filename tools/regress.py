# -*- coding: utf-8 -*-
"""配布者用ツール: 手元の切り抜き画像で、照合の結果が悪くなっていないかを確かめる。

mdript が保存した切り抜き画像（ファイル名＝カード名）を正解データとして同梱ハッシュDBと
照合し、結果を基準値（tests/regress_baseline.json）と比べる。しきい値・ハッシュの領域・
同梱DBを変えたときに、リリース前に手元で回す。

画像もカード名もリポジトリに含めない方針のため、CI では実行しない。
基準値に書くのはカードIDと件数・距離だけ。

使い方:
    python tools/regress.py --crops 切り抜きのフォルダ
    # 枠の決定まで含めて確かめる（log.csv に残っている元のスクショを使う）
    python tools/regress.py --crops 切り抜きのフォルダ --screenshots
    # 今の結果を基準値として保存する
    python tools/regress.py --crops 切り抜きのフォルダ --screenshots --update-baseline

確かめる内容:
  * 切り抜き画像 … 画像全体をカードとして照合する（枠の決定は通らない）。
                    照合・しきい値・同梱DBの確認になる。
  * 元のスクショ … 原本のほか、縮小・JPEG・ウィンドウモードを模した画像を通す。

失敗（終了コード 1）にする条件:
  * 切り抜き画像に誤確定がある
  * 基準値では確定していたカードが、確定しなくなった
  * 元のスクショで誤確定が増えた、または確定率が 1 ポイントを超えて下がった
距離の分布は、領域や縮小方法を変えると全体が動くので、参考として表示するだけにする。

あわせて、フォルダ側の誤り（別のカードの名前で保存された画像）と、
同梱DBに足すべきカード（MD とマスター画像でイラストの切り方が違うもの）も一覧にする。
"""
import argparse
import collections
import csv
import hashlib
import io
import json
import os
import sys
from concurrent.futures import ProcessPoolExecutor

import numpy as np

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from build_hash_db import EXTS, file_key, stem_to_key  # noqa: E402
from mdript.catalog import (BUNDLED_HASHES, SPECIAL_CIDS, USER_NAMES, Catalog,  # noqa: E402
                            _hex_list, _popcount, bundled_data_dir, user_data_dir)
from mdript.layout import TRUSTED  # noqa: E402
from mdript.phash import HASH_HEX, HEX_PER_REGION, hamming  # noqa: E402
from mdript.pipeline import (CONFIRM_DIFF_WIDE, CONFIRMED, DUPLICATE_DIFF, NOT_CARD,  # noqa: E402
                             REVIEW, REVIEW_DIFF, Cropper, Options)

WRONG = "誤確定"
STATUSES = (CONFIRMED, REVIEW, NOT_CARD, WRONG)
DEFAULT_BASELINE = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                "..", "tests", "regress_baseline.json")
BASELINE_FORMAT = 1

# 元のスクショから作る画像の種類。先頭の3つは全画面、最後はウィンドウモードの模擬
SHOT_CONDITIONS = ("原本", "1280x720", "FHD+JPEG50", "ウィンドウ模擬")
RATE_TOLERANCE = 1.0      # 確定率がこのポイント数を超えて下がったら失敗にする

_cropper = None


def bundled_catalog():
    """同梱のハッシュDBだけを読んだ Catalog（手元で追加したハッシュは結果に混ぜない）。"""
    catalog = Catalog()
    with open(os.path.join(bundled_data_dir(), BUNDLED_HASHES), encoding="utf-8") as f:
        bundled = json.load(f)
    catalog.hashes = {cid: _hex_list(hexes) for cid, hexes in bundled["hashes"].items()}
    catalog._reindex()
    return catalog


def _init_worker():
    global _cropper
    _cropper = Cropper(bundled_catalog(), Options())


def judge(found, cid):
    """照合の結果を、正解のカードIDと比べて4つの状態に分ける。"""
    if found is None:
        return NOT_CARD
    if Cropper.is_confirmed(found):
        return CONFIRMED if found["card_id"] == cid else WRONG
    return REVIEW if found["distance"] <= REVIEW_DIFF else NOT_CARD


# ---------- 切り抜き画像 ----------
def check_crop(arg):
    path, cid = arg
    try:
        from PIL import Image
        with Image.open(path) as raw:
            img = raw.convert("RGB")
        found = _cropper._evaluate(img, (0, 0) + img.size, TRUSTED)
        if found is None:
            return {"file": os.path.basename(path), "cid": cid, "error": "画像が小さすぎます"}
        known = _cropper.catalog.hashes.get(cid, [])
        own = min((hamming(h[:HASH_HEX], found["hash"]) for h in known), default=None)
        return {"file": os.path.basename(path), "cid": cid, "status": judge(found, cid),
                "own": own, "distance": found["distance"], "margin": found["margin"],
                "top": found["card_id"], "hash": found["hash"]}
    except Exception as exc:
        return {"file": os.path.basename(path), "cid": cid,
                "error": f"{type(exc).__name__}: {exc}"}


def load_names(path):
    """カード名（ファイル名にしたときの形）→ カードID。"""
    with open(path, encoding="utf-8") as f:
        cards = json.load(f).get("cards", {})
    key_to_id = {}
    for cid, info in cards.items():
        for name in (info.get("n"), info.get("r")):
            if name:
                key_to_id.setdefault(file_key(name), str(cid))
    return key_to_id, {str(cid): info.get("n") or str(cid) for cid, info in cards.items()}


def same_artwork_pairs(results):
    """別のカードIDなのに同じイラストになっている切り抜きの組。"""
    rows = [r for r in results if "hash" in r]
    ids = np.array([r["cid"] for r in rows], dtype=object)
    upper = np.array([int(r["hash"][:HEX_PER_REGION], 16) for r in rows], dtype=np.uint64)
    lower = np.array([int(r["hash"][HEX_PER_REGION:], 16) for r in rows], dtype=np.uint64)
    pairs = []
    for i, row in enumerate(rows[:-1]):
        dist = (_popcount(upper[i + 1:] ^ upper[i]).astype(np.int64)
                + _popcount(lower[i + 1:] ^ lower[i]).astype(np.int64))
        for j in np.nonzero(dist <= DUPLICATE_DIFF)[0]:
            other = rows[i + 1 + int(j)]
            if other["cid"] == row["cid"]:
                continue
            if row["cid"] in SPECIAL_CIDS and other["cid"] in SPECIAL_CIDS:
                continue              # ウィジャ盤・死のメッセージは上下の領域が元から同じ
            pairs.append((int(dist[j]), row["file"], other["file"]))
    return sorted(pairs)


def summarize_crops(results):
    """切り抜き画像の結果を、基準値と同じ形にまとめる。"""
    ok = [r for r in results if "status" in r]
    status = collections.Counter(r["status"] for r in ok)
    own = np.array([r["own"] for r in ok if r["own"] is not None])
    not_confirmed = collections.Counter(r["cid"] for r in ok if r["status"] != CONFIRMED)
    return {
        "files": len(ok),
        "cards": len({r["cid"] for r in ok}),
        "status": {s: status.get(s, 0) for s in STATUSES},
        "top1_wrong": sum(1 for r in ok if r["top"] != r["cid"]),
        "own_distance": {
            "median": int(np.median(own)) if own.size else 0,
            "p99": int(np.percentile(own, 99)) if own.size else 0,
            "max": int(own.max()) if own.size else 0,
        },
        "not_confirmed": dict(sorted(not_confirmed.items())),
    }


def compare_crops(now, base):
    """悪くなった点を文章の一覧で返す。base が None のときは誤確定だけを見る。"""
    problems = []
    if now["status"][WRONG]:
        problems.append(f"切り抜き画像に誤確定が {now['status'][WRONG]} 枚あります")
    if base:
        before = base.get("not_confirmed", {})
        worse = sorted(cid for cid, n in now["not_confirmed"].items() if n > before.get(cid, 0))
        if worse:
            problems.append(f"確定しなくなった画像のあるカードが {len(worse)} 種あります"
                            f"（カードID: {', '.join(worse[:20])}"
                            f"{' ほか' if len(worse) > 20 else ''}）")
    return problems


def run_crops(args, key_to_id):
    files = sorted(f for f in os.listdir(args.crops) if f.lower().endswith(EXTS))
    todo, unresolved = [], []
    for name in files:
        cid = key_to_id.get(stem_to_key(name))
        if cid:
            todo.append((os.path.join(args.crops, name), cid))
        else:
            unresolved.append(name)
    print(f"切り抜き画像: {len(files)} 枚（カードIDが分かったもの {len(todo)} 枚）")

    results = []
    with ProcessPoolExecutor(max_workers=args.workers, initializer=_init_worker) as ex:
        for i, r in enumerate(ex.map(check_crop, todo, chunksize=64), 1):
            results.append(r)
            if i % 2000 == 0:
                print(f"  {i}/{len(todo)}")
    return results, unresolved


def report_crops(results, unresolved, summary, base, names, top):
    def name_of(cid):
        return names.get(cid, f"cid_{cid}")

    def show(title, rows, line):
        if not rows:
            return
        print(f"\n[{title}] {len(rows)} 件")
        for r in rows[:top]:
            print("  " + line(r))
        if len(rows) > top:
            print(f"  … ほか {len(rows) - top} 件")

    st, dist = summary["status"], summary["own_distance"]
    print(f"\n確定 {st[CONFIRMED]} / 要確認 {st[REVIEW]} / カードなし {st[NOT_CARD]} / "
          f"誤確定 {st[WRONG]}   1位が別のカード {summary['top1_wrong']}")
    line = f"正解までの距離: 中央 {dist['median']} / p99 {dist['p99']} / 最大 {dist['max']}"
    if base:
        b = base["own_distance"]
        line += f"   （基準値: 中央 {b['median']} / p99 {b['p99']} / 最大 {b['max']}）"
    print(line)

    ok = [r for r in results if "status" in r]
    show("読めなかった画像", [r for r in results if "error" in r],
         lambda r: f"{r['file']}  {r['error']}")
    show("カード名に当たらないファイル名", unresolved, lambda f: f)
    show("同梱DBにカードIDが無い", [r for r in ok if r["own"] is None],
         lambda r: f"{r['file']}  カードID {r['cid']}")
    show(WRONG, [r for r in ok if r["status"] == WRONG],
         lambda r: f"{r['file']} → {name_of(r['top'])}（{r['top']}）"
                   f" 距離 {r['distance']} 差 {r['margin']}")
    show("確定しなかった画像", [r for r in ok if r["status"] in (REVIEW, NOT_CARD)],
         lambda r: f"{r['status']}  {r['file']}（{r['cid']}）"
                   f" 距離 {r['distance']} 差 {r['margin']} 正解まで {r['own']}")
    # ここから下はフォルダと同梱DBの点検。名前違いか、マスター画像と切り方が違うカード
    far = sorted((r for r in ok if r["own"] is not None and r["own"] > CONFIRM_DIFF_WIDE),
                 key=lambda r: -r["own"])
    show(f"正解までの距離が {CONFIRM_DIFF_WIDE} を超える（名前違い、または同梱DBに足す候補）", far,
         lambda r: f"{r['file']}（{r['cid']}） 正解まで {r['own']}"
                   f" / 最も近いのは {name_of(r['top'])} 距離 {r['distance']}")
    show("別のカードと同じイラスト（どちらかが名前違い）", same_artwork_pairs(results),
         lambda p: f"距離 {p[0]}  {p[1]}  ⇔  {p[2]}")


# ---------- 元のスクショ ----------
def pick_screenshots(log_path, count):
    """log.csv から、カード1種につき1枚のスクショを選ぶ。

    カードIDのハッシュ順に取るので、log.csv に行が増えても選ばれる顔ぶれはあまり変わらない。
    """
    first = {}
    with open(log_path, encoding="utf-8-sig", newline="") as f:
        for row in csv.DictReader(f):
            cid = row.get("カードID") or ""
            if row.get("状態") == CONFIRMED and cid and row.get("入力ファイル"):
                first.setdefault(cid, row["入力ファイル"])
    picked = []
    for cid in sorted(first, key=lambda c: hashlib.sha1(c.encode()).hexdigest()):
        if os.path.exists(first[cid]):
            picked.append((first[cid], cid))
            if len(picked) >= count:
                break
    return picked


def shot_variants(img):
    """元のスクショから、条件の悪い入力を作る（SHOT_CONDITIONS と同じ順）。"""
    from PIL import Image
    yield img
    yield img.resize((1280, 720), Image.LANCZOS)
    buf = io.BytesIO()
    img.resize((1920, 1080), Image.LANCZOS).save(buf, "JPEG", quality=50)
    yield Image.open(io.BytesIO(buf.getvalue())).convert("RGB")
    # タイトルバーの分だけ縦にずれた画像。解像度からの推定が外れ、枠の検出に回る
    window = Image.new("RGB", (1600, 932), (32, 32, 32))
    window.paste(img.resize((1600, 900), Image.LANCZOS), (0, 32))
    yield window


def check_shot(arg):
    path, cid = arg
    try:
        from PIL import Image
        with Image.open(path) as raw:
            img = raw.convert("RGB")
        if abs(img.size[0] / img.size[1] - 16 / 9) > 0.01:
            return None                # 16:9 の全画面スクショだけを使う
        return [judge(_cropper.identify(v), cid) for v in shot_variants(img)]
    except Exception:
        return None


def run_screenshots(args):
    picked = pick_screenshots(args.log, args.sample)
    print(f"\n元のスクショ: {len(picked)} 枚を {len(SHOT_CONDITIONS)} 通りで照合 ...")
    counts = {c: collections.Counter() for c in SHOT_CONDITIONS}
    used = 0
    with ProcessPoolExecutor(max_workers=args.workers, initializer=_init_worker) as ex:
        for statuses in ex.map(check_shot, picked, chunksize=4):
            if statuses is None:
                continue
            used += 1
            for cond, status in zip(SHOT_CONDITIONS, statuses):
                counts[cond][status] += 1
    return {"sample": used,
            "conditions": {c: {s: counts[c].get(s, 0) for s in STATUSES}
                           for c in SHOT_CONDITIONS}}


def _rate(row):
    total = sum(row.get(s, 0) for s in STATUSES)
    return 100.0 * row.get(CONFIRMED, 0) / total if total else 0.0


def compare_screenshots(now, base):
    problems = []
    for cond, row in now["conditions"].items():
        before = (base or {}).get("conditions", {}).get(cond)
        if before is None:
            continue
        if row[WRONG] > before.get(WRONG, 0):
            problems.append(f"スクショ（{cond}）の誤確定が {before.get(WRONG, 0)} → {row[WRONG]} 枚に増えました")
        if _rate(row) < _rate(before) - RATE_TOLERANCE:
            problems.append(f"スクショ（{cond}）の確定率が {_rate(before):.1f}% → {_rate(row):.1f}% に下がりました")
    return problems


def report_screenshots(now, base):
    for cond, row in now["conditions"].items():
        line = (f"  {cond:<12} 確定 {row[CONFIRMED]} ({_rate(row):.1f}%) / 要確認 {row[REVIEW]} / "
                f"カードなし {row[NOT_CARD]} / 誤確定 {row[WRONG]}")
        before = (base or {}).get("conditions", {}).get(cond)
        if before:
            line += f"   （基準値: 確定 {_rate(before):.1f}% / 誤確定 {before.get(WRONG, 0)}）"
        print(line)


# ---------- 基準値 ----------
def load_baseline(path):
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return {}
    if not isinstance(data, dict) or (data.get("meta") or {}).get("format") != BASELINE_FORMAT:
        return {}
    return data


def save_baseline(path, baseline):
    baseline["meta"] = {"format": BASELINE_FORMAT}
    os.makedirs(os.path.dirname(os.path.abspath(path)), exist_ok=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(baseline, f, ensure_ascii=False, indent=1, sort_keys=True)
        f.write("\n")


def main():
    ap = argparse.ArgumentParser(description="切り抜き画像で照合の結果を確かめる（配布者用）")
    ap.add_argument("--crops", required=True, help="mdript が保存した切り抜き画像のフォルダ")
    ap.add_argument("--names", default=os.path.join(user_data_dir(), USER_NAMES),
                    help="カード名のファイル（既定は［カード名を取得］で保存したもの）")
    ap.add_argument("--baseline", default=DEFAULT_BASELINE)
    ap.add_argument("--update-baseline", action="store_true", help="今の結果を基準値として保存する")
    ap.add_argument("--screenshots", action="store_true",
                    help="log.csv に残っている元のスクショも通す（枠の決定まで確かめる）")
    ap.add_argument("--log", help="元のスクショを探す log.csv（既定は切り抜きのフォルダの中）")
    ap.add_argument("--sample", type=int, default=300, help="通すスクショの枚数")
    ap.add_argument("--top", type=int, default=30, help="一覧に出す件数")
    ap.add_argument("--workers", type=int, default=0)
    args = ap.parse_args()
    args.workers = args.workers or max(1, (os.cpu_count() or 2) - 1)
    args.log = args.log or os.path.join(args.crops, "log.csv")
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(errors="replace")      # 端末で表示できない字があっても止めない

    if not os.path.isdir(args.crops):
        ap.error(f"フォルダがありません: {args.crops}")
    if not os.path.exists(args.names):
        ap.error(f"カード名のファイルがありません: {args.names}")
    if args.screenshots and not os.path.exists(args.log):
        ap.error(f"log.csv がありません: {args.log}")

    key_to_id, names = load_names(args.names)
    baseline = load_baseline(args.baseline)
    if not baseline:
        print("基準値がまだありません（--update-baseline で保存できます）")

    results, unresolved = run_crops(args, key_to_id)
    crops = summarize_crops(results)
    report_crops(results, unresolved, crops, baseline.get("crops"), names, args.top)
    problems = compare_crops(crops, baseline.get("crops"))
    new_baseline = dict(baseline, crops=crops)

    if args.screenshots:
        shots = run_screenshots(args)
        report_screenshots(shots, baseline.get("screenshots"))
        problems += compare_screenshots(shots, baseline.get("screenshots"))
        new_baseline["screenshots"] = shots

    print()
    for p in problems:
        print(f"[失敗] {p}")
    if args.update_baseline:
        save_baseline(args.baseline, new_baseline)
        print(f"基準値を保存しました: {os.path.normpath(args.baseline)}")
        return 0
    if not problems:
        print("問題なし")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
