# -*- coding: utf-8 -*-
"""カードデータの取得。

アプリにはカード名もマスター画像も同梱しない方針なので、
必要なデータは遊戯王公式データベース (db.yugioh-card.com) から取得する。

  * fetch_card_names() : カード名・ルビの一覧（数十秒）
  * update_new_sets()  : 新しいカードセットの分だけハッシュを取得・更新

標準ライブラリだけで動かす（requests を使わない）ことで、
配布物の依存を Pillow / numpy だけに保っている。
"""
from __future__ import annotations

import html
import io
import re
import time
import urllib.error
import urllib.parse
import urllib.request
import zlib
from http.cookiejar import CookieJar

BASE = "https://www.db.yugioh-card.com/yugiohdb/"
SEARCH = BASE + "card_search.action"
UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36")
PAGE_SIZE = 2000          # 公式DBが受け付けるページサイズ（中途半端な値だと10件に落ちる）
REQUEST_INTERVAL = 0.4      # 公式サイトに負荷をかけないための待ち時間（秒）
ALLOWED_HOST = "www.db.yugioh-card.com"
MAX_RESPONSE = 32 * 1024 * 1024     # 応答1件の上限（展開後）。想定外に巨大な応答でメモリを使い切らないため
MAX_IMAGE_PIXELS = 4000 * 4000      # カード画像として受け付ける最大画素数

_ROW = re.compile(r'class="t_row')
_CID = re.compile(r'class="cid"\s+value="(\d+)"')
_RUBY = re.compile(r'<span class="card_ruby">(.*?)</span>', re.S)
_NAME = re.compile(r'<span class="card_name">(.*?)</span>', re.S)
_IMG_SRC = re.compile(r'(get_image\.action\?[^"\'\s>]+)')
_CIID = re.compile(r'ciid=(\d+)')
_PID = re.compile(r'pid=(\d+)')            # 収録シリーズ（パック）のID


class Cancelled(Exception):
    """利用者が中断した。"""


