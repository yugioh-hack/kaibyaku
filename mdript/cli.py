# -*- coding: utf-8 -*-
"""コマンドライン版。引数なしで起動すると GUI を開く。"""
from __future__ import annotations

import argparse
import sys

from .catalog import Catalog
from .layout import TRUSTED, card_boxes
from .pipeline import CONFIRMED, ERROR, NOT_CARD, REVIEW, Cropper, Options


def _resolution(text):
    if not text or text == "auto":
        return "auto"
    if text == "fixed":
        return "fixed"
    if "x" in text.lower():
        w, h = (int(v) for v in text.lower().split("x"))
        if w > 0 and h > 0:
            return (w, h)
    raise argparse.ArgumentTypeError("解像度は 1920x1080 のように指定してください")


def _quality(text):
    value = int(text)
    if not 1 <= value <= 100:
        raise argparse.ArgumentTypeError("画質は 1〜100 で指定してください")
    return value


def build_parser():
    p = argparse.ArgumentParser(
        prog="mdript",
        description="遊戯王のスクリーンショットからカードを切り抜いてカード名を付ける")
    p.add_argument("paths", nargs="*", help="画像ファイルまたはフォルダ")
    p.add_argument("-o", "--output", default="output", help="出力先フォルダ（既定: output）")
    p.add_argument("--format", choices=["webp", "png", "both"], default="webp")
    p.add_argument("--quality", type=_quality, default=90, help="WebP の画質 1〜100（既定: 90）")
    p.add_argument("--no-rounded", action="store_true", help="角丸・透過をしない")
    p.add_argument("--no-recursive", action="store_true", help="サブフォルダを見ない")
    p.add_argument("--keep-duplicates", action="store_true", help="同じイラストでも保存する")
    p.add_argument("--no-review", action="store_true", help="要確認の画像を保存しない")
    p.add_argument("--no-not-card", action="store_true",
                   help="カードと判定できなかった画像を保存しない")
    p.add_argument("--resolution", type=_resolution, default="auto",
                   help="auto / fixed / 1920x1080 のように元の解像度を指定")
    p.add_argument("--gui", action="store_true", help="GUI を開く")
    p.add_argument("--update-names", action="store_true",
                   help="公式データベースからカード名を取得する")
    p.add_argument("--update-hashes", action="store_true",
                   help="新しいカードセット（新カード＋そのパックの再録カード）を取り込む")
    p.add_argument("--learn", metavar="カードID",
                   help="指定した画像1枚を、このカードIDの照合用ハッシュとして追加する"
                        "（マスター画像と切り方が違い、カードなしになってしまうカード向け）")
    return p


def learn(catalog, cid, path):
    """画像1枚（カードの拡大画面）のハッシュを、カードIDの照合用に追加して保存する。"""
    from PIL import Image

    cid = str(cid)
    if cid not in catalog.hashes:
        print(f"カードID {cid} は照合用データにありません", file=sys.stderr)
        return 1
    cropper = Cropper(catalog, Options())
    with Image.open(path) as raw:
        img = raw.convert("RGB")
    # マスター画像と同じ画角の枠（trusted）だけを使う。画像から推測した枠で取ったハッシュは
    # 少しずれた切り出しになり、足しても他の画像に当たらない
    found = next((f for origin, box in card_boxes(img, cropper.options.resolution)
                  if origin == TRUSTED and (f := cropper._evaluate(img, box, origin))), None)
    if not found:
        print("カードの位置を決められませんでした（カードの拡大画面を全画面で撮った画像を使ってください）",
              file=sys.stderr)
        return 1
    print(f"{catalog.display_name(cid)}（{cid}）  現在の照合: "
          f"{catalog.display_name(found['card_id'])}（{found['card_id']}）距離 {found['distance']}")
    if found["card_id"] == cid and Cropper.is_confirmed(found):
        print("すでに確定できるので、追加しません")
        return 0
    if not catalog.add_hash(cid, found["hash"]):
        print("近いハッシュがすでにあるので、追加しません")
        return 0
    catalog.save_user_hashes()
    print("照合用ハッシュを追加しました")
    return 0


def main(argv=None):
    # カード名に出力先の文字コードで表せない字があっても止まらないようにする
    for stream in (sys.stdout, sys.stderr):
        if stream is not None and hasattr(stream, "reconfigure"):
            stream.reconfigure(errors="replace")
    args = build_parser().parse_args(argv)

    if args.learn:
        if len(args.paths) != 1:
            print("--learn には画像を1枚だけ指定してください", file=sys.stderr)
            return 2
        return learn(Catalog(), args.learn, args.paths[0])

    if args.gui or (not args.paths and not args.update_names and not args.update_hashes):
        from .gui import main as gui_main
        return gui_main()

    catalog = Catalog()
    if args.update_names or args.update_hashes:
        from . import updater
        try:
            cards = updater.fetch_card_names(progress=lambda m: print(m, end="\r"))
        except (OSError, RuntimeError) as exc:      # 通信エラー・メンテナンス中など
            print(f"\nカード名を取得できませんでした: {exc}", file=sys.stderr)
            return 1
        catalog.save_names(cards)
        print(f"\nカード名 {len(cards)} 件を保存しました")
        if args.update_hashes:
            r = updater.update_new_sets(catalog, cards,
                                        progress=lambda m: print(m, end="\r"))
            catalog.save_user_hashes()
            if r["seeded"]:
                print(f"\n収録シリーズ {r['seeded']} 個を記録しました（同梱ハッシュでカバー済み）")
            print(f"\n新しいカード {r['new_cards']} 件 / 再録カードの確認 {r['refreshed']} 件"
                  f"（収録シリーズ {r['packs']} 個、イラスト {r['new_hashes']} 枚を追加、"
                  f"失敗 {len(r['failed'])} 件）")
            for name in r["pack_names"][:5]:
                print(f"  取り込んだシリーズ: {name}")
            if r["skipped_packs"]:
                print("収録数が多いシリーズがあったため、再録カードの確認は省略しました")
            if r["pack_list_failed"]:
                print("収録シリーズの一覧を取得できなかったため、再録カードの確認は省略しました")
        catalog.load()
        if not args.paths:
            return 0

    options = Options(
        output_dir=args.output,
        save_format=args.format,
        quality=args.quality,
        rounded=not args.no_rounded,
        resolution=args.resolution,
        skip_duplicate=not args.keep_duplicates,
        recursive=not args.no_recursive,
        save_review=not args.no_review,
        save_not_card=not args.no_not_card,
    )
    cropper = Cropper(catalog, options)

    def show(result):
        mark = {CONFIRMED: "[確定]", REVIEW: "[要確認]",
                NOT_CARD: "[カードなし]", ERROR: "[エラー]"}[result.status]
        detail = result.name or result.note
        print(f"{mark} {detail} (距離 {result.distance}) <- {result.source}")

    summary = cropper.process(args.paths, on_result=show)
    print(f"\n確定 {summary.confirmed} / 要確認 {summary.review} / "
          f"カードなし {summary.not_card} / エラー {summary.error} "
          f"（重複スキップ {summary.skipped}） {summary.seconds:.1f} 秒")
    return 0 if summary.error == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
