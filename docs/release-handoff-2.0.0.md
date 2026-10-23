# 開闢 2.0.0 の公開手順（main への統合から公開まで）

作成日: 2026-10-24。この手順は未実行。mainへのマージ、push、タグ作成、公開は、まだ行っていない。強制push・`reset --hard` は使わない。

## 現在のリポジトリの状態（2026-10-24）

| 場所 | 内容 |
| --- | --- |
| `origin/main` = `b6162f3` | 旧アプリ（Python版）のソースに、READMEだけをベータ版向けに更新した履歴 |
| `claude/tauri-ui`（`origin` と同じ） | 新アプリ。`main` の `ce949e8` から約40コミット。`b6162f3` を含まない |
| PR #1（`claude/tauri-ui` → `main`） | **`README.md` が競合**（`CONFLICTING`）。ほかのファイルは競合しない |
| ローカル `main`（`C:\dev\mdript`） | `ce949e8`。`origin/main` に対して、1つ進み（`ce949e8`）、1つ遅れている（`b6162f3`） |
| ローカル `main` の未コミット | `README.md`（`origin/main` の `README.md` と同じ内容）、未追跡の `docs/images/`・`docs/release-readiness-2026-10-23.md`（ブランチに入っているものの古い写し） |
| タグ | `v1.4.1`（旧アプリ）、`v2.0.0-beta.1`（プレリリース、`e990d69`） |
| Releases | `v1.4.1` が Latest、`v2.0.0-beta.1` がプレリリース |

競合の原因は、`origin/main` の `README.md` と、ブランチの `README.md` が、同じ元（`ce949e8` と `37b9b87`）から別々に書き換えられていること。新しい `README.md` はブランチのものを採用すれば足りる。

## 公開の順序で守ること

- **READMEのダウンロードリンクは `v2.0.0` のリリースを指す。** リンク先のリリースを先に公開してから、READMEを `main` に入れる（マージする）。`main` のREADMEは、いまベータ版のリンクを指しており、これは公開済みで正しい。
- **マージは「Create a merge commit」で行う。** squash は使わない。`v2.0.0` のタグが指す履歴が `main` に残る。
- リリースに添付するzipは、**タグのCIが作った成果物**を使う（ローカルでビルドしたものではなく）。

## 手順

### 0. 準備（公開前）

- [ ] 特許情報プラットフォーム（J-PlatPat）で「開闢」の商標を確認する（第9類・第42類）。調査の範囲は `release-readiness-2026-10-23.md` の「名称」を参照。
- [ ] できれば、開発環境のないWindowsで、CIが作ったzipを展開して起動を確認する（手順4の後に、公開前の最終確認として行ってもよい）。

### 1. 旧アプリの保守先を残す

```powershell
git fetch origin --tags
git branch maintenance/1.x v1.4.1
git push origin maintenance/1.x
```

旧アプリへの修正が要らなければ省略できる。タグ `v1.4.1` が旧アプリの記録として残る。

### 2. `main` の競合を、ブランチ側で解消する

`claude/tauri-ui` のワークツリー（`C:\dev\mdript-tauri`）で行う。

```powershell
cd C:\dev\mdript-tauri
git fetch origin
git switch claude/tauri-ui
git merge origin/main
```

`README.md` だけが競合する。ブランチ側の内容を採用する。

```powershell
git checkout --ours README.md
git add README.md
git commit
git log --oneline -3
git diff origin/main --stat -- . ':!rust' ':!gui' ':!docs' ':!tools' ':!.github'
```

最後の `git diff` で、旧アプリのファイル（`mdript/`・`tests/`・`app.py` など）に意図しない差が出ていないことを確認する。差が出るのは `.gitignore`（ブランチで3行追加）と `README.md` だけのはず（一時的なワークツリーで、この手順を試して確認した）。ここまでで、PR #1 の競合は消える。

### 3. ブランチを push する（PR #1 を更新する）

```powershell
git push origin claude/tauri-ui
```

PR #1 の `mergeStateStatus` が `CLEAN` になり、PRのCIが成功することを確認する。**マージはまだしない。**

### 4. タグを付け、CIの成果物を作る

