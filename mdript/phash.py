# -*- coding: utf-8 -*-
"""知覚ハッシュ（pHash）。

カードのイラストを2か所に分けてハッシュ化し、64bit×2 = 128bit を1本の
16進文字列（32文字）として扱う。前半が上側、後半が下側の領域。

2か所にする理由:
  マスター画像には SAMPLE の透かしが 0.40〜0.48 あたりに乗っているため、
  1領域だけだと透かしの上（0.22〜0.38）しか使えず、情報が足りない。
  透かしの下（0.50〜0.62）にもイラストが残っているので、そこを2つ目に使う。
  実測ではこれで「要確認」が 19枚→3枚、カードではない画像を誤って確定する
  割合が 0.56%→0.00% になった（スクショ9,524枚・マスター14,753枚で検証）。

前半の64bitだけを使う「粗い照合」もできるようにしてある。
ビット数が増えるほど切り出し位置のズレに敏感になるので、
枠を探している間は前半だけで採点し、確定の判断は128bit全体で行う。

imagehash パッケージの phash と各領域はビット単位で同じ値になるが、
scipy を使わずに numpy だけで DCT を行う（配布サイズを小さく保つため）。
"""
from __future__ import annotations

import numpy as np
from PIL import Image

HASH_SIZE = 8
HIGHFREQ_FACTOR = 4
_DCT_N = HASH_SIZE * HIGHFREQ_FACTOR

# カード画像のうち、照合に使う領域の比率 (left, top, right, bottom)。
# 上: SAMPLE 透かしの上側 / 下: 透かしの下に残るイラスト部分
ART_BOX_RATIO = (0.18, 0.22, 0.82, 0.38)
ART_BOX_RATIO_LOWER = (0.18, 0.50, 0.82, 0.62)
ART_BOXES = (ART_BOX_RATIO, ART_BOX_RATIO_LOWER)

# 中央（SAMPLE 透かしの下）。上下の領域だけでは見分けがつかない
# ウィジャ盤・死のメッセージ「E」「A」「T」「H」の判定にだけ使う。
# 透かしごと取り込むので、通常のカードには使わない。
CENTER_BOX_RATIO = (0.18, 0.38, 0.82, 0.50)

BITS_PER_REGION = HASH_SIZE * HASH_SIZE          # 64
HEX_PER_REGION = BITS_PER_REGION // 4            # 16
HASH_HEX = HEX_PER_REGION * len(ART_BOXES)       # 32

_K = np.arange(_DCT_N)[:, None]
_M = 2.0 * np.cos(np.pi * _K * (2 * np.arange(_DCT_N)[None, :] + 1) / (2 * _DCT_N))


def phash_hex(img: Image.Image) -> str:
    """画像1枚ぶんの pHash を 16 進文字列（16文字 / 64bit）で返す。"""
    px = np.asarray(img.convert("L").resize((_DCT_N, _DCT_N), Image.LANCZOS),
                    dtype=np.float64)
    low = (_M @ px @ _M.T)[:HASH_SIZE, :HASH_SIZE]
    bits = (low > np.median(low)).flatten()
    return np.packbits(bits).tobytes().hex()     # 先頭のビットが最上位


def crop_region(card_img: Image.Image, box) -> Image.Image:
    """カード1枚の画像から、比率で指定した領域を切り出す。"""
    if card_img.mode != "RGB":
        card_img = card_img.convert("RGB")
    w, h = card_img.size
    l, t, r, b = box
    return card_img.crop((int(w * l), int(h * t), int(w * r), int(h * b)))


def center_phash_hex(card_img: Image.Image) -> str:
    """中央の領域だけのハッシュ（64bit / 16文字）。"""
    return phash_hex(crop_region(card_img, CENTER_BOX_RATIO))


def artwork_phash_hex(card_img: Image.Image, coarse: bool = False,
                      with_center: bool = False) -> str:
    """カード画像から 128bit のハッシュ（32文字）を作る。

    coarse=True のときは上側の領域だけ（64bit / 16文字）を計算する。
    粗い照合では下側を使わないので、枠を探す間の計算を半分にできる。
    with_center=True のときは末尾に中央の領域を足した 192bit（48文字）にする。
    """
    if card_img.mode != "RGB":
        card_img = card_img.convert("RGB")
    boxes = ART_BOXES[:1] if coarse else ART_BOXES
    hex_hash = "".join(phash_hex(crop_region(card_img, box)) for box in boxes)
    if with_center and not coarse:
        hex_hash += center_phash_hex(card_img)
    return hex_hash


def hamming(a: str, b: str) -> int:
    return bin(int(a, 16) ^ int(b, 16)).count("1")
