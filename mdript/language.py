# -*- coding: utf-8 -*-
"""カードのテキスト欄が英語か日本語かの判定。

マスターデュエルのイラストは言語で変わらないため、英語クライアントのスクショも
日本語版と同じハッシュに当たる。言語の違いが出るのはテキスト欄の文字だけなので、
OCR を使わずに字の形で見分ける。

  * 日本語: 漢字・かなは横画が多く、上下方向の明るさの変化が大きい
  * 英語  : 小文字は縦画（l, i, n, m, h の縦線）が多く、左右方向の変化が大きい

テキスト欄の文字のまわりで、上下方向と左右方向の明るさの変化を足し合わせ、
その比（上下 / 左右）を見る。字の形の性質なので、縮小や JPEG でもほとんど変わらない。

実測（日本語クライアントのスクショ 4,139 枚、英語クライアントのスクショ 102 枚）:
                 日本語の最小値   英語の最大値
  WQHD 原本        0.895        0.612
  FullHD に縮小     0.946        0.613
  1280x720 に縮小   0.907        0.672
  FullHD + JPEG50  0.939        0.618
"""
from __future__ import annotations

import numpy as np
from PIL import Image

# テキスト欄（カード画像に対する比率）。ATK/DEF の行より上
TEXT_BOX_RATIO = (0.07, 0.755, 0.93, 0.895)

INK_CONTRAST = 60        # 地の色（中央値）よりこれだけ暗い画素を文字とみなす
MIN_INK = 0.005          # 文字の画素がテキスト欄のこの割合に満たなければ判定しない

# 比がこれ以下なら英語。英語の最大値 0.672 と日本語の最小値 0.895 の間で、
# 日本語を英語と取り違えないよう、やや英語寄りに置く
ENGLISH_MAX_RATIO = 0.75


def stroke_ratio(card_img: Image.Image):
    """テキスト欄の文字の、上下方向と左右方向の明るさの変化の比。文字が無ければ None。"""
    w, h = card_img.size
    l, t, r, b = TEXT_BOX_RATIO
    gray = np.asarray(card_img.crop((int(w * l), int(h * t), int(w * r), int(h * b)))
                      .convert("L"), dtype=np.float32)
    if gray.shape[0] < 3 or gray.shape[1] < 3:
        return None
    ink = gray < (np.median(gray) - INK_CONTRAST)
    if ink.mean() < MIN_INK:
        return None
    # 背景の模様（羊皮紙・きらめき）を拾わないよう、文字のすぐそばの変化だけを足す
    near = ink[:-1, :-1] | ink[1:, :-1] | ink[:-1, 1:]
    horizontal = np.abs(np.diff(gray, axis=1))[:-1, :][near].sum()
    vertical = np.abs(np.diff(gray, axis=0))[:, :-1][near].sum()
    return float(vertical / horizontal) if horizontal > 0 else None


def is_english(card_img: Image.Image) -> bool:
    """テキスト欄が英語ならTrue。判断できないときは日本語として扱う（False）。"""
    ratio = stroke_ratio(card_img)
    return ratio is not None and ratio <= ENGLISH_MAX_RATIO
