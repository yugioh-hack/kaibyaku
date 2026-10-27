"""Measure language classification against independently reviewed image labels.

Usage: python tools/validate_language.py IMAGES LABELS_JSON OUTPUT_DIRECTORY
Labels map filenames to ja/en/non_card. Inputs are never modified.
"""
import argparse
import csv
import hashlib
import io
import json
from pathlib import Path
import sys

import numpy as np
from PIL import Image, __version__ as pillow_version

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mdript.language import ENGLISH_MAX_RATIO, stroke_ratio
from mdript.layout import predict_card_box


def variants(image):
    yield "original", image, (0, 0)
    for height in (1080, 720, 480):
        size = (round(image.width * height / image.height), height)
        resized = image.resize(size, Image.Resampling.LANCZOS)
        yield f"height{height}", resized, (0, 0)
        if height in (1080, 720):
            for quality in (50, 20):
                buffer = io.BytesIO()
                resized.save(buffer, "JPEG", quality=quality)
                buffer.seek(0)
                with Image.open(buffer) as decoded:
                    yield f"height{height}_jpeg{quality}", decoded.convert("RGB"), (0, 0)
    for dx, dy in ((-4, 0), (4, 0), (0, -4), (0, 4)):
        yield f"shift_{dx}_{dy}", image, (dx, dy)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("images", type=Path)
    parser.add_argument("labels", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    labels = json.loads(args.labels.read_text(encoding="utf-8"))
    if not labels or any(v not in {"ja", "en", "non_card"} for v in labels.values()):
        parser.error("labels must map filenames to ja/en/non_card")
    rows, inputs = [], []
    for name, expected in sorted(labels.items()):
        path = args.images / name
        if Path(name).name != name:
            parser.error("labels must use filenames without directories")
        inputs.append({"name": name, "label": expected,
                       "sha256": hashlib.sha256(path.read_bytes()).hexdigest()})
        with Image.open(path) as source:
            image = source.convert("RGB")
        for case, transformed, (dx, dy) in variants(image):
            box = predict_card_box(transformed.size)
            box = tuple(v + (dx if i % 2 == 0 else dy) for i, v in enumerate(box))
            ratio = stroke_ratio(transformed.crop(box))
            measured = "unknown" if ratio is None else "en" if ratio <= ENGLISH_MAX_RATIO else "ja"
            rows.append({"file": name, "case": case, "expected": expected,
                         "ratio": ratio, "measured": measured,
                         "effective": "en" if measured == "en" else "ja",
                         "box": json.dumps(box)})
    summary = {}
    for case in dict.fromkeys(r["case"] for r in rows):
        subset = [r for r in rows if r["case"] == case and r["expected"] != "non_card"]
        summary[case] = {
            "total": len(subset),
            "correct": sum(r["effective"] == r["expected"] for r in subset),
            "unknown": sum(r["measured"] == "unknown" for r in subset),
            "errors": [r["file"] for r in subset if r["effective"] != r["expected"]],
        }
        for lang in ("ja", "en"):
            values = [r["ratio"] for r in subset if r["expected"] == lang and r["ratio"] is not None]
            summary[case][lang] = {"min": min(values) if values else None,
                                  "max": max(values) if values else None}
    report = {"threshold": ENGLISH_MAX_RATIO,
              "scope": "Python stroke_ratio on predicted card crops; non-cards excluded from accuracy",
              "versions": {"python": sys.version, "pillow": pillow_version, "numpy": np.__version__},
              "inputs": inputs, "summary": summary}
    args.output.mkdir(parents=True, exist_ok=True)
    with (args.output / "measurements.csv").open("w", encoding="utf-8-sig", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    (args.output / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(summary, ensure_ascii=False, indent=2))
    return int(any(v["errors"] for v in summary.values()))


if __name__ == "__main__":
    raise SystemExit(main())
