<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, triggerRef, watch } from "vue";
import { ask, imageUrl, invoke, listen, message, onClose, onDrop, pickFiles, pickFolder } from "./backend";
import type {
  AppState,
  Candidate,
  CardRef,
  Config,
  CsvError,
  DataSummary,
  IdentifyDone,
  IdentifyProgress,
  IdentifyResult,
  LearnResult,
  Migration,
  ProcessArgs,
  Reprocessed,
  UpdateDone,
} from "./types";

// 保存形式（画面の表示と、設定・処理に渡す値）
const FORMATS = [
  { label: "webp", value: "webp" },
  { label: "png", value: "png" },
  { label: "両方", value: "both" },
];
const ENGLISH: Record<string, string> = {
  英語名で保存: "name",
  日本語名で保存: "ja",
  区別しない: "off",
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

// いま走っている（直近の）処理の条件。届いた結果に付けて、あとから登録するときに使う
let runArgs: ProcessArgs | null = null;

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
    syncScroll();
  });
}

function queueRow(result: IdentifyResult) {
  if (runArgs) result.args = runArgs;
  pending.push(result);
  if (!flushScheduled) {
    flushScheduled = true;
    requestAnimationFrame(flushRows);
  }
}

function clearRows() {
  pending = [];
  rows.value = [];
  closePanel();
}

// ---- 絞り込み ----
const FILTERS = ["すべて", ...STATUSES];
const filter = ref("すべて");
const counts = computed(() => {
  const c: Record<string, number> = { すべて: rows.value.length, 確定: 0, 要確認: 0, カードなし: 0, エラー: 0 };
  for (const r of rows.value) c[r.status]++;
  return c;
});
const visible = computed(() => {
  const out: Array<{ r: IdentifyResult; i: number }> = [];
  rows.value.forEach((r, i) => {
    if (filter.value === "すべて" || r.status === filter.value) out.push({ r, i });
  });
  return out;
});

// 見えている行（と前後の数行）だけを描く。行の高さは CSS の .item と同じ 46px で固定
const ROW_HEIGHT = 46;
const OVERSCAN = 8;
const scrollTop = ref(0);
const viewHeight = ref(0);
const windowStart = computed(() => Math.max(0, Math.floor(scrollTop.value / ROW_HEIGHT) - OVERSCAN));
const windowRows = computed(() => {
  const end = Math.ceil((scrollTop.value + viewHeight.value) / ROW_HEIGHT) + OVERSCAN;
  return visible.value.slice(windowStart.value, end);
});
// スクロール位置は描画のたびに読み直す（requestAnimationFrame を待つと、窓が隠れている間や
// 自動スクロールの直後に古い位置のまま描いて、一覧が空白に見える）
function syncScroll() {
  const el = listEl.value;
  if (el) {
    scrollTop.value = el.scrollTop;
    viewHeight.value = el.clientHeight;
  }
}
function onListScroll() {
  syncScroll();
}
// 行数が変わると（追加・絞り込み）ブラウザーがスクロール位置を詰めることがあるので合わせ直す
watch(
  () => visible.value.length,
  () => nextTick(syncScroll),
);
const listResize = new ResizeObserver(([entry]) => (viewHeight.value = entry.contentRect.height));
watch(listEl, (el, old) => {
  if (old) listResize.unobserve(old);
  if (el) listResize.observe(el);
});

watch(
  () => counts.value[filter.value] === 0,
  (empty) => {
    if (empty) filter.value = "すべて";
  },
);

// ---- 確認パネル ----
const REVIEWABLE = ["要確認", "カードなし"];
// 登録できるのは要確認とカードなしだけ。確定・エラーは画像と判定を見るだけ
const canRegister = computed(() => !!current.value && REVIEWABLE.includes(current.value.status));
const selected = ref<number | null>(null);
const current = computed(() => (selected.value === null ? null : (rows.value[selected.value] ?? null)));
const candidates = ref<Candidate[]>([]);
const query = ref("");
const found = ref<CardRef[]>([]);
const registering = ref(false);
// ウィンドウを閉じる途中。処理の完了を待っているあいだは新しい操作を始めない
const closing = ref(false);
const locked = computed(() => !!busy.value || registering.value || closing.value);
let panelToken = 0;
let searchToken = 0;
let searchTimer: number | undefined;

function closePanel() {
  panelToken++;
  searchToken++;
  clearTimeout(searchTimer);
  selected.value = null;
  candidates.value = [];
  found.value = [];
  query.value = "";
}

