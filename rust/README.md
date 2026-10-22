# Rustコア・CLI試作

現行Python版の照合・切り抜き・保存を段階移植するCargo workspace。`mdript-core`はPillow互換ハッシュ、DB順位付け、言語判別、枠検出・探索を担当し、`mdript-cli`は画像入力、結果JSON、保存を担当します。画像の判定と書き出しは複数ワーカーで処理できます。

## ビルド

Windows 11 / Rust 1.99 MSVCで確認。リポジトリのルートから:

```powershell
cargo build --release --manifest-path rust/Cargo.toml --locked --bin mdript-cli
```

並列処理のpanicから復帰して予約を解放するため、dev/releaseとも`panic=unwind`が必須で、`panic=abort`ではビルドを拒否します。

## 画像を処理する

```powershell
rust/target/release/mdript-cli.exe identify C:/images --output C:/tmp/mdript-rust-output --user-data C:/tmp/mdript-rust-user --format both
```

`--output`と`--user-data`には**異なる絶対ディレクトリ**を指定します。後者は省略可能なコピー済み`card_names.json`と`card_hashes_user.json`の読み取り先で、存在しなくても動きます。普段の`%APPDATA%/mdript`は読み書きせず、その場所を指定しても拒否します。

画像1枚の照合用ハッシュは、隔離したユーザーデータへ追加できます:

```powershell
rust/target/release/mdript-cli.exe learn 4138 C:/images/shot.png --user-data C:/tmp/mdript-rust-user
```

学習はPython版と同じtrusted枠だけを使い、既に確定できるか同じハッシュがあれば追加しません。保存時は同梱DBとの差分だけを形式2のJSONに一時ファイル経由で原子的に置き換えます。同一プロセス内の学習はmutexで直列化し、ファイルの破損と更新の消失を防ぎます。複数プロセス間の同時学習は未調整です。

既定はWebP品質90、角丸透過、再帰、重複スキップ、要確認・カードなしを保存、言語判別と日本語/英語名、`log.csv`への追記。ワーカー数の既定は`min(8, max(2, 論理CPU数))`で、`--workers 1..256`で指定できます。`--workers 1`は逐次処理です。`--format png|webp|both`、`--quality 1..100`、`--resolution auto|fixed|WxH`、`--english name|ja|off`、`--split none|en|both`、`--no-rounded`、`--no-recursive`、`--keep-duplicates`、`--no-review`、`--no-not-card`、`--no-csv`を使用できます。`--card`は画像全体をカードとして、`--box L T R B`は明示した枠を使います。`--dry-run`は判定だけ行い、出力ディレクトリ・画像・CSVを作りません。

`log.csv`はPython版と同じ9列、UTF-8 BOM、CRLF、追記形式です。入力値が表計算ソフトで数式にならないよう先頭の記号を保護します。CSVの書き込みに失敗しても、画像の処理結果JSONは返します。

標準出力は`confirmed/review/not_card/error/skipped/seconds/results`を含むJSONです。各結果にはPython版と同じ状態・ID・名前・距離・2位との差・保存先・備考・言語に加え、枠、枠の信頼区分、ハッシュ、候補、言語比を含みます。入力画像のエラーがあっても他の画像を処理し、結果一覧を返して終了コード1にします。画像判定は並列化し、保存名の予約・重複判定は入力順に進めます。先行画像の書き出しは待たず、同名の予約が残る場合のみ完了を待ちます。保存失敗や判定・書き出し中のpanicはその画像のエラー結果に変え、順番待ちと予約を解放します。panicで中断した画像の予約ファイルも清掃します。結果JSONとCSVの行順は入力順です。

従来の読み取り専用ハッシュ調査コマンドも使えます:

```powershell
rust/target/release/mdript-cli.exe C:/images/shot.jpg --center
rust/target/release/mdript-cli.exe C:/images/crop.png --card --coarse
```

## 検証

```powershell
cargo test --manifest-path rust/Cargo.toml --workspace --locked
cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets --locked -- -D warnings
cargo fmt --manifest-path rust/Cargo.toml --all --check
```

Rustの33テストと`cargo fmt --all --check`、`cargo clippy --workspace --all-targets --locked -- -D warnings`は通過。panicを差し込むテストは子プロセスに10秒の制限を設け、判定・保存・mutex poison後も後続の画像とJSONが返ることを確認します。既存のPython版75テストも移植前の検証で通過しています。独立レビュー用の`tools/review_reference.py`、`review_compare.py`、`review_scenarios.py`、`review_safety.py`で次を再確認しました。

