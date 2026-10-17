# -*- coding: utf-8 -*-
"""ドラッグ＆ドロップで使う GUI。

tkinterdnd2 が入っていればドロップを受け付け、無ければ「ファイルを選ぶ」だけで動く。
重い処理は必ずワーカースレッドに逃がし、画面の更新はキュー越しに行う。
"""
from __future__ import annotations

import os
import queue
import sys
import threading
import tkinter as tk
import traceback
from tkinter import filedialog, messagebox, ttk

from . import __version__, updater
from .catalog import Catalog, load_config, save_config, user_data_dir
from .layout import PRESETS
from .pipeline import CONFIRMED, ERROR, NOT_CARD, REVIEW, Cropper, Options, collect_images

APP_TITLE = "MasterDuel切り抜きツール"

try:                                     # ドラッグ＆ドロップ（無くても動く）
    from tkinterdnd2 import DND_FILES, TkinterDnD
    _DND = True
except Exception:                        # pragma: no cover
    DND_FILES, TkinterDnD, _DND = None, None, False

# 英語版（英語クライアントのスクショ）の名前。画面の表示 -> Options.english
ENGLISH_CHOICES = {
    "英語名で保存": "name",
    "日本語名で保存": "ja",
    "判別しない": "off",
}
# 言語ごとのフォルダ分け。画面の表示 -> Options.split
SPLIT_CHOICES = {
    "分けない": "none",
    "英語版を en に": "en",
    "ja と en に分ける": "both",
}


def _label_of(choices, value, default):
    return next((label for label, v in choices.items() if v == value), default)

STATUS_COLORS = {
    CONFIRMED: "#1b7f3b",
    REVIEW: "#b8860b",
    NOT_CARD: "#777777",
    ERROR: "#c0392b",
}


def parse_drop(root, data: str):
    """ドロップされた文字列をパスの一覧にする。

    Tcl のリスト形式で届く（空白入りのパスは {} で囲まれ、{ } を含む名前は
    エスケープされる）ので、自前で区切らずに Tcl に分解させる。
    """
    return [p for p in root.tk.splitlist(data) if p]


def _create_root():
    """ウィンドウを作り、使えればドラッグ＆ドロップを有効にする。

    TkinterDnD.Tk() は tkdnd を読めないとウィンドウを作ったまま例外になるので、
    通常の Tk に後から tkdnd を読み込む（ドロップ用のメソッドは全ウィジェット共通）。
    """
    global _DND
    root = tk.Tk()
    if _DND:
        try:
            TkinterDnD._require(root)
        except (tk.TclError, RuntimeError):   # exe に tkdnd が入っていない場合など
            _DND = False
    return root


