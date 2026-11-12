import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import { transformWithOxc } from "vite";
import type { PreparedImport } from "./ImportReviewDialog";

// Actual App and controlled review TSX at the external desktop/Channel boundary.
type Element = { type: unknown; props: Record<string, any> };
type Request = { args: any; resolve: (value: any) => void; reject: (error: unknown) => void };
const rootUrl = new URL("./", import.meta.url);
const transformed = new Map<string, string>();
for (const name of readdirSync(rootUrl).filter((name) => name.endsWith(".tsx"))) {
  const url = new URL(name, rootUrl);
  transformed.set(url.href, (await transformWithOxc(readFileSync(url, "utf8"), fileURLToPath(url))).code);
}
const globals = globalThis as typeof globalThis & { importHarness: Harness; window: any };
const mocks: Record<string, string> = {
  react: `export const useState = (...args) => globalThis.importHarness.useState(...args); export const useRef = (...args) => globalThis.importHarness.useRef(...args); export const useEffect = (...args) => globalThis.importHarness.useEffect(...args); export const useId = () => 'owned-test-id';`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({type, props}); export const jsxs = jsx; export const Fragment = 'fragment';`,
  "@tauri-apps/api/core": `export const isTauri = () => true; export const convertFileSrc = value => value; export const invoke = (...args) => globalThis.importHarness.invoke(...args); export class Channel { constructor(){ globalThis.importHarness.channels.push(this); } }`,
  "@tauri-apps/api/webview": `export const getCurrentWebview = () => ({onDragDropEvent: async callback => { const app=globalThis.importHarness; app.drop=callback; return () => app.unlistened++; }});`,
  "@tauri-apps/plugin-dialog": `export const open = args => globalThis.importHarness.dialogOpen(args); export const confirm = (...args) => globalThis.importHarness.confirm(...args); export const save = async () => null;`,
};
const hooks = registerHooks({
  resolve(specifier, context, next) {
    if (specifier in mocks) return { url: `import-test:${specifier}`, shortCircuit: true };
    if (specifier.startsWith(".") && context.parentURL?.startsWith(rootUrl.href)) {
      const url = new URL(specifier, context.parentURL);
      for (const extension of ["", ".ts", ".tsx"]) { const candidate = new URL(url.href + extension); if (existsSync(candidate)) return { url: candidate.href, shortCircuit: true }; }
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith("import-test:")) return { format: "module", source: mocks[url.slice(12)], shortCircuit: true };
    if (transformed.has(url)) return { format: "module", source: transformed.get(url)!, shortCircuit: true };
    if (url.endsWith(".css")) return { format: "module", source: "", shortCircuit: true };
    return next(url, context);
  },
});
const { default: App } = await import(new URL("App.tsx", rootUrl).href);
const library = { root: "/owned/library", name: "Library", packages: [], folders: [], assetCount: 0 };
const imported = { id: 1, name: "Owned Package", relativePath: "packages/owned", assetCount: 1, sourceKind: "managed", available: true, externalRoot: null, tags: [] };
const clean: PreparedImport = { requestId: "", source: { path: "/owned/source", isZip: false }, binding: "verified", zipEstimate: null,
  evidence: { status: "complete", incomingFiles: 3, candidates: 1, examinedCandidates: 1, duplicateIncomingFiles: 0, unavailableCandidates: 0, existingVariants: 2, unavailableExistingVariants: 0, matches: [], issues: [], detailsTruncated: false } };
const duplicate: PreparedImport = { ...clean, evidence: { ...clean.evidence, duplicateIncomingFiles: 1, matches: [{ incomingRelativePath: "FBX/Zombie.fbx", sizeBytes: 18, existingFileId: 11, existingAssetId: 7, existingPackageId: 3, existingPackageName: "Apocalypse", existingOriginalFilename: "SK_Zombie.fbx", existingRelativePath: "FBX/Characters/SK_Zombie.fbx", sha256: "a".repeat(64) }] } };
const zip: PreparedImport = { ...clean, source: { path: "/owned/source.zip", isZip: true }, binding: "unverified", evidence: { ...clean.evidence, status: "unavailable", incomingFiles: null, candidates: null, examinedCandidates: 0, existingVariants: null, issues: ["ZIP-Inhalt wurde nicht auf Duplikate geprüft"] } };
class Harness {
  private contexts = new Map<string, { slots: any[]; cursor: number }>(); context = "app";
  dirty = true; private pending: (() => void)[] = []; private visited = new Set<string>(); private disposed = false;
  tree: Element | null = null; private timers = new Map<number, () => void>(); private timerId = 0;
  channels: any[] = []; picks: Request[] = []; prepares: Request[] = []; authorizations: Request[] = []; consumes: Request[] = []; cancellations: Request[] = [];
  confirmations: Request[] = []; openDialogs: Request[] = []; openLibraries: Request[] = [];
  legacy: string[] = []; commands: string[] = []; deferredCancel = false; initialLibrary = library;
  drop: (event: any) => void = () => {}; unlistened = 0;
  afterRender: (() => void) | null = null;
  private deferred(list: Request[], args: any) { return new Promise((resolve, reject) => list.push({ args, resolve, reject })); }
  invoke = (command: string, args: any) => {
    this.commands.push(command);
    if (command === "select_import_source") return this.deferred(this.picks, args);
    if (command === "prepare_import_package") return this.deferred(this.prepares, args);
    if (command === "authorize_prepared_import") return this.deferred(this.authorizations, args);
    if (command === "import_prepared_package") return this.deferred(this.consumes, args);
    if (command === "cancel_prepared_import") { const value = this.deferred(this.cancellations, args); if (!this.deferredCancel) this.cancellations[this.cancellations.length - 1].resolve(undefined); return value; }
    if (command === "dropped_import_sources") return Promise.resolve(args.sources.map((path: string) => ({ path, isZip: path.endsWith(".zip") })));
    if (command === "get_library") return Promise.resolve(this.initialLibrary);
    if (command === "query_catalog") return Promise.resolve({ items: [], total: 0, offset: 0, limit: 40 });
    if (command === "catalog_filters") return Promise.resolve({ folders: [], formats: [], tags: [] });
    if (command === "open_library") return this.deferred(this.openLibraries, args);
    if (command === "import_folder" || command === "import_zip") { this.legacy.push(command); return Promise.resolve(imported); }
    if (command === "estimate_zip" || command === "cancel_import") { this.legacy.push(command); return Promise.resolve(command === "cancel_import" ? true : { hasSpace: true, entryCount: 3, uncompressedBytes: 3, requiredBytes: 6, availableBytes: 100 }); }
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
    if (typeof node.type === "function" && node.type.name === "ImportReviewDialog") return this.render(`review:${node.props.prepared.requestId}`, () => node.type(node.props));
    return { ...node, props: { ...node.props, children: this.expand(node.props.children) } }; }
  async settle() { globals.importHarness = this; globals.window = { setTimeout: (callback: () => void) => { const id = ++this.timerId; this.timers.set(id, callback); return id; }, clearTimeout: (id: number) => this.timers.delete(id) };
    for (let turn = 0; turn < 40; turn++) { if (this.dirty && !this.disposed) { this.dirty = false; this.visited.clear(); this.tree = this.expand(this.render("app", App)); this.afterRender?.();
        for (const [key, state] of this.contexts) if (!this.visited.has(key)) { for (const slot of state.slots) slot?.cleanup?.(); this.contexts.delete(key); }
        for (const effect of this.pending.splice(0)) effect(); }
      await Promise.resolve(); }
    if (!this.disposed) assert.equal(this.dirty, false, "Render did not settle"); }
  elements(node: any): Element[] { return Array.isArray(node) ? node.flatMap((child) => this.elements(child)) : node && typeof node === "object" && "props" in node ? [node, ...this.elements(node.props.children)] : []; }
  text(node: any): string { return Array.isArray(node) ? node.map((child) => this.text(child)).join(" ") : node && typeof node === "object" ? this.text(node.props?.children) : typeof node === "number" || typeof node === "string" ? String(node) : ""; }
  button(label: string) { const button = this.elements(this.tree).find((node) => node.type === "button" && (node.props["aria-label"] ?? this.text(node).trim()) === label); assert.ok(button, `Missing ${label}`); return button; }
  async click(label: string) { const button = this.button(label); assert.notEqual(button.props.disabled, true, `${label} disabled`); button.props.onClick(); await this.settle(); }
  async mount() { await this.settle(); for (const [id, callback] of this.timers) { this.timers.delete(id); callback(); } await this.settle(); }
  async pick(path = "/owned/source", isZip = false) { await this.click("Paket importieren"); this.picks[this.picks.length - 1].resolve({ path, isZip }); await this.settle(); }
  async prepared(value: PreparedImport = clean) { const request = this.prepares[this.prepares.length - 1]; request.resolve({ ...value, requestId: request.args.requestId }); await this.settle(); }
  dispose() { if (this.disposed) return; this.disposed = true; for (const state of this.contexts.values()) for (const slot of state.slots) slot?.cleanup?.(); this.tree = null; }
}
test.after(() => hooks.deregister());

test("picker prepares complete folder evidence before token-only import and preserves original whole-package intent", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.pick();
    assert.equal(app.prepares.length, 1); assert.equal(app.consumes.length, 0); assert.deepEqual(app.legacy, []);
    assert.equal(app.prepares[0].args.root, "/owned/library"); assert.equal(app.prepares[0].args.source, "/owned/source");
    await app.prepared();
    assert.equal(app.consumes.length, 1); assert.equal(app.consumes[0].args.requestId, app.prepares[0].args.requestId);
    assert.equal("source" in app.consumes[0].args, false); assert.equal(app.authorizations.length, 0);
    app.consumes[0].resolve({ status: "imported", package: imported }); await app.settle();
    assert.match(app.text(app.tree), /Owned Package.*importiert/);
    assert.equal(app.button("Paket importieren").props.disabled, false);
  } finally { app.dispose(); }
});

test("duplicate review displays exact incoming/existing provenance and Cancel disposes the whole package without copy", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.pick(); await app.prepared(duplicate);
    assert.match(app.text(app.tree), /Duplikatprüfung vollständig/);
    assert.match(app.text(app.tree), /FBX\/Zombie.fbx.*Apocalypse.*FBX\/Characters\/SK_Zombie.fbx/);
    assert.match(app.text(app.tree), /Datei #.*11.*Asset #.*7.*Paket #.*3/);
    assert.match(app.text(app.tree), /18 bytes|18 B/);
    assert.match(app.text(app.tree), /gesamte.*Ordnerstruktur.*Begleitdateien/i);
    assert.equal(app.consumes.length, 0); assert.equal(app.authorizations.length, 0);
    await app.click("Abbrechen");
    assert.equal(app.cancellations[0].args.requestId, app.prepares[0].args.requestId);
    assert.equal(app.consumes.length, 0); assert.deepEqual(app.legacy, []);
    assert.equal(app.button("Paket importieren").props.disabled, false);
  } finally { app.dispose(); }
});

test("ZIP remains explicitly unknown and unverified until whole-package consent, then estimates and confirms space before token consume", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.pick("/owned/source.zip", true); await app.prepared(zip);
    assert.match(app.text(app.tree), /Duplikatprüfung nicht verfügbar/); assert.match(app.text(app.tree), /Unverifizierter Ganzpaket-Import/);
    assert.match(app.text(app.tree), /Nicht ermittelt/); assert.equal(app.authorizations.length, 0); assert.equal(app.confirmations.length, 0);
    assert.deepEqual(app.legacy, []); assert.equal(app.consumes.length, 0);
    await app.click("Gesamtes Paket trotzdem importieren");
    assert.equal(app.authorizations[0].args.requestId, app.prepares[0].args.requestId); assert.deepEqual(Object.keys(app.authorizations[0].args), ["requestId"]);
    app.authorizations[0].resolve({ requestId: app.prepares[0].args.requestId, zipEstimate: { hasSpace: true, entryCount: 8, uncompressedBytes: 120, requiredBytes: 240, availableBytes: 1000 } }); await app.settle();
    assert.equal(app.consumes.length, 0); assert.match(app.confirmations[0].args.message, /8 ZIP-Einträge.*Inhalt:.*120.*Speicherreserve:.*240.*Originalarchiv bleibt erhalten/s);
    app.confirmations[0].resolve(true); await app.settle();
    assert.equal(app.consumes[0].args.requestId, app.prepares[0].args.requestId);
    app.consumes[0].resolve({ status: "imported", package: imported }); await app.settle();
    assert.match(app.text(app.tree), /Owned Package.*importiert/); assert.deepEqual(app.legacy, []);
  } finally { app.dispose(); }
});

test("Stop during unresolved preparation holds ownership through late reply and cancellation acknowledgments", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.pick();
    app.prepares[0].args.onProgress.onmessage({ stage: "compare", examinedCandidates: 2, totalCandidates: null, bytesRead: 18 }); await app.settle();
    assert.match(app.text(app.tree), /Untersucht:.*2.*Gesamtzahl nicht ermittelt.*18/s);
    await app.click("Import abbrechen");
    assert.equal(app.cancellations.length, 1); assert.equal(app.cancellations[0].args.requestId, app.prepares[0].args.requestId);
    assert.equal(app.button("Paket importieren").props.disabled, true);
    app.cancellations[0].resolve(undefined); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, true, "native preparation still owns this operation");
    await app.prepared();
    assert.equal(app.consumes.length, 0); assert.equal(app.cancellations.length, 2, "late prepared token is disposed again");
    assert.equal(app.button("Paket importieren").props.disabled, true);
    app.cancellations[1].resolve(undefined); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, false); assert.match(app.text(app.tree), /Import abgebrochen/);
    assert.deepEqual(app.legacy, []);
  } finally { app.dispose(); }
});

test("Stop holds the copy owner until both native commit and ACK settle, retaining a late committed import", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.pick(); await app.prepared();
    app.consumes[0].args.onProgress.onmessage({ stage: "copying", completedBytes: 2, totalBytes: 3, completedEntries: 1, totalEntries: 3, currentPath: "FBX/Owned.fbx" }); await app.settle();
    const stop = app.button("Import abbrechen"); stop.props.onClick(); stop.props.onClick(); await app.settle();
    assert.equal(app.cancellations.length, 1, "same owner cancels only once while pending");
    app.consumes[0].resolve({ status: "imported", package: imported }); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, true, "commit does not finish cancellation ACK");
    app.cancellations[0].resolve(undefined); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, false);
    assert.match(app.text(app.tree), /1 importierte Pakete bleiben erhalten/);
    await app.pick("/owned/next"); await app.prepared();
    const before = app.text(app.tree);
    app.consumes[0].args.onProgress.onmessage({ stage: "copying", completedBytes: 999, totalBytes: 999, completedEntries: 99, totalEntries: 99, currentPath: "STALE-COPY" });
    app.prepares[0].args.onProgress.onmessage({ stage: "verify", examinedCandidates: 99, totalCandidates: 99, bytesRead: 999 }); await app.settle();
    assert.equal(app.text(app.tree), before); assert.equal(app.consumes.length, 2);
  } finally { app.dispose(); }
});

test("review cancellation rejection is visible and can be retried without duplicate proceed or copy", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.pick(); await app.prepared(duplicate);
    const cancel = app.button("Abbrechen"); cancel.props.onClick(); cancel.props.onClick(); await app.settle();
    assert.equal(app.cancellations.length, 1); assert.equal(app.button("Gesamtes Paket trotzdem importieren").props.disabled, true);
    app.cancellations[0].reject(new Error("native cancellation unavailable")); await app.settle();
    assert.match(app.text(app.tree), /Abbruch nicht bestätigt: native cancellation unavailable/);
    assert.equal(app.consumes.length, 0);
    await app.click("Abbrechen"); app.cancellations[1].resolve(undefined); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, false);
  } finally { app.dispose(); }
});

test("an older library-opening finalizer cannot release a new import after the new root renders", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.click("Bibliothek öffnen");
    app.openDialogs[0].resolve("/owned/other-library"); await app.settle();
    app.initialLibrary = { ...library, root: "/owned/other-library", name: "Other library" };
    app.afterRender = () => {
      if (!app.text(app.tree).includes("Other library") || app.button("Paket importieren").props.disabled) return;
      app.afterRender = null; app.button("Paket importieren").props.onClick();
    };
    app.openLibraries[0].resolve(app.initialLibrary); await app.settle();
    assert.equal(app.picks.length, 1, "new root accepts a new owned import");
    assert.equal(app.button("Paket importieren").props.disabled, true, "older act finally must not clear a new owner");
    app.picks[0].resolve({ path: "/owned/next", isZip: false }); await app.settle();
    assert.equal(app.prepares[0].args.root, "/owned/other-library");
  } finally { app.dispose(); }
});

test("sequential dropped packages preserve prior successes and full package skips with fresh request IDs", async () => {
  const app = new Harness();
  try {
    await app.mount(); app.drop({ payload: { type: "drop", paths: ["/owned/one", "/owned/two", "/owned/three"] } }); await app.settle();
    await app.prepared(); app.consumes[0].resolve({ status: "imported", package: imported }); await app.settle();
    assert.equal(app.prepares[1].args.source, "/owned/two"); await app.prepared(duplicate);
    assert.match(app.text(app.tree), /Abbrechen überspringt dieses ganze Paket.*1.*weitere/s);
    await app.click("Abbrechen"); assert.equal(app.prepares[2].args.source, "/owned/three");
    assert.equal(new Set(app.prepares.map((request) => request.args.requestId)).size, 3);
    await app.prepared({ ...clean, evidence: { ...clean.evidence, status: "incomplete", unavailableExistingVariants: 1, issues: ["Existing offline source"] } });
    assert.match(app.text(app.tree), /Duplikatprüfung unvollständig/); assert.match(app.text(app.tree), /Kopieren ist an den geprüften Paketinhalt gebunden/);
    await app.click("Gesamtes Paket trotzdem importieren"); assert.equal(app.authorizations.length, 0);
    app.consumes[1].resolve({ status: "imported", package: imported }); await app.settle();
    assert.match(app.text(app.tree), /2 Pakete importiert.*1 ganze Pakete übersprungen/); assert.deepEqual(app.legacy, []);
  } finally { app.dispose(); }
});

test("Stop during copy or later source failure halts a dropped queue and retains earlier committed packages", async () => {
  for (const stopped of [true, false]) {
    const app = new Harness();
    try {
      await app.mount(); app.drop({ payload: { type: "drop", paths: ["/owned/one", "/owned/two", "/owned/three"] } }); await app.settle();
      await app.prepared(); app.consumes[0].resolve({ status: "imported", package: imported }); await app.settle();
      await app.prepared();
      if (stopped) { await app.click("Import abbrechen"); app.consumes[1].resolve({ status: "cancelled" }); }
      else app.consumes[1].reject(new Error("Source changed before commit"));
      await app.settle();
      assert.equal(app.prepares.length, 2); assert.equal(app.consumes.length, 2);
      assert.match(app.text(app.tree), stopped ? /1 importierte Pakete bleiben erhalten/ : /\/owned\/two: Source changed before commit.*1 vorherige Pakete bleiben importiert.*Verbleibende Quellen wurden nicht importiert/s);
      assert.equal(app.button("Paket importieren").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("copy cancellation rejection stays visible while work is pending and a later ACK still owns retry", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.pick(); await app.prepared(); await app.click("Import abbrechen");
    app.cancellations[0].reject(new Error("IPC cancellation failed")); await app.settle();
    const alerts = app.elements(app.tree).filter((node) => node.props.role === "alert");
    assert.ok(alerts.some((node) => app.text(node).includes("IPC cancellation failed")), "cancellation error is visible before the worker finishes");
    await app.click("Import abbrechen");
    app.consumes[0].resolve({ status: "cancelled" }); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, true);
    app.cancellations[1].resolve(undefined); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, false);
    assert.doesNotMatch(app.text(app.tree), /IPC cancellation failed/);
  } finally { app.dispose(); }
});

test("picker Stop and unmount prevent late selection from preparing, and library selection cannot open after unmount", async () => {
  for (const stop of [true, false]) {
    const app = new Harness();
    try {
      await app.mount(); await app.click("Paket importieren");
      if (stop) await app.click("Import abbrechen"); else app.dispose();
      app.picks[0].resolve({ path: "/owned/late-selection", isZip: false }); await app.settle();
      assert.equal(app.prepares.length, 0); assert.equal(app.cancellations.length, 0);
      if (stop) { assert.match(app.text(app.tree), /Import abgebrochen/); assert.equal(app.button("Paket importieren").props.disabled, false); }
    } finally { app.dispose(); }
  }
  const app = new Harness();
  await app.mount(); await app.click("Bibliothek öffnen"); app.dispose();
  app.openDialogs[0].resolve("/owned/late-root"); await app.settle();
  assert.equal(app.openLibraries.length, 0);
});

test("unmount cleans unresolved preparation and discards its late success or rejection without copy", async () => {
  for (const reject of [false, true]) {
    const app = new Harness();
    try {
      await app.mount(); await app.pick(); const id = app.prepares[0].args.requestId;
      app.dispose(); assert.equal(app.cancellations[0].args.requestId, id);
      if (reject) app.prepares[0].reject(new Error("late prepare error"));
      else app.prepares[0].resolve({ ...clean, requestId: id });
      await app.settle();
      assert.equal(app.consumes.length, 0); assert.equal(app.authorizations.length, 0);
      if (!reject) assert.equal(app.cancellations.length, 2, "late successful preparation is defensively disposed");
      assert.equal(app.unlistened, 1);
    } finally { app.dispose(); }
  }
});

test("Stop or unmount during native authorization and ZIP confirmation cannot consume a late approval", async () => {
  for (const phase of ["authorize", "confirm"]) for (const stop of [true, false]) {
    const app = new Harness();
    try {
      await app.mount(); await app.pick("/owned/source.zip", true); await app.prepared(zip); await app.click("Gesamtes Paket trotzdem importieren");
      if (phase === "confirm") { app.authorizations[0].resolve({ requestId: app.prepares[0].args.requestId, zipEstimate: { hasSpace: true, entryCount: 3, uncompressedBytes: 3, requiredBytes: 6, availableBytes: 100 } }); await app.settle(); }
      if (stop) await app.click("Import abbrechen"); else app.dispose();
      if (phase === "confirm") app.confirmations[0].resolve(true);
      else app.authorizations[0].resolve({ requestId: app.prepares[0].args.requestId, zipEstimate: { hasSpace: true, entryCount: 3, uncompressedBytes: 3, requiredBytes: 6, availableBytes: 100 } });
      await app.settle();
      assert.equal(app.consumes.length, 0); assert.equal(app.cancellations[0].args.requestId, app.prepares[0].args.requestId);
      if (phase === "authorize") assert.equal(app.confirmations.length, 0);
      if (stop) assert.equal(app.button("Paket importieren").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("unmount while review cancellation is unresolved preserves one cancellation owner and no late copy", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.pick(); await app.prepared(duplicate); await app.click("Abbrechen");
    app.dispose(); assert.equal(app.cancellations.length, 1);
    app.cancellations[0].resolve(undefined); await app.settle();
    assert.equal(app.consumes.length, 0); assert.equal(app.authorizations.length, 0);
  } finally { app.dispose(); }
});

test("review pages exact bounded evidence and issues, preserves unknown counts, and Escape cancels the whole package", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.pick();
    const matches = Array.from({ length: 30 }, (_, index) => ({ ...duplicate.evidence.matches[0], incomingRelativePath: `incoming-${index}.fbx`, existingFileId: 100 + index }));
    await app.prepared({ ...duplicate, evidence: { ...duplicate.evidence, candidates: null, existingVariants: null, matches, issues: Array.from({ length: 30 }, (_, index) => `reason-${index}`), detailsTruncated: true } });
    assert.match(app.text(app.tree), /Nicht ermittelt/); assert.match(app.text(app.tree), /Weitere Belege oder Hinweise/);
    assert.match(app.text(app.tree), /incoming-0\.fbx/); assert.doesNotMatch(app.text(app.tree), /incoming-10\.fbx/);
    await app.click("Belege: Weiter"); await app.click("Hinweise: Weiter");
    assert.match(app.text(app.tree), /incoming-10\.fbx/); assert.doesNotMatch(app.text(app.tree), /incoming-0\.fbx/);
    assert.match(app.text(app.tree), /reason-10/); assert.doesNotMatch(app.text(app.tree), /reason-0(?:\s|$)/);
    await app.click("Belege: Weiter"); assert.equal(app.button("Belege: Weiter").props.disabled, true);
    assert.match(app.text(app.tree), /incoming-29\.fbx/);
    const dialog = app.elements(app.tree).find((node) => node.type === "dialog")!;
    let prevented = false; dialog.props.onCancel({ preventDefault: () => { prevented = true; } }); await app.settle();
    assert.equal(prevented, true); assert.equal(app.cancellations.length, 1); assert.equal(app.consumes.length, 0);
  } finally { app.dispose(); }
});

test("ZIP refusal, absent space estimate and native-owner mismatches never consume or use legacy imports", async () => {
  for (const choice of ["reviewCancel", "spaceCancel", "missingEstimate", "noSpace", "mismatchedAuthorization", "mismatchedPreparation"]) {
    const app = new Harness();
    try {
      await app.mount(); await app.pick("/owned/source.zip", true);
      if (choice === "mismatchedPreparation") { app.prepares[0].resolve({ ...zip, requestId: "other-request" }); await app.settle(); }
      else {
        await app.prepared(zip);
        if (choice === "reviewCancel") await app.click("Abbrechen");
        else {
          await app.click("Gesamtes Paket trotzdem importieren");
          app.authorizations[0].resolve({ requestId: choice === "mismatchedAuthorization" ? "other-request" : app.prepares[0].args.requestId, zipEstimate: choice === "missingEstimate" ? null : { hasSpace: choice !== "noSpace", entryCount: 8, uncompressedBytes: 120, requiredBytes: 240, availableBytes: 1000 } }); await app.settle();
          if (choice === "spaceCancel") { app.confirmations[0].resolve(false); await app.settle(); }
        }
      }
      assert.equal(app.consumes.length, 0); assert.deepEqual(app.legacy, []); assert.equal(app.cancellations.length, 1);
      assert.equal(app.button("Paket importieren").props.disabled, false);
      if (choice === "reviewCancel") assert.equal(app.authorizations.length, 0);
      if (choice === "missingEstimate") assert.match(app.text(app.tree), /Speicherabschätzung.*nicht verfügbar/);
      if (choice === "noSpace") assert.match(app.text(app.tree), /Nicht genug Speicherplatz/);
      if (choice.startsWith("mismatched")) assert.match(app.text(app.tree), /gehört nicht zu dieser Anfrage/);
    } finally { app.dispose(); }
  }
});

test("copy failure and cancellation failure retain both errors without permitting another copy", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.pick(); await app.prepared();
    app.consumes[0].reject(new Error("Original copy failure")); await app.settle();
    app.cancellations[0].reject(new Error("Cleanup not acknowledged")); await app.settle();
    assert.match(app.text(app.tree), /Original copy failure.*Cleanup not acknowledged/s);
    assert.equal(app.button("Paket importieren").props.disabled, false); assert.equal(app.consumes.length, 1);
  } finally { app.dispose(); }
});
