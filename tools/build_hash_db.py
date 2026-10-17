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

英語版の公式DBにしか無いカード（TCG先行・TCGの賞品カードなど）:
    ygo の en_cards.py が作る master_images_en/（ファイル名がカードID。イラスト違いは
    <カードID>_1.jpg, _2.jpg）と json/yugioh_cards_en_only.json を、次の引数で読む。
    カードIDは日本語版と共通なので、名前を通さずにファイル名をそのまま cid として扱う。
    python tools/build_hash_db.py --master-dir ... --card-json ... \
                                  --master-dir-en ../YGO-list/master_images_en \
                                  --card-json-en  ../YGO-list/json/yugioh_cards_en_only.json
    # 今の同梱DB（--learn で足したハッシュも入っている）に英語版だけのカードを足す場合
    python tools/build_hash_db.py --base data/card_hashes.json \
                                  --master-dir-en ... --card-json-en ...
"""
import argparse
import collections
import json
import os
import re
import sys
import unicodedata
from concurrent.futures import ProcessPoolExecutor

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from mdript.catalog import SPECIAL_CIDS  # noqa: E402
from mdript.phash import ART_BOXES, HASH_HEX, HASH_SIZE, artwork_phash_hex, hamming  # noqa: E402

EN_MIN_DISTANCE = 12   # 英語版の画像を既存のハッシュに足す、最小の距離（実測で決めた値。README 参照）

EXTS = (".jpg", ".jpeg", ".png", ".webp")
_VARIANT = re.compile(r"(?:_alt\d+|_\d+| \(\d+\))+$")


def file_key(name: str) -> str:
    safe = name.translate(str.maketrans(r'\/:*?"<>|', '￥／：＊？”＜＞｜'))
    safe = "".join(c for c in safe if ord(c) >= 32).strip().rstrip(".")
    return unicodedata.normalize("NFKC", safe).casefold()


def stem_to_key(path: str) -> str:
    stem = os.path.splitext(os.path.basename(path))[0]
    return file_key(_VARIANT.sub("", stem))


def stem_to_cid(path: str):
    """master_images_en の画像のファイル名からカードIDを返す（数字でなければ None）。"""
    stem = _VARIANT.sub("", os.path.splitext(os.path.basename(path))[0])
    return stem if stem.isdigit() else None


def list_images(folder):
    """フォルダ内のカード画像のパスを名前順に返す。"""
    return sorted(os.path.join(folder, f) for f in os.listdir(folder)
                  if f.lower().endswith(EXTS))


def hash_one(path):
    try:
        from PIL import Image
        with Image.open(path) as im:
            return os.path.basename(path), artwork_phash_hex(im, with_center=True)   # 中央の領域つき（48文字）
    except Exception:
        return os.path.basename(path), None


def hash_files(paths, workers):
    """画像ごとのハッシュ（中央の領域つき）を {ファイル名: hex} で返す。"""
    out = {}
    if not paths:
        return out
    print(f"ハッシュ計算: {len(paths)} 枚 ...")
    with ProcessPoolExecutor(max_workers=workers) as ex:
        for i, (name, h) in enumerate(ex.map(hash_one, paths, chunksize=32), 1):
            if h:
                out[name] = h
            if i % 1000 == 0:
                print(f"  {i}/{len(paths)}")
    return out


def load_base(path):
    """既存のハッシュDB（--base）を {cid: [hex, ...]} で読む。生成条件が違えば止める。"""
    with open(path, encoding="utf-8") as f:
        raw = json.load(f)
    meta = raw.get("meta", {})
    if meta.get("art_boxes") != [list(b) for b in ART_BOXES] or meta.get("hash_size") != HASH_SIZE:
        sys.exit(f"{path} は今と生成条件が違うため、--base に使えません")
    return {cid: list(v) for cid, v in (raw.get("hashes") or {}).items()}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--master-dir", help="日本語版のマスター画像のフォルダ（ファイル名がカード名）")
    ap.add_argument("--card-json", help="id / name / english_name を含むカードJSON")
    ap.add_argument("--names-json", help="updater.fetch_card_names() が作った cards.json（新カードの解決に使う）")
    ap.add_argument("--hash-cache", help="既存の master_hashes.json（あれば再計算を省く）")
    ap.add_argument("--master-dir-en", help="英語版だけのカードの画像のフォルダ（ファイル名がカードID）")
    ap.add_argument("--card-json-en",
                    help="英語版だけのカードのJSON（yugioh_cards_en_only.json）。画像の抜けの確認に使う")
    ap.add_argument("--en-min-distance", type=int, default=EN_MIN_DISTANCE,
                    help="英語版の画像を、同じカードの既存のハッシュに足す最小の距離（これ未満は足さない）")
    ap.add_argument("--base", help="このハッシュDBに足す（入っているハッシュは消さない）")
    ap.add_argument("--out", default="data/card_hashes.json")
    ap.add_argument("--workers", type=int, default=0)
    args = ap.parse_args()
    if not args.master_dir and not args.master_dir_en:
        ap.error("--master-dir か --master-dir-en のどちらかが必要です")
    workers = args.workers or max(1, (os.cpu_count() or 2) - 1)

    hashes = load_base(args.base) if args.base else {}
    if args.base:
        print(f"元のハッシュDB: カード {len(hashes)} 種 / ハッシュ {sum(map(len, hashes.values()))} 件")

    def add(cid, fname, h):
        """1枚分のハッシュを hashes に足す（通常のカードは上下の2領域だけにする）。"""
        if cid in SPECIAL_CIDS:
            if len(h) == HASH_HEX:
                print(f"[警告] {fname}: 中央の領域が無いキャッシュです（キャッシュを作り直してください）")
        else:
            h = h[:HASH_HEX]
        cur = hashes.setdefault(cid, [])
        if h not in cur:
            cur.append(h)

    unresolved = []
    if args.master_dir:
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
            ap.error("--master-dir には --card-json か --names-json のどちらかが必要です")
        print(f"カードDB: {len(cards)} 件 / 照合キー {len(key_to_id)} 件")

        files = list_images(args.master_dir)
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

        cache.update(hash_files([p for p in files if os.path.basename(p) not in cache], workers))
        for fname, h in cache.items():
            cid = key_to_id.get(stem_to_key(fname))
            if cid:
                add(cid, fname, h)
            else:
                unresolved.append(fname)

    if args.master_dir_en:
        # ファイル名がカードIDなので、名前の対応表を通さない
        # （キャッシュはファイル名で引くので、日本語版の画像と混ぜないよう使わない）
        files = list_images(args.master_dir_en)
        print(f"英語版の画像: {len(files)} 枚")
        en_cids, skipped, added_ja, dists = set(), 0, 0, []
        for fname, h in sorted(hash_files(files, workers).items(), key=lambda kv: (stem_to_cid(kv[0]) or "", kv[0])):
            cid = stem_to_cid(fname)
            if not cid:
                unresolved.append(fname)
                continue
            if cid in hashes:
                # すでにあるカード（日本語版の画像など）は、ほとんど同じ絵のハッシュでDBを膨らませない。
                # 規制で描き変えられた絵のように、既存のどれからも離れているものだけ足す
                d = min(hamming(h[:HASH_HEX], x[:HASH_HEX]) for x in hashes[cid])
                dists.append(d)
                if d < args.en_min_distance:
                    skipped += 1
                    continue
                added_ja += 1
                print(f"  描き変え候補: {fname} 既存との距離 {d}")
            add(cid, fname, h)
            en_cids.add(cid)
        print(f"英語版の画像から追加したカード: {len(en_cids)} 種（うち既存のカードに別の絵を足したもの {added_ja} 種のハッシュ）、"
              f"既存と近いので足さなかった画像 {skipped} 枚")
        if dists:
            hist = collections.Counter(min(d // 4 * 4, 40) for d in dists)
            print("  既存のカードとの距離の分布: " + " ".join(
                f"{lo}-{lo + 3 if lo < 40 else '以上'}:{hist[lo]}" for lo in sorted(hist)))
        if args.card_json_en:
            with open(args.card_json_en, encoding="utf-8") as f:
                listed = {str(c.get("id")) for c in json.load(f) if c.get("id")}
            lacking = sorted(listed - {stem_to_cid(os.path.basename(p)) for p in files}, key=int)
            if lacking:
                print(f"[警告] 画像の無い英語版だけのカード {len(lacking)} 件（例: {lacking[:5]}）")

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
