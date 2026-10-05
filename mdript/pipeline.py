# -*- coding: utf-8 -*-
"""切り抜き → 照合 → 保存の本体。"""
from __future__ import annotations

import csv
import os
import threading
import time
import unicodedata
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field

from PIL import Image, ImageDraw

from .catalog import Catalog
from .layout import (COARSE_SCALES, COARSE_SHIFTS, CORNER_RADIUS_RATIO, ESTIMATED,
                     FINE_SCALES, FINE_SHIFTS, card_boxes, nudged_boxes)
from .phash import artwork_phash_hex, center_phash_hex, hamming

IMAGE_EXTS = (".png", ".jpg", ".jpeg", ".webp", ".bmp")

# ハッシュ距離のしきい値（128bit / 上下2領域の pHash）。
#
# スクショ9,524枚とマスター14,753枚で測った実測値:
#   本物のカード    … 正解までの距離は中央14・p99が24・最大32（外れ値1枚を除く）
#   カード以外の画像 … 最も近いカードでも距離34以上（4,000回試して1件も32以下に入らない）
# 32と34の間できれいに分かれるので、そこを境にしている。
CONFIRM_DIFF = 16         # ここまで近ければ2位との差が小さくてもよい
CONFIRM_MARGIN = 2
CONFIRM_DIFF_WIDE = 32
CONFIRM_MARGIN_WIDE = 4
ESTIMATED_DIFF = 16       # 推定枠から確定してよい距離（取り違えを避けるため厳しめ）
ESTIMATED_MARGIN = 6
REVIEW_DIFF = 36          # ここまでは「要確認」として残す
DUPLICATE_DIFF = 10       # 同じ場所にある同名ファイルとの重複イラスト判定

SKIPPED_NOTE = "同じイラストがすでにあるのでスキップ"
_EXTS_BY_FORMAT = {"webp": (".webp",), "png": (".png",), "both": (".webp", ".png")}

CONFIRMED = "確定"
REVIEW = "要確認"
NOT_CARD = "カードなし"
ERROR = "エラー"


@dataclass
class Options:
    output_dir: str = "output"
    review_subdir: str = "review"
    not_card_subdir: str = "not_card"
    save_review: bool = True
    save_not_card: bool = True         # カードと判定できなかった画像も残す
    save_format: str = "webp"          # webp / png / both
    quality: int = 90
    rounded: bool = True               # 四隅を角丸にして透過させる
    resolution: object = "auto"        # "auto" / "fixed" / (幅, 高さ)
    skip_duplicate: bool = True
    recursive: bool = True
    write_csv: bool = True
    workers: int = 0                   # 0 で自動


@dataclass
class Result:
    source: str
    status: str
    card_id: str = ""
    name: str = ""
    distance: int = 999
    margin: int = 0
    saved_path: str = ""
    note: str = ""


@dataclass
class Summary:
    confirmed: int = 0
    review: int = 0
    not_card: int = 0
    error: int = 0
    skipped: int = 0
    results: list = field(default_factory=list)
    seconds: float = 0.0


_INVALID = str.maketrans('\\/:*?"<>|', '￥／：＊？”＜＞｜')

# Windows ではこの名前（拡張子が付いても）のファイルを作れない
_RESERVED = {"CON", "PRN", "AUX", "NUL",
             *(f"COM{i}" for i in range(1, 10)), *(f"LPT{i}" for i in range(1, 10))}
_MAX_NAME = 150           # 連番と拡張子を足しても 255 文字に収まる長さ


def safe_filename(name: str) -> str:
    # NFC で互換漢字（神 U+FA19 など）は普通の字になるが、意図どおり（CLAUDE.md 参照）
    safe = unicodedata.normalize("NFC", name).translate(_INVALID)
    # 末尾の空白・ピリオドは Windows が黙って削るので、先に落としておく
    safe = "".join(c for c in safe if ord(c) >= 32)[:_MAX_NAME].strip().rstrip(" .")
    if not safe:
        return "unknown"
    if safe.split(".")[0].upper() in _RESERVED:
        safe = "_" + safe
    return safe