class _SameSiteRedirect(urllib.request.HTTPRedirectHandler):
    """リダイレクト先が公式DBの https でなければ辿らない（想定外のサイトへ誘導されないため）。"""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        parts = urllib.parse.urlsplit(newurl)
        if parts.scheme != "https" or parts.hostname != ALLOWED_HOST:
            raise urllib.error.URLError(f"想定外のリダイレクト先です: {parts.hostname}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _opener():
    return urllib.request.build_opener(urllib.request.HTTPCookieProcessor(CookieJar()),
                                       _SameSiteRedirect())


def _read_limited(res):
    """応答本体を MAX_RESPONSE バイトまでに制限して読む（gzip の展開後も同じ上限）。"""
    raw = res.read(MAX_RESPONSE + 1)
    if len(raw) > MAX_RESPONSE:
        raise ValueError("応答が大きすぎます")
    if res.headers.get("Content-Encoding") == "gzip":
        inflater = zlib.decompressobj(16 + zlib.MAX_WBITS)
        raw = inflater.decompress(raw, MAX_RESPONSE + 1)
        if len(raw) > MAX_RESPONSE or inflater.unconsumed_tail:
            raise ValueError("応答が大きすぎます")
    return raw


def _get(op, url, referer=None, retries=3):
    req = urllib.request.Request(url, headers={
        "User-Agent": UA,
        "Accept-Language": "ja,en;q=0.8",
        "Accept-Encoding": "gzip",
        **({"Referer": referer} if referer else {}),
    })
    last = None
    for attempt in range(retries):
        try:
            with op.open(req, timeout=60) as res:
                return _read_limited(res)
        except Exception as exc:          # 一時的な失敗は少し待って再試行
            last = exc
            if attempt + 1 < retries:
                time.sleep(1.5 * (attempt + 1))
    raise last


def _text(raw):
    return raw.decode("utf-8", "replace")


def _clean(s):
    return html.unescape(re.sub(r"\s+", " ", s)).strip()


def _parse_rows(page_html):
    out = []
    for block in _ROW.split(page_html)[1:]:
        cid = _CID.search(block)
        name = _NAME.search(block)
        if not cid or not name:
            continue
        ruby = _RUBY.search(block)
        out.append({
            "id": cid.group(1),
            "name": _clean(name.group(1)),
            "ruby": _clean(ruby.group(1)) if ruby else "",
        })
    return out


def fetch_card_names(progress=None, should_stop=None):
    """公式DBからカード名・ルビの一覧を取得して {cid: {"n":名前, "r":ルビ}} を返す。"""
    op = _opener()
    cards, page = {}, 1
    while True:
        if should_stop and should_stop():
            raise Cancelled()
        params = urllib.parse.urlencode({
            "ope": "1", "sess": "1", "rp": str(PAGE_SIZE), "page": str(page),
            "sort": "1", "keyword": "", "stype": "1", "othercon": "2",
            "request_locale": "ja",
        })
        rows = _parse_rows(_text(_get(op, f"{SEARCH}?{params}")))
        for r in rows:
            cards[r["id"]] = {"n": r["name"], "r": r["ruby"]}
        if progress:
            progress(f"カード名を取得中... {len(cards)} 件")
        if len(rows) < PAGE_SIZE:
            break
        page += 1
        time.sleep(REQUEST_INTERVAL)
        if page > 60:                      # 想定外のページ数で無限ループしない
            break
    if not cards:
        # メンテナンス中やページの作りが変わった場合。取得済みのカード名を空で上書きしない
        raise RuntimeError("カード名を1件も取得できませんでした。"
                           "公式データベースがメンテナンス中の可能性があります。")
    return cards


def fetch_card_page(cid, op=None):
    """1枚のカードページから (公式画像のリスト, 収録パックIDのリスト) を返す。

    画像URLには毎回変わる enc トークンが必要なので、必ずカードページを経由する。
    イラスト違いは ciid 違いの画像として並んでいるので、まとめて取る。
    """
    from PIL import Image
    op = op or _opener()
    page = f"{SEARCH}?ope=2&cid={cid}&request_locale=ja"
    body = _text(_get(op, page))
    seen, images = set(), []
    for src in _IMG_SRC.findall(body):
        src = html.unescape(src)
        if "type=1" not in src:            # type=1 がカード全体の画像
            continue
        ciid = _CIID.search(src)
        key = ciid.group(1) if ciid else src
        if key in seen:
            continue
        seen.add(key)
        raw = _get(op, BASE + src, referer=page)
        try:
            im = Image.open(io.BytesIO(raw))
            if im.width * im.height > MAX_IMAGE_PIXELS:
                continue
            images.append(im.convert("RGB"))
        except Exception:
            continue
    packs = sorted(set(_PID.findall(body)))
    return images, packs


def fetch_pack_cards(pid, op=None):
    """1つの収録シリーズ（パック）に入っているカードIDの一覧を返す。

    rp=99999 は公式サイトのパック一覧のリンクと同じ値（全件が1ページで返る）。
    """
    op = op or _opener()
    body = _text(_get(op, f"{SEARCH}?ope=1&sess=1&pid={pid}&rp=99999&request_locale=ja"))
    return [row["id"] for row in _parse_rows(body)]


def update_hashes(hash_db, cids, progress=None, should_stop=None, label="カード画像を取得中"):
    """cids のカード画像を取得して hash_db（{cid: [hex, ...]}）に追加する。

    既にあるハッシュは消さずに、新しいイラストの分だけ足す
    （古いイラストのスクショも判定できるようにするため）。
    戻り値は (画像を取得できたカード数, 新しく増えたハッシュ数,
              失敗した cid のリスト, 見つかった収録パックIDの集合)。
    """
    from .catalog import SPECIAL_CIDS
    from .phash import artwork_phash_hex
    op = _opener()
    done, new_hashes, failed, packs = 0, 0, [], set()
    total = len(cids)
    for i, cid in enumerate(cids, 1):
        if should_stop and should_stop():
            raise Cancelled()
        try:
            images, card_packs = fetch_card_page(cid, op)
            packs.update(card_packs)
            special = cid in SPECIAL_CIDS     # 中央の領域も持たせるカード
            hexes = [artwork_phash_hex(im, with_center=special) for im in images]
            if hexes:
                cur = hash_db.setdefault(cid, [])
                for h in hexes:
                    if h not in cur:
                        cur.append(h)
                        new_hashes += 1
                done += 1
            else:
                failed.append(cid)
        except Cancelled:
            raise
        except Exception:
            failed.append(cid)
        if progress:
            progress(f"{label}... {i}/{total}")
        time.sleep(REQUEST_INTERVAL)
    return done, new_hashes, failed, packs


def fetch_pack_list(op=None):
    """収録シリーズ（パック）の一覧を新しい順に返す。1リクエストで全件取れる。"""
    op = op or _opener()
    body = _text(_get(op, BASE + "card_list.action?request_locale=ja"))
    packs, seen = [], set()
    for block in re.split(r'<div class="t_row', body)[1:]:
        pid = _PID.search(block)
        if not pid or pid.group(1) in seen:
            continue
        seen.add(pid.group(1))
        date = re.search(r'class="time">\s*([\d/]+)', block)
        name = re.search(r"<p>(.*?)</p>", block, re.S)
        packs.append({
            "pid": pid.group(1),
            "date": date.group(1) if date else "",
            "name": _clean(name.group(1)) if name else "",
        })
    return packs


def update_new_sets(catalog, cards, progress=None, should_stop=None, max_cards=4000):
    """新しいカードセットの分だけデータを更新する。

    イラストが新しくなるのは新しいカードセットが出たときなので、
      1. 収録シリーズの一覧（1リクエスト）を見て、前回以降に増えたシリーズを調べる
      2. ハッシュを持っていないカード（＝新カード）の画像を取得する
      3. 新しいシリーズに入っているカードは、既存カードでもイラストを取り直す
         （新規イラストでの再録に追従するため）
    という順で、必要なぶんだけ公式DBに問い合わせる。

    初回は同梱ハッシュが現時点の全シリーズをカバーしている前提で、
    シリーズ一覧を「取り込み済み」として記録するだけにする
    （全14,000枚を取り直すと数時間かかるため）。

    戻り値は結果のサマリ（辞書）。
    """
    result = {"new_cards": 0, "new_hashes": 0, "packs": 0, "refreshed": 0,
              "failed": [], "skipped_packs": [], "seeded": 0, "pack_names": [],
              "pack_list_failed": False}
    op = _opener()

    if progress:
        progress("収録シリーズの一覧を確認中...")
    try:
        all_packs = fetch_pack_list(op)
    except Exception:
        all_packs = []
    if not all_packs:
        # 一覧が取れないと「どのシリーズが新しいか」が分からない。
        # 全シリーズを新しいとみなして大量に取り直さないよう、新カードの取得だけにする。
        result["pack_list_failed"] = True
        new_packs = []
    elif not catalog.known_packs:
        catalog.known_packs = {p["pid"] for p in all_packs}
        result["seeded"] = len(all_packs)
        new_packs = []
    else:
        new_packs = [p for p in all_packs if p["pid"] not in catalog.known_packs]

    # --- 新しいカード（ハッシュを1件も持っていないもの） ---
    missing = [cid for cid in cards if cid not in catalog.hashes]
    if missing:
        if progress:
            progress(f"ハッシュの無いカード {len(missing)} 件を取得します...")
        done, new_hashes, failed, card_packs = update_hashes(
            catalog.hashes, missing, progress, should_stop, "新しいカードを取得中")
        result.update(new_cards=done, new_hashes=new_hashes, failed=failed)
        # 一覧に出ていないシリーズ（先行販売など）はカード側から拾う
        if all_packs:
            listed = {p["pid"] for p in new_packs}
            for pid in sorted(card_packs - catalog.known_packs - listed):
                new_packs.append({"pid": pid, "date": "", "name": ""})

    # --- 新しいシリーズに入っているカードのイラストを取り直す ---
    missing = set(missing)
    todo, seen_packs = [], set()
    for pack in new_packs:
        if should_stop and should_stop():
            raise Cancelled()
        try:
            pack_cids = fetch_pack_cards(pack["pid"], op)
        except Exception:
            continue
        time.sleep(REQUEST_INTERVAL)
        seen_packs.add(pack["pid"])
        if pack.get("name"):
            result["pack_names"].append(pack["name"])
        todo += [cid for cid in pack_cids if cid not in missing]
    todo = list(dict.fromkeys(todo))
    result["packs"] = len(seen_packs)

    if len(todo) > max_cards:
        # 大規模な再録セットなどで件数が膨らんだ場合は打ち切る（時間がかかりすぎるため）
        result["skipped_packs"] = sorted(seen_packs)
    elif todo:
        done, new_hashes, failed, _ = update_hashes(
            catalog.hashes, todo, progress, should_stop, "再録カードのイラストを確認中")
        result["refreshed"] = done
        result["new_hashes"] += new_hashes
        result["failed"] += failed
        catalog.known_packs |= seen_packs
    else:
        catalog.known_packs |= seen_packs
    return result
