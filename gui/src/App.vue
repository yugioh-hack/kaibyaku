<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, triggerRef } from "vue";
import { ask, invoke, listen, message, onClose, onDrop, pickFiles, pickFolder } from "./backend";
import type {
  AppState,
  Config,
  DataSummary,
  IdentifyDone,
  IdentifyProgress,
  IdentifyResult,
  UpdateDone,
} from "./types";

const RESOLUTIONS: Record<string, string> = {
  自動: "auto",
  "FullHD (1920x1080)": "1920x1080",
  "WQHD (2560x1440)": "2560x1440",
  "4K (3840x2160)": "3840x2160",
};
const FORMATS = ["webp", "png", "both"];
const ENGLISH: Record<string, string> = {
  英語名で保存: "name",
  日本語名で保存: "ja",
  判別しない: "off",
};
const SPLIT: Record<string, string> = {
  分けない: "none",
  "英語版を en に": "en",
  "ja と en に分ける": "both",
};
const STATUSES = ["確定", "要確認", "カードなし", "エラー"];

const labelOf = (table: Record<string, string>, value: string | undefined, fallback: string) =>
  Object.keys(table).find((label) => table[label] === value) ?? fallback;

// ---- 設定 ----
const output = ref("");
const resolution = ref("自動");
const format = ref("webp");
const rounded = ref(true);
const recursive = ref(true);
const skipDuplicate = ref(true);
const saveReview = ref(true);
const saveNotCard = ref(true);
const english = ref("英語名で保存");
const split = ref("分けない");
let config: Config = {};

// ---- 画面の状態 ----
const data = ref<DataSummary>({ cards: 0, hashes: 0, names: 0, names_fetched: false });
const busy = ref<"identify" | "update" | null>(null);
const updateOpen = ref(false);
const updateProgress = ref("");
const status = ref("");
const done = ref(0);
const total = ref(0);
const dragOver = ref(false);
const rows = shallowRef<IdentifyResult[]>([]);
const listEl = ref<HTMLElement | null>(null);

const percent = computed(() => (total.value ? (done.value / total.value) * 100 : 0));
const dataText = computed(() => `カード ${data.value.cards} 種 / ハッシュ ${data.value.hashes} 件`);
const namesText = computed(() => (data.value.names > 0 ? `カード名 ${data.value.names} 件` : "カード名 未取得"));

function currentConfig(): Config {
  return {
    output_dir: output.value.trim(),
    resolution: resolution.value,
    save_format: format.value,
    rounded: rounded.value,
    recursive: recursive.value,
    skip_duplicate: skipDuplicate.value,
    save_review: saveReview.value,
    save_not_card: saveNotCard.value,
    english: ENGLISH[english.value] ?? "name",
    split: SPLIT[split.value] ?? "none",
  };
}

async function saveConfig(extra: Config = {}) {
  config = { ...config, ...currentConfig(), ...extra };
  try {
    await invoke("save_config", { config });
  } catch (e) {
    console.error(e);
  }
}

async function refreshData() {
  try {
    data.value = await invoke<DataSummary>("data_summary");
  } catch (e) {
    console.error(e);
  }
}

// ---- 結果一覧（まとめて反映して、行が増えても重くならないようにする） ----
let pending: IdentifyResult[] = [];
let flushScheduled = false;

function flushRows() {
  flushScheduled = false;
  if (!pending.length) return;
  rows.value.push(...pending);
  pending = [];
  triggerRef(rows);
  nextTick(() => {
    const el = listEl.value;
    if (el) el.scrollTop = el.scrollHeight;
  });
}

function queueRow(result: IdentifyResult) {
  pending.push(result);
  if (!flushScheduled) {
    flushScheduled = true;
    requestAnimationFrame(flushRows);
  }
}

function clearRows() {
  pending = [];
  rows.value = [];
}

// ---- 入力 ----
async function guardBusy(): Promise<boolean> {
  if (!busy.value) return false;
  await message("いま処理中です。終わるまでお待ちください。");
  return true;
}

