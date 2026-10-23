"""Check all English naming and split combinations on an isolated copied image set."""
import argparse
from dataclasses import asdict
import json
import os
from pathlib import Path
import subprocess
import sys
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def python_run(args):
    sys.path.insert(0, str(ROOT))
    from mdript.catalog import Catalog
    from mdript.pipeline import Cropper, Options

    with patch("mdript.catalog.user_data_dir", return_value=str(args.user_data)):
        catalog = Catalog()
    options = Options(output_dir=str(args.output), english=args.english, split=args.split, workers=1)
    print(json.dumps(asdict(Cropper(catalog, options).process([str(args.images)])), ensure_ascii=False))


def details(result, output):
    rows = {}
    for row in result["results"]:
        saved = row["saved_path"]
        rows[Path(row["source"]).name] = {
            "saved": str(Path(saved).relative_to(output)) if saved else "",
            "note": row["note"],
            "status": row["status"],
        }
    return {"rows": rows, "skipped": result["skipped"]}


def main(args):
    root = args.output.resolve()
    root.mkdir(parents=True, exist_ok=False)
    images = args.images.resolve()
    assert images.is_dir() and images.is_relative_to(ROOT / "rust/local-results"), "pass a copied image set in local-results"
    user = root / "isolated-user-data"
    user.mkdir()
    isolated_appdata = root / "isolated-appdata"
    isolated_appdata.mkdir()
    env = os.environ.copy()
    env["APPDATA"] = str(isolated_appdata)
    env["PYTHONIOENCODING"] = "utf-8"
    report = {}
    for english in ("name", "ja", "off"):
        for split in ("none", "en"):
            mode = f"{english}-{split}"
            left = root / mode / "python"
            right = root / mode / "rust"
            left.mkdir(parents=True)
            right.mkdir()
            base = ["--images", str(images), "--user-data", str(user), "--english", english, "--split", split]
            py = subprocess.run(
                [sys.executable, str(Path(__file__).resolve()), "--python-run", *base, "--output", str(left)],
                capture_output=True, text=True, encoding="utf-8", env=env, timeout=600,
            )
            assert py.returncode == 0, py.stderr
            rust = subprocess.run(
                [str(args.exe.resolve()), "identify", str(images), "--output", str(right),
                 "--user-data", str(user), "--english", english, "--split", split, "--workers", "1"],
                capture_output=True, text=True, encoding="utf-8", env=env, timeout=600,
            )
            assert rust.returncode == 0, rust.stderr
            expected = details(json.loads(py.stdout), left)
            actual = details(json.loads(rust.stdout), right)
            assert expected == actual, (mode, expected, actual)
            if english == "off":
                assert expected["skipped"] == 0
                assert sum("_alt1" in row["saved"] for row in expected["rows"].values()) >= 5
            report[mode] = {"matched": True, "skipped": expected["skipped"]}
            print(mode, report[mode], flush=True)
    (root / "report.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--python-run", action="store_true")
    parser.add_argument("--images", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--user-data", type=Path)
    parser.add_argument("--exe", type=Path)
    parser.add_argument("--english", choices=("name", "ja", "off"))
    parser.add_argument("--split", choices=("none", "en"))
    values = parser.parse_args()
    python_run(values) if values.python_run else main(values)
