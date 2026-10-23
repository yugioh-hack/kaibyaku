// ブラウザーで画面を確認するためのモック（開発時のみ読み込まれる）。
import type { IdentifyResult } from "./types";

type Handler = (payload: unknown) => void;
const listeners = new Map<string, Set<Handler>>();
let busy = false;
let timers: number[] = [];
let finish: ((cancelled: boolean) => void) | null = null;
const data = { cards: 14000, hashes: 15230, names: 0, names_fetched: false };
let config: Record<string, unknown> = {};

function emit(event: string, payload: unknown) {
  listeners.get(event)?.forEach((h) => h(payload));
}

export async function listen(event: string, handler: Handler): Promise<() => void> {
  if (!listeners.has(event)) listeners.set(event, new Set());
  listeners.get(event)!.add(handler);
  return () => listeners.get(event)?.delete(handler);
}

let dropHandlers: { hover: (a: boolean) => void; drop: (p: string[]) => void } | null = null;
(window as unknown as Record<string, unknown>).__mockDrop = (paths: string[] = ["C:\\mock\\shots"]) =>
  dropHandlers?.drop(paths);

export async function onDrop(hover: (a: boolean) => void, drop: (p: string[]) => void) {
  dropHandlers = { hover, drop };
  return () => {
    dropHandlers = null;
  };
}

export async function pickFiles() {
  return ["C:\\mock\\shots\\a.png"];
}
export async function pickFolder() {
  return "C:\\mock\\folder";
}
export async function message(text: string, kind: string) {
  console.info(`[mock message:${kind}]`, text);
}
export async function ask(text: string) {
  console.info("[mock ask]", text);
  return new URLSearchParams(location.search).get("ask") === "yes";
}

const SAMPLE_NAMES = ["ブラック・マジシャン", "青眼の白龍", "灰流うらら", "增殖するG", "サンダー・ドラゴン"];

function makeResult(i: number): IdentifyResult {
  const kind = i % 7 === 3 ? "要確認" : i % 11 === 5 ? "カードなし" : i % 17 === 9 ? "エラー" : "確定";
  const english = i % 5 === 4;
  const none = kind === "カードなし" || kind === "エラー";
  const saved = `C:\\mock\\out\\${kind === "確定" ? "" : kind === "要確認" ? "review\\" : "not_card\\"}card_${i}.webp`;
  return {
    source: `C:\\mock\\shots\\スクリーンショット ${String(i + 1).padStart(3, "0")}.png`,
    status: kind,
    card_id: none ? null : 4000 + i,
    name: none ? null : english ? "Blue-Eyes White Dragon" : SAMPLE_NAMES[i % 5],
    distance: none ? 999 : kind === "要確認" ? 14 : i % 6,
    margin: 5,
    saved_path: kind === "エラー" ? null : saved,
    saved_paths: kind === "エラー" ? [] : [saved],
    ranked:
      kind === "要確認" || (kind === "カードなし" && i % 2 === 0)
        ? [
            [kind === "要確認" ? 14 : 31, String(4000 + i)],
            [kind === "要確認" ? 16 : 33, String(5000 + i)],
            [kind === "要確認" ? 21 : 38, String(6000 + i)],
          ]
        : [],
    note: kind === "エラー" ? "画像を読み込めません" : kind === "要確認" ? "2位と近い" : "",
    english,
  };
}

function startIdentify(args: Record<string, unknown>) {
  if (busy) throw "いま処理中です。終わるまでお待ちください。";
  const output = String(args.args ? (args.args as Record<string, unknown>).output : "").trim();
  if (!output) throw "出力先フォルダを指定してください。";
  if (!/^[A-Za-z]:\\/.test(output)) throw "出力先フォルダは C:\\ から始まる場所を指定してください。";
  busy = true;
  const total = Number(new URLSearchParams(location.search).get("n") ?? 35);
  const counts = { 確定: 0, 要確認: 0, カードなし: 0, エラー: 0 };
  let done = 0;
  finish = (cancelled) => {
    timers.forEach(clearInterval);
    timers = [];
    busy = false;
    finish = null;
    emit("identify-done", {
      confirmed: counts.確定,
      review: counts.要確認,
      not_card: counts.カードなし,
      error: counts.エラー,
      skipped: 2,
      seconds: done * 0.05,
      total,
      cancelled,
    });
  };
  if (total === 0) {
    queueMicrotask(() => finish?.(false));
    return;
  }
  timers.push(
    window.setInterval(() => {
      // ?batch=200 で1回に流す件数を増やす（大量の行の表示確認用）
      const batch = Number(new URLSearchParams(location.search).get("batch") ?? 1);
      for (let k = 0; k < batch && done < total; k++) {
        const result = makeResult(done);
        counts[result.status]++;
        done++;
        emit("identify-progress", { done, total, result });
      }
      if (done >= total) finish?.(false);
    }, 50),
  );
}

