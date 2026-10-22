# Rust試作の実測と採用判断

2026-10-21。判断: **Rustの段階的な移植開発を採用する。現行版の置き換え・公開は保留。**
WebP圧縮とPNG読込に実測の改善があり、同じ画素を入力したハッシュと圧縮結果も一致した。一方、JPEGデコードに起因するハッシュ差が残る。Rust CLIの通し処理は下に追記した。GUIの起動・Tauriの容量は未測定。

## 条件

AMD Ryzen 7 5825U（8コア16スレッド）、Windows 11、Rust 1.99.0 release（opt-level=3、LTO）、Python 3.14.7、Pillow 12.2.0、NumPy 2.4.5。
Rustはimage 0.25.10、webp 0.3.1、libwebp-sys 0.9.6。Pillowのlibwebpは1.6.0。
手元の256画像を名前順に並べ、均等間隔の12枚を選択。すべて2560×1440のJPEG。
PNG読込は、その12枚をPythonでRGB化してPNGに変換したものを用いた。
1回ウォームアップ後5回測り、画像ごとの中央値を12枚分合算。Python→Rustの順で実行。
画像読込はファイル内容をメモリへ読み込んでから計測し、WebP圧縮もメモリへ出力する。WebPは同一の角丸RGBA画像、品質90・method 4。
ビルドと競合した予備実行は採用せず、ビルド完了後の実行を下表に使用した。CPU温度・電力状態やOSの他プロセスは固定しておらず、統計的な性能保証ではない。

## 結果

12枚分の工程中央値の合計。倍率はPython時間÷Rust時間。

| 工程 | Python | Rust | 倍率 |
|---|---:|---:|---:|
| JPEG読込・RGB化 | 168.24 ms | 151.58 ms | 1.11倍 |
| PNG読込・RGB化 | 538.53 ms | 271.28 ms | 1.99倍 |
| 上下128bitハッシュ（同じRGB入力） | 21.11 ms | 15.73 ms | 1.34倍 |
| WebP圧縮（同じRGBA入力） | 2,284.84 ms | 1,180.57 ms | 1.94倍 |

JPEG読込＋ハッシュ＋WebPの工程合計は2,474.18→1,347.88ms（1.84倍）。PNG読込の場合は2,844.47→1,467.58ms（1.94倍）。これは別々に測った工程の合算であり、アプリの通し処理を計測したものではない。照合・探索・言語判別・角丸生成・ディスク保存・重複・GUI・並列実行を含まない。採用後の全体速度として外挿しない。

12枚ともWebPが**バイト単位で一致**し、復号後のRGBAも一致。同じ入力画素での128bitハッシュも12/12一致した。
前段の256枚×3モード検証では、同一画素なら768/768一致。ただし元JPEGをそれぞれのデコーダーで読むと13画像・29ケースで最大2bitの差がある。今回の同一画素での一致はこの問題を解決したものではない。

## 採用範囲と次の条件

- RustコアとCLIの移植開発を進めることは採用する。特にWebP保存に有望な改善がある。
- 速度だけを目的とした全面移植の確定判断は、通し計測を含めても保留。画像コーデックの実装・ビルド差も含む測定なので、「Rustという言語だけの効果」とは扱わない。
- JPEGのデコーダー差を解消するか、全検証画像でカードID・状態・距離・margin・保存結果への影響を検証し、許容基準を定める。厳密なPython互換を求めるなら現在のままでは不合格。
- 現行の安全性修正を含む照合・保存を実装し、同じ入力・DB・出力条件で1 workerと既定workerの通し処理を測定した。結果は以下の追記を参照。
- Tauri/Web UIは未実装であり、今回の測定だけでは採用可否を確定しない。Rust CLIの互換性確認後に、GUIの操作可能までの起動時間・配布サイズ・メモリを比較する。

## 再現

リポジトリのルートから実行する。

```powershell
cargo build --release --manifest-path rust/Cargo.toml --bin stage-bench
python rust/tools/benchmark_stages.py C:/dev/mdript-sample-img --output rust/local-results/benchmark
```

計測中はビルド等を並行実行しない。原画像とユーザーデータは変更しない。生成画像はGit管理外のlocal-resultsに保存する。
生の各回タイミング・入力画像のSHA-256・一致確認は `rust/benchmark-result.json` に保存。比較スクリプトは終了コード0だけで一致を保証せず、JSONのchecksを確認する。
Rustテスト4件・Clippyは成功。今回追加したベンチマークは固定されたローカル入力用であり、製品CLIの安全な入力処理を代替しない。

## 初回: 256枚の通し処理（2026-10-22）