- 実スクリーンショット256枚: 状態・カードID・名前・言語・備考は全件一致。距離か2位との差だけ8枚で不一致（最大4）。主因はJPEGデコードの画素差。全件の厳密一致判定は248/256。
- JPEG差の境界分析: 距離16から3以内の画像は140枚ですが、確定判定を変える距離32・差4などの境界から3以内は0枚。状態変更までの最小変化量は5で、元画像256件と720p・JPEG50・ウィンドウ模擬12件に状態の入れ替わりはありません。全画像の値と境界距離は`rust/jpeg-boundary-report.json`、一覧と解釈は`docs/rust-prototype-decision.md`に記録しています。
- 4枚を720p、JPEG50、ウィンドウ模擬へ変形した12ケース: 全項目一致（11確定、1カードなし）。
- 合成入力の5設定×12画像: 状態・ID・名前・保存名・出力画像の復号画素が全件一致。特殊な中央ハッシュ、枠色、英語版、重複、壊れた画像、角丸、both形式を含む。
- 35枚の日英ペアを含む画像群: `--english name|ja|off`と`--split none|en`の6設定でPython版と保存名・備考・スキップ件数が一致。`off`では英語画像に`_alt1`が付き、スキップは0件。
- 合成画像の学習: 新規・既存パック情報・旧形式・破損JSONの4設定でPython版と保存JSONが一致。同一プロセスで2件同時に学習してもJSONは有効で、ハッシュは1件だけ追加。
- 引数拒否と4プロセスの同時保存など8項目は通過。

今回の比較結果の詳細はGit管理外の`rust/local-results/parallel-review/`に保存（Python参照結果は`rust/local-results/current/reference.json`）。実画像の比較スクリプトは、既知のJPEG差8件があるため終了コード1を返します。Rust CLI自体は終了コード0で256件を処理しました。JPEGの全画素一致はまだ達成していません。試したlibjpeg-turbo Rust連携はこの環境にCMakeがないためビルドできず、完成コードには入れていません。

256枚の通し処理もAC接続・高パフォーマンス電源プランで再計測しました。1ワーカーではPython中央値57.98秒、Rust中央値31.49秒（1.84倍）、既定の8ワーカーでは14.11秒と7.49秒（1.88倍）でした。起動から終了まで、ウォームアップ後に各5回測り、毎回空の出力先を使用しました。状態件数は確定256・重複スキップ4、保存名252件が全条件で一致。測定20回のJSON・CSVも、日時・出力先パスと既知のJPEG差8件の距離・2位との差を除いて一致しました。5回の時間に25%超の外れ値はありません。初回はPCが遅い状態だった可能性があり、当時の全測定値は`rust/benchmark-e2e.json`内に保持しています。計測結果と解釈は`docs/rust-prototype-decision.md`に記録しています。

単色・ほぼ単色の領域は DCT 係数が丸め誤差だけになるため、Python 版とハッシュが一致しない（照合結果への影響はなし）

エラー時の結果 JSON・CSV の備考は、Python 版の「例外の型名: メッセージ」と文言が異なります。画像処理の成否や他の入力の処理継続には影響しません。

16bit PNGはPillow 12.2.0の読み込み・`convert("RGB")`結果に合わせています。グレースケール（PNG color type 0、Pillow `I;16`）は各値を255で切り捨て、LA・RGB・RGBA（color type 4・2・6、Pillowが8bit `RGBA`/`RGB`として開く）は下位8bitを落とし、アルファを無視します。各形式について0〜255の値と0〜65535の値で固定PNGを作り、Pythonの`artwork_phash_hex(Image.open(p).convert("RGB"))`と同じハッシュになることを確認しました。これら以外の16bit PNG形式・メタデータの組み合わせは未確認です。既存DBとの完全互換や製品版の速度は保証しません。

## 未実装

公式DBからの名前・ハッシュ更新、Tauri/Vue GUIは未実装。ユーザーデータを製品版でどう移行するかも今後決めます。通し処理の速度は測定済みですが、JPEG差8枚の厳密一致、GUIの起動時間・配布サイズ・メモリは未確認です。これらを確認してから置き換えを判断します。

縮小・DCT実装の出発点は利用者提供の「mdriptのRust検討案.md」。Pillow Resample.c由来コードの許諾表示は`third-party/Pillow-LICENSE.txt`にあります。
