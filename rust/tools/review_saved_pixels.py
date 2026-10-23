"""Measure decoded-output pixel difference for one Python/Rust benchmark pair."""
import argparse
import json
from pathlib import Path

import numpy as np
from PIL import Image


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("python_output", type=Path)
    parser.add_argument("rust_output", type=Path)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    left = args.python_output.resolve()
    right = args.rust_output.resolve()
    per_image = []
    total_sum = 0
    total_pixels = 0
    maximum = 0
    for path in sorted(left.rglob("*.webp")):
        pair = right / path.relative_to(left)
        with Image.open(path) as a, Image.open(pair) as b:
            x = np.asarray(a.convert("RGBA"), dtype=np.int16)
            y = np.asarray(b.convert("RGBA"), dtype=np.int16)
        assert x.shape == y.shape, path
        difference = np.abs(x - y)
        total_sum += int(difference.sum())
        total_pixels += difference.size
        current_max = int(difference.max())
        maximum = max(maximum, current_max)
        per_image.append({"file": str(path.relative_to(left)), "mean": float(difference.mean()), "max": current_max})
    result = {"image_count": len(per_image), "mean_abs_channel_difference": total_sum / total_pixels,
              "max_abs_channel_difference": maximum, "images": per_image}
    args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps({key: value for key, value in result.items() if key != "images"}, ensure_ascii=False))


if __name__ == "__main__":
    main()