def _csv_cell(value):
    """表計算ソフトで開いたときに数式として実行されないよう、先頭の記号を無効化する。"""
    if isinstance(value, str) and value[:1] in ("=", "+", "-", "@", chr(9), chr(13)):
        return "'" + value
    return value


def rounded_alpha(card: Image.Image) -> Image.Image:
    """四隅を角丸に切り抜いて透過させる（カードの実物に近い見た目にする）。"""
    card = card.convert("RGBA")
    radius = max(2, int(round(card.size[0] * CORNER_RADIUS_RATIO)))
    mask = Image.new("L", card.size, 0)
    ImageDraw.Draw(mask).rounded_rectangle((0, 0, card.size[0] - 1, card.size[1] - 1),
                                           radius=radius, fill=255)
    card.putalpha(mask)
    return card


def collect_images(paths, recursive=True, exclude=None):
    """ファイル・フォルダの一覧から、処理対象の画像パスを集める。

    exclude（出力先フォルダ）はサブフォルダを辿るときに飛ばす。
    スクショのフォルダの中に出力先を作っていても、前回の出力を読み直さないため。
    """
    skip = os.path.normcase(os.path.abspath(exclude)) if exclude else None
    found = []
    for p in paths:
        p = os.path.abspath(p)
        if os.path.isdir(p):
            if recursive:
                for root, dirs, files in os.walk(p):
                    dirs[:] = sorted(d for d in dirs
                                     if os.path.normcase(os.path.join(root, d)) != skip)
                    found += [os.path.join(root, f) for f in sorted(files)
                              if f.lower().endswith(IMAGE_EXTS)]
            else:
                found += [os.path.join(p, f) for f in sorted(os.listdir(p))
                          if f.lower().endswith(IMAGE_EXTS)]
        elif p.lower().endswith(IMAGE_EXTS):
            found.append(p)
    seen, out = set(), []
    for f in found:
        key = os.path.normcase(f)
        if key not in seen:
            seen.add(key)
            out.append(f)
    return out


