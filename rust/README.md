# Rustハッシュ互換性試作

現行Python版を残したまま、Rust移植の最初の段階を検証するCargo workspaceです。
`mdript-core`はPillow互換Lanczos縮小・グレースケール・DCT・ハッシュ生成と既定枠の計算、`mdript-cli`は画像読み込みとJSON出力を担当します。

## ビルドと使用

Rust 1.99.0 / Windows MSVCで検証。リポジトリのルートから実行します。

```powershell
cargo build --release --manifest-path rust/Cargo.toml
rust/target/release/mdript-cli.exe "C:/images/screenshot.jpg" --center
rust/target/release/mdript-cli.exe "C:/images/card.png" --card
rust/target/release/mdript-cli.exe "C:/images/screenshot.jpg" --box 833 69 1727 1371 --coarse
```

指定なしでは画面の高さと中央位置からカード枠を計算します。
`--card`は画像全体をカードとして扱い、`--box L T R B`は明示した枠を使います。
ハッシュは通常128bit、`--coarse`は上側64bit、`--center`は上下と中央の192bitです。
`--coarse`と`--center`、`--card`と`--box`の同時指定はエラーです。
エラーは標準エラーにJSONを出し、終了コード1を返します。

## Pythonとの比較

```powershell
python rust/tools/compare_python.py C:/dev/mdript-sample-img --report rust/local-results/sample-comparison.json
```

PillowとNumPyが必要です。全画像を3つのハッシュモードで比較します。
`native_decode`は各言語で元ファイルをデコードした結果、`same_pixels`はPythonでRGB化してPNGにした同じ画素をRustへ渡した結果です。
前者だけが不一致なら、まず画像デコーダーを調べます。どちらかに不一致・入力エラーがある場合は終了コード1です。
比較用PNGは指定レポートの隣の一時ディレクトリに作り、終了時に削除します。元画像とAPPDATAは書き換えません。
詳細レポート・ビルド生成物はGit管理外です。比較時間にはプロセス起動とPNG生成が含まれるため、速度ベンチマークには使えません。

## テスト

```powershell
cd rust
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

既定枠、不正な枠、各モード、Pythonから得た固定期待値をテストします。

## 今回の範囲と未実装

このCLIはハッシュを調べる試作で、既存CLIの置き換えではありません。
カードDB照合、探索・検出、言語判別、切り抜き保存、重複処理、更新、学習、Tauri/Vue GUIはまだありません。
画像は最大幅・高さ16,384、デコード割当上限512MiB、RGB化前の画素数64百万、カード領域16百万画素で制限します。これらはプロセス全体のメモリ上限を保証するものではありません。
200枚一致という引き継ぎ資料の結果を、今回の実測としては扱いません。DCTの浮動小数の加算順やJPEGデコードがPythonと異なるため、すべての画像でのビット一致は保証しません。
次はデコード差を確認してから、現在のPython版を基準に照合と保存を移植します。

## 出典

縮小・DCT実装の出発点は利用者提供の「mdriptのRust検討案.md」の掲載試作です。
縮小処理はPillowのResample.c由来です。著作権・許諾表示は `third-party/Pillow-LICENSE.txt` に同梱しています。

## 2026-10-21の実測

手元の256画像×3モード、768ケースで比較しました。

| 条件 | 一致 | 不一致 |
|---|---:|---:|
| PythonでRGB化した同一画素をPNG経由で渡す | 768/768 | 0 |
| 各実装で元ファイルをデコード | 739/768 | 29ケース（13画像、最大2bit） |

不一致の各ケースも同一画素条件では一致しています。画像デコード経路の差を解決するまでは既存DBとの完全互換を宣言しません。カードの照合結果が変わるかどうかはまだ測定していません。
リリースexeは876,544 bytes（ハッシュCLIのみ）。完成版の容量や速度の見積もりには使えません。
Rustテスト4件、Clippy警告ゼロ、フォーマット確認に合格しました。GitHub Actionsは追加のみで、リモート実行は未確認です。