async function selectRow(i: number) {
  const r = rows.value[i];
  if (!r || selected.value === i) return;
  closePanel();
  selected.value = i;
  const token = panelToken;
  const ranked = r.ranked ?? [];
  if (!ranked.length) return;
  try {
    const names = await invoke<CardRef[]>("describe_cards", { cids: ranked.map(([, cid]) => String(cid)) });
    if (token !== panelToken) return;
    candidates.value = ranked.map(([distance, cid]) => ({
      cid: String(cid),
      name: names.find((n) => String(n.cid) === String(cid))?.name ?? String(cid),
      distance,
    }));
  } catch (e) {
    console.error(e);
  }
}

watch(query, (q) => {
  clearTimeout(searchTimer);
  const token = ++searchToken;
  if (!q.trim()) {
    found.value = [];
    return;
  }
  searchTimer = window.setTimeout(async () => {
    try {
      const list = await invoke<CardRef[]>("search_cards", { query: q });
      if (token === searchToken) found.value = list;
    } catch (e) {
      if (token === searchToken) found.value = [];
      console.error(e);
    }
  }, 250);
});

async function register(cid: string) {
  const index = selected.value;
  const row = current.value;
  if (index === null || !row || locked.value) return;
  // 再処理と古いコピーの掃除は、この結果を作ったときの出力先・設定で行う（画面の現在値は使わない）
  const args = row.args;
  if (!args) {
    await message("この結果の処理条件が分からないため、登録できません。", "error");
    return;
  }
  registering.value = true;
  try {
    const learned = await invoke<LearnResult>("learn_card", { cid, source: row.source });
    if (!learned.reprocess) {
      await message(learned.message);
    } else {
      const again = await invoke<Reprocessed>("identify_one", { path: row.source, args });
      const fresh: IdentifyResult = { ...again.result, args };
      const old = row.saved_paths ?? (row.saved_path ? [row.saved_path] : []);
      if (rows.value[index] !== row) return; // 一覧が入れ替わっていたら書き戻さない
      rows.value[index] = fresh;
      triggerRef(rows);
      closePanel();
      if (fresh.status === "確定") {
        const kept = new Set(fresh.saved_paths ?? []);
        const stale = old.filter((path) => !kept.has(path));
        if (stale.length) {
          try {
            await invoke("remove_review_copy", { paths: stale, output: args.output });
          } catch (e) {
            console.error(e);
          }
        }
      } else {
        await selectRow(index);
      }
      status.value = `${learned.message}: ${fresh.status}`;
      if (again.csv_error) await warnCsv(again.csv_error);
    }
    await refreshData();
  } catch (e) {
    if (!closing.value) await message(String(e), "error");
  } finally {
    registering.value = false;
  }
}

function onKey(e: KeyboardEvent) {
  if (e.key === "Escape" && selected.value !== null && !updateOpen.value) closePanel();
}

// ---- 入力 ----
async function guardBusy(): Promise<boolean> {
  // 登録（learn → 判定し直し）の途中も処理中として扱い、一覧を空にさせない
  if (closing.value) return true;
  if (!busy.value && !registering.value) return false;
  await message("いま処理中です。終わるまでお待ちください。");
  return true;
}

