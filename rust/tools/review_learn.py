"""Compare isolated Python and Rust learn results on a synthetic image."""
import argparse
import json
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT))


def synthetic(path):
    image = Image.new("RGB", (400, 600))
    pixels = image.load()
    for y in range(600):
        for x in range(400):
            pixels[x, y] = ((x * 7 + y * 3) % 256, (x * y // 17) % 256, (x * 11 ^ y * 5) % 256)
    draw = ImageDraw.Draw(image)
    draw.rectangle((31, 96, 285, 260), outline="white", width=5)
    image.save(path)


def python_learn(cid, image, user_dir):
    from mdript.catalog import Catalog
    from mdript.cli import learn

    with patch("mdript.catalog.user_data_dir", return_value=str(user_dir)):
        return learn(Catalog(), cid, str(image))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    image = root / "synthetic.png"
    synthetic(image)
    cid = next(iter(json.loads((ROOT / "data/card_hashes.json").read_text(encoding="utf-8"))["hashes"]))
    scenarios = {
        "fresh": None,
        "packs": {"meta": {"format": 2, "cards": 0, "packs": ["7", "3"]}, "hashes": {}},
        "invalid": {"meta": {"format": 1, "cards": 0, "packs": ["7"]}, "hashes": {}},
        "broken": "{broken",
    }
    for name, initial in scenarios.items():
        python_dir, rust_dir = root / name / "python", root / name / "rust"
        python_dir.mkdir(parents=True)
        rust_dir.mkdir()
        if initial is not None:
            contents = initial if isinstance(initial, str) else json.dumps(initial)
            for directory in (python_dir, rust_dir):
                (directory / "card_hashes_user.json").write_text(contents, encoding="utf-8")
        for repeat in range(2):
            assert python_learn(cid, image, python_dir) == 0
            result = subprocess.run(
                [str(args.exe.resolve()), "learn", cid, str(image), "--user-data", str(rust_dir)],
                capture_output=True, text=True, encoding="utf-8", timeout=120,
            )
            assert result.returncode == 0, (name, repeat, result.stderr)
            assert json.loads(result.stdout)["message"] in (
                "すでに確定できるので、追加しません", "近いハッシュがすでにあるので、追加しません", "照合用ハッシュを追加しました"
            )
            a = python_dir / "card_hashes_user.json"
            b = rust_dir / "card_hashes_user.json"
            assert a.exists() == b.exists(), (name, repeat)
            if a.exists():
                assert json.loads(a.read_text(encoding="utf-8")) == json.loads(b.read_text(encoding="utf-8")), (name, repeat)
    print(json.dumps({"cid": cid, "scenarios": list(scenarios), "matching": True}, ensure_ascii=False))


if __name__ == "__main__":
    main()
