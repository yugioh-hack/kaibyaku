# -*- coding: utf-8 -*-
"""マスター画像が無くても動く基本的なテスト。

    python -m unittest discover -s tests
"""
import io
import os
import sys
import tempfile
import unittest

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from mdript.layout import (CARD_ASPECT, REF_CARD_BOX, detect_card_box,  # noqa: E402
                               nudged_boxes, predict_card_box)
from mdript.phash import (ART_BOXES, HASH_HEX, HEX_PER_REGION,  # noqa: E402
                              artwork_phash_hex, crop_region, hamming, phash_hex)
from mdript.pipeline import collect_images, rounded_alpha, safe_filename  # noqa: E402


def fake_card(width=894, height=1302, seed=0):
    """カードらしい見た目（明るい枠＋中にイラスト）の画像を作る。"""
    rng = np.random.default_rng(seed)
    card = np.full((height, width, 3), 210, dtype=np.uint8)          # 枠は明るい
    art = rng.integers(0, 255, (int(height * 0.45), int(width * 0.8), 3), dtype=np.uint8)
    art = np.asarray(Image.fromarray(art).resize((art.shape[1], art.shape[0]), Image.NEAREST))
    top, left = int(height * 0.18), int(width * 0.1)
    card[top:top + art.shape[0], left:left + art.shape[1]] = art
    return Image.fromarray(card)


def fake_screenshot(size=(2560, 1440), seed=0):
    """カードを画面中央（基準枠の位置）に置いた、暗い背景のスクショを作る。"""
    box = predict_card_box(size)
    screen = Image.new("RGB", size, (12, 12, 16))
    card = fake_card(seed=seed).resize((box[2] - box[0], box[3] - box[1]), Image.LANCZOS)
    screen.paste(card, (box[0], box[1]))
    return screen


class TestLayout(unittest.TestCase):
    def test_wqhd_matches_reference(self):
        self.assertEqual(predict_card_box((2560, 1440)), REF_CARD_BOX)

    def test_scales_with_height(self):
        for size, scale in (((1920, 1080), 0.75), ((3840, 2160), 1.5)):
            box = predict_card_box(size)
            self.assertAlmostEqual((box[2] - box[0]) / (REF_CARD_BOX[2] - REF_CARD_BOX[0]),
                                   scale, places=2)
            self.assertAlmostEqual((box[0] + box[2]) / 2, size[0] / 2, delta=1)

    def test_aspect_is_preserved(self):
        for size in ((1920, 1080), (2560, 1440), (3840, 2160), (2560, 1080), (1280, 720)):
            box = predict_card_box(size)
            self.assertAlmostEqual((box[2] - box[0]) / (box[3] - box[1]),
                                   CARD_ASPECT, delta=0.01)

    def test_detection_finds_the_card(self):
        box = detect_card_box(fake_screenshot())
        self.assertIsNotNone(box)
        for got, expected in zip(box, REF_CARD_BOX):
            self.assertAlmostEqual(got, expected, delta=12)

    def test_detection_ignores_tiny_images(self):
        self.assertIsNone(detect_card_box(Image.new("RGB", (1, 300))))

    def test_nudged_boxes_stay_inside(self):
        for box in nudged_boxes((100, 100, 400, 537), (1920, 1080)):
            self.assertGreaterEqual(box[0], 0)
            self.assertLessEqual(box[2], 1920)


class TestPhash(unittest.TestCase):
    def test_is_deterministic(self):
        card = fake_card(seed=1)
        self.assertEqual(artwork_phash_hex(card), artwork_phash_hex(card))

    def test_survives_resize_and_jpeg(self):
        card = fake_card(seed=2)
        base = artwork_phash_hex(card)
        small = card.resize((447, 651), Image.LANCZOS)
        self.assertLessEqual(hamming(base, artwork_phash_hex(small)), 16)
        buf = io.BytesIO()
        card.save(buf, "JPEG", quality=50)
        buf.seek(0)
        self.assertLessEqual(hamming(base, artwork_phash_hex(Image.open(buf))), 16)

    def test_different_images_are_far_apart(self):
        a = artwork_phash_hex(fake_card(seed=3))
        b = artwork_phash_hex(fake_card(seed=4))
        self.assertGreater(hamming(a, b), 24)

    def test_hex_length(self):
        self.assertEqual(len(phash_hex(fake_card())), HEX_PER_REGION)
        self.assertEqual(len(artwork_phash_hex(fake_card())), HASH_HEX)

    def test_two_regions_are_independent(self):
        """上と下は別の場所を見ているので、同じビット列にはならない。"""
        self.assertEqual(len(ART_BOXES), 2)
        h = artwork_phash_hex(fake_card(seed=5))
        self.assertNotEqual(h[:HEX_PER_REGION], h[HEX_PER_REGION:])

    def test_coarse_hash_is_the_upper_region(self):
        card = fake_card(seed=6)
        coarse = artwork_phash_hex(card, coarse=True)
        self.assertEqual(coarse, phash_hex(crop_region(card, ART_BOXES[0])))
        self.assertEqual(coarse, artwork_phash_hex(card)[:HEX_PER_REGION])

    def test_bits_are_packed_most_significant_first(self):
        """ハッシュの並びが変わると同梱DBが使えなくなるので、既知の値で固定しておく。"""
        # 32x32 なので縮小が入らず、Pillow のバージョンに左右されない
        px = np.fromfunction(lambda y, x: (x * x * 3 + y * 5 + x * y) % 256, (32, 32))
        self.assertEqual(phash_hex(Image.fromarray(px.astype(np.uint8))), "80000f3fa7bfc35d")


