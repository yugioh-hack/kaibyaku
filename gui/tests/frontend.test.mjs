import { createRequire } from "node:module";
import { compile } from "@vue/compiler-ssr";
import { createSSRApp } from "vue";
import { renderToString } from "vue/server-renderer";
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";

const source = readFileSync(new URL("../src/App.vue", import.meta.url), "utf8");
const script = source.match(/<script setup lang="ts">([\s\S]*?)<\/script>/)[1];
const ast = ts.createSourceFile("App.ts", script, ts.ScriptTarget.Latest, true);

// 実際の登録関数を実行し、Tauriへの呼び出しと結果一覧の更新を確かめる。
async function reviewCopies() {
  const image = ast.statements.find((s) => ts.isFunctionDeclaration(s) && s.name.text === "cardImage");
  const imageCode = ts.transpileModule(image.getText(ast), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  const preview = vm.runInNewContext(imageCode + '\ncardImage({ source: "private/source.png", saved_path: "", box: null })', { imageUrl: (path) => path });
  assert.equal(preview.src, "private/source.png");
  const declaration = ast.statements.find((s) => ts.isFunctionDeclaration(s) && s.name.text === "register");
  const code = ts.transpileModule(declaration.getText(ast), { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText;
  for (const folder of ["review", "detail/review", "not_card"]) {
    for (const status of ["確定", "要確認", "カードなし"]) {
      for (const saved of [true, false]) {
        const old = { source: "input.png", saved_paths: [`${folder}/old.webp`, `${folder}/old.png`], args: { output: "original-output", detail: folder === "detail/review" ? "folder" : "suffix" } };
        const fresh = { status, saved_path: saved ? `${folder}/new.webp` : "", saved_paths: saved ? [`${folder}/new.webp`, `${folder}/new.png`] : [] };
        const calls = [];
        const context = {
          selected: { value: 0 }, current: { value: old }, locked: { value: false },
          registering: { value: false }, closing: { value: false }, rows: { value: [old] }, status: { value: "" },
          triggerRef() {}, closePanel() {}, async selectRow() {}, async refreshData() {}, async warnCsv() {},
          async message(error) { throw new Error(error); }, console,
          async invoke(command, args) {
            calls.push({ command, args });
            if (command === "learn_card") return { reprocess: true, message: "登録しました" };
            if (command === "identify_one") return { result: fresh };
          },
        };
        await vm.runInNewContext(code + '\nregister("42")', context);
        assert.equal(calls.find((c) => c.command === "identify_one").args.args.detail, old.args.detail);
        const removed = calls.filter((c) => c.command === "remove_review_copy");
        assert.equal(removed.length, saved ? 1 : 0, `${status}: ${saved}`);
        if (saved) {
          assert.deepEqual(Array.from(removed[0].args.paths), old.saved_paths);
          assert.equal(removed[0].args.output, old.args.output);
        }
        assert.equal(context.rows.value[0].status, status);
        assert.equal(context.registering.value, false);
      }
    }
  }
}

test("登録後は新しい保存ができた場合だけ古いコピーを削除する", reviewCopies);

// 実際の設定欄をVueで描画し、無効属性を確かめる。
async function settings(overrides) {
  const template = source.match(/<section class="panel">[\s\S]*?<\/section>/)[0];
  const { code } = compile(template, { mode: "function" });
  const ssrRender = new Function("require", code)(createRequire(import.meta.url));
  const state = { output: "C:/cards", format: "webp", english: "英語名で保存", split: "ja と en に分ける",
    rounded: true, recursive: true, skipDuplicate: true, saveReview: true, saveNotCard: true, locked: false, detail: "detail/ に分ける", DETAIL: { "detail/ に分ける": "folder", "名前に _detail を付ける": "suffix" },
    FORMATS: [{ value: "webp", label: "webp" }], ENGLISH: { 英語名で保存: "name", 区別しない: "off" },
    SPLIT: { 分けない: "none", "ja と en に分ける": "both" }, chooseOutput() {}, openOutput() {}, ...overrides };
  return renderToString(createSSRApp({ ssrRender, setup: () => state }));
}

async function splitSetting() {
  for (const english of ["英語名で保存", "日本語名で保存", "区別しない"]) {
    const html = await settings({ english });
    const selects = [...html.matchAll(/<select([^>]*)>/g)];
    assert.equal(selects[2][1].includes("disabled"), english === "区別しない");
    assert.match(html, /<option>ja と en に分ける<\/option>/);
  }
}


test("言語を区別しない間は言語別フォルダを無効にする", splitSetting);