class Cropper:
    def __init__(self, catalog=None, options=None):
        self.catalog = catalog or Catalog()
        self.options = options or Options()
        self._lock = threading.Lock()
        self._saved = {}               # 出力パス -> ハッシュ（同時保存の衝突回避）

    # ---------- 1枚の判定 ----------
    def _evaluate(self, img, box, origin, coarse=False):
        """枠を1つ試して、最も近いカードと距離を返す。

        coarse=True では上側64bitだけで採点する。枠がずれているほど
        ビット数の多いハッシュは当たらなくなるため、枠を探す段階で使う。
        """
        card = img.crop(box)
        if card.size[0] < 40 or card.size[1] < 40:
            return None
        hex_hash = artwork_phash_hex(card, coarse=coarse)
        ranked = self.catalog.rank(hex_hash, top=2, coarse=coarse,
                                   center=lambda: center_phash_hex(card))
        if not ranked:
            return None
        diff, cid = ranked[0]
        margin = (ranked[1][0] - diff) if len(ranked) > 1 else 99
        return {"box": box, "origin": origin, "hash": hex_hash, "distance": diff,
                "margin": margin, "card_id": cid, "card": card, "coarse": coarse}

    @staticmethod
    def _rank_key(found):
        # 距離が同じなら、検証済みの枠（＝マスター画像と同じ画角）を優先する。
        # 推定枠は少しずれた切り出しなので、たまたま別のカードに近づくことがある。
        return (found["distance"],
                1 if found.get("origin") == ESTIMATED else 0,
                -found["margin"])

    @classmethod
    def _better(cls, a, b):
        if b is None:
            return a
        if a is None:
            return b
        return a if cls._rank_key(a) <= cls._rank_key(b) else b

    def identify(self, img):
        """候補の枠を順に試し、最もよく一致したものを返す。見つからなければ None。"""
        best, detected_box = None, None
        for origin, box in card_boxes(img, self.options.resolution):
            if origin == ESTIMATED:
                detected_box = box
            best = self._better(self._evaluate(img, box, origin), best)
            if best and self.is_confirmed(best):
                return best
        if best is None or self.options.resolution != "auto":
            return best

        # ここまでで決まらないのは、ウィンドウモードなど枠がずれている場合。
        # 検出した枠を中心に、粗く→細かく動かして最も一致する位置を探す。
        # この探索は上側64bitだけで採点する（128bit全体だと枠が数pxずれただけで
        # 当たらなくなり、正しい位置を見つけられないため）。
        center = detected_box or best["box"]
        found = None
        for shifts, scales in ((COARSE_SHIFTS, COARSE_SCALES), (FINE_SHIFTS, FINE_SCALES)):
            round_best = None
            for box in nudged_boxes(center, img.size, shifts, scales):
                cand = self._evaluate(img, box, ESTIMATED, coarse=True)
                round_best = self._better(cand, round_best)
                if cand and cand["distance"] <= 6 and cand["margin"] >= 4:
                    break                  # ここまで近ければ位置は決まり
            found = self._better(round_best, found)
            if round_best:
                center = round_best["box"]            # 細かい探索は良かった位置を中心に
                if round_best["distance"] <= 6 and round_best["margin"] >= 4:
                    break

        # 位置が決まったら、あらためて128bit全体で評価し直す
        if found is not None:
            best = self._better(self._evaluate(img, found["box"], ESTIMATED), best)
        return best

    @staticmethod
    def is_confirmed(found):
        """このカードで確定してよいか。

        推定枠（検出・追い込み探索）は画角がマスター画像とわずかにずれるので、
        同じ基準で確定させると取り違えが起きる。より近い一致を要求する。
        """
        diff, margin = found["distance"], found["margin"]
        if found.get("origin") == ESTIMATED:
            return diff <= ESTIMATED_DIFF and margin >= ESTIMATED_MARGIN
        return ((diff <= CONFIRM_DIFF and margin >= CONFIRM_MARGIN)
                or (diff <= CONFIRM_DIFF_WIDE and margin >= CONFIRM_MARGIN_WIDE))

    # ---------- 保存 ----------
    def _save(self, card, target_dir, name, hex_hash):
        os.makedirs(target_dir, exist_ok=True)
        exts = _EXTS_BY_FORMAT.get(self.options.save_format, (".webp",))
        base = safe_filename(name)
        with self._lock:
            stem = os.path.join(target_dir, base)
            index = 1
            while True:
                # both のときは .webp と .png のどちらかが使われていれば別名にする
                taken = [stem + ext for ext in exts
                         if stem + ext in self._saved or os.path.exists(stem + ext)]
                if not taken:
                    break
                if self.options.skip_duplicate:
                    for path in taken:
                        other = self._saved.get(path)
                        if other is None:
                            other = self._artwork_hash_of(path) or ""
                            self._saved[path] = other
                        if other and hamming(other, hex_hash) <= DUPLICATE_DIFF:
                            return None, SKIPPED_NOTE
                stem = os.path.join(target_dir, f"{base}_alt{index}")
                index += 1
            for ext in exts:
                self._saved[stem + ext] = hex_hash

        image = rounded_alpha(card) if self.options.rounded else card.convert("RGB")
        for ext in exts:
            self._encode(image, stem + ext, ext)
        return stem + exts[0], ""

    def _encode(self, image, path, ext):
        if ext == ".webp":
            image.save(path, "WEBP", quality=self.options.quality, method=4)
        else:
            image.save(path, "PNG", compress_level=6)

    def _artwork_hash_of(self, path):
        try:
            with Image.open(path) as im:
                return artwork_phash_hex(im)
        except Exception:
            return None

    # ---------- 1ファイルの処理 ----------
    def process_file(self, path):
        try:
            with Image.open(path) as raw:
                img = raw.convert("RGB")
            found = self.identify(img)
            if not found:
                return Result(path, NOT_CARD, note="照合できる画像がありません")

            diff, margin = found["distance"], found["margin"]
            cid = found["card_id"]
            name = self.catalog.display_name(cid)

            if self.is_confirmed(found):
                status, target = CONFIRMED, self.options.output_dir
            elif diff <= REVIEW_DIFF:
                status = REVIEW
                target = os.path.join(self.options.output_dir, self.options.review_subdir)
                if not self.options.save_review:
                    return Result(path, REVIEW, cid, name, diff, margin,
                                  note="要確認のため保存しませんでした")
            else:
                # 知らないカード（＝マスター画像が無い新イラスト）の可能性もあるので、
                # 黙って捨てずに not_card/ へ元のファイル名で残す
                if not self.options.save_not_card:
                    return Result(path, NOT_CARD, distance=diff,
                                  note="カードの拡大画面ではないようです")
                stem = os.path.splitext(os.path.basename(path))[0]
                saved, note = self._save(
                    found["card"], os.path.join(self.options.output_dir,
                                                self.options.not_card_subdir),
                    stem, found["hash"])
                return Result(path, NOT_CARD, distance=diff, margin=margin,
                              saved_path=saved or "",
                              note=note or "カードの拡大画面ではないようです")

            saved, note = self._save(found["card"], target, name, found["hash"])
            return Result(path, status, cid, name, diff, margin, saved or "", note)
        except Exception as exc:
            return Result(path, ERROR, note=f"{type(exc).__name__}: {exc}")

    # ---------- まとめて処理 ----------
    def process(self, paths, on_result=None, should_stop=None):
        files = collect_images(paths, self.options.recursive, exclude=self.options.output_dir)
        return self.process_files(files, on_result, should_stop)

    def process_files(self, files, on_result=None, should_stop=None):
        """collect_images() で集めた画像ファイルを処理する。"""
        summary = Summary()
        started = time.time()
        workers = self.options.workers or min(8, max(2, (os.cpu_count() or 4)))

        def run(path):
            if should_stop and should_stop():
                return None
            return self.process_file(path)

        with ThreadPoolExecutor(max_workers=workers) as pool:
            for result in pool.map(run, files):
                if result is None:
                    continue
                summary.results.append(result)
                if result.status == CONFIRMED:
                    summary.confirmed += 1
                elif result.status == REVIEW:
                    summary.review += 1
                elif result.status == NOT_CARD:
                    summary.not_card += 1
                else:
                    summary.error += 1
                if result.note == SKIPPED_NOTE:
                    summary.skipped += 1
                if on_result:
                    on_result(result)
        summary.seconds = time.time() - started
        if self.options.write_csv and summary.results:
            self._write_csv(summary)
        return summary

    def _write_csv(self, summary):
        path = os.path.join(self.options.output_dir, "log.csv")
        try:
            os.makedirs(self.options.output_dir, exist_ok=True)
            new = not os.path.exists(path)
            with open(path, "a", encoding="utf-8-sig", newline="") as f:
                writer = csv.writer(f)
                if new:
                    writer.writerow(["日時", "状態", "カード名", "カードID", "距離",
                                     "2位との差", "出力ファイル", "備考", "入力ファイル"])
                now = time.strftime("%Y-%m-%d %H:%M:%S")
                for r in summary.results:
                    writer.writerow([_csv_cell(v) for v in (
                        now, r.status, r.name, r.card_id, r.distance,
                        r.margin, r.saved_path, r.note, r.source)])
        except OSError:
            pass          # ログが書けなくても処理結果は失わない