function identifyArgs(out: string): ProcessArgs {
  return {
    output: out,
    save_format: format.value,
    rounded: rounded.value,
    recursive: recursive.value,
    skip_duplicate: skipDuplicate.value,
    save_review: saveReview.value,
    save_not_card: saveNotCard.value,
    english: ENGLISH[english.value] ?? "name",
    split: SPLIT[split.value] ?? "none",
    // カードの位置は自動で見つける（解像度を指定しても、見つけられる範囲は広がらない）
    resolution: "auto",
  };
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
  runArgs = identifyArgs(out);
  done.value = 0;
  total.value = 0;
  status.value = "処理中...";
  busy.value = "identify";
  try {
    await invoke("start_identify", {
      paths,
      args: runArgs,
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

async function warnCsv(csv: CsvError) {
  if (closing.value) return;
  await message(`ログを保存できませんでした。
${csv.path}
${csv.message}`, "warning");
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
  if (d.csv_error) {
    status.value += " / ログを保存できませんでした";
    warnCsv(d.csv_error);
  }
}

function onUpdateProgress(text: string) {
  updateProgress.value = text;
  status.value = text;
}

async function onUpdateDone(d: UpdateDone) {
  busy.value = null;
  updateOpen.value = false;
  await refreshData();
  if (closing.value) return;
  if (d.error) {
    status.value = "エラーが発生しました";
    await message(d.error, "error");
  } else {
    status.value = d.message ?? "";
    await message(d.message ?? "", "info");
  }
}

// 旧アプリのデータを取り込んだとき（初回だけ）に知らせる
const MIGRATED_NAMES: Record<string, string> = {
  "config.json": "設定",
  "card_names.json": "カード名",
  "card_hashes_user.json": "登録した照合データ",
};
async function reportMigration(m: Migration) {
  const names = (files: string[]) => files.map((f) => MIGRATED_NAMES[f] ?? f).join("、");
  const lines: string[] = [];
  if (m.error) lines.push(`旧アプリのデータを取り込めませんでした: ${m.error}`);
  if (m.copied?.length) lines.push(`旧アプリの${names(m.copied)}を引き継ぎました。`);
  if (m.corrupt?.length) lines.push(`読めなかったため引き継がなかったもの: ${names(m.corrupt)}`);
  if (lines.length) await message(lines.join("\n"), m.error || m.corrupt?.length ? "warning" : "info");
}

// ---- 表示 ----
const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path;

const unlisten: Array<() => void> = [];
// 表示する切り抜き画像。保存した画像 → 重複でスキップした元の既存画像 → 元画像のカードの枠だけ、の順
function cardImage(r: IdentifyResult): { src: string; style?: Record<string, string> } {
  const saved = r.saved_path || r.existing_path;
  if (saved) return { src: imageUrl(saved) };
  if (r.box && r.size) {
    const [l, t, right, bottom] = r.box;
    const [w, h] = r.size;
    const inset = `inset(${t}px ${w - right}px ${h - bottom}px ${l}px)`;
    return { src: imageUrl(r.source), style: { objectViewBox: inset } };
  }
  return { src: imageUrl(r.source) };
}

onMounted(async () => {
  window.addEventListener("keydown", onKey);
  const state = await invoke<AppState>("get_state");
  config = state.config;
  data.value = state.data;
  output.value = config.output_dir || state.default_output;
  format.value = FORMATS.some((f) => f.value === config.save_format) ? config.save_format! : "webp";
  rounded.value = config.rounded ?? true;
  recursive.value = config.recursive ?? true;
  skipDuplicate.value = config.skip_duplicate ?? true;
  saveReview.value = config.save_review ?? true;
  saveNotCard.value = config.save_not_card ?? true;
  english.value = labelOf(ENGLISH, config.english, "英語名で保存");
  split.value = labelOf(SPLIT, config.split, "分けない");
  status.value = "画像かフォルダをここにドロップしてください";
  if (state.migration) await reportMigration(state.migration);

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
    // 処理・更新・登録のあとの再処理が、中止を受けて保存と記録を終えるまで閉じない
    closing.value = true;
    if (busy.value || registering.value) status.value = "終了しています...";
    try {
      await invoke("shutdown");
    } catch (e) {
      console.error(e);
    }
    await saveConfig();
  });
  setTimeout(offerFirstFetch, 300);
});

onBeforeUnmount(() => {
  window.removeEventListener("keydown", onKey);
  unlisten.forEach((fn) => fn());
});
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
          形式
          <select v-model="format">
            <option v-for="f in FORMATS" :key="f.value" :value="f.value">{{ f.label }}</option>
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

    <div class="work">
      <section class="results">
        <div class="filters">
          <button
            v-for="f in FILTERS"
            :key="f"
            class="chip"
            :class="{ on: filter === f }"
            :disabled="counts[f] === 0"
            @click="filter = f"
          >
            {{ f }} {{ counts[f] }}
          </button>
        </div>
        <div class="head row">
          <span></span>
          <span>状態</span>
          <span>カード名</span>
          <span>元ファイル</span>
        </div>
        <div ref="listEl" class="list" @scroll="onListScroll">
          <div class="spacer" :style="{ height: visible.length * ROW_HEIGHT + 'px' }">
          <div class="window" :style="{ transform: `translateY(${windowStart * ROW_HEIGHT}px)` }">
          <div
            v-for="{ r, i } in windowRows"
            :key="i"
            class="row item"
            :class="['s-' + STATUSES.indexOf(r.status), { sel: selected === i }]"
            @click="selectRow(i)"
          >
            <span class="thumb">
              <img
                v-if="r.status !== 'エラー'"
                :src="cardImage(r).src"
                :style="cardImage(r).style"
                loading="lazy"
                decoding="async"
                alt=""
              />
            </span>
            <span class="state">{{ r.status }}</span>
            <span class="name">
              {{ r.name || "-" }}
              <em v-if="r.english" class="badge">英語版</em>
            </span>
            <span class="file" :title="r.source">
              {{ fileName(r.source) }}
              <small v-if="r.note">{{ r.note }}</small>
            </span>
          </div>
          </div>
          </div>
        </div>
      </section>

      <aside v-if="current" class="review" aria-label="確認">
        <div class="review-head">
          <span class="file" :title="current.source">{{ fileName(current.source) }}</span>
          <button class="plain close" aria-label="閉じる" @click="closePanel">×</button>
        </div>
        <div class="review-body">
          <img
            v-if="current.status !== 'エラー'"
            class="crop"
            :src="cardImage(current).src"
            :style="cardImage(current).style"
            alt=""
          />
          <div class="now" :class="'s-' + STATUSES.indexOf(current.status)">
            <span class="state">{{ current.status }}</span>
            <span class="name">{{ current.name || "-" }}</span>
          </div>
          <div v-if="current.note" class="note">{{ current.note }}</div>
          <ul v-if="candidates.length" class="cands">
            <li v-for="(c, rank) in candidates" :key="c.cid">
              <span class="name" :title="c.name">{{ c.name }}</span>
              <span class="num">{{ rank + 1 }}位</span>
              <button v-if="canRegister" :disabled="locked" @click="register(c.cid)">登録</button>
            </li>
          </ul>
          <input v-if="canRegister" v-model="query" type="search" placeholder="カード名・IDで検索" spellcheck="false" />
          <ul v-if="canRegister && found.length" class="cands">
            <li v-for="c in found" :key="c.cid">
              <span class="name" :title="c.name">{{ c.name }}</span>
              <span class="num">{{ c.cid }}</span>
              <button :disabled="locked" @click="register(c.cid)">登録</button>
            </li>
          </ul>
        </div>
      </aside>
    </div>

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
            <div class="update-choice">
              <button aria-describedby="update-names-description" @click="runUpdate(false)">カード名を取得</button>
              <p id="update-names-description">保存名に使うカード名を更新します。</p>
            </div>
            <div class="update-choice">
              <button aria-describedby="update-cards-description" @click="runUpdate(true)">新しいカードセットを取り込む</button>
              <p id="update-cards-description">新カードや再録イラストの判定用データを追加します。カード名も更新します。</p>
            </div>
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

.work {
  display: flex;
  flex: 1;
  min-height: 320px;
  gap: 10px;
}
.results {
  display: flex;
  flex-direction: column;
  flex: 1;
  min-width: 0;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  overflow: hidden;
}
.row {
  display: grid;
  grid-template-columns: 30px 4.8em minmax(0, 1.1fr) minmax(0, 1.4fr);
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
.spacer {
  position: relative;
}
.window {
  position: absolute;
  inset: 0 0 auto 0;
}
.item {
  height: 46px;
  align-items: center;
  border-bottom: 1px solid color-mix(in srgb, var(--border) 50%, transparent);
}
.thumb {
  display: block;
  width: 30px;
  height: 36px;
  text-align: center;
}
.thumb img {
  height: 36px;
  aspect-ratio: 894 / 1302;
  object-fit: cover;
  border-radius: 2px;
  background: var(--border);
}
.review-body .note {
  color: var(--muted);
  font-size: 12px;
  overflow-wrap: anywhere;
}
.item {
  cursor: pointer;
}
.item:hover {
  background: var(--accent-soft);
}
.item.sel {
  background: var(--accent-soft);
  box-shadow: inset 3px 0 0 var(--accent);
}
.filters {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  padding: 6px 12px;
  border-bottom: 1px solid var(--border);
}
.chip {
  padding: 2px 10px;
  border-radius: 12px;
}
.chip.on {
  background: var(--accent-soft);
  border-color: var(--accent);
  color: var(--accent);
}
.review {
  display: flex;
  flex-direction: column;
  flex: 0 0 280px;
  min-width: 0;
  background: var(--panel);
  border: 1px solid var(--border);
  border-radius: 10px;
  overflow: hidden;
}
.review-head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 6px 6px 6px 12px;
  border-bottom: 1px solid var(--border);
}
.review-head .file {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.close {
  padding: 0 10px;
  font-size: 18px;
  line-height: 1.4;
}
.review-body {
  display: flex;
  flex-direction: column;
  gap: 10px;
  padding: 10px 12px;
  overflow-y: auto;
}
.crop {
  align-self: center;
  max-width: 100%;
  max-height: 130px;
  object-fit: contain;
  border-radius: 4px;
  background: var(--border);
}
.now {
  display: flex;
  gap: 8px;
  align-items: baseline;
}
.now .name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cands {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.cands li {
  display: flex;
  align-items: center;
  gap: 8px;
}
.cands .name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.cands button {
  padding: 2px 10px;
}
.review input[type="search"] {
  background: var(--field);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 5px 8px;
  min-width: 0;
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
.update-choice {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.update-choice + .update-choice {
  margin-top: 8px;
}
.update-choice p {
  margin: 0;
  color: var(--muted);
  font-size: 12px;
  line-height: 1.5;
}
.busy-text {
  min-height: 1.4em;
}
</style>
