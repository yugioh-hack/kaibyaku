"""実データのレビューで見つかった保存・更新・入力検証の回帰テスト。"""
import json
import os
import tempfile
import threading
import unittest
from concurrent.futures import ThreadPoolExecutor
from types import SimpleNamespace
from unittest.mock import patch

from PIL import Image

from mdript import catalog, updater
from mdript.layout import TRUSTED, detect_card_box
from mdript.pipeline import NOT_CARD, Cropper, Options


class TestSafeCrops(unittest.TestCase):
    def test_invalid_boxes_are_rejected_before_allocating(self):
        cropper = Cropper(SimpleNamespace())
        image = Image.new("RGB", (40, 12000))
        with patch.object(Image.Image, "crop", side_effect=AssertionError("allocated")):
            self.assertIsNone(cropper._evaluate(image, (-3705, 575, 3745, 11425), TRUSTED))
        large = SimpleNamespace(width=8000, height=8000)
        self.assertIsNone(cropper._evaluate(large, (0, 0, 8000, 8000), TRUSTED))

    def test_portrait_detection_uses_a_bounded_work_image(self):
        image = Image.new("RGB", (40, 12000))
        original = Image.Image.resize
        sizes = []

        def record(im, size, *args, **kwargs):
            sizes.append(size)
            return original(im, size, *args, **kwargs)

        with patch.object(Image.Image, "resize", record):
            self.assertIsNone(detect_card_box(image))
        self.assertTrue(sizes)
        self.assertTrue(all(max(size) <= 640 for size in sizes))


class TestSaving(unittest.TestCase):
    def test_failed_save_can_be_retried_in_both_formats(self):
        with tempfile.TemporaryDirectory() as tmp:
            cropper = Cropper(SimpleNamespace(), Options(save_format="both"))
            image = Image.new("RGB", (100, 150))
            original = cropper._encode

            def fail_png(im, path, ext):
                if ext == ".png":
                    raise OSError("disk full")
                original(im, path, ext)

            with patch.object(cropper, "_encode", fail_png):
                with self.assertRaises(OSError):
                    cropper._save(image, tmp, "card", "0" * 32)
            self.assertEqual(os.listdir(tmp), [])
            self.assertFalse(cropper._pending)
            saved, note = cropper._save(image, tmp, "card", "0" * 32)
            self.assertEqual(note, "")
            self.assertEqual(os.path.basename(saved), "card.webp")
            self.assertEqual(sorted(os.listdir(tmp)), ["card.png", "card.webp"])

    def test_independent_writers_do_not_overwrite(self):
        with tempfile.TemporaryDirectory() as tmp:
            barrier = threading.Barrier(2)
            croppers = [Cropper(SimpleNamespace(), Options(skip_duplicate=False)) for _ in range(2)]
            images = [Image.new("RGB", (100, 150), color) for color in ("red", "blue")]

            def encode(im, path, ext):
                barrier.wait(timeout=10)
                im.save(path, "PNG")

            for cropper in croppers:
                cropper._encode = encode
            with ThreadPoolExecutor(2) as pool:
                futures = [pool.submit(c._save, im, tmp, "card", str(i) * 32)
                           for i, (c, im) in enumerate(zip(croppers, images))]
                saved = [f.result(timeout=15)[0] for f in futures]
            self.assertEqual(len(set(saved)), 2)
            for path, color in zip(saved, ((255, 0, 0), (0, 0, 255))):
                with Image.open(path) as im:
                    self.assertEqual(im.convert("RGB").getpixel((50, 75)), color)

    def test_reservation_collision_preserves_an_existing_second_format(self):
        with tempfile.TemporaryDirectory() as tmp:
            existing = os.path.join(tmp, "card.png")
            Image.new("RGB", (100, 150), "blue").save(existing)
            with open(existing, "rb") as f:
                before = f.read()
            cropper = Cropper(SimpleNamespace(), Options(save_format="both", skip_duplicate=False))
            # 存在確認の直後に別プロセスが PNG を作った状況を再現する。
            with patch("mdript.pipeline.os.path.exists", return_value=False):
                saved, _ = cropper._save(Image.new("RGB", (100, 150), "red"), tmp, "card", "0" * 32)
            self.assertEqual(os.path.basename(saved), "card_alt1.webp")
            self.assertFalse(os.path.exists(os.path.join(tmp, "card.webp")))
            with open(existing, "rb") as f:
                self.assertEqual(f.read(), before)
            self.assertTrue(os.path.isfile(os.path.join(tmp, "card_alt1.png")))

    def test_pending_failure_does_not_skip_a_later_duplicate(self):
        with tempfile.TemporaryDirectory() as tmp:
            cropper = Cropper(SimpleNamespace(), Options(rounded=False))
            entered, release = threading.Event(), threading.Event()
            original = cropper._encode
            attempts = []

            def fail_first(im, path, ext):
                attempts.append(path)
                if len(attempts) == 1:
                    entered.set()
                    if not release.wait(10):
                        raise TimeoutError()
                    raise OSError("disk full")
                original(im, path, ext)

            cropper._encode = fail_first
            image = Image.new("RGB", (100, 150))
            with ThreadPoolExecutor(2) as pool:
                first = pool.submit(cropper._save, image, tmp, "card", "0" * 32)
                self.assertTrue(entered.wait(10))
                second = pool.submit(cropper._save, image, tmp, "card", "0" * 32)
                release.set()
                with self.assertRaises(OSError):
                    first.result(timeout=10)
                saved, note = second.result(timeout=10)
            self.assertTrue(os.path.isfile(saved))
            self.assertEqual(note, "")

    def test_no_candidate_preserves_the_whole_image(self):
        for keep in (True, False):
            with self.subTest(keep=keep), tempfile.TemporaryDirectory() as tmp:
                source = os.path.join(tmp, "tiny.png")
                Image.new("RGB", (16, 16), "red").save(source)
                cropper = Cropper(SimpleNamespace(), Options(
                    output_dir=os.path.join(tmp, "out"), save_format="png", save_not_card=keep))
                result = cropper.process_file(source)
                self.assertEqual(result.status, NOT_CARD)
                if keep:
                    with Image.open(result.saved_path) as im:
                        self.assertEqual(im.size, (16, 16))
                        self.assertEqual(im.getpixel((0, 0)), (255, 0, 0))
                else:
                    self.assertEqual(result.saved_path, "")
                    self.assertFalse(os.path.exists(cropper.options.output_dir))