class TestPipelineHelpers(unittest.TestCase):
    def test_safe_filename(self):
        self.assertEqual(safe_filename("A/B:C"), "A／B：C")
        self.assertEqual(safe_filename(""), "unknown")
        self.assertEqual(safe_filename("   "), "unknown")
        self.assertEqual(safe_filename("A ."), "A")          # 末尾の空白とピリオド
        self.assertEqual(safe_filename(" . "), "unknown")
        self.assertEqual(safe_filename("con"), "_con")       # Windows の予約名
        self.assertEqual(safe_filename("NUL.txt"), "_NUL.txt")
        self.assertEqual(safe_filename("CONSOLE"), "CONSOLE")
        self.assertLessEqual(len(safe_filename("あ" * 500)), 150)

    def test_rounded_alpha_makes_corners_transparent(self):
        card = rounded_alpha(fake_card(width=200, height=291))
        self.assertEqual(card.mode, "RGBA")
        self.assertEqual(card.getpixel((0, 0))[3], 0)
        self.assertEqual(card.getpixel((100, 145))[3], 255)

    def test_collect_images(self):
        with tempfile.TemporaryDirectory() as tmp:
            sub = os.path.join(tmp, "sub")
            os.makedirs(sub)
            for path in (os.path.join(tmp, "a.png"), os.path.join(sub, "b.jpg")):
                Image.new("RGB", (8, 8)).save(path)
            open(os.path.join(tmp, "memo.txt"), "w").close()
            self.assertEqual(len(collect_images([tmp], recursive=True)), 2)
            self.assertEqual(len(collect_images([tmp], recursive=False)), 1)

    def test_broken_json_is_ignored(self):
        from mdript.catalog import _read_json
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "config.json")
            for text in ("[]", "{broken", '"text"'):
                with open(path, "w", encoding="utf-8") as f:
                    f.write(text)
                self.assertIsNone(_read_json(path), text)

    def test_cli_rejects_zero_resolution(self):
        import argparse
        from mdript.cli import _resolution
        self.assertEqual(_resolution("1920x1080"), (1920, 1080))
        for bad in ("0x0", "1920x0", "-1x1080"):
            with self.assertRaises(argparse.ArgumentTypeError, msg=bad):
                _resolution(bad)

    def test_collect_images_skips_the_output_folder(self):
        """スクショのフォルダの中にある出力先は、前回の出力なので読まない。"""
        with tempfile.TemporaryDirectory() as tmp:
            out = os.path.join(tmp, "out")
            os.makedirs(os.path.join(out, "review"))
            Image.new("RGB", (8, 8)).save(os.path.join(tmp, "shot.png"))
            Image.new("RGB", (8, 8)).save(os.path.join(out, "card.webp"))
            Image.new("RGB", (8, 8)).save(os.path.join(out, "review", "card.webp"))
            found = collect_images([tmp], recursive=True, exclude=out)
            self.assertEqual([os.path.basename(f) for f in found], ["shot.png"])

    def test_parse_drop_handles_braces_and_spaces(self):
        import tkinter
        try:
            tcl = tkinter.Tcl()
        except tkinter.TclError:
            self.skipTest("Tcl が使えない環境")
        from mdript.gui import parse_drop
        data = r"C:/a.png {C:/my shots/b.png} C:/card\{1\}.png"
        self.assertEqual(parse_drop(tcl, data),
                         ["C:/a.png", "C:/my shots/b.png", "C:/card{1}.png"])