function startUpdate(hashes: boolean) {
  if (busy) throw "いま処理中です。終わるまでお待ちください。";
  busy = true;
  let n = 0;
  finish = (cancelled) => {
    timers.forEach(clearInterval);
    timers = [];
    busy = false;
    finish = null;
    if (cancelled) return emit("update-done", { message: "中止しました" });
    data.names = 12345;
    data.names_fetched = true;
    if (hashes) data.hashes += 48;
    emit("update-done", {
      message: hashes ? "カード名 12345 件を取得しました / 新しいカードセットはありませんでした" : "カード名 12345 件を取得しました",
    });
  };
  timers.push(
    window.setInterval(() => {
      n++;
      if (n > 5) return finish?.(false);
      emit("update-progress", n < 4 ? `カード名を取得中... ${n * 2000} 件` : "新しいカードセットを確認中...");
    }, 400),
  );
}

export function imageUrl(path: string): string {
  let h = 0;
  for (const c of path) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const hue = h % 360;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="89" height="130" viewBox="0 0 89 130">` +
    `<rect width="89" height="130" rx="6" fill="hsl(${hue} 45% 45%)"/>` +
    `<rect x="8" y="8" width="73" height="114" rx="3" fill="none" stroke="hsl(${hue} 60% 85%)" stroke-width="3"/>` +
    `<rect x="14" y="30" width="61" height="52" fill="hsl(${hue} 40% 70%)"/></svg>`;
  return "data:image/svg+xml," + encodeURIComponent(svg);
}

const pause = (ms: number) => new Promise((r) => setTimeout(r, ms));
const cardName = (cid: string) => `モックカード ${cid}`;
const CATALOG = ["4003", "4010", "5003", "6003", "7001"].map((cid, i) => ({
  cid,
  name: i < SAMPLE_NAMES.length ? SAMPLE_NAMES[i] : cardName(cid),
}));

export async function invoke(command: string, args: Record<string, unknown> = {}): Promise<unknown> {
  switch (command) {
    case "get_state":
      return {
        version: "0.1.0",
        user_data: "C:\\Users\\mock\\AppData\\Roaming\\mdript-v2",
        default_output: "C:\\Users\\mock\\Pictures\\mdript",
        config,
        data: { ...data },
      };
    case "save_config":
      config = { ...config, ...(args.config as object) };
      return;
    case "start_identify":
      return startIdentify(args);
    case "start_update":
      return startUpdate(Boolean(args.hashes));
    case "stop":
      finish?.(true);
      return;
    case "data_summary":
      return { ...data };
    case "identify_one": {
      if (busy) throw "いま処理中です。終わるまでお待ちください。";
      await pause(400);
      const source = String(args.path);
      const saved = `C:\\mock\\out\\card_new_${Date.now() % 100000}.webp`;
      return {
        result: {
          source,
          status: "確定",
          card_id: 4003,
          name: SAMPLE_NAMES[0],
          distance: 0,
          margin: 20,
          saved_path: saved,
          saved_paths: [saved],
          ranked: [[0, "4003"]],
          note: "",
          english: false,
        },
        csv_error: null,
      };
    }
    case "describe_cards":
      await pause(80);
      return (args.cids as string[]).map((cid) => ({
        cid,
        name: CATALOG.find((c) => c.cid === cid)?.name ?? cardName(cid),
      }));
    case "search_cards": {
      await pause(120);
      const q = String(args.query).trim().toLowerCase();
      if (!q) return [];
      if (q === "error") throw "検索に失敗しました";
      return CATALOG.filter((c) => c.name.toLowerCase().includes(q) || c.cid.includes(q));
    }
    case "learn_card":
      await pause(300);
      if (args.cid === "6003") throw "カードの位置を決められませんでした（モック）";
      if (args.cid === "5003") return { message: "近いハッシュがすでにあるので、追加しません", reprocess: false };
      data.hashes += 1;
      return { message: "照合用ハッシュを追加しました", reprocess: true };
    case "remove_review_copy":
      console.info("[mock remove_review_copy]", args.paths, args.output);
      return;
    case "shutdown":
      finish?.(true);
      return;
    case "open_folder":
      console.info("[mock open_folder]", args.path);
      return;
  }
  throw `unknown command: ${command}`;
}
