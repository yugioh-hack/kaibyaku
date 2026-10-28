# Rust／Tauri GUI

Vue 3／TypeScript と Tauri 2 によるWindows用の画面です。使い方・保存先・制限は [ルートのREADME](../README.md)、公開前の評価は [リリース品質確認](../docs/release-readiness-2026-10-23.md) を参照してください。

`gui/` で実行します。

```powershell
npm ci
npm run tauri -- dev
```

```powershell
npm run build
cargo test --manifest-path src-tauri/Cargo.toml --locked
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --locked -- -D warnings
cargo fmt --manifest-path src-tauri/Cargo.toml --all --check
npm run tauri -- build --no-bundle
```

exeは `src-tauri/target/release/mdript.exe` に生成します。現設定はインストーラを作りません。WindowsではWebView2が必要です。配布用のzipとSHA-256は、リポジトリのルートで `python tools/package_release.py` を実行して `dist/` に作ります。

単体起動用のexeは上記の `tauri build` で作ってください。直接Cargoを使う場合は `cargo build --manifest-path src-tauri/Cargo.toml --release --locked --features tauri/custom-protocol` が必要です。`cargo build --release` だけでは開発用の `http://localhost:1420` を参照するため、開発サーバーなしでは画面が開きません。

`npm run dev` だけでブラウザに表示した場合はモックを使います。ドラッグ＆ドロップ・実画像処理・保存・公式DB取得の検証にはTauriで起動してください。

ユーザーデータは `%APPDATA%\mdript-v2` に保存します。初回起動時に、旧Python版の `%APPDATA%\mdript` から設定・カード名・登録データをコピーします（`src-tauri/src/migrate.rs`）。旧版のフォルダは変更しません。