class TestIdentify(unittest.TestCase):
    """作り物のハッシュDBで、切り抜き→照合→保存まで通す。"""

    def setUp(self):
        from mdript.catalog import Catalog
        self.catalog = Catalog.__new__(Catalog)
        self.catalog.names = {"1": {"n": "テストカード"}}
        self.catalog.hashes = {}
        for seed in range(6):
            self.catalog.hashes[str(seed + 1)] = [artwork_phash_hex(fake_card(seed=seed))]
        self.catalog._reindex()
        self.catalog.names_fetched = ""

    def test_identifies_and_saves(self):
        from mdript.pipeline import CONFIRMED, Cropper, Options
        with tempfile.TemporaryDirectory() as tmp:
            source = os.path.join(tmp, "shot.png")
            fake_screenshot(seed=0).save(source)
            cropper = Cropper(self.catalog,
                              Options(output_dir=os.path.join(tmp, "out"), write_csv=False))
            result = cropper.process_file(source)
            self.assertEqual(result.status, CONFIRMED)
            self.assertEqual(result.name, "テストカード")
            self.assertTrue(os.path.exists(result.saved_path))

    def test_both_format_does_not_overwrite_an_existing_png(self):
        from mdript.pipeline import Cropper, Options
        with tempfile.TemporaryDirectory() as tmp:
            existing = os.path.join(tmp, "テストカード.png")
            fake_card(seed=5).save(existing)                # 別のイラスト
            before = os.path.getmtime(existing), os.path.getsize(existing)
            cropper = Cropper(self.catalog, Options(output_dir=tmp, save_format="both",
                                                    write_csv=False))
            path, _ = cropper._save(fake_card(seed=0), tmp, "テストカード",
                                    artwork_phash_hex(fake_card(seed=0)))
            self.assertEqual(os.path.basename(path), "テストカード_alt1.webp")
            self.assertTrue(os.path.exists(os.path.join(tmp, "テストカード_alt1.png")))
            self.assertEqual((os.path.getmtime(existing), os.path.getsize(existing)), before)

    def test_skips_the_same_artwork(self):
        from mdript.pipeline import SKIPPED_NOTE, Cropper, Options
        with tempfile.TemporaryDirectory() as tmp:
            cropper = Cropper(self.catalog, Options(output_dir=tmp, write_csv=False))
            card = fake_card(seed=0)
            first, _ = cropper._save(card, tmp, "テストカード", artwork_phash_hex(card))
            again, note = Cropper(self.catalog, Options(output_dir=tmp, write_csv=False))._save(
                card, tmp, "テストカード", artwork_phash_hex(card))
            self.assertTrue(first)
            self.assertIsNone(again)
            self.assertEqual(note, SKIPPED_NOTE)

    def test_works_at_other_resolutions(self):
        from mdript.pipeline import Cropper, Options
        cropper = Cropper(self.catalog, Options(write_csv=False))
        for size in ((1920, 1080), (3840, 2160), (2560, 1080)):
            found = cropper.identify(fake_screenshot(size, seed=2))
            self.assertIsNotNone(found, size)
            self.assertEqual(found["card_id"], "3", size)

    def test_coarse_ranking_uses_only_the_upper_region(self):
        """下側だけ全ビット違うハッシュは、粗い照合では距離0になる。"""
        cid, base = "1", self.catalog.hashes["1"][0]
        flipped = base[:HEX_PER_REGION] + format(
            int(base[HEX_PER_REGION:], 16) ^ ((1 << 64) - 1), "016x")
        self.assertEqual(self.catalog.rank(flipped, top=1, coarse=True)[0], (0, cid))
        full = dict((c, d) for d, c in self.catalog.rank(flipped, top=6))
        self.assertEqual(full[cid], 64)          # 下側の64bitが効いている

    def test_special_cards_are_told_apart_by_the_center_region(self):
        """上下が同じでも、中央が違うウィジャ盤系は中央の距離で順位が決まる。通常のカードでは中央を見ない。"""
        base = artwork_phash_hex(fake_card(seed=7))
        center_a, center_b = "0" * 16, "f" * 16
        self.catalog.hashes["5271"] = [base + center_a]
        self.catalog.hashes["5272"] = [base + center_b]
        self.catalog._reindex()
        # 中央が B に近い（2bit違い）→ 5272、同じ上下なので 128bit の距離は 0 のまま
        ranked = dict((c, d) for d, c in self.catalog.rank(
            base, top=8, center=lambda: "fffffffffffffffc"))
        self.assertEqual(ranked["5272"], 0)
        self.assertEqual(ranked["5271"], 60)          # 5272 との差は 192bit の差
        self.assertEqual(min(ranked, key=ranked.get), "5272")
        # 先頭が通常のカードなら、中央のハッシュは計算されない
        normal = self.catalog.hashes["1"][0]
        self.catalog.rank(normal, top=2, center=lambda: self.fail("中央は不要"))

    def test_does_not_confirm_a_non_card_image(self):
        """カードではない画像を、確定として出力しないこと。"""
        from mdript.pipeline import CONFIRMED, Cropper, Options
        with tempfile.TemporaryDirectory() as tmp:
            source = os.path.join(tmp, "noise.png")
            rng = np.random.default_rng(99)
            Image.fromarray(rng.integers(0, 255, (1440, 2560, 3), dtype=np.uint8)).save(source)
            cropper = Cropper(self.catalog,
                              Options(output_dir=os.path.join(tmp, "out"), write_csv=False))
            self.assertNotEqual(cropper.process_file(source).status, CONFIRMED)


