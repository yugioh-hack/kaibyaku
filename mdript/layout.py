# -*- coding: utf-8 -*-
"""カード領域の決定。

基準は WQHD (2560x1440) で実測したカード枠。ゲームのUIは画面の高さに合わせて
等倍で拡縮され、カードは水平方向の中央に置かれるため、
「高さ基準のスケール＋水平中央寄せ」で FullHD / WQHD / 4K / ウルトラワイドまで
同じ式で扱える（2560基準の左右端 833/1727 はちょうど画面中央に対称）。

この式で決まらなかった画像だけ、輪郭の強さからカード矩形を検出して切り抜く。
ウィンドウモードのスクショや、すでに切り抜き済みの画像への保険。
"""
from __future__ import annotations

import numpy as np
from PIL import Image

REF_W, REF_H = 2560, 1440
REF_CARD_BOX = (833, 69, 1727, 1371)          # WQHD での実測値
CARD_W = REF_CARD_BOX[2] - REF_CARD_BOX[0]    # 894
CARD_H = REF_CARD_BOX[3] - REF_CARD_BOX[1]    # 1302
CARD_ASPECT = CARD_W / CARD_H                 # ≒ 0.687
CORNER_RADIUS_RATIO = 22 / CARD_W             # WQHD で半径22px

# 検出結果を受け入れる範囲
ASPECT_TOLERANCE = 0.08
MIN_HEIGHT_RATIO = 0.30
MAX_HEIGHT_RATIO = 0.99

# 枠の出どころ。TRUSTED はマスター画像と同じ画角になることが検証済みの枠、
# ESTIMATED は画像から推測した枠（判定をより厳しくする）。
TRUSTED = "trusted"
ESTIMATED = "estimated"

PRESETS = {
    "FullHD (1920x1080)": (1920, 1080),
    "WQHD (2560x1440)": (2560, 1440),
    "4K (3840x2160)": (3840, 2160),
}


def predict_card_box(size):
    """画面サイズから、カード拡大表示の枠を推定する。"""
    w, h = size
    scale = h / REF_H
    cw, ch = CARD_W * scale, CARD_H * scale
    left = w / 2.0 - cw / 2.0
    top = REF_CARD_BOX[1] * scale
    return (int(round(left)), int(round(top)),
            int(round(left + cw)), int(round(top + ch)))


def _edge_peaks(profile, count=8, min_gap=6, frac=0.25):
    """エッジ強度プロファイルから、強い順に極大位置を選ぶ（近すぎる点はまとめる）。"""
    peaks = []
    threshold = float(profile.max()) * frac
    for i in np.argsort(profile)[::-1]:
        if profile[i] < threshold:
            break
        if all(abs(int(i) - j) >= min_gap for j in peaks):
            peaks.append(int(i))
        if len(peaks) >= count:
            break
    return sorted(peaks)


def detect_card_box(img: Image.Image, work_width=640, aspect_tol=ASPECT_TOLERANCE):
    """画面からカードの矩形を検出する。見つからなければ None。

    カードの外枠は画面内で最も長くまっすぐな輪郭なので、
    縦横それぞれの輪郭強度を射影して強い線を拾い、
    カードの縦横比（約0.687）に合う4本の組み合わせを選ぶ。
    背景はゲーム側でぼかされているため、輪郭は card 枠に集中する。
    """
    w, h = img.size
    if w < 16 or h < 16:                # 輪郭を測れないほど小さい
        return None
    scale = min(1.0, work_width / w, work_width / h)
    sw = max(2, int(round(w * scale)))
    sh = max(2, int(round(h * scale)))
    gray = np.asarray(img.convert("L").resize((sw, sh), Image.BILINEAR), dtype=np.float32)
    gx = np.abs(np.diff(gray, axis=1)).sum(axis=0)     # 列ごとの縦線の強さ
    gy = np.abs(np.diff(gray, axis=0)).sum(axis=1)     # 行ごとの横線の強さ
    if gx.max() <= 0 or gy.max() <= 0:
        return None
    cols, rows = _edge_peaks(gx), _edge_peaks(gy)
    if len(cols) < 2 or len(rows) < 2:
        return None

    min_w, min_h = sw * 0.10, sh * MIN_HEIGHT_RATIO
    best = None
    for i, left in enumerate(cols):
        for right in cols[i + 1:]:
            bw = right - left
            if bw < min_w:
                continue
            for j, top in enumerate(rows):
                for bottom in rows[j + 1:]:
                    bh = bottom - top
                    if bh < min_h or bh > sh * MAX_HEIGHT_RATIO:
                        continue
                    if abs(bw / bh - CARD_ASPECT) > aspect_tol:
                        continue
                    # 辺の長さで割って「1ピクセルあたりの輪郭の濃さ」で比べる
                    score = (gx[left] + gx[right]) / bh + (gy[top] + gy[bottom]) / bw
                    if best is None or score > best[0]:
                        best = (score, left, top, right, bottom)
    if best is None:
        return None
    _, left, top, right, bottom = best
    return (int(round(left * w / sw)), int(round(top * h / sh)),
            int(round((right + 1) * w / sw)), int(round((bottom + 1) * h / sh)))


