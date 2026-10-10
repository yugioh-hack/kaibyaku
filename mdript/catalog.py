# -*- coding: utf-8 -*-
"""照合用データの置き場所と読み書き。

  * 同梱データ    : アプリに同梱するハッシュDB（カードIDとハッシュ値だけ）
  * ユーザーデータ : %APPDATA%/mdript/ 以下。
                    追加ハッシュ・取得したカード名・設定を置く。

同梱データは書き換えず、更新分はユーザーデータ側に重ねる。
"""
from __future__ import annotations

import json
import os
import sys
import time

import numpy as np

from .phash import HASH_HEX, HEX_PER_REGION

APP_NAME = "mdript"
BUNDLED_HASHES = "card_hashes.json"
USER_HASHES = "card_hashes_user.json"
USER_NAMES = "card_names.json"
CONFIG_FILE = "config.json"
HASH_FORMAT = 2          # 1=64bit 1領域 / 2=128bit 2領域

# 上下の領域だけでは見分けがつかないカード（ウィジャ盤・死のメッセージ「E」「A」「T」「H」）。
# この5枚だけは、ハッシュの末尾に中央の領域（64bit）を足した 48 文字で持つ。
SPECIAL_CIDS = frozenset({"5224", "5271", "5272", "5273", "5274"})
HASH_HEX_CENTER = HASH_HEX + HEX_PER_REGION


def app_dir() -> str:
    """アプリ本体（exe 化された場合はその展開先）のディレクトリ。"""
    if getattr(sys, "frozen", False):
        return getattr(sys, "_MEIPASS", os.path.dirname(sys.executable))
    return os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def bundled_data_dir() -> str:
    return os.path.join(app_dir(), "data")


def user_data_dir() -> str:
    base = os.environ.get("APPDATA") or os.path.expanduser("~/.config")
    path = os.path.join(base, APP_NAME)
    os.makedirs(path, exist_ok=True)
    return path


def _read_json(path):
    """JSON の辞書を読む。無い・壊れている・辞書でない場合は None。"""
    try:
        with open(path, encoding="utf-8") as f:
            data = json.load(f)
    except (OSError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def _hex_list(value):
    """ハッシュの一覧として使える文字列だけを取り出す（壊れた・書き換えられたファイル対策）。"""
    if not isinstance(value, list):
        return []
    return [h for h in value if isinstance(h, str) and len(h) in (HASH_HEX, HASH_HEX_CENTER)
            and all(c in "0123456789abcdef" for c in h)]


def _write_json(path, data):
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False, separators=(",", ":"))
    os.replace(tmp, path)