async function start(paths: string[]) {
  if (!paths.length || (await guardBusy())) return;
  if (!data.value.cards) {
    await message("照合用のハッシュデータが見つかりません。", "error");
    return;
  }
  const out = output.value.trim();
  if (!out) {
    await message("出力先フォルダを指定してください。", "warning");
    return;
  }
  await saveConfig();
  clearRows();
  done.value = 0;
  total.value = 0;
  status.value = "処理中...";
  busy.value = "identify";
  try {
    await invoke("start_identify", {
      paths,
      args: {
        output: out,
        save_format: format.value,
        rounded: rounded.value,
        recursive: recursive.value,
        skip_duplicate: skipDuplicate.value,
        save_review: saveReview.value,
        save_not_card: saveNotCard.value,
        english: ENGLISH[english.value] ?? "name",
        split: SPLIT[split.value] ?? "none",
        resolution: RESOLUTIONS[resolution.value] ?? "auto",
      },
    });
  } catch (e) {
    busy.value = null;
    status.value = "";
    await message(String(e), "warning");
  }
}

async function chooseFiles() {
  if (await guardBusy()) return;
  await start(await pickFiles());
}

async function chooseFolder() {
  if (await guardBusy()) return;
  const folder = await pickFolder("フォルダを選ぶ");
  if (folder) await start([folder]);
}

async function chooseOutput() {
  const folder = await pickFolder("出力先を選ぶ", output.value.trim());
  if (folder) output.value = folder;
}

async function openOutput() {
  const path = output.value.trim();
  if (!path) {
    await message("出力先フォルダを指定してください。", "warning");
    return;
  }
  try {
    await invoke("open_folder", { path });
  } catch (e) {
    await message(String(e), "error");
  }
}

async function stop() {
  status.value = "中止しています...";
  try {
    await invoke("stop");
  } catch (e) {
    console.error(e);
  }
}

// ---- カードデータ更新 ----
async function runUpdate(hashes: boolean) {
  if (await guardBusy()) return;
  updateProgress.value = "";
  busy.value = "update";
  updateOpen.value = true;
  try {
    await invoke("start_update", { hashes });
  } catch (e) {
    busy.value = null;
    updateOpen.value = false;
    await message(String(e), "warning");
  }
}

async function openUpdate() {
  if (await guardBusy()) return;
  updateOpen.value = true;
}

async function offerFirstFetch() {
  if (data.value.names > 0 || config.asked_names) return;
  config.asked_names = true;
  await saveConfig({ asked_names: true });
  const yes = await ask(
    "カード名のデータがまだありません。\nこのまま使うと、ファイル名がカードIDになります。\n\n公式データベースからカード名を取得しますか？（30秒ほど）",
  );
  if (yes) await runUpdate(false);
}

// ---- イベント ----
function onIdentifyProgress(p: IdentifyProgress) {
  done.value = p.done;
  total.value = p.total;
  status.value = `処理中... ${p.done}/${p.total}`;
  queueRow(p.result);
}

function onIdentifyDone(d: IdentifyDone) {
  flushRows();
  busy.value = null;
  if (typeof d.error === "string" && d.total === undefined) {
    status.value = "エラーが発生しました";
    message(d.error, "error");
    return;
  }
  if (!d.total) {
    status.value = "画像が見つかりませんでした";
    return;
  }
  status.value =
    `${d.cancelled ? "中止" : "完了"}: 確定 ${d.confirmed} / 要確認 ${d.review} / ` +
    `カードなし ${d.not_card} / エラー ${d.error}（重複スキップ ${d.skipped}） ${(d.seconds ?? 0).toFixed(1)} 秒`;
}

function onUpdateProgress(text: string) {
  updateProgress.value = text;
  status.value = text;
}

