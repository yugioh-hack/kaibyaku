"""新アプリの製品バージョンが各ファイルで一致していること（と、指定したタグと一致すること）を確かめる。

    python tools/check_release_version.py [--tag v2.0.0-beta.1]

旧Pythonアプリ（v1.x）のタグは受け付けない。
"""

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

for stream in (sys.stdout, sys.stderr):
    stream.reconfigure(encoding="utf-8")

ROOT = Path(__file__).resolve().parent.parent
SEMVER = re.compile(r"^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$")


def versions() -> dict[str, str]:
    gui = ROOT / "gui"
    found = {
        "gui/src-tauri/tauri.conf.json": json.loads((gui / "src-tauri/tauri.conf.json").read_text("utf-8"))["version"],
        "gui/src-tauri/Cargo.toml": tomllib.loads((gui / "src-tauri/Cargo.toml").read_text("utf-8"))["package"]["version"],
        "gui/package.json": json.loads((gui / "package.json").read_text("utf-8"))["version"],
    }
    lock = json.loads((gui / "package-lock.json").read_text("utf-8"))
    found["gui/package-lock.json"] = lock["version"]
    found["gui/package-lock.json (packages[''])"] = lock["packages"][""]["version"]
    cargo_lock = tomllib.loads((gui / "src-tauri/Cargo.lock").read_text("utf-8"))
    [entry] = [p for p in cargo_lock["package"] if p["name"] == "mdript-gui"]
    found["gui/src-tauri/Cargo.lock"] = entry["version"]
    return found


def check(tag: str | None) -> str:
    found = versions()
    unique = set(found.values())
    if len(unique) != 1:
        lines = "\n".join(f"  {name}: {value}" for name, value in found.items())
        raise SystemExit(f"製品バージョンがファイルごとに違います。\n{lines}")
    (version,) = unique
    if not SEMVER.match(version):
        raise SystemExit(f"バージョン {version} は X.Y.Z（または X.Y.Z-prerelease）の形式ではありません。")
    if tag is not None:
        if not tag.startswith("v") or tag[1:] != version:
            raise SystemExit(f"タグ {tag} が製品バージョン v{version} と一致しません。")
        if int(version.split(".")[0]) < 2:
            raise SystemExit(f"タグ {tag} は旧アプリの版番号です。新アプリは v2 以降で配布します。")
    return version


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="リリースタグ（例: v2.0.0-beta.1）。省略するとファイル間の一致だけ見る")
    args = parser.parse_args()
    print(check(args.tag))
    sys.exit(0)