# 枠がカードの縁に乗っているかを見るときの幅（カードの高さに対する比率）
EDGE_SEARCH_RATIO = 0.06       # この範囲でいちばん強い輪郭と比べる
EDGE_TOLERANCE_RATIO = 0.004   # 枠の辺からこの範囲に輪郭があればよい
EDGE_STRENGTH = 0.3
EDGE_SIDES = 3                 # 4辺のうち、輪郭が要る辺の数


def box_is_on_card_edges(img: Image.Image, box):
    """枠の各辺が、カードの縁（輪郭）に乗っているか。

    解像度から推定した枠は、全画面のスクショでは縁にぴったり乗る。タイトルバーごと撮った
    ウィンドウモードの画像では数%ずれて、辺の位置に輪郭が無くなる。
    各辺について、辺のすぐ近くの輪郭の強さを、その周りでいちばん強い輪郭と比べる。
    実測では、全画面（原本・縮小・JPEG）は4辺とも 0.39 以上、ずれた枠は3辺が 0.1 以下だった。
    """
    left, top, right, bottom = box
    width, height = right - left, bottom - top
    if width < 16 or height < 16:
        return False
    reach = max(3, int(round(height * EDGE_SEARCH_RATIO)))
    near = max(2, int(round(height * EDGE_TOLERANCE_RATIO)))
    # 角丸や隣の辺の影響を避けるため、辺の中ほど8割だけを見る
    inner_x = (left + width // 10, right - width // 10)
    inner_y = (top + height // 10, bottom - height // 10)
    sides = 0
    for vertical, edge in ((True, left), (True, right), (False, top), (False, bottom)):
        # 辺をまたぐ細い帯だけを取り出し、辺に直交する向きの輪郭の強さを並べる
        limit = img.size[0] if vertical else img.size[1]
        start, stop = max(0, edge - 1 - reach), min(limit, edge + reach + 1)
        strip = ((start, inner_y[0], stop, inner_y[1]) if vertical
                 else (inner_x[0], start, inner_x[1], stop))
        gray = np.asarray(img.crop(strip).convert("L"), dtype=np.float32)
        profile = np.abs(np.diff(gray, axis=1 if vertical else 0)).sum(axis=0 if vertical else 1)
        pos = edge - 1 - start
        close = profile[max(0, pos - near):pos + near + 1]
        if close.size and profile.max() > 0 and close.max() >= profile.max() * EDGE_STRENGTH:
            sides += 1
    return sides >= EDGE_SIDES


def card_boxes(img: Image.Image, mode="auto"):
    """試す順に候補の枠を返す。

    1. 画像全体（画像そのものがカードの形をしている場合のみ）
    2. 解像度からの推定（全画面のスクショはこれでぴったり合う）
    3. 輪郭からの検出（ウィンドウモードや想定外のUI倍率の保険）

    呼び出し側は先頭から試し、確実に一致した時点で打ち切る。
    推定枠はマスター画像と同じ画角になるため、まずこれを使うのが最も精度が高い。
    検出は重いので、呼び出し側が2つ目を要求したときに初めて実行する（ジェネレータ）。
    """
    predicted = predict_card_box(img.size)
    if isinstance(mode, (tuple, list)):
        # 「このスクショは元々この解像度だった」と指定された場合（拡大縮小済みの画像向け）
        sx, sy = img.size[0] / mode[0], img.size[1] / mode[1]
        base = predict_card_box(tuple(mode))
        yield TRUSTED, (int(base[0] * sx), int(base[1] * sy),
                        int(base[2] * sx), int(base[3] * sy))
        yield TRUSTED, predicted
        return

    if abs(img.size[0] / img.size[1] - CARD_ASPECT) <= ASPECT_TOLERANCE:
        # 画像そのものがカードの形をしている＝すでに切り抜き済みとみなす
        yield TRUSTED, (0, 0, img.size[0], img.size[1])
    yield TRUSTED, predicted
    if mode == "fixed":
        return

    detected = detect_card_box(img)
    if detected and max(abs(a - b) for a, b in zip(detected, predicted)) > 2:
        yield ESTIMATED, detected


COARSE_SHIFTS = (-0.03, -0.015, 0.0, 0.015, 0.03)
COARSE_SCALES = (0.94, 0.97, 1.0, 1.03, 1.06)
FINE_SHIFTS = (-0.008, -0.004, 0.0, 0.004, 0.008)
FINE_SCALES = (0.99, 1.0, 1.01)


def nudged_boxes(box, image_size, shifts=FINE_SHIFTS, scales=FINE_SCALES):
    """枠を少しずつ動かした候補を返す（検出がわずかにずれている場合の追い込み用）。

    pHash は切り出し位置が 1% ずれるだけで距離が大きく変わるため、
    どの枠でも決め手にならなかったときだけ、この総当たりで拾い直す。
    """
    l, t, r, b = box
    w, h = r - l, b - t
    cx, cy = (l + r) / 2.0, (t + b) / 2.0
    iw, ih = image_size
    seen = set()
    for scale in scales:
        sw, sh = w * scale, h * scale
        for dx in shifts:
            for dy in shifts:
                nx, ny = cx + w * dx, cy + h * dy
                cand = (int(round(nx - sw / 2)), int(round(ny - sh / 2)),
                        int(round(nx + sw / 2)), int(round(ny + sh / 2)))
                if cand in seen:
                    continue
                seen.add(cand)
                if cand[0] < 0 or cand[1] < 0 or cand[2] > iw or cand[3] > ih:
                    continue
                yield cand