async function onUpdateDone(d: UpdateDone) {
  busy.value = null;
  updateOpen.value = false;
  await refreshData();
  if (d.error) {
    status.value = "エラーが発生しました";
    await message(d.error, "error");
  } else {
    status.value = d.message ?? "";
    await message(d.message ?? "", "info");
  }
}

// ---- 表示 ----
const distanceText = (r: IdentifyResult) => (r.distance < 999 ? String(r.distance) : "-");
const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

const unlisten: Array<() => void> = [];

onMounted(async () => {
  const state = await invoke<AppState>("get_state");
  config = state.config;
  data.value = state.data;
  output.value = config.output_dir || state.default_output;
  resolution.value = config.resolution && config.resolution in RESOLUTIONS ? config.resolution : "自動";
  format.value = FORMATS.includes(config.save_format ?? "") ? config.save_format! : "webp";
  rounded.value = config.rounded ?? true;
  recursive.value = config.recursive ?? true;
  skipDuplicate.value = config.skip_duplicate ?? true;
  saveReview.value = config.save_review ?? true;
  saveNotCard.value = config.save_not_card ?? true;
  english.value = labelOf(ENGLISH, config.english, "英語名で保存");
  split.value = labelOf(SPLIT, config.split, "分けない");
  status.value = "画像かフォルダをここにドロップしてください";

  unlisten.push(
    await listen<IdentifyProgress>("identify-progress", onIdentifyProgress),
    await listen<IdentifyDone>("identify-done", onIdentifyDone),
    await listen<string>("update-progress", onUpdateProgress),
    await listen<UpdateDone>("update-done", onUpdateDone),
    await onDrop(
      (active) => (dragOver.value = active && !busy.value),
      (paths) => start(paths),
    ),
  );
  await onClose(async () => {
    try {
      await invoke("stop");
    } catch {
      // 閉じる処理を止めない
    }
    await saveConfig();
  });
  setTimeout(offerFirstFetch, 300);
});

onBeforeUnmount(() => unlisten.forEach((fn) => fn()));
</script>

