"""Synthetic Python stage timings; NOT a Rust benchmark or real-card workload.

Run from the repository root. Writes no user data and makes no network requests.
"""
import io
import json
import platform
import statistics
import sys
import time
from pathlib import Path
from unittest.mock import patch

import numpy as np
import PIL
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mdript.catalog import Catalog
from mdript.language import stroke_ratio
from mdript.layout import predict_card_box
from mdript.phash import artwork_phash_hex
from mdript.pipeline import rounded_alpha


def measure(fn, count=15):
    fn()
    samples = []
    for _ in range(count):
        start = time.perf_counter()
        fn()
        samples.append((time.perf_counter() - start) * 1000)
    return round(statistics.median(samples), 3)


def main():
    # Redirect catalog lookups to a nonexistent path; never touch APPDATA.
    with patch('mdript.catalog.user_data_dir', return_value=str(Path(__file__).parent / '__probe_no_user_data__')):
        catalog = Catalog()
    rows = []
    for width, height in [(1920, 1080), (2560, 1440), (3840, 2160)]:
        rng = np.random.default_rng(123)
        # Upscaled low-resolution noise: repeatable texture, not a real card.
        screen = Image.fromarray(rng.integers(0, 256, (180, 320, 3), dtype=np.uint8)).resize((width, height), Image.Resampling.BICUBIC)
        box = predict_card_box(screen.size)
        card = screen.crop(box)
        rgba = rounded_alpha(card)
        h = artwork_phash_hex(card)
        source = io.BytesIO()
        screen.save(source, 'PNG')
        data = source.getvalue()

        def decode():
            with Image.open(io.BytesIO(data)) as im:
                return im.convert('RGB')

        def encode(fmt):
            output = io.BytesIO()
            if fmt == 'WEBP':
                rgba.save(output, fmt, quality=90, method=4)
            else:
                rgba.save(output, fmt, compress_level=6)

        stages = {
            'png_decode_rgb': measure(decode),
            'crop': measure(lambda: screen.crop(box)),
            'phash_128': measure(lambda: artwork_phash_hex(card)),
            'rank': measure(lambda: catalog.rank(h)),
            'language': measure(lambda: stroke_ratio(card)),
            'rounded_alpha': measure(lambda: rounded_alpha(card)),
            'webp_encode': measure(lambda: encode('WEBP')),
            'png_encode': measure(lambda: encode('PNG')),
        }
        total = sum(v for k, v in stages.items() if k != 'png_encode')
        # Optimistic upper bound if every stage except decode/encode became free.
        fixed = stages['png_decode_rgb'] + stages['webp_encode']
        rows.append({'resolution': [width, height], 'median_ms': stages,
                     'sum_webp_stages_ms': round(total, 3),
                     'upper_bound_same_decode_encode': round(total / fixed, 3)})
    print(json.dumps({'kind': 'synthetic single-thread in-memory stage microbenchmark',
                      'python': platform.python_version(), 'pillow': PIL.__version__,
                      'numpy': np.__version__, 'hash_count': catalog.hash_count,
                      'samples_per_stage': 15, 'rows': rows}, indent=2))


if __name__ == '__main__':
    main()