PACK_LIST_HTML = """
<div id="update_list" class="list"><div class="t_body">
<div class="t_row packc12 new_p"><div class="inside">
  <div class="sub"><div class="time">2026/09/18</div></div>
  <div class="main"><p>新しいパック</p>
  <input type="hidden" class="link_value"
   value="/yugiohdb/card_search.action?ope=1&amp;sess=1&amp;pid=1000000002000&amp;rp=99999"></div>
</div></div>
<div class="t_row packc12"><div class="inside">
  <div class="sub"><div class="time">2020/01/01</div></div>
  <div class="main"><p>昔のパック</p>
  <input type="hidden" class="link_value"
   value="/yugiohdb/card_search.action?ope=1&amp;sess=1&amp;pid=1000000001000&amp;rp=99999"></div>
</div></div>
</div></div>
"""


class FakeCatalog:
    """通信テスト用の最小限のカタログ。"""

    def __init__(self, hashes, packs=()):
        self.hashes = dict(hashes)
        self.known_packs = set(packs)
        self.names = {cid: {"n": f"カード{cid}"} for cid in hashes}


class TestUpdater(unittest.TestCase):
    """通信部分を差し替えて、新しいカードセットの取り込み手順を確認する。"""

    def setUp(self):
        from mdript import updater
        self.updater = updater
        self._orig = (updater._get, updater.fetch_pack_cards,
                      updater.fetch_card_page, updater.REQUEST_INTERVAL)
        updater.REQUEST_INTERVAL = 0
        updater._get = lambda op, url, **kw: PACK_LIST_HTML.encode("utf-8")
        self.pack_cards = {"1000000001000": ["1"], "1000000002000": ["1", "2"]}
        updater.fetch_pack_cards = lambda pid, op=None: self.pack_cards[pid]
        self.fetched = []

        def fake_card_page(cid, op=None):
            self.fetched.append(cid)
            return [fake_card(seed=int(cid))], ["1000000002000"]

        updater.fetch_card_page = fake_card_page

    def tearDown(self):
        (self.updater._get, self.updater.fetch_pack_cards,
         self.updater.fetch_card_page, self.updater.REQUEST_INTERVAL) = self._orig

    def test_pack_list_is_parsed_newest_first(self):
        packs = self.updater.fetch_pack_list(op=object())
        self.assertEqual([p["pid"] for p in packs],
                         ["1000000002000", "1000000001000"])
        self.assertEqual(packs[0]["name"], "新しいパック")
        self.assertEqual(packs[0]["date"], "2026/09/18")

    def test_first_run_only_records_the_packs(self):
        catalog = FakeCatalog({"1": ["0" * 16]})
        result = self.updater.update_new_sets(catalog, catalog.names)
        self.assertEqual(result["seeded"], 2)
        self.assertEqual(self.fetched, [])            # 1枚も取りに行かない
        self.assertEqual(len(catalog.known_packs), 2)

    def test_new_pack_refreshes_its_reprints(self):
        # 既存カードのハッシュは持っているが、新しいパックは未取り込み
        catalog = FakeCatalog({"1": ["0" * 16]}, packs=["1000000001000"])
        catalog.names["2"] = {"n": "新カード"}
        result = self.updater.update_new_sets(catalog, catalog.names)
        self.assertEqual(result["new_cards"], 1)       # ハッシュの無い "2"
        self.assertEqual(result["refreshed"], 1)       # 同じパックの再録 "1"
        self.assertEqual(sorted(self.fetched), ["1", "2"])
        self.assertIn("1000000002000", catalog.known_packs)
        self.assertEqual(len(catalog.hashes["1"]), 2)  # 古いイラストも残す

    def test_pack_list_failure_does_not_refresh_everything(self):
        """シリーズ一覧が取れないときは、全シリーズを取り直しに行かない。"""
        def fail(op, url, **kw):
            raise OSError("offline")
        self.updater._get = fail
        catalog = FakeCatalog({"1": ["0" * 16]})
        catalog.names["2"] = {"n": "新カード"}
        result = self.updater.update_new_sets(catalog, catalog.names)
        self.assertTrue(result["pack_list_failed"])
        self.assertEqual(self.fetched, ["2"])          # 新カードだけ
        self.assertEqual(result["refreshed"], 0)
        self.assertEqual(catalog.known_packs, set())   # 次回あらためて記録する

    def test_empty_name_list_is_an_error(self):
        """メンテナンス中などで0件のとき、取得済みのカード名を空で上書きさせない。"""
        self.updater._get = lambda op, url, **kw: b"<html>maintenance</html>"
        with self.assertRaises(RuntimeError):
            self.updater.fetch_card_names()

    def test_no_wait_after_the_last_retry(self):
        waits = []
        orig_sleep = self.updater.time.sleep
        self.updater._get = self._orig[0]
        self.updater.time.sleep = waits.append

        class Offline:
            def open(self, req, timeout=None):
                raise OSError("offline")
        try:
            with self.assertRaises(OSError):
                self.updater._get(Offline(), "https://example.invalid/", retries=3)
        finally:
            self.updater.time.sleep = orig_sleep
        self.assertEqual(len(waits), 2)

    def test_huge_pack_is_skipped(self):
        catalog = FakeCatalog({"1": ["0" * 16]}, packs=["1000000001000"])
        result = self.updater.update_new_sets(catalog, catalog.names, max_cards=0)
        self.assertEqual(result["skipped_packs"], ["1000000002000"])
        self.assertEqual(result["refreshed"], 0)