<template>
  <div class="app">
    <header class="bar">
      <div class="data">
        <span>{{ dataText }}</span>
        <span class="sep">/</span>
        <span :class="{ warn: data.names === 0 }">{{ namesText }}</span>
      </div>
      <button :disabled="!!busy" @click="openUpdate">カードデータ更新</button>
    </header>

    <section class="input">
      <div
        class="drop"
        :class="{ active: dragOver, disabled: !!busy }"
        role="button"
        tabindex="0"
        @click="chooseFiles"
        @keydown.enter.prevent="chooseFiles"
        @keydown.space.prevent="chooseFiles"
      >
        ここにスクリーンショット（またはフォルダ）をドロップ
      </div>
      <div class="actions">
        <button :disabled="!!busy" @click="chooseFiles">ファイルを選ぶ</button>
        <button :disabled="!!busy" @click="chooseFolder">フォルダを選ぶ</button>
        <span class="grow"></span>
        <button class="danger" :disabled="busy !== 'identify'" @click="stop">中止</button>
      </div>
    </section>

    <section class="panel">
      <div class="line">
        <span class="caption">出力先</span>
        <input v-model="output" class="grow" spellcheck="false" />
        <button @click="chooseOutput">変更</button>
        <button @click="openOutput">開く</button>
      </div>
      <div class="line options">
        <label class="field">
          解像度
          <select v-model="resolution">
            <option v-for="label in Object.keys(RESOLUTIONS)" :key="label">{{ label }}</option>
          </select>
        </label>
        <label class="field">
          形式
          <select v-model="format">
            <option v-for="f in FORMATS" :key="f">{{ f }}</option>
          </select>
        </label>
        <label class="field">
          英語版
          <select v-model="english">
            <option v-for="label in Object.keys(ENGLISH)" :key="label">{{ label }}</option>
          </select>
        </label>
        <label class="field">
          言語でフォルダを
          <select v-model="split">
            <option v-for="label in Object.keys(SPLIT)" :key="label">{{ label }}</option>
          </select>
        </label>
      </div>
      <div class="line checks">
        <label><input v-model="rounded" type="checkbox" />角丸・透過</label>
        <label><input v-model="recursive" type="checkbox" />サブフォルダも処理</label>
        <label><input v-model="skipDuplicate" type="checkbox" />同じイラストは保存しない</label>
        <label><input v-model="saveReview" type="checkbox" />要確認も保存</label>
        <label><input v-model="saveNotCard" type="checkbox" />判定できない画像も残す</label>
      </div>
    </section>

    <section class="progress">
      <div class="meter" :class="{ waiting: busy === 'identify' && !total }">
        <div class="fill" :style="{ width: percent + '%' }"></div>
      </div>
      <div class="status">{{ status }}</div>
    </section>

    <section class="results">
      <div class="head row">
        <span>状態</span>
        <span>カード名</span>
        <span class="num">距離</span>
        <span>元ファイル</span>
      </div>
      <div ref="listEl" class="list">
        <div v-for="(r, i) in rows" :key="i" class="row item" :class="'s-' + STATUSES.indexOf(r.status)">
          <span class="state">{{ r.status }}</span>
          <span class="name">
            {{ r.name || "-" }}
            <em v-if="r.english" class="badge">英語版</em>
          </span>
          <span class="num">{{ distanceText(r) }}</span>
          <span class="file" :title="r.source">
            {{ fileName(r.source) }}
            <small v-if="r.note">{{ r.note }}</small>
          </span>
        </div>
      </div>
    </section>

    <div v-if="updateOpen" class="overlay">
      <div class="dialog" role="dialog" aria-label="カードデータ更新">
        <template v-if="busy === 'update'">
          <div class="busy-text">{{ updateProgress || "準備中..." }}</div>
          <div class="meter waiting"><div class="fill"></div></div>
          <div class="dialog-actions">
            <button class="danger" @click="stop">中止</button>
          </div>
        </template>
        <template v-else>
          <div class="dialog-title">カードデータ更新</div>
          <div class="dialog-actions column">
            <button @click="runUpdate(false)">カード名を取得</button>
            <button @click="runUpdate(true)">新しいカードセットを取り込む</button>
            <button class="plain" @click="updateOpen = false">閉じる</button>
          </div>
        </template>
      </div>
    </div>
  </div>
</template>

<style>
:root {
  --bg: #f3f4f6;
  --panel: #ffffff;
  --text: #1f2328;
  --muted: #6b7280;
  --border: #d4d8de;
  --accent: #2563eb;
  --accent-soft: #e3ecfd;
  --danger: #c0392b;
  --field: #ffffff;
  --btn: #eef0f3;
  --btn-hover: #e2e5ea;
  --ok: #1b7f3b;
  --review: #b8860b;
  --none: #777777;
  --error: #c0392b;
  --overlay: rgba(0, 0, 0, 0.35);
  font-family: "Yu Gothic UI", "Meiryo UI", system-ui, sans-serif;
  font-size: 14px;
  color-scheme: light dark;
}

@media (prefers-color-scheme: dark) {
  :root {
    --bg: #1b1d21;
    --panel: #25282d;
    --text: #e6e8eb;
    --muted: #9aa1ab;
    --border: #3a3f46;
    --accent: #6b9bff;
    --accent-soft: #263552;
    --danger: #ef6b5b;
    --field: #1b1d21;
    --btn: #31353b;
    --btn-hover: #3b4047;
    --ok: #4cc27a;
    --review: #e0b341;
    --none: #9a9a9a;
    --error: #ef6b5b;
    --overlay: rgba(0, 0, 0, 0.55);
  }
}

* {
  box-sizing: border-box;
}
html,
body,
#app {
  height: 100%;
  margin: 0;
}
body {
  background: var(--bg);
  color: var(--text);
}

