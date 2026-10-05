# -*- coding: utf-8 -*-
"""配布者用ツール: マスター画像 → 同梱ハッシュDB (data/card_hashes.json) を作る。

アプリ本体には「カード名」も「マスター画像」も含めない方針のため、
ハッシュDB のキーは公式データベースのカードID (cid) にする。
カード名はアプリ側で `カードデータ更新` から取得する。

使い方:
    python tools/build_hash_db.py --master-dir ../YGO-list/master_images \
                                  --card-json  ../YGO-list/json/yugioh_cards.json \
                                  --out data/card_hashes.json
    # すでに作ってあるハッシュキャッシュを流用する場合
    python tools/build_hash_db.py --hash-cache ../MD/json/master_hashes.json ...
"""
import argparse
import json
import os
import re
import sys
import unicodedata
from concurrent.futures import ProcessPoolExecutor

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from mdript.catalog import SPECIAL_CIDS  # noqa: E402
from mdript.phash import ART_BOXES, HASH_HEX, HASH_SIZE, artwork_phash_hex  # noqa: E402

EXTS = (".jpg", ".jpeg", ".png", ".webp")
_VARIANT = re.compile(r"(?:_alt\d+|_\d+| \(\d+\))+$")


def file_key(name: str) -> str:
    safe = name.translate(str.maketrans(r'\/:*?"<>|', '￥／：＊？”＜＞｜'))
    safe = "".join(c for c in safe if ord(c) >= 32).strip().rstrip(".")
    return unicodedata.normalize("NFKC", safe).casefold()


def stem_to_key(path: str) -> str:
    stem = os.path.splitext(os.path.basename(path))[0]
    return file_key(_VARIANT.sub("", stem))


def hash_one(path):
    try:
        from PIL import Image
        with Image.open(path) as im:
            return os.path.basename(path), artwork_phash_hex(im, with_center=True)   # 中央の領域つき（48文字）
    except Exception:
        return os.path.basename(path), None


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--master-dir", required=True)
    ap.add_argument("--card-json", help="id / name / english_name を含むカードJSON")
    ap.add_argument("--names-json", help="updater.fetch_card_names() が作った cards.json（新カードの解決に使う）")
    ap.add_argument("--hash-cache", help="既存の master_hashes.json（あれば再計算を省く）")
    ap.add_argument("--out", default="data/card_hashes.json")
    ap.add_argument("--workers", type=int, default=0)
    args = ap.parse_args()

    key_to_id = {}
    if args.names_json:
        with open(args.names_json, encoding="utf-8") as f:
            fetched = json.load(f).get("cards", {})
        for cid, v in fetched.items():
            key_to_id.setdefault(file_key(v["n"]), str(cid))
        print(f"取得済みカード名: {len(fetched)} 件")
    cards = []
    if args.card_json:
        with open(args.card_json, encoding="utf-8") as f:
            cards = json.load(f)
    for c in cards:
        cid = str(c.get("id") or "")
        if not cid or "name" not in c:
            continue
        key_to_id.setdefault(file_key(c["name"]), cid)
        if c.get("english_name"):
            key_to_id.setdefault(file_key(c["english_name"]), cid)
    if not key_to_id:
        ap.error("--card-json か --names-json のどちらかが必要です")
    print(f"カードDB: {len(cards)} 件 / 照合キー {len(key_to_id)} 件")

    files = []
    for ext in EXTS:
        files += [os.path.join(args.master_dir, f) for f in os.listdir(args.master_dir)
                  if f.lower().endswith(ext)]
    files.sort()
    print(f"マスター画像: {len(files)} 枚")


    cache = {}
    if args.hash_cache and os.path.exists(args.hash_cache):
        with open(args.hash_cache, encoding="utf-8") as f:
            raw = json.load(f)
        meta = raw.get("meta", {})
        if (meta.get("art_boxes") == [list(b) for b in ART_BOXES]
                and meta.get("hash_size") == HASH_SIZE):
            cache = raw.get("hashes", {})
            print(f"既存キャッシュを流用: {len(cache)} 件")
        else:
            print("既存キャッシュは生成条件が違うため無視します")

    todo = [p for p in files if os.path.basename(p) not in cache]
    if todo:
        print(f"ハッシュ計算: {len(todo)} 枚 ...")
        workers = args.workers or max(1, (os.cpu_count() or 2) - 1)
        with ProcessPoolExecutor(max_workers=workers) as ex:
            for i, (name, h) in enumerate(ex.map(hash_one, todo, chunksize=32), 1):
                if h:
                    cache[name] = h
                if i % 1000 == 0:
                    print(f"  {i}/{len(todo)}")

    hashes, unresolved = {}, []
    for fname, h in cache.items():
        cid = key_to_id.get(stem_to_key(fname))
        if not cid:
            unresolved.append(fname)
            continue
        if cid in SPECIAL_CIDS:
            if len(h) == HASH_HEX:
                print(f"[警告] {fname}: 中央の領域が無いキャッシュです（キャッシュを作り直してください）")
        else:
            h = h[:HASH_HEX]             # 通常のカードは上下の2領域だけ
        hashes.setdefault(cid, [])
        if h not in hashes[cid]:
            hashes[cid].append(h)

    out = {
        "meta": {
            "format": 2,
            "art_boxes": [list(b) for b in ART_BOXES],
            "hash_size": HASH_SIZE,
            "cards": len(hashes),
            "images": sum(len(v) for v in hashes.values()),
        },
        "hashes": hashes,
    }
    os.makedirs(os.path.dirname(os.path.abspath(args.out)), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(out, f, ensure_ascii=False, separators=(",", ":"))
    print(f"出力: {args.out}  カード {len(hashes)} 種 / ハッシュ {out['meta']['images']} 件")
    if unresolved:
        print(f"[警告] カードIDに紐付かない画像 {len(unresolved)} 枚（例: {unresolved[:5]}）")


if __name__ == "__main__":
    main()
