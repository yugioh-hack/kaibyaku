// Tauri との橋渡し。ブラウザー（開発サーバー）ではモックに差し替える。
import { convertFileSrc, invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { ask as tauriAsk, message as tauriMessage, open } from "@tauri-apps/plugin-dialog";

const TITLE = "開闢 - MDカード画像ツール";
const IMAGE_FILTER = [{ name: "画像", extensions: ["png", "jpg", "jpeg", "webp", "bmp"] }];

type Mock = typeof import("./mock");
let mock: Mock | null = null;

export async function initBackend(): Promise<void> {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    mock = await import("./mock");
  }
}

export function invoke<T = void>(command: string, args?: Record<string, unknown>): Promise<T> {
  return mock ? (mock.invoke(command, args) as Promise<T>) : tauriInvoke<T>(command, args);
}

export async function listen<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
  if (mock) return mock.listen(event, handler as (payload: unknown) => void);
  return tauriListen<T>(event, (e) => handler(e.payload));
}

/** ドロップ状態（enter/leave）とドロップされたパスを受け取る。 */
export async function onDrop(
  hover: (active: boolean) => void,
  drop: (paths: string[]) => void,
): Promise<() => void> {
  if (mock) return mock.onDrop(hover, drop);
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload;
    if (p.type === "enter" || p.type === "over") hover(true);
    else if (p.type === "leave") hover(false);
    else if (p.type === "drop") {
      hover(false);
      drop(p.paths);
    }
  });
}

export async function onClose(handler: () => Promise<void>): Promise<void> {
  if (mock) return;
  await getCurrentWindow().onCloseRequested(async () => {
    await handler();
  });
}

export async function pickFiles(): Promise<string[]> {
  if (mock) return mock.pickFiles();
  const chosen = await open({ title: "スクリーンショットを選ぶ", multiple: true, filters: IMAGE_FILTER });
  return chosen ?? [];
}

export async function pickFolder(title: string, defaultPath?: string): Promise<string | null> {
  if (mock) return mock.pickFolder();
  const chosen = await open({ title, directory: true, multiple: false, defaultPath: defaultPath || undefined });
  return chosen ?? null;
}

export async function message(text: string, kind: "info" | "warning" | "error" = "info"): Promise<void> {
  if (mock) return mock.message(text, kind);
  await tauriMessage(text, { title: TITLE, kind });
}

export async function ask(text: string): Promise<boolean> {
  if (mock) return mock.ask(text);
  return tauriAsk(text, { title: TITLE, kind: "info" });
}

/** 画像ファイルの表示用URL。ブラウザー（モック）では代わりの絵を返す。 */
export function imageUrl(path: string): string {
  return mock ? mock.imageUrl(path) : convertFileSrc(path);
}