button,
input,
select {
  font: inherit;
  color: inherit;
}
button {
  background: var(--btn);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 5px 14px;
  cursor: pointer;
}
button:hover:not(:disabled) {
  background: var(--btn-hover);
}
button:disabled {
  opacity: 0.45;
  cursor: default;
}
button.danger:not(:disabled) {
  color: var(--danger);
  border-color: var(--danger);
}
button.plain {
  background: transparent;
  border-color: transparent;
  color: var(--muted);
}
input:not([type]),
select {
  background: var(--field);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 5px 8px;
  min-width: 0;
}
input[type="checkbox"] {
  accent-color: var(--accent);
  margin-right: 6px;
  vertical-align: -2px;
}
:focus-visible {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
}

.app {
  display: flex;
  flex-direction: column;
  gap: 10px;
  height: 100%;
  padding: 12px 14px;
}
.grow {
  flex: 1;
}

.bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.data {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.sep {
  color: var(--muted);
}
.warn {
  color: var(--review);
}

.input {
  display: flex;
  flex-direction: column;
  gap: 8px;
}
.drop {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 112px;
  border: 2px dashed var(--border);
  border-radius: 10px;
  background: var(--panel);
  color: var(--muted);
  font-size: 15px;
  cursor: pointer;
  transition: border-color 0.12s, background 0.12s, color 0.12s;
}
.drop:hover:not(.disabled),
.drop.active {
  border-color: var(--accent);
  color: var(--accent);
  background: var(--accent-soft);
}
.drop.disabled {
  cursor: default;
  opacity: 0.6;
}
.actions {
  display: flex;
  gap: 8px;
}

.panel {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px 12px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
}
.line {
  display: flex;
  align-items: center;
  gap: 8px;
}
.caption {
  color: var(--muted);
}
.options {
  flex-wrap: wrap;
  gap: 8px 18px;
}
.field {
  display: flex;
  align-items: center;
  gap: 6px;
  color: var(--muted);
}
.field select {
  color: var(--text);
}
.checks {
  flex-wrap: wrap;
  gap: 6px 18px;
}
.checks label {
  white-space: nowrap;
}

.progress {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.meter {
  height: 8px;
  border-radius: 4px;
  background: var(--border);
  overflow: hidden;
  position: relative;
}
.fill {
  height: 100%;
  background: var(--accent);
  transition: width 0.08s linear;
}
.meter.waiting .fill {
  position: absolute;
  width: 30% !important;
  animation: slide 1.1s ease-in-out infinite;
}
@keyframes slide {
  from {
    left: -30%;
  }
  to {
    left: 100%;
  }
}
.status {
  min-height: 1.4em;
}

.results {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-height: 120px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  overflow: hidden;
}
.row {
  display: grid;
  grid-template-columns: 4.8em minmax(0, 1.1fr) 3.5em minmax(0, 1.4fr);
  gap: 10px;
  padding: 5px 12px;
  align-items: baseline;
}
.row > span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.head {
  color: var(--muted);
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.num {
  text-align: right;
}
.list {
  flex: 1;
  overflow-y: auto;
}
.item {
  content-visibility: auto;
  contain-intrinsic-size: auto 28px;
  border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
}
.item .state {
  font-weight: 600;
}
.s-0 .state {
  color: var(--ok);
}
.s-1 .state {
  color: var(--review);
}
.s-2 .state,
.s-2 .name {
  color: var(--none);
}
.s-3 .state {
  color: var(--error);
}
.badge {
  font-style: normal;
  font-size: 11px;
  margin-left: 6px;
  padding: 1px 6px;
  border-radius: 8px;
  background: var(--accent-soft);
  color: var(--accent);
}
.file small {
  margin-left: 8px;
  color: var(--muted);
}

.overlay {
  position: fixed;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: var(--overlay);
}
.dialog {
  display: flex;
  flex-direction: column;
  gap: 14px;
  width: 340px;
  padding: 18px;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 12px;
}
.dialog-title {
  font-weight: 600;
}
.dialog-actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
}
.dialog-actions.column {
  flex-direction: column;
}
.busy-text {
  min-height: 1.4em;
}
</style>
