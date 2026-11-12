import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import { transformWithOxc } from "vite";

// Actual App and Trash TSX at the external desktop and native confirmation boundaries.
type Element = { type: unknown; props: Record<string, any> };
type Request = { args: any; resolve: (value: any) => void; reject: (error: unknown) => void };
const rootUrl = new URL("./", import.meta.url);
const transformed = new Map<string, string>();
for (const name of readdirSync(rootUrl).filter((name) => name.endsWith(".tsx"))) {
  const url = new URL(name, rootUrl);
  transformed.set(url.href, (await transformWithOxc(readFileSync(url, "utf8"), fileURLToPath(url))).code);
}
const globals = globalThis as typeof globalThis & { trashRemovalHarness: Harness; window: any };
const mocks: Record<string, string> = {
  react: `export const useState = (...args) => globalThis.trashRemovalHarness.useState(...args); export const useRef = (...args) => globalThis.trashRemovalHarness.useRef(...args); export const useEffect = (...args) => globalThis.trashRemovalHarness.useEffect(...args); export const useId = () => 'owned-test-id';`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({type, props}); export const jsxs = jsx; export const Fragment = 'fragment';`,
  "@tauri-apps/api/core": `export const isTauri = () => true; export const convertFileSrc = value => value; export const invoke = (...args) => globalThis.trashRemovalHarness.invoke(...args); export class Channel {}`,
  "@tauri-apps/api/webview": `export const getCurrentWebview = () => ({onDragDropEvent: async () => () => {}});`,
  "@tauri-apps/plugin-dialog": `export const open = args => globalThis.trashRemovalHarness.dialogOpen(args); export const confirm = (...args) => globalThis.trashRemovalHarness.confirm(...args); export const save = async () => null;`,
};
const hooks = registerHooks({
  resolve(specifier, context, next) {
    if (specifier in mocks) return { url: `trash-removal-test:${specifier}`, shortCircuit: true };
    if (specifier.startsWith(".") && context.parentURL?.startsWith(rootUrl.href)) {
      const url = new URL(specifier, context.parentURL);
      for (const extension of ["", ".ts", ".tsx"]) { const candidate = new URL(url.href + extension); if (existsSync(candidate)) return { url: candidate.href, shortCircuit: true }; }
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith("trash-removal-test:")) return { format: "module", source: mocks[url.slice(19)], shortCircuit: true };
    if (transformed.has(url)) return { format: "module", source: transformed.get(url)!, shortCircuit: true };
    if (url.endsWith(".css")) return { format: "module", source: "", shortCircuit: true };
    return next(url, context);
  },
});
const { default: App } = await import(new URL("App.tsx", rootUrl).href);
const library = { root: "/owned/library", name: "Library A", packages: [], folders: [], assetCount: 0 };
const external = { id: 9, packageId: 7, fileId: null, name: "Nature", packageName: "Nature", deletedAt: 1780000000, originalFolder: null, parentTrashed: false };
const fileEntry = { ...external, id: 10, fileId: 14, name: "wind.wav", originalFolder: "WAV" };
const managed = { ...external, id: 11, packageId: 8, name: "Managed", packageName: "Managed" };
const prepared = { token: "39181e80-31eb-4423-a0fb-20fcc29d0500", packageName: "Nature", sourcePath: "/owned/original-import", externalRoot: "/owned/current-external", deletedAt: 1780000000, assetCount: 3, fileCount: 5 };
function last<T>(items: T[]): T { return items[items.length - 1]; }
class Harness {
  private contexts = new Map<string, { slots: any[]; cursor: number }>(); context = "app";
  dirty = true; private pending: (() => void)[] = []; private visited = new Set<string>(); private disposed = false;
  tree: Element | null = null; private timers = new Map<number, () => void>(); private timerId = 0;
  commands: string[] = []; preparations: Request[] = []; removals: Request[] = []; discards: Request[] = []; restores: Request[] = [];
  confirmations: Request[] = []; openDialogs: Request[] = []; openLibraries: Request[] = []; refreshes: Request[] = [];
  initialLibrary: any = library; entries = [external, fileEntry, managed]; deferRefresh = false; deferDiscard = false; trashLoads = 0;
  private deferred(list: Request[], args: any) { return new Promise((resolve, reject) => list.push({ args, resolve, reject })); }
  invoke = (command: string, args: any) => {
    this.commands.push(command);
    if (command === "prepare_trash_removal") return this.deferred(this.preparations, args);
    if (command === "remove_prepared_trash") return this.deferred(this.removals, args);
    if (command === "discard_trash_removal") { const result = this.deferred(this.discards, args); if (!this.deferDiscard) last(this.discards).resolve(undefined); return result; }
    if (command === "restore_trash") return this.deferred(this.restores, args);
    if (command === "trash_entries") { this.trashLoads++; return Promise.resolve(this.entries); }
    if (command === "get_library") return this.deferRefresh ? this.deferred(this.refreshes, args) : Promise.resolve(this.initialLibrary);
    if (command === "query_catalog") return Promise.resolve({ items: [], total: 0, offset: 0, limit: 40 });
    if (command === "catalog_filters") return Promise.resolve({ folders: [], formats: [], tags: [] });
    if (command === "open_library") return this.deferred(this.openLibraries, args);
    throw new Error(`Unexpected desktop command ${command}`);
  };
  confirm = (message: string, options: any) => this.deferred(this.confirmations, { message, options });
  dialogOpen = (args: any) => this.deferred(this.openDialogs, args);
  private current() { let state = this.contexts.get(this.context); if (!state) { state = { slots: [], cursor: 0 }; this.contexts.set(this.context, state); } return state; }
  useState(initial: any) { const state = this.current(), index = state.cursor++; if (!(index in state.slots)) state.slots[index] = typeof initial === "function" ? initial() : initial;
    return [state.slots[index], (value: any) => { const next = typeof value === "function" ? value(state.slots[index]) : value; if (!Object.is(next, state.slots[index])) { state.slots[index] = next; this.dirty = true; } }]; }
  useRef(initial: any) { const state = this.current(), index = state.cursor++; if (!(index in state.slots)) state.slots[index] = { current: initial }; return state.slots[index]; }
  useEffect(effect: () => void | (() => void), dependencies: unknown[]) { const state = this.current(), index = state.cursor++, previous = state.slots[index];
    if (!previous || dependencies.some((value, position) => !Object.is(value, previous.dependencies[position]))) this.pending.push(() => { previous?.cleanup?.(); state.slots[index] = { dependencies, cleanup: effect() }; }); }
  private render(key: string, component: () => Element) { const older = this.context; this.context = key; this.current().cursor = 0; this.visited.add(key); const node = component(); this.context = older; return node; }
  private expand(node: any): any { if (Array.isArray(node)) return node.map((child) => this.expand(child)); if (!node || typeof node !== "object" || !("props" in node)) return node;
    if (typeof node.type === "function" && node.type.name === "TrashDialog") return this.render("trash", () => node.type(node.props));
    return { ...node, props: { ...node.props, children: this.expand(node.props.children) } }; }
  async settle() { globals.trashRemovalHarness = this; globals.window = { setTimeout: (callback: () => void) => { const id = ++this.timerId; this.timers.set(id, callback); return id; }, clearTimeout: (id: number) => this.timers.delete(id) };
    for (let turn = 0; turn < 40; turn++) { if (this.dirty && !this.disposed) { this.dirty = false; this.visited.clear(); this.tree = this.expand(this.render("app", App));
        for (const [key, state] of this.contexts) if (!this.visited.has(key)) { for (const slot of state.slots) if (typeof slot?.cleanup === "function") slot.cleanup(); this.contexts.delete(key); }
        for (const effect of this.pending.splice(0)) effect(); }
      await Promise.resolve(); }
    if (!this.disposed) assert.equal(this.dirty, false, "Render did not settle"); }
  elements(node: any): Element[] { return Array.isArray(node) ? node.flatMap((child) => this.elements(child)) : node && typeof node === "object" && "props" in node ? [node, ...this.elements(node.props.children)] : []; }
  text(node: any): string { return Array.isArray(node) ? node.map((child) => this.text(child)).join(" ") : node && typeof node === "object" ? this.text(node.props?.children) : typeof node === "number" || typeof node === "string" ? String(node) : ""; }
  button(label: string) { const button = this.elements(this.tree).find((node) => node.type === "button" && (node.props["aria-label"] ?? this.text(node).trim()) === label); assert.ok(button, `Missing ${label}`); return button; }
  async click(label: string) { const button = this.button(label); assert.notEqual(button.props.disabled, true, `${label} disabled`); button.props.onClick(); await this.settle(); }
  async mount() { await this.settle(); for (const [id, callback] of this.timers) { this.timers.delete(id); callback(); } await this.settle(); }
  async openTrash() { await this.click("Interner Papierkorb"); }
  async prepare(entry = external) { await this.click(`${entry.name}: Entfernen vorbereiten`); }
  async ready(value = prepared) { last(this.preparations).resolve(value); await this.settle(); }
  async switchLibrary(root: string, name = "Library B") {
    await this.click("Bibliothek öffnen"); last(this.openDialogs).resolve(root); await this.settle();
    last(this.openLibraries).resolve({ ...library, root, name }); await this.settle();
  }
  dispose() { if (this.disposed) return; this.disposed = true; for (const state of this.contexts.values()) for (const slot of state.slots) if (typeof slot?.cleanup === "function") slot.cleanup(); this.tree = null; }
}
test.after(() => hooks.deregister());

test("only packages expose explicit removal preparation; cancellation discards the exact native token without removing", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.openTrash();
    assert.equal(app.preparations.length, 0);
    assert.equal(app.elements(app.tree).some(node => node.props["aria-label"] === "wind.wav: Entfernen vorbereiten"), false);
    await app.prepare();
    assert.deepEqual(app.preparations[0].args, { root: "/owned/library", entry: { id: 9, packageId: 7, name: "Nature", deletedAt: 1780000000 } });
    assert.equal(app.button("Paket importieren").props.disabled, true);
    assert.equal(app.button("Nature wiederherstellen").props.disabled, true);
    await app.ready();
    assert.equal(app.removals.length, 0);
    assert.match(app.confirmations[0].args.message, /Nature/);
    assert.match(app.confirmations[0].args.message, /\/owned\/current-external/);
    assert.match(app.confirmations[0].args.message, /\/owned\/original-import/);
    assert.match(app.confirmations[0].args.message, /3 Assets.*5 Dateien/s);
    assert.match(app.confirmations[0].args.message, /unwiderruflich.*Katalog.*Annotationen.*Projektzuordnungen/s);
    assert.match(app.confirmations[0].args.message, /Originale.*unverändert.*0 Byte/s);
    assert.deepEqual(app.confirmations[0].args.options, { title: "Externes Paket aus dem Katalog entfernen?", kind: "warning", okLabel: "Endgültig aus Katalog entfernen", cancelLabel: "Abbrechen" });
    app.confirmations[0].resolve(false); await app.settle();
    assert.deepEqual(app.discards.map(request => request.args), [{ token: prepared.token }]);
    assert.equal(app.removals.length, 0);
    assert.equal(app.button("Nature wiederherstellen").props.disabled, false);
  } finally { app.dispose(); }
});

test("explicit native approval removes once with only the token and refreshes catalogue and trash after completion", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.openTrash(); app.deferRefresh = true;
    const prepare = app.button("Nature: Entfernen vorbereiten"); prepare.props.onClick(); prepare.props.onClick(); await app.settle();
    assert.equal(app.preparations.length, 1);
    await app.ready(); assert.equal(app.removals.length, 0);
    app.confirmations[0].resolve(true); app.confirmations[0].resolve(true); await app.settle();
    assert.deepEqual(app.removals.map(request => request.args), [{ token: prepared.token }]);
    assert.equal(app.refreshes.length, 0); assert.equal(app.trashLoads, 1);
    assert.equal(app.button("Nature: Entfernen vorbereiten").props.disabled, true);
    app.removals[0].resolve(undefined); await app.settle();
    assert.equal(app.refreshes.length, 1); assert.equal(app.trashLoads, 1);
    assert.equal(app.button("Paket importieren").props.disabled, true);
    app.refreshes[0].resolve({ ...library, name: "Refreshed library" }); await app.settle();
    assert.equal(app.trashLoads, 2); assert.match(app.text(app.tree), /Refreshed library/);
    assert.match(app.text(app.tree), /Nature.*aus dem Katalog entfernt.*Originale.*unverändert.*0 Byte/s);
    assert.equal(app.button("Paket importieren").props.disabled, false);
    assert.equal(app.discards.length, 0);
  } finally { app.dispose(); }
});

test("closing and reopening during preparation discards only the late old token and cannot release or close the new owner", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.openTrash(); await app.prepare();
    const oldClose = app.button("Schließen").props.onClick;
    await app.click("Schließen");
    assert.equal(app.button("Paket importieren").props.disabled, false);
    await app.openTrash(); await app.prepare();
    oldClose(); await app.settle(); assert.ok(app.button("Schließen"));
    app.preparations[0].resolve({ ...prepared, token: "late-old-token" }); await app.settle();
    assert.deepEqual(app.discards.map(request => request.args), [{ token: "late-old-token" }]);
    assert.equal(app.confirmations.length, 0); assert.equal(app.removals.length, 0);
    assert.equal(app.button("Nature: Entfernen vorbereiten").props.disabled, true);
    await app.ready({ ...prepared, token: "new-token" });
    assert.equal(app.confirmations.length, 1);
    app.confirmations[0].resolve(false); await app.settle();
    assert.deepEqual(app.discards.map(request => request.args), [{ token: "late-old-token" }, { token: "new-token" }]);
    assert.equal(app.button("Nature wiederherstellen").props.disabled, false);
  } finally { app.dispose(); }
});

test("unmount discards prepared confirmation immediately and ignores unresolved preparation, rejection or approval", async () => {
  for (const phase of ["prepare-success", "prepare-error", "confirm"] as const) {
    const app = new Harness();
    await app.mount(); await app.openTrash(); await app.prepare();
    if (phase === "confirm") await app.ready();
    app.dispose();
    if (phase === "confirm") {
      assert.deepEqual(app.discards.map(request => request.args), [{ token: prepared.token }]);
      app.confirmations[0].resolve(true);
    } else if (phase === "prepare-success") app.preparations[0].resolve(prepared);
    else app.preparations[0].reject("Late preparation failed");
    await app.settle();
    assert.equal(app.removals.length, 0); assert.equal(app.refreshes.length, 0);
    assert.deepEqual(app.discards.map(request => request.args), phase === "prepare-error" ? [] : [{ token: prepared.token }]);
  }
});

test("delayed preparation and confirmation cannot survive reopen or A-B-A library changes or affect the new owner", async () => {
  for (const phase of ["prepare", "confirm"] as const) {
    const app = new Harness();
    try {
      await app.mount(); await app.openTrash(); await app.prepare();
      const oldPrepare = app.button("Nature: Entfernen vorbereiten").props.onClick;
      const oldClose = app.button("Schließen").props.onClick;
      if (phase === "confirm") await app.ready();
      await app.click("Schließen");
      for (const [index, root] of [library.root, "/owned/B", library.root].entries()) {
        await app.switchLibrary(root); await app.openTrash(); await app.prepare();
        assert.equal(last(app.preparations).args.root, root);
        oldClose(); oldPrepare(); await app.settle();
        if (index === 2) {
          if (phase === "prepare") app.preparations[0].resolve({ ...prepared, token: "old-root-token" });
          else app.confirmations[0].resolve(true);
          await app.settle();
        }
        assert.equal(app.preparations.length, index + 2);
        assert.equal(app.button("Paket importieren").props.disabled, true);
        assert.equal(app.removals.length, 0);
        await app.ready({ ...prepared, token: `current-token-${index}` });
        last(app.confirmations).resolve(false); await app.settle(); await app.click("Schließen");
      }
      assert.equal(app.removals.length, 0);
      assert.deepEqual(app.discards.map(request => request.args.token).sort(), phase === "prepare"
        ? ["current-token-0", "current-token-1", "current-token-2", "old-root-token"]
        : ["current-token-0", "current-token-1", "current-token-2", prepared.token].sort());
    } finally { app.dispose(); }
  }
});

test("close or unmount at either removal or refresh await suppresses stale success and never cancels consumed work", async () => {
  for (const boundary of ["remove", "refresh"] as const) for (const disposal of ["close", "unmount"] as const) {
    const app = new Harness();
    try {
      await app.mount(); await app.openTrash(); await app.prepare(); await app.ready(); app.deferRefresh = true;
      app.confirmations[0].resolve(true); await app.settle();
      if (boundary === "refresh") { app.removals[0].resolve(undefined); await app.settle(); }
      if (disposal === "close") { await app.click("Schließen"); assert.equal(app.button("Paket importieren").props.disabled, true); }
      else app.dispose();
      assert.equal(app.discards.length, 0);
      if (boundary === "remove") { app.removals[0].resolve(undefined); await app.settle(); assert.equal(app.refreshes.length, 0); }
      else { app.refreshes[0].resolve({ ...library, name: "Stale refresh" }); await app.settle(); }
      assert.equal(app.trashLoads, 1); assert.equal(app.discards.length, 0);
      assert.doesNotMatch(app.text(app.tree), /Stale refresh|aus dem Katalog entfernt/);
      if (disposal === "close") assert.equal(app.button("Paket importieren").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("native refusal, confirmation, removal and refresh errors stay visible and permit Restore and explicit preparation again", async () => {
  for (const boundary of ["prepare", "confirm", "remove", "refresh"] as const) {
    const app = new Harness();
    try {
      await app.mount(); await app.openTrash(); await app.prepare(boundary === "prepare" ? managed : external);
      const message = boundary === "prepare" ? "Nur vollständig gelöschte externe Pakete können aus dem Katalog entfernt werden" : `${boundary} failed`;
      if (boundary === "prepare") app.preparations[0].reject(message);
      else {
        await app.ready();
        if (boundary === "confirm") app.confirmations[0].reject(message);
        else {
          app.deferRefresh = boundary === "refresh"; app.confirmations[0].resolve(true); await app.settle();
          if (boundary === "remove") app.removals[0].reject(message);
          else { app.removals[0].resolve(undefined); await app.settle(); app.refreshes[0].reject(message); }
        }
      }
      await app.settle();
      assert.ok(app.elements(app.tree).some(node => node.props.role === "alert" && app.text(node).includes(message)));
      assert.equal(app.button("Nature wiederherstellen").props.disabled, false);
      assert.equal(app.button("Nature: Entfernen vorbereiten").props.disabled, false);
      assert.equal(app.trashLoads, 1); assert.equal(app.discards.length, boundary === "confirm" ? 1 : 0);
      await app.prepare(); assert.equal(app.preparations.length, 2);
      app.preparations[1].reject("Explicit retry failed"); await app.settle();
      assert.equal(app.button("Nature wiederherstellen").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("a Restore operation shares the busy guard and stale Prepare callbacks cannot start removal preparation", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.openTrash();
    const prepare = app.button("Nature: Entfernen vorbereiten").props.onClick;
    const restore = app.button("wind.wav wiederherstellen").props.onClick;
    restore(); restore(); prepare(); await app.settle();
    assert.deepEqual(app.restores.map(request => request.args), [{ trashId: 10 }]);
    assert.equal(app.preparations.length, 0); assert.equal(app.button("Nature: Entfernen vorbereiten").props.disabled, true);
    app.restores[0].resolve(undefined); await app.settle();
    assert.match(app.text(app.tree), /wind.wav wiederhergestellt/);
    assert.equal(app.button("Nature: Entfernen vorbereiten").props.disabled, false);
    assert.equal(app.removals.length, 0);
  } finally { app.dispose(); }
});

test("failed cancellation remains an error and never removes or reuses its token", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.openTrash(); await app.prepare(); await app.ready(); app.deferDiscard = true;
    app.confirmations[0].resolve(false); await app.settle();
    assert.deepEqual(app.discards[0].args, { token: prepared.token });
    assert.equal(app.button("Nature wiederherstellen").props.disabled, true);
    app.discards[0].reject("Discard transport failed"); await app.settle();
    assert.match(app.text(app.tree), /Discard transport failed/); assert.equal(app.removals.length, 0);
    assert.equal(app.discards.length, 1); assert.equal(app.button("Nature wiederherstellen").props.disabled, false);
    await app.prepare(); assert.equal(app.preparations.length, 2); assert.equal(app.removals.length, 0);
    app.preparations[1].reject("Retry refused"); await app.settle();
  } finally { app.dispose(); }
});