class App:
    def __init__(self):
        self.root = _create_root()
        self.root.title(f"{APP_TITLE} v{__version__}")
        self.root.geometry("880x620")
        self.root.minsize(760, 520)

        self.config = load_config()
        self.catalog = Catalog()
        self.queue = queue.Queue()
        self.worker = None
        self.stop_flag = threading.Event()

        self._build_vars()
        self._build_widgets()
        self._refresh_data_label()
        self.root.after(80, self._drain_queue)
        self.root.after(300, self._offer_first_fetch)

    # ------------------------------------------------------------------
    def _build_vars(self):
        cfg = self.config
        default_out = cfg.get("output_dir") or os.path.join(
            os.path.expanduser("~"), "Pictures", "mdript")
        self.var_output = tk.StringVar(value=default_out)
        self.var_resolution = tk.StringVar(value=cfg.get("resolution", "自動"))
        self.var_format = tk.StringVar(value=cfg.get("save_format", "webp"))
        self.var_rounded = tk.BooleanVar(value=cfg.get("rounded", True))
        self.var_recursive = tk.BooleanVar(value=cfg.get("recursive", True))
        self.var_skip_dup = tk.BooleanVar(value=cfg.get("skip_duplicate", True))
        self.var_save_review = tk.BooleanVar(value=cfg.get("save_review", True))
        self.var_save_not_card = tk.BooleanVar(value=cfg.get("save_not_card", True))
        self.var_english = tk.StringVar(
            value=_label_of(ENGLISH_CHOICES, cfg.get("english", "name"), "英語名で保存"))
        self.var_split = tk.StringVar(
            value=_label_of(SPLIT_CHOICES, cfg.get("split", "none"), "分けない"))
        self.var_status = tk.StringVar(value="画像かフォルダをここにドロップしてください")

    def _build_widgets(self):
        root = self.root
        pad = {"padx": 8, "pady": 4}

        # --- データの状態 ---
        top = ttk.Frame(root)
        top.pack(fill="x", **pad)
        self.data_label = ttk.Label(top, text="")
        self.data_label.pack(side="left")
        ttk.Button(top, text="カードデータ更新…",
                   command=self._open_update_dialog).pack(side="right")

        # --- ドロップ領域 ---
        self.drop = tk.Label(
            root, relief="ridge", borderwidth=2, height=6,
            text=("ここにスクリーンショット（またはフォルダ）をドロップ"
                  if _DND else "「ファイルを選ぶ」または「フォルダを選ぶ」から追加してください"),
            bg="#f4f6f8", fg="#333333")
        self.drop.pack(fill="both", expand=False, **pad)
        if _DND:
            self.drop.drop_target_register(DND_FILES)
            self.drop.dnd_bind("<<Drop>>", self._on_drop)
        self.drop.bind("<Button-1>", lambda _e: self._choose_files())

        buttons = ttk.Frame(root)
        buttons.pack(fill="x", **pad)
        ttk.Button(buttons, text="ファイルを選ぶ", command=self._choose_files).pack(side="left")
        ttk.Button(buttons, text="フォルダを選ぶ", command=self._choose_folder).pack(side="left", padx=4)
        self.stop_button = ttk.Button(buttons, text="中止", command=self._stop, state="disabled")
        self.stop_button.pack(side="right")

        # --- 出力先 ---
        out = ttk.LabelFrame(root, text="出力先")
        out.pack(fill="x", **pad)
        ttk.Entry(out, textvariable=self.var_output).pack(side="left", fill="x", expand=True,
                                                          padx=6, pady=6)
        ttk.Button(out, text="変更", command=self._choose_output).pack(side="left", padx=2)
        ttk.Button(out, text="開く", command=self._open_output).pack(side="left", padx=6)

        # --- 設定 ---
        opt = ttk.LabelFrame(root, text="設定")
        opt.pack(fill="x", **pad)
        row = ttk.Frame(opt)
        row.pack(fill="x", padx=6, pady=4)
        ttk.Label(row, text="解像度").pack(side="left")
        ttk.Combobox(row, textvariable=self.var_resolution, width=18, state="readonly",
                     values=["自動"] + list(PRESETS)).pack(side="left", padx=4)
        ttk.Label(row, text="形式").pack(side="left", padx=(12, 0))
        ttk.Combobox(row, textvariable=self.var_format, width=8, state="readonly",
                     values=["webp", "png", "both"]).pack(side="left", padx=4)
        ttk.Checkbutton(row, text="角丸・透過", variable=self.var_rounded).pack(side="left", padx=8)
        row2 = ttk.Frame(opt)
        row2.pack(fill="x", padx=6, pady=(0, 6))
        ttk.Checkbutton(row2, text="サブフォルダも処理", variable=self.var_recursive).pack(side="left")
        ttk.Checkbutton(row2, text="同じイラストは保存しない",
                        variable=self.var_skip_dup).pack(side="left", padx=12)
        ttk.Checkbutton(row2, text="要確認も保存する（review）",
                        variable=self.var_save_review).pack(side="left")
        ttk.Checkbutton(row2, text="判定できない画像も残す（not_card）",
                        variable=self.var_save_not_card).pack(side="left", padx=12)
        row3 = ttk.Frame(opt)
        row3.pack(fill="x", padx=6, pady=(0, 6))
        ttk.Label(row3, text="英語版").pack(side="left")
        ttk.Combobox(row3, textvariable=self.var_english, width=16, state="readonly",
                     values=list(ENGLISH_CHOICES)).pack(side="left", padx=4)
        ttk.Label(row3, text="言語でフォルダを").pack(side="left", padx=(12, 0))
        ttk.Combobox(row3, textvariable=self.var_split, width=16, state="readonly",
                     values=list(SPLIT_CHOICES)).pack(side="left", padx=4)

        # --- 進捗 ---
        prog = ttk.Frame(root)
        prog.pack(fill="x", **pad)
        self.progress = ttk.Progressbar(prog, mode="determinate")
        self.progress.pack(fill="x", side="top")
        ttk.Label(prog, textvariable=self.var_status).pack(anchor="w", pady=(4, 0))

        # --- 結果一覧 ---
        table = ttk.Frame(root)
        table.pack(fill="both", expand=True, **pad)
        columns = ("status", "name", "diff", "file")
        self.tree = ttk.Treeview(table, columns=columns, show="headings", height=10)
        for key, text, width in (("status", "状態", 80), ("name", "カード名", 260),
                                 ("diff", "距離", 60), ("file", "元ファイル", 380)):
            self.tree.heading(key, text=text)
            self.tree.column(key, width=width, anchor="w")
        scroll = ttk.Scrollbar(table, orient="vertical", command=self.tree.yview)
        self.tree.configure(yscrollcommand=scroll.set)
        self.tree.pack(side="left", fill="both", expand=True)
        scroll.pack(side="right", fill="y")
        for status, color in STATUS_COLORS.items():
            self.tree.tag_configure(status, foreground=color)

        root.protocol("WM_DELETE_WINDOW", self._on_close)

    # ------------------------------------------------------------------
    def _refresh_data_label(self):
        names = (f"カード名 {len(self.catalog.names)} 件"
                 if self.catalog.has_names() else "カード名 未取得（ファイル名がカードIDになります）")
        self.data_label.config(
            text=f"照合データ: カード {self.catalog.card_count} 種 / "
                 f"ハッシュ {self.catalog.hash_count} 件　|　{names}")

    def _offer_first_fetch(self):
        """初回起動時に、カード名の取得を1度だけ案内する。"""
        if self.catalog.has_names() or self.config.get("asked_names"):
            return
        self.config["asked_names"] = True
        save_config(self.config)
        if messagebox.askyesno(APP_TITLE, (
                "カード名のデータがまだありません。\n"
                "このまま使うと、ファイル名がカードIDになります。\n\n"
                "公式データベースからカード名を取得しますか？（30秒ほど）")):
            self._run_update(names=True)

    # ---------- 入力 ----------
    def _on_drop(self, event):
        self._start(parse_drop(self.root, event.data))

    def _choose_files(self):
        files = filedialog.askopenfilenames(
            title="スクリーンショットを選ぶ",
            filetypes=[("画像", "*.png *.jpg *.jpeg *.webp *.bmp"), ("すべて", "*.*")])
        if files:
            self._start(list(files))

    def _choose_folder(self):
        folder = filedialog.askdirectory(title="フォルダを選ぶ")
        if folder:
            self._start([folder])

    def _choose_output(self):
        folder = filedialog.askdirectory(title="出力先を選ぶ",
                                         initialdir=self.var_output.get() or None)
        if folder:
            self.var_output.set(folder)

    def _output_dir(self):
        """出力先フォルダ。空欄なら警告して None を返す。"""
        path = self.var_output.get().strip()
        if not path:
            messagebox.showwarning(APP_TITLE, "出力先フォルダを指定してください。")
            return None
        return path

    def _busy(self):
        """処理中なら知らせて True を返す（処理とデータ更新を同時に走らせない）。"""
        if self.worker and self.worker.is_alive():
            messagebox.showinfo(APP_TITLE, "いま処理中です。終わるまでお待ちください。")
            return True
        return False

    def _open_output(self):
        path = self._output_dir()
        if not path:
            return
        try:
            os.makedirs(path, exist_ok=True)
            os.startfile(path)                      # noqa: S606 (Windows 専用)
        except AttributeError:
            messagebox.showinfo(APP_TITLE, path)
        except OSError as exc:
            messagebox.showerror(APP_TITLE, f"出力先を開けませんでした。\n{exc}")

    # ---------- 実行 ----------
    def _options(self, output_dir):
        resolution = "auto"
        chosen = self.var_resolution.get()
        if chosen in PRESETS:
            resolution = PRESETS[chosen]
        return Options(
            output_dir=output_dir,
            save_format=self.var_format.get(),
            rounded=self.var_rounded.get(),
            resolution=resolution,
            skip_duplicate=self.var_skip_dup.get(),
            recursive=self.var_recursive.get(),
            save_review=self.var_save_review.get(),
            save_not_card=self.var_save_not_card.get(),
            english=ENGLISH_CHOICES.get(self.var_english.get(), "name"),
            split=SPLIT_CHOICES.get(self.var_split.get(), "none"),
        )

    def _start(self, paths):
        if self._busy():
            return
        if not self.catalog.card_count:
            messagebox.showerror(APP_TITLE, "照合用のハッシュデータが見つかりません。"
                                            "data/card_hashes.json を確認してください。")
            return
        output_dir = self._output_dir()
        if not output_dir:
            return
        options = self._options(output_dir)
        files = collect_images(paths, options.recursive, exclude=output_dir)
        if not files:
            self.var_status.set("画像が見つかりませんでした")
            return

        self._save_config()
        self.tree.delete(*self.tree.get_children())
        self.progress.configure(maximum=len(files), value=0)
        self.stop_flag.clear()
        self.stop_button.config(state="normal")
        self.var_status.set(f"処理中... 0/{len(files)}")

        def run():
            cropper = Cropper(self.catalog, options)
            done = [0]

            def on_result(result):
                done[0] += 1
                self.queue.put(("result", result, done[0], len(files)))

            try:
                summary = cropper.process_files(files, on_result=on_result,
                                                should_stop=self.stop_flag.is_set)
                self.queue.put(("done", summary))
            except Exception:
                self.queue.put(("failed", traceback.format_exc()))

        self.worker = threading.Thread(target=run, daemon=True)
        self.worker.start()

    def _stop(self):
        self.stop_flag.set()
        self.var_status.set("中止しています...")

    # ---------- 画面更新 ----------
    def _drain_queue(self):
        try:
            while True:
                item = self.queue.get_nowait()
                kind = item[0]
                if kind == "result":
                    _, result, done, total = item
                    self.progress.configure(value=done)
                    self.var_status.set(f"処理中... {done}/{total}")
                    self.tree.insert("", "end", tags=(result.status,), values=(
                        result.status,
                        (result.name or "-") + ("  [英語版]" if result.english else ""),
                        result.distance if result.distance < 999 else "-",
                        os.path.basename(result.source) + (f"  [{result.note}]" if result.note else ""),
                    ))
                    self.tree.yview_moveto(1.0)
                elif kind == "done":
                    summary = item[1]
                    self.stop_button.config(state="disabled")
                    self.var_status.set(
                        f"完了: 確定 {summary.confirmed} / 要確認 {summary.review} / "
                        f"カードなし {summary.not_card} / エラー {summary.error}"
                        f"（重複スキップ {summary.skipped}） {summary.seconds:.1f} 秒")
                elif kind == "failed":
                    self.stop_button.config(state="disabled")
                    self.catalog.load()             # 更新の途中で失敗した場合に戻す
                    self._refresh_data_label()
                    self.var_status.set("エラーが発生しました")
                    messagebox.showerror(APP_TITLE, item[1])
                elif kind == "progress":
                    self.var_status.set(item[1])
                elif kind == "update_done":
                    self.stop_button.config(state="disabled")
                    self.catalog.load()
                    self._refresh_data_label()
                    self.var_status.set(item[1])
                    messagebox.showinfo(APP_TITLE, item[1])
        except queue.Empty:
            pass
        self.root.after(80, self._drain_queue)

    # ---------- データ更新 ----------
    def _open_update_dialog(self):
        if self._busy():
            return
        win = tk.Toplevel(self.root)
        win.title("カードデータ更新")
        win.geometry("560x280")
        win.transient(self.root)
        win.grab_set()                      # 開いている間はメイン画面を操作させない
        ttk.Label(win, justify="left", text=(
            "カード名とマスター画像はアプリに含まれていません。\n"
            "必要なときに遊戯王公式データベースから取得します。\n\n"
            "・カード名を取得: 出力ファイル名を日本語のカード名にします（約30秒）\n"
            "　（日本語版に無いカードは英語名。英語版の保存に使う英語名も取得します）\n"
            "・新しいカードを追加: ハッシュに無いカードの画像を取得して照合できるようにします\n"
            "　（1枚ずつ取得するため、枚数によっては時間がかかります）"
        )).pack(anchor="w", padx=12, pady=12)
        ttk.Label(win, text=f"保存先: {user_data_dir()}",
                  foreground="#666666").pack(anchor="w", padx=12)
        row = ttk.Frame(win)
        row.pack(fill="x", padx=12, pady=16)
        ttk.Button(row, text="カード名を取得",
                   command=lambda: (win.destroy(), self._run_update(names=True))).pack(side="left")
        ttk.Button(row, text="新しいカードセットを取り込む",
                   command=lambda: (win.destroy(), self._run_update(hashes=True))).pack(side="left", padx=8)
        ttk.Button(row, text="閉じる", command=win.destroy).pack(side="right")

    def _run_update(self, names=False, hashes=False):
        if self._busy():
            return
        self.stop_flag.clear()
        self.stop_button.config(state="normal")

        def progress(message):
            self.queue.put(("progress", message))

        def run():
            try:
                message = []
                cards = None
                if names or hashes:
                    cards = updater.fetch_card_names(progress, self.stop_flag.is_set)
                    self.catalog.save_names(cards)
                    message.append(f"カード名 {len(cards)} 件を取得しました")
                if hashes:
                    r = updater.update_new_sets(self.catalog, cards, progress,
                                                self.stop_flag.is_set)
                    self.catalog.save_user_hashes()
                    if r["seeded"]:
                        message.append(f"収録シリーズ {r['seeded']} 個を記録しました"
                                       "（同梱ハッシュでカバー済み）")
                    if r["new_cards"] or r["refreshed"]:
                        packs = "、".join(r["pack_names"][:3]) or f"{r['packs']} 個"
                        message.append(
                            f"新しいカード {r['new_cards']} 件 / "
                            f"再録カードの確認 {r['refreshed']} 件 / "
                            f"イラスト {r['new_hashes']} 枚を追加（{packs}）"
                            + (f" / 取得できなかったもの {len(r['failed'])} 件"
                               if r["failed"] else ""))
                    elif not r["seeded"]:
                        message.append("新しいカードセットはありませんでした")
                    if r["pack_list_failed"]:
                        message.append("収録シリーズの一覧を取得できなかったため、"
                                       "再録カードの確認は省略しました")
                    if r["skipped_packs"]:
                        message.append("収録数が多いシリーズがあったため、"
                                       "再録カードの確認は省略しました")
                self.queue.put(("update_done", " / ".join(message) or "更新するものはありませんでした"))
            except updater.Cancelled:
                self.queue.put(("update_done", "中止しました"))
            except Exception:
                self.queue.put(("failed", traceback.format_exc()))

        self.worker = threading.Thread(target=run, daemon=True)
        self.worker.start()

    # ---------- 終了 ----------
    def _save_config(self):
        self.config.update({
            "output_dir": self.var_output.get().strip(),
            "resolution": self.var_resolution.get(),
            "save_format": self.var_format.get(),
            "rounded": self.var_rounded.get(),
            "recursive": self.var_recursive.get(),
            "skip_duplicate": self.var_skip_dup.get(),
            "save_review": self.var_save_review.get(),
            "save_not_card": self.var_save_not_card.get(),
            "english": ENGLISH_CHOICES.get(self.var_english.get(), "name"),
            "split": SPLIT_CHOICES.get(self.var_split.get(), "none"),
        })
        save_config(self.config)

    def _on_close(self):
        self.stop_flag.set()
        self._save_config()
        self.root.destroy()

    def run(self):
        self.root.mainloop()


def main():
    try:
        App().run()
    except Exception:
        traceback.print_exc()
        if sys.stdout is None or not sys.stdout.isatty():
            try:
                tk.Tk().withdraw()
                messagebox.showerror(APP_TITLE, traceback.format_exc())
            except Exception:
                pass
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
