"""ビルド済みの mdript.exe から配布物（zip と SHA256SUMS.txt）を作る。

    python tools/package_release.py [--tag v2.0.0-beta.1] [--out dist]

先に `npm run tauri -- build --no-bundle`（gui/ で）で exe を作っておく。
公開（リリースへの添付）はしない。作った物は CI の artifact として確認する。
"""

import argparse
import hashlib
import sys
import tempfile
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import check_release_version
import third_party_notices

ROOT = Path(__file__).resolve().parent.parent
EXE = ROOT / "gui/src-tauri/target/release/mdript.exe"

README = r"""マスターデュエル カード画像ツール（mdript） {version}

遊戯王 マスターデュエルのスクリーンショットからカード画像を切り抜いて、カード名で保存する Windows 向けの非公式ツールです。
株式会社KONAMIおよび遊戯王公式とは関係ありません。

■ 起動
mdript.exe をダブルクリックします。インストーラはありません。好きなフォルダに置いて使えます。
使い方は https://github.com/yugioh-hack/mdript の README を参照してください。

■ 動作環境
64ビット版 Windows と、WebView2 Runtime が必要です。動作を確認した環境は Windows 11 のみです。
WebView2 Runtime は多くの環境で導入済みです。起動しない、または画面が表示されないときは、
Microsoft の配布ページ（https://developer.microsoft.com/microsoft-edge/webview2/ ）から
「Evergreen Standalone Installer」を入手してインストールしてください。

■ 署名について
この exe にはコード署名がありません。Windows の SmartScreen が警告を出すことがあります。
SHA256SUMS.txt のハッシュと照合してから実行してください。

  PowerShell: Get-FileHash .\mdript.exe -Algorithm SHA256

■ 保存されるデータ
設定・カード名・登録した照合データは %APPDATA%\mdript-v2 に保存します。
初回起動時に、旧アプリ（v1.x）の %APPDATA%\mdript から設定・カード名・登録データをコピーします。
旧アプリのフォルダは変更しません。コピーできなかったファイルは、%APPDATA%\mdript-v2\legacy-backup に控えます。

■ ライセンス
LICENSE が本ツールのライセンスです。含まれる第三者の部品は THIRD-PARTY-NOTICES.txt を参照してください。
カード画像・カード名などの権利は各権利者に帰属します。
"""


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tag", help="リリースタグ。指定すると製品バージョンとの一致を確かめる")
    parser.add_argument("--out", default="dist", help="出力先フォルダ（既定: dist）")
    args = parser.parse_args()

    version = check_release_version.check(args.tag)
    if not EXE.is_file():
        raise SystemExit(f"{EXE} がありません。先に gui/ で `npm run tauri -- build --no-bundle` を実行してください。")

    out = (ROOT / args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    archive = out / f"mdript-v{version}-windows-x64.zip"
    with tempfile.TemporaryDirectory() as temp:
        notices = Path(temp) / "THIRD-PARTY-NOTICES.txt"
        third_party_notices.main(notices)
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as zf:
            zf.write(EXE, "mdript.exe")
            zf.write(ROOT / "LICENSE", "LICENSE")
            zf.write(notices, "THIRD-PARTY-NOTICES.txt")
            zf.writestr("README.txt", README.format(version=version).replace("\n", "\r\n").encode("utf-8-sig"))

    exe_copy = out / "mdript.exe"
    exe_copy.write_bytes(EXE.read_bytes())
    lines = [f"{sha256(path)}  {path.name}" for path in (archive, exe_copy)]
    (out / "SHA256SUMS.txt").write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