class TestBrokenUserData(unittest.TestCase):
    def test_nested_wrong_types_do_not_prevent_loading(self):
        for value in ([1], "bad", 1, None):
            with self.subTest(value=value), tempfile.TemporaryDirectory() as tmp:
                with patch.object(catalog, "user_data_dir", return_value=tmp):
                    for meta in (value, {"format": 2, "packs": value}):
                        with open(os.path.join(tmp, catalog.USER_HASHES), "w", encoding="utf-8") as f:
                            json.dump({"meta": meta, "hashes": value}, f)
                        with open(os.path.join(tmp, catalog.USER_NAMES), "w", encoding="utf-8") as f:
                            json.dump({"meta": value, "cards": value}, f)
                        db = catalog.Catalog()
                        self.assertGreater(db.hash_count, 0)
                        self.assertEqual(db.names, {})

    def test_invalid_name_fields_and_config_types_are_ignored(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(catalog, "user_data_dir", return_value=tmp):
            with open(os.path.join(tmp, catalog.USER_NAMES), "w", encoding="utf-8") as f:
                json.dump({"meta": {"fetched": [1]}, "cards": {"1": {"n": "card", "e": [1]}, "2": []}}, f)
            db = catalog.Catalog()
            self.assertEqual(db.display_name("1", english=True), "card")
            self.assertFalse(db.has_english_name("1"))
            self.assertEqual(db.names_fetched, "")
            with open(os.path.join(tmp, catalog.CONFIG_FILE), "w", encoding="utf-8") as f:
                json.dump({"rounded": [1], "output_dir": {}, "english": [], "recursive": True}, f)
            self.assertEqual(catalog.load_config(), {"recursive": True})


class TestRetryUpdates(unittest.TestCase):
    def test_failed_reprint_is_retried_but_successful_pack_is_not(self):
        db = SimpleNamespace(hashes={"1": ["0" * 32], "2": ["1" * 32]}, known_packs={"old"})
        packs = [{"pid": "new"}, {"pid": "good"}]
        members = {"new": ["1"], "good": ["2"]}
        with patch.object(updater, "fetch_pack_list", return_value=packs), \
                patch.object(updater, "fetch_pack_cards", side_effect=lambda pid, op: members[pid]), \
                patch.object(updater, "update_hashes", side_effect=[(1, 0, ["1"], set()), (1, 1, [], set())]) as update, \
                patch.object(updater.time, "sleep"):
            first = updater.update_new_sets(db, {"1": {}, "2": {}})
            self.assertEqual(first["failed"], ["1"])
            self.assertEqual(db.known_packs, {"old", "good"})
            second = updater.update_new_sets(db, {"1": {}, "2": {}})
            self.assertEqual(second["failed"], [])
            self.assertEqual(update.call_args.args[1], ["1"])
            self.assertEqual(db.known_packs, {"old", "good", "new"})

    def test_empty_pack_response_is_not_marked_complete(self):
        db = SimpleNamespace(hashes={"1": ["0" * 32]}, known_packs={"old"})
        with patch.object(updater, "fetch_pack_list", return_value=[{"pid": "new"}]), \
                patch.object(updater, "fetch_pack_cards", return_value=[]), \
                patch.object(updater.time, "sleep"):
            updater.update_new_sets(db, {"1": {}})
        self.assertEqual(db.known_packs, {"old"})


if __name__ == "__main__":
    unittest.main()