現行Python版の`Cropper.process`とRust release版CLIを、同じ256枚の実画像・WebP品質90・角丸・重複スキップ・同じ空のユーザーデータで比較した。1ワーカーと各版の既定ワーカー（いずれも8）を使用。起動から終了までを計測し、毎回空の出力先を用意した。各条件につきウォームアップ1回の後、5回測定した。4条件を交互に実行し、ビルド等は並行しなかった。

計測PCはAMD Ryzen 7 5825U（8コア/16論理CPU）、Windows 11、Windowsの電源プランは「バランス」、Python 3.14.7、Rust 1.99.0 release。OSの他プロセス、CPU温度、電力状態は固定していない。値は秒で、倍率は同じワーカー条件のPython中央値÷Rust中央値。

| ワーカー | Python中央値（最小〜最大） | Rust中央値（最小〜最大） | 倍率 |
|---|---:|---:|---:|
| 1 | 131.11（61.02〜132.17） | 72.62（30.67〜73.29） | 1.81倍 |
| 既定の8 | 23.39（11.41〜24.47） | 11.31（6.35〜11.57） | 2.07倍 |

最終測定回は4条件とも、それまでの約半分の時間になった。PCが遅い状態（電力・温度などの制限）で前半を測った可能性がある。原因は特定できていない。この初回の全5回の値は`rust/benchmark-e2e.json`の`previous_measurement`に残した。

全24実行（4条件×ウォームアップ1回＋測定5回）で保存ファイルは252件だった。測定20回では状態が確定256・重複スキップ4・その他0で、ファイル名、結果JSONの入力順、`log.csv`の行順・内容は、日時と保存先の絶対パスを除いて一致。RustとPythonの既知のJPEG差8枚は距離・2位との差だけを例外として照合し、新たな差はなかった。画像のWebPバイト列まではこの通し比較の判定対象にしていない。保存出力の代表回と各回の照合結果は`rust/local-results/e2e-20261022-run2/`と`rust/benchmark-e2e.json`にある。

```powershell
cargo build --release --manifest-path rust/Cargo.toml --locked --bin mdript-cli
python rust/tools/benchmark_e2e.py C:/dev/mdript-sample-img --exe rust/target/release/mdript-cli.exe --output rust/local-results/e2e-20261022-run2 --report rust/benchmark-e2e.json --known rust/local-results/parallel-review/report.json
```

この結果はRustコアとCLIの開発継続を支持する。Python版の置き換え判断には、残るJPEG差8枚の許容基準とGUIの起動・配布サイズ・メモリ比較が必要。

## 再計測: AC接続・高パフォーマンス（2026-10-22）

同じPC・256枚・4条件、同じ出力設定で測り直した。AC接続をWindows APIで確認し、電源プランを「バランス」から「高パフォーマンス」へ切り替えて計測した。終了後に元の「バランス」へ復元できた。各条件はウォームアップ1回＋実測5回、毎回空の保存先を使用し、4条件を交互に実行。起動から終了まで測り、ビルド等は並行しなかった。

| ワーカー | Python中央値（最小〜最大） | Rust中央値（最小〜最大） | 倍率 |
|---|---:|---:|---:|
| 1 | 57.98（56.61〜60.59）秒 | 31.49（31.16〜33.28）秒 | 1.84倍 |
| 既定の8 | 14.11（13.55〜14.91）秒 | 7.49（7.33〜7.61）秒 | 1.88倍 |

各実行の直前・直後にWindowsの`Processor Information(_Total)\% Processor Performance`を読み取り、AC接続状態と共に`rust/benchmark-e2e.json`へ記録した。これはCPUの相対性能の瞬時値で、処理中の平均クロックや温度ではない。温度は取得できなかった。各条件の5回中央値から25%超離れた時間を異常値とする基準では、該当回は0。除外した回もない。

全24実行で保存ファイルは252件、確定256・重複スキップ4・その他0。測定20回の保存名、状態件数、JSONの入力順、CSVの行順と内容は、日時・保存先の絶対パス・既知のJPEG差8枚の距離と2位との差を除いて一致した。新たな差はない。計測の生値・各回のCPU指標・比較結果と初回の生値は`rust/benchmark-e2e.json`に、代表回の出力はGit管理外の`rust/local-results/e2e-20261022-ac/`にある。

```powershell
python rust/tools/benchmark_e2e.py C:/dev/mdript-sample-img --exe rust/target/release/mdript-cli.exe --output rust/local-results/e2e-20261022-ac --report rust/benchmark-e2e.json --known rust/local-results/panic-guard-review/report.json
```

初回は低速状態だった可能性があり、現時点の速度比較には再計測値を使う。ただし電源プラン以外の環境要因は固定しておらず、速度差はRustという言語だけの効果ではない。