class HardeningTest(unittest.TestCase):
    """公開前のセキュリティ見直しで入れた対策のテスト。"""

    class _Res:
        def __init__(self, body, gzip_encoded=False):
            self._body = body
            self.headers = {"Content-Encoding": "gzip"} if gzip_encoded else {}

        def read(self, n=-1):
            return self._body if n < 0 else self._body[:n]

    def test_response_size_is_limited(self):
        import gzip
        from mdript import updater
        self.assertEqual(updater._read_limited(self._Res(b"abc")), b"abc")
        self.assertEqual(updater._read_limited(self._Res(gzip.compress(b"abc"), True)), b"abc")
        bomb = gzip.compress(b"x" * (updater.MAX_RESPONSE + 10))      # 展開すると上限超え
        with self.assertRaises(ValueError):
            updater._read_limited(self._Res(bomb, True))

    def test_redirect_only_to_official_https(self):
        import urllib.error
        import urllib.request
        from mdript import updater
        handler = updater._SameSiteRedirect()
        req = urllib.request.Request("https://" + updater.ALLOWED_HOST + "/a")
        for bad in ("http://" + updater.ALLOWED_HOST + "/b", "https://example.com/b",
                    "https://" + updater.ALLOWED_HOST + ".example.com/b", "file:///etc/passwd"):
            with self.assertRaises(urllib.error.URLError):
                handler.redirect_request(req, None, 302, "Found", {}, bad)
        ok = handler.redirect_request(req, None, 302, "Found", {},
                                      "https://" + updater.ALLOWED_HOST + "/b")
        self.assertIsNotNone(ok)

    def test_csv_cells_are_not_formulas(self):
        from mdript.pipeline import _csv_cell
        for danger in ("=1+1", "+1", "-1", "@SUM(A1)"):
            self.assertTrue(_csv_cell(danger).startswith("'"))
        self.assertEqual(_csv_cell("ブラック・マジシャン"), "ブラック・マジシャン")
        self.assertEqual(_csv_cell(12), 12)

    def test_broken_user_data_is_ignored(self):
        from mdript import catalog
        self.assertEqual(catalog._hex_list("not a list"), [])
        good = "0123456789abcdef" * 2
        self.assertEqual(catalog._hex_list([good, 5, "zz", good[:-1], "G" * 32]), [good])


if __name__ == "__main__":
    unittest.main()
