"""配布物に入れる第三者ライセンス一覧（THIRD-PARTY-NOTICES.txt）を作る。

    python tools/third_party_notices.py OUTPUT.txt

新アプリ（gui/src-tauri）が Windows 向けにリンクする Rust のクレートと、画面に組み込む npm の本番依存を対象にする。
各パッケージのライセンス表記と、パッケージに同梱されているライセンス文書を載せる。
"""

import hashlib
import json
import subprocess
import sys
from pathlib import Path

for stream in (sys.stdout, sys.stderr):
    stream.reconfigure(encoding="utf-8")

ROOT = Path(__file__).resolve().parent.parent
LICENSE_NAMES = ("license", "licence", "copying", "notice", "unlicense")


# クレートにライセンス文が同梱されていないものは、上流のリポジトリから取った文を置いて補う
SUPPLEMENT = ROOT / "rust/third-party/supplement"
SUPPLEMENTED = {
    "alloc-stdlib": "alloc-stdlib.txt",
    "selectors": "selectors.txt",
    "webview2-com": "webview2-com.txt",
    "webview2-com-macros": "webview2-com.txt",
    "webview2-com-sys": "webview2-com.txt",
}


def license_files(directory: Path) -> list[Path]:
    found: list[Path] = []
    # libwebp-sys のように、同梱した外部ソースのライセンスが vendor/ にあるものも拾う
    for folder in (directory, directory / "vendor"):
        if folder.is_dir():
            found += sorted(
                p for p in folder.iterdir() if p.is_file() and p.name.lower().startswith(LICENSE_NAMES) and p.stat().st_size < 200_000
            )
    return found


def rust_packages() -> list[dict]:
    manifest = ROOT / "gui/src-tauri/Cargo.toml"
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--manifest-path", str(manifest), "--filter-platform", "x86_64-pc-windows-msvc"],
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        ).stdout
    )
    by_id = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = meta["resolve"]["root"]
    seen, stack = set(), [root]
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        for dep in nodes[current]["deps"]:
            # ビルド専用（kind == "build"）・開発専用の依存は実行ファイルに入らない
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    packages = [by_id[i] for i in seen if i != root]
    # 同じリポジトリ内のこのプロジェクト自身のクレートは除く
    return sorted(
        (p for p in packages if p["source"] is not None),
        key=lambda p: (p["name"], p["version"]),
    )


def npm_packages() -> list[dict]:
    lock = json.loads((ROOT / "gui/package-lock.json").read_text("utf-8"))
    out = []
    for path, info in lock["packages"].items():
        if not path or info.get("dev") or info.get("devOptional") or info.get("optional"):
            continue
        directory = ROOT / "gui" / path
        out.append(
            {
                "name": path.split("node_modules/")[-1],
                "version": info["version"],
                "license": info.get("license", "UNKNOWN"),
                "directory": directory,
            }
        )
    return sorted(out, key=lambda p: (p["name"], p["version"]))


def main(output: Path) -> None:
    sections: list[str] = []
    texts: dict[str, str] = {}

    def add_text(text: str) -> str:
        key = hashlib.sha256(text.encode("utf-8")).hexdigest()[:12]
        texts.setdefault(key, text)
        return key

    missing: list[str] = []

    def entry(kind: str, name: str, version: str, license_id: str, repository: str | None, directory: Path) -> None:
        keys = []
        files = license_files(directory)
        if not files and name in SUPPLEMENTED:
            files = [SUPPLEMENT / SUPPLEMENTED[name]]
        if not files:
            missing.append(f"{name} {version}")
        for path in files:
            keys.append(add_text(path.read_text("utf-8", errors="replace").strip()))
        line = f"- [{kind}] {name} {version} — {license_id or 'UNKNOWN'}"
        if repository:
            line += f" — {repository}"
        if keys:
            line += "\n    ライセンス文: " + ", ".join(f"#{k}" for k in keys)
        else:
            line += "\n    ライセンス文: なし"
        sections.append(line)

    for package in rust_packages():
        entry("Rust", package["name"], package["version"], package["license"], package.get("repository"), Path(package["manifest_path"]).parent)
    for package in npm_packages():
        entry("npm", package["name"], package["version"], package["license"], None, package["directory"])

    if missing:
        raise SystemExit("ライセンス文がない依存があります。rust/third-party/supplement/ に補ってください:\n  " + "\n  ".join(missing))

    header = [
        "第三者ライセンス（開闢 - MDカード画像ツール / mdript）",
        "",
        "このアプリ自身のライセンスは同梱の LICENSE（MIT）です。",
        "以下は、実行ファイルに含まれる（または画面に組み込まれる）部品です。",
        "",
        "■ Pillow（角丸の描画処理の由来）",
        (ROOT / "rust/third-party/Pillow-LICENSE.txt").read_text("utf-8").strip(),
        "",
        "■ 部品一覧",
    ]
    body = ["", "■ ライセンス文（同じ文は1回だけ載せています）"]
    for key, text in sorted(texts.items()):
        body += ["", f"#{key}", "-" * 60, text]
    output.write_text("\n".join(header + sections + body) + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main(Path(sys.argv[1]))