class Catalog:
    """カードIDごとのハッシュ一覧と、カードID→カード名の対応。"""

    def __init__(self):
        self.hashes: dict[str, list[str]] = {}
        self.names: dict[str, dict] = {}
        self.names_fetched = ""
        self.known_packs: set[str] = set()   # イラストを取り込み済みの収録シリーズ
        self.bundled_count = 0
        self.user_count = 0
        self._ids = np.zeros(0, dtype=object)
        self._upper = np.zeros(0, dtype=np.uint64)   # 上側の領域（粗い照合用）
        self._lower = np.zeros(0, dtype=np.uint64)   # 下側の領域
        self._center = {}                            # 行番号 -> 中央の領域（特別扱いのカードだけ）
        self.load()

    # ---------- 読み込み ----------
    def load(self):
        self.hashes = {}
        bundled = _read_json(os.path.join(bundled_data_dir(), BUNDLED_HASHES)) or {}
        for cid, hexes in (bundled.get("hashes") or {}).items():
            self.hashes[cid] = _hex_list(hexes)
        self.bundled_count = len(self.hashes)

        user = _read_json(os.path.join(user_data_dir(), USER_HASHES)) or {}
        added = 0
        if (user.get("meta") or {}).get("format") == HASH_FORMAT:
            packs = (user.get("meta") or {}).get("packs")
            self.known_packs = {p for p in packs if isinstance(p, str)} if isinstance(packs, list) else set()
            for cid, hexes in (user.get("hashes") or {}).items():
                cur = self.hashes.setdefault(cid, [])
                for h in _hex_list(hexes):
                    if h not in cur:
                        cur.append(h)
                added += 1
        else:
            # ハッシュの作り方が変わった場合は、古い追加分を捨てて取り直してもらう
            self.known_packs = set()
        self.user_count = added

        names = _read_json(os.path.join(user_data_dir(), USER_NAMES)) or {}
        cards = names.get("cards")
        self.names = {k: v for k, v in cards.items() if isinstance(v, dict)}             if isinstance(cards, dict) else {}
        self.names_fetched = (names.get("meta") or {}).get("fetched", "")
        self._reindex()

    def _reindex(self):
        ids, upper, lower = [], [], []
        self._center = {}
        for cid, hexes in self.hashes.items():
            for h in hexes:
                try:
                    hi = int(h[:HEX_PER_REGION], 16)
                    lo = int(h[HEX_PER_REGION:HASH_HEX], 16)
                    mid = int(h[HASH_HEX:], 16) if len(h) == HASH_HEX_CENTER else None
                except ValueError:
                    continue
                if mid is not None and cid in SPECIAL_CIDS:
                    self._center[len(ids)] = mid
                ids.append(cid)
                upper.append(hi)
                lower.append(lo)
        self._ids = np.array(ids, dtype=object)
        self._upper = np.array(upper, dtype=np.uint64)
        self._lower = np.array(lower, dtype=np.uint64)

    def add_hash(self, cid, hex_hash, min_distance=1):
        """カードに照合用のハッシュを1つ足す。足したら True。

        マスター画像と MD の描画でイラストの切り方が違うカード向け。
        すでに近いハッシュ（距離 min_distance 未満）があるときは足さない。
        保存は save_user_hashes() で行う。
        """
        cid = str(cid)
        if cid not in self.hashes or len(hex_hash) != HASH_HEX:
            return False
        if any(len(h) >= HASH_HEX and bin(int(h[:HASH_HEX], 16) ^ int(hex_hash, 16)).count("1") < min_distance
               for h in self.hashes[cid]):
            return False
        self.hashes[cid].append(hex_hash)
        self._reindex()
        return True

    # ---------- 保存 ----------
    def save_user_hashes(self):
        bundled = _read_json(os.path.join(bundled_data_dir(), BUNDLED_HASHES)) or {}
        base = bundled.get("hashes") or {}
        extra = {}
        for cid, hexes in self.hashes.items():
            diff = [h for h in hexes if h not in base.get(cid, [])]
            if diff:
                extra[cid] = diff
        _write_json(os.path.join(user_data_dir(), USER_HASHES),
                    {"meta": {"format": HASH_FORMAT, "cards": len(extra),
                              "packs": sorted(self.known_packs)},
                     "hashes": extra})

    def save_names(self, cards, fetched=""):
        meta = {"format": 1, "fetched": fetched or time.strftime("%Y-%m-%d %H:%M"),
                "count": len(cards)}
        _write_json(os.path.join(user_data_dir(), USER_NAMES), {"meta": meta, "cards": cards})
        self.names = cards
        self.names_fetched = meta["fetched"]

    # ---------- 照合 ----------
    @property
    def card_count(self):
        return len(self.hashes)

    @property
    def hash_count(self):
        return int(self._upper.size)

    def has_names(self):
        return bool(self.names)

    def display_name(self, cid):
        info = self.names.get(str(cid))
        if info and isinstance(info.get("n"), str) and info["n"]:
            return info["n"]
        return f"cid_{cid}"

    def rank(self, hex_hash, top=3, coarse=False, center=None):
        """距離の近い順に [(距離, カードID), ...] を返す（同じカードは1件にまとめる）。

        coarse=True のときは上側の領域（64bit）だけで採点する。
        ビット数が多いほど切り出し位置のズレに弱いので、
        枠を探している間はこちらを使い、確定の判断は128bit全体で行う。

        center は中央の領域のハッシュ（16文字）を返す関数。先頭がウィジャ盤・死のメッセージ
        のどれかだったときだけ呼ばれ、この5枚の間の順位を中央の領域も足して決め直す。
        """
        if self._upper.size == 0:
            return []
        distances = _popcount(self._upper ^ np.uint64(int(hex_hash[:HEX_PER_REGION], 16)))
        if not coarse:
            distances = distances + _popcount(
                self._lower ^ np.uint64(int(hex_hash[HEX_PER_REGION:], 16)))
        limit = min(distances.size, 256)
        idx = np.argpartition(distances, limit - 1)[:limit]
        idx = idx[np.argsort(distances[idx], kind="stable")]
        if center is not None and not coarse and self._ids[idx[0]] in SPECIAL_CIDS:
            out = self._rank_special(idx, distances, center(), top)
            if out:
                return out
        out, seen = [], set()
        for i in idx:
            cid = self._ids[i]
            if cid in seen:
                continue
            seen.add(cid)
            out.append((int(distances[i]), cid))
            if len(out) >= top:
                break
        return out

    def _rank_special(self, idx, distances, center_hex, top):
        """ウィジャ盤・死のメッセージの間の順位を、中央の領域も足して決め直す。

        「この5枚のどれか」かどうかは従来どおり上下128bitの距離で見る（確定の基準を変えない）。
        どれになるかだけを192bitの距離で決め、2位との差（margin）も192bitの差にする。
        """
        c = np.uint64(int(center_hex, 16))
        family = int(distances[idx[0]])              # 5枚のどれかとしての距離
        totals, others = {}, []
        seen = set()
        for i in idx:
            cid = self._ids[i]
            if cid in SPECIAL_CIDS:
                if int(i) in self._center:
                    total = int(distances[i]) + int(_popcount(c ^ np.uint64(self._center[int(i)])))
                else:
                    # 中央を持たない古い追加ハッシュ。最悪値の側に寄せて比べる
                    total = int(distances[i]) + 32
                if cid not in totals or total < totals[cid]:
                    totals[cid] = total
            elif cid not in seen:
                seen.add(cid)
                others.append((int(distances[i]), cid))
        ordered = sorted(totals.items(), key=lambda kv: kv[1])
        best_total = ordered[0][1]
        merged = [(family + (t - best_total), cid) for cid, t in ordered] + others
        merged.sort(key=lambda x: x[0])
        return merged[:top]


if hasattr(np, "bitwise_count"):
    def _popcount(v):
        return np.bitwise_count(v)
else:                                    # numpy < 2.0 向けの手計算
    def _popcount(v):
        v = v - ((v >> np.uint64(1)) & np.uint64(0x5555555555555555))
        v = (v & np.uint64(0x3333333333333333)) + ((v >> np.uint64(2)) & np.uint64(0x3333333333333333))
        v = (v + (v >> np.uint64(4))) & np.uint64(0x0F0F0F0F0F0F0F0F)
        return (v * np.uint64(0x0101010101010101)) >> np.uint64(56)


# ---------- 設定 ----------
def load_config():
    return _read_json(os.path.join(user_data_dir(), CONFIG_FILE)) or {}


def save_config(cfg):
    _write_json(os.path.join(user_data_dir(), CONFIG_FILE), cfg)