```powershell
python tools/check_release_version.py --tag v2.0.0
git tag -a v2.0.0 -m "開闢 2.0.0"
git push origin v2.0.0
```

タグのCI（`build`）の全ジョブが成功し、artifact `mdript-windows-x64` ができることを確認する。ジョブの「製品バージョンの確認」が、タグと製品版の一致を検査する。

### 5. CIの成果物を取り出して検証する

```powershell
gh run list --workflow build --limit 3
gh run download <タグのrun-id> -n mdript-windows-x64 -D $env:TEMP\mdript-2.0.0
cd $env:TEMP\mdript-2.0.0
Get-Content SHA256SUMS.txt
Get-FileHash .\mdript-v2.0.0-windows-x64.zip -Algorithm SHA256
```

`SHA256SUMS.txt` とzipのハッシュが一致することを確認し、zipを別フォルダへ展開して起動を確認する。

### 6. リリースを公開する

`docs/release-notes-2.0.0.md` を、リリースノートにする。`SHA256SUMS.txt` には、zipと単体の `mdript.exe` の2行がある。リリースに添付するのはzipと `SHA256SUMS.txt` なので、`mdript.exe` も添付するか、`SHA256SUMS.txt` の `mdript.exe` の行を除く（ベータ版では、添付しなかった `mdript.exe` の行が残った）。

```powershell
gh release create v2.0.0 --verify-tag --latest `
  --title "v2.0.0 開闢" `
  --notes-file docs\release-notes-2.0.0.md `
  $env:TEMP\mdript-2.0.0\mdript-v2.0.0-windows-x64.zip `
  $env:TEMP\mdript-2.0.0\SHA256SUMS.txt
```

公開後、READMEのダウンロードリンク（`.../releases/download/v2.0.0/mdript-v2.0.0-windows-x64.zip`）が開くことを確認する。「Latest」が `v2.0.0` になる（旧アプリのREADMEや記事が、最新のリリースを旧アプリとして案内している場合は、あとで直す）。

### 7. PR #1 をマージする

リリースが公開され、リンクが開くことを確認してから、**「Create a merge commit」**で PR #1 をマージする。`main` のREADMEが、公開済みの `v2.0.0` を指す形になる。

### 8. ローカルの `main` を追従させる

`C:\dev\mdript` の未コミットの変更（`origin/main` と同じ `README.md`、ブランチの古い写しのdocs）は、マージ後に衝突する。内容を確認してから、退避して追従する。

```powershell
cd C:\dev\mdript
git status --short
New-Item -ItemType Directory -Force $env:TEMP\mdript-main-backup | Out-Null
Copy-Item README.md, docs\release-readiness-2026-10-23.md $env:TEMP\mdript-main-backup\
Copy-Item docs\images -Recurse $env:TEMP\mdript-main-backup\
git restore README.md
Remove-Item docs\release-readiness-2026-10-23.md
Remove-Item docs\images -Recurse
git pull --ff-only
```

`git restore README.md` は、`origin/main` と同じ内容の未コミットの変更を捨てるだけで、内容は退避済み。`git pull --ff-only` は、ローカルの `ce949e8` がマージ後の `main` に含まれるので、早送りで進む。進まなければ、そこで止めて確認する（強制はしない）。

### 9. 公開後の確認

- `main` で `python tools/check_release_version.py` が `2.0.0` を返す。
- `main` のCIが成功する。
- ベータ版のプレリリース（`v2.0.0-beta.1`）は、そのまま残す。必要なら、リリースの説明に「2.0.0 が正式版」と追記する。
- 公開した `mdript.exe` の起動、旧アプリのデータの移行を、新規PCで確認し、結果をIssueなどで受ける。

## 戻し方

- 手順6の前: タグ `v2.0.0` を消す（`git push origin :refs/tags/v2.0.0` と `git tag -d v2.0.0`）。リリースを作っていなければ、他に影響はない。
- 手順6の後で、問題が見つかったとき: リリースを下書き（draft）またはプレリリースに戻す。`main` のマージ（手順7）前なら、READMEは変わっていない。マージ後は、`main` のREADMEのダウンロード案内を、修正するPRで直す。
- いずれの場合も、`reset --hard` や強制pushは使わない。
