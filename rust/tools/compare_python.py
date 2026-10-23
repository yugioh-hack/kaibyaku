"""Compare the hash prototype against Python; never reads APPDATA or writes source images."""
import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))
from PIL import Image
from mdript.layout import predict_card_box
from mdript.phash import artwork_phash_hex


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("images", type=Path)
    parser.add_argument("--exe", type=Path, default=ROOT / "rust/target/release/mdript-cli.exe")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    files = sorted(p for p in args.images.rglob("*") if p.suffix.lower() in {".png", ".jpg", ".jpeg", ".webp"})
    if not files:
        parser.error("no images found")
    rows = []
    args.report.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.report.parent) as tmp:
        normalized = Path(tmp) / "decoded.png"
        for index, path in enumerate(files, 1):
            if index == 1 or index % 25 == 0:
                print(f"Comparing {index}/{len(files)}", file=sys.stderr, flush=True)
            with Image.open(path) as im:
                rgb = im.convert("RGB")
            box = predict_card_box(rgb.size)
            if not (0 <= box[0] < box[2] <= rgb.width and 0 <= box[1] < box[3] <= rgb.height):
                rows.append({"file": path.name, "error": "predicted box outside image"})
                continue
            card = rgb.crop(box)
            rgb.save(normalized)
            for mode, flags in [("normal", []), ("coarse", ["--coarse"]), ("center", ["--center"])]:
                expected = artwork_phash_hex(card, coarse=mode == "coarse", with_center=mode == "center")
                row = {"file": path.name, "mode": mode}
                for label, source in [("native_decode", path), ("same_pixels", normalized)]:
                    proc = subprocess.run([str(args.exe), str(source), *flags], capture_output=True, text=True, encoding="utf-8")
                    if proc.returncode:
                        row[label] = {"error": proc.stderr.strip()}
                    else:
                        actual = json.loads(proc.stdout)
                        row[label] = {"distance": (int(expected, 16) ^ int(actual["hash"], 16)).bit_count(), "box_equal": actual["box"] == list(box)}
                rows.append(row)
    summary = {"images": len(files), "cases": len(rows), "results": {}}
    for label in ["native_decode", "same_pixels"]:
        matches = sum(r.get(label, {}).get("distance") == 0 and r.get(label, {}).get("box_equal") for r in rows)
        summary["results"][label] = {"matches": matches, "total": len(rows)}
    args.report.write_text(json.dumps({"summary": summary, "rows": rows}, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(summary, indent=2))
    return 0 if all(v["matches"] == v["total"] for v in summary["results"].values()) else 1

if __name__ == "__main__":
    raise SystemExit(main())
