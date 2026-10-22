"""Check byte-identical WebP outputs from Pillow-decoded PNG inputs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

from PIL import Image


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("images", type=Path)
    parser.add_argument("--exe", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    images = root / "png-inputs"
    images.mkdir()
    for source in sorted(args.images.glob("*.jpg")):
        with Image.open(source) as image:
            image.convert("RGB").save(images / (source.stem + ".png"))
    user = root / "isolated-user-data"
    user.mkdir()
    appdata = root / "isolated-appdata"
    appdata.mkdir()
    pyout, rsout = root / "python", root / "rust"
    pyout.mkdir()
    rsout.mkdir()
    env = os.environ.copy()
    env["APPDATA"] = str(appdata)
    env["PYTHONIOENCODING"] = "utf-8"
    script = Path(__file__).with_name("benchmark_e2e.py")
    commands = [
        [sys.executable, str(script), "--python-run", "--images", str(images), "--output", str(pyout),
         "--user-data", str(user), "--workers", "1"],
        [str(args.exe.resolve()), "identify", str(images), "--output", str(rsout), "--user-data", str(user), "--workers", "1"],
    ]
    summaries = []
    for command in commands:
        done = subprocess.run(command, env=env, capture_output=True, text=True, encoding="utf-8", timeout=600)
        assert done.returncode == 0, done.stderr
        summaries.append(json.loads(done.stdout))
    left = sorted(p.relative_to(pyout) for p in pyout.rglob("*.webp"))
    right = sorted(p.relative_to(rsout) for p in rsout.rglob("*.webp"))
    assert left == right
    differences = []
    for relative in left:
        a = hashlib.sha256((pyout / relative).read_bytes()).digest()
        b = hashlib.sha256((rsout / relative).read_bytes()).digest()
        if a != b:
            differences.append(str(relative))
    report = {"input_count": len(list(images.glob("*.png"))), "output_count": len(left),
              "byte_differences": differences,
              "python_counts": {key: summaries[0][key] for key in ("confirmed", "not_card", "review", "error", "skipped")},
              "rust_counts": {key: summaries[1][key] for key in ("confirmed", "not_card", "review", "error", "skipped")}}
    (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(report, ensure_ascii=False))
    return 0 if not differences and report["python_counts"] == report["rust_counts"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
