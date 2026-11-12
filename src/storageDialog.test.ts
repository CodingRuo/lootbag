import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import { transformWithOxc } from "vite";

type Element = { type: unknown; props: Record<string, any> };
type Request = { args: any; resolve: (result: any) => void; reject: (error: unknown) => void };
const source = new URL("./StorageDialog.tsx", import.meta.url);
const transformed = (await transformWithOxc(readFileSync(source, "utf8"), fileURLToPath(source))).code;
const globals = globalThis as typeof globalThis & { storageHarness: Harness };
const mocks: Record<string, string> = {
  react: `export const useState = (...args) => globalThis.storageHarness.useState(...args); export const useRef = (...args) => globalThis.storageHarness.useRef(...args); export const useEffect = (...args) => globalThis.storageHarness.useEffect(...args);`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({ type, props }); export const jsxs = jsx; export const Fragment = "fragment";`,
  "@tauri-apps/api/core": `export const invoke = (...args) => globalThis.storageHarness.invoke(...args); export class Channel { constructor() { globalThis.storageHarness.channels.push(this); } }`,
};
const hooks = registerHooks({
  resolve(specifier, context, next) { return specifier in mocks ? { url: `storage-test:${specifier}`, shortCircuit: true } : next(specifier, context); },
  load(url, context, next) {
    if (url.startsWith("storage-test:")) return { format: "module", source: mocks[url.slice(13)], shortCircuit: true };
    if (url === source.href) return { format: "module", source: transformed, shortCircuit: true };
    return next(url, context);
  },
});
const { StorageDialog } = await import(source.href);
const complete = { complete: true, cancelled: false, startedAtMs: 0, finishedAtMs: 1000, totalBytes: 18,
  categories: [{ category: "managed", bytes: 18, files: 4, paths: 4 }], visitedEntries: 6, excludedEntries: 0, issueCount: 0, issues: [],
  external: { recordedBytes: 5, recordedVersions: 1, references: 2, activeVersions: 0, trashVersions: 0, sharedVersions: 1, unqualifiedReferences: 0 } };
class Harness {
  slots: any[] = []; cursor = 0; dirty = true; pending: (() => void)[] = [];
  root = "/owned/library"; closed = 0; tree: Element | null = null;
  requests: Request[] = []; channels: any[] = []; cancellations: string[] = [];
  invoke = (command: string, args: any) => {
    if (command === "storage_report") return new Promise((resolve, reject) => this.requests.push({ args, resolve, reject }));
    if (command === "cancel_storage_report") { this.cancellations.push(args.requestId); return Promise.resolve(); }
    throw new Error(`Unexpected operation: ${command}`);
  };
  useState(initial: any) {
    const index = this.cursor++; if (!(index in this.slots)) this.slots[index] = initial;
    return [this.slots[index], (value: any) => { const next = typeof value === "function" ? value(this.slots[index]) : value; if (!Object.is(next, this.slots[index])) { this.slots[index] = next; this.dirty = true; } }];
  }
  useRef(initial: any) { const index = this.cursor++; if (!(index in this.slots)) this.slots[index] = { current: initial }; return this.slots[index]; }
  useEffect(effect: () => void | (() => void), dependencies: unknown[]) {
    const index = this.cursor++; const previous = this.slots[index];
    if (!previous || dependencies.some((value, position) => !Object.is(value, previous.dependencies[position]))) this.pending.push(() => { previous?.cleanup?.(); this.slots[index] = { dependencies, cleanup: effect() }; });
  }
  async settle() {
    globals.storageHarness = this;
    for (let turn = 0; turn < 20; turn++) {
      if (this.dirty) { this.dirty = false; this.cursor = 0; this.tree = StorageDialog({ root: this.root, formatBytes: (bytes: number) => `${bytes} bytes`, onClose: () => { this.closed++; } }); for (const effect of this.pending.splice(0)) effect(); }
      await Promise.resolve(); await Promise.resolve(); if (!this.dirty && !this.pending.length) return;
    }
    throw new Error("Render did not settle");
  }
  elements(node: any): Element[] { if (Array.isArray(node)) return node.flatMap((item) => this.elements(item)); return !node || typeof node !== "object" || !("props" in node) ? [] : [node, ...this.elements(node.props.children)]; }
  text(node: any): string { return Array.isArray(node) ? node.map((item) => this.text(item)).join(" ") : node && typeof node === "object" ? this.text(node.props?.children) : typeof node === "string" || typeof node === "number" ? String(node) : ""; }
  button(label: string) { const button = this.elements(this.tree).find((node) => node.type === "button" && this.text(node) === label); assert.ok(button, `Missing ${label}`); return button; }
  async click(label: string) { this.button(label).props.onClick(); await this.settle(); }
  dispose() { for (const slot of this.slots) slot?.cleanup?.(); }
}
test.after(() => hooks.deregister());

test("storage dialog waits for explicit Refresh and exposes bounded progress and honest separate metrics", async () => {
  const app = new Harness();
  try {
    await app.settle(); assert.equal(app.requests.length, 0);
    await app.click("Aktualisieren");
    assert.equal(app.requests[0].args.root, "/owned/library");
    assert.equal(app.button("Aktualisieren").props.disabled, true);
    app.channels[0].onmessage({ stage: "verify", visitedEntries: 123 }); await app.settle();
    assert.match(app.text(app.tree), /123\s+Einträge geprüft/);
    app.requests[0].resolve(complete); await app.settle();
    assert.match(app.text(app.tree), /Messung vollständig/);
    assert.match(app.text(app.tree), /Gesamt 18 bytes/);
    assert.match(app.text(app.tree), /Externe Quellen.*historische Katalogwerte/);
    assert.match(app.text(app.tree), /gehören nicht zur Bibliothekssumme/);
    assert.match(app.text(app.tree), /keine Schätzung des freigebbaren Speicherplatzes/);
    assert.equal(app.button("Aktualisieren").props.disabled, false);
  } finally { app.dispose(); }
});

test("storage stop uses the active token and displays incomplete partial values without implicit refresh", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.click("Aktualisieren"); await app.click("Messung abbrechen");
    assert.deepEqual(app.cancellations, [app.requests[0].args.requestId]);
    assert.equal(app.button("Abbruch angefordert …").props.disabled, true);
    app.requests[0].resolve({ ...complete, complete: false, cancelled: true }); await app.settle();
    assert.match(app.text(app.tree), /Messung abgebrochen.*unvollständig/);
    assert.match(app.text(app.tree), /Bisher gemessen 18 bytes/);
    assert.equal(app.requests.length, 1);
  } finally { app.dispose(); }
});

test("root changes cancel old work and old progress/completion cannot replace a newer report", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.click("Aktualisieren");
    app.root = "/owned/other"; app.dirty = true; await app.settle();
    assert.deepEqual(app.cancellations, [app.requests[0].args.requestId]);
    await app.click("Aktualisieren");
    app.channels[0].onmessage({ stage: "walk", visitedEntries: 999 });
    app.requests[0].resolve(complete); await app.settle();
    assert.equal(app.button("Aktualisieren").props.disabled, true);
    assert.doesNotMatch(app.text(app.tree), /999|Messung vollständig/);
    app.requests[1].resolve({ ...complete, complete: false, issueCount: 2, issues: ["Missing source", "Root changed"], excludedEntries: 1 }); await app.settle();
    assert.match(app.text(app.tree), /Messung unvollständig/);
    assert.match(app.text(app.tree), /Missing source/);
    assert.match(app.text(app.tree), /1\s+Einträge ausgelassen/);
  } finally { app.dispose(); }
});

test("closing cancels the active job and ignores late completion", async () => {
  const app = new Harness();
  await app.settle(); await app.click("Aktualisieren"); await app.click("Schließen");
  assert.equal(app.closed, 1); assert.deepEqual(app.cancellations, [app.requests[0].args.requestId]);
  app.requests[0].resolve(complete); await app.settle(); assert.doesNotMatch(app.text(app.tree), /Messung vollständig/);
  app.dispose(); assert.equal(app.cancellations.length, 1);
});

test("unsupported or failed reports remain errors and allow another explicit attempt", async () => {
  const app = new Harness();
  try { await app.settle(); await app.click("Aktualisieren"); app.requests[0].reject("Unavailable on this platform"); await app.settle();
    assert.match(app.text(app.tree), /Unavailable on this platform/); assert.doesNotMatch(app.text(app.tree), /Messung vollständig/);
    await app.click("Aktualisieren"); assert.equal(app.requests.length, 2);
  } finally { app.dispose(); }
});


test("cancel before catalogue capture keeps external figures unknown", async () => {
  const app = new Harness();
  try { await app.settle(); await app.click("Aktualisieren"); app.requests[0].resolve({ ...complete, complete: false, cancelled: true, external: null }); await app.settle();
    assert.match(app.text(app.tree), /Externe Zuordnung wurde noch nicht vollständig erfasst/);
    assert.doesNotMatch(app.text(app.tree), /0 aufgezeichneten Dateiversionen/);
  } finally { app.dispose(); }
});
