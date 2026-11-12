import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import { transformWithOxc } from "vite";

// Actual App and controlled review TSX at the external desktop/Channel boundary.
type Element = { type: unknown; props: Record<string, any> };
type Request = { args: any; resolve: (value: any) => void; reject: (error: unknown) => void };
const rootUrl = new URL("./", import.meta.url);
const transformed = new Map<string, string>();
for (const name of readdirSync(rootUrl).filter((name) => name.endsWith(".tsx"))) {
  const url = new URL(name, rootUrl);
  transformed.set(url.href, (await transformWithOxc(readFileSync(url, "utf8"), fileURLToPath(url))).code);
}
const globals = globalThis as typeof globalThis & { recoveryHarness: Harness; window: any };
const mocks: Record<string, string> = {
  react: `export const useState = (...args) => globalThis.recoveryHarness.useState(...args); export const useRef = (...args) => globalThis.recoveryHarness.useRef(...args); export const useEffect = (...args) => globalThis.recoveryHarness.useEffect(...args); export const useId = () => 'owned-test-id';`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({type, props}); export const jsxs = jsx; export const Fragment = 'fragment';`,
  "@tauri-apps/api/core": `export const isTauri = () => true; export const convertFileSrc = value => value; export const invoke = (...args) => globalThis.recoveryHarness.invoke(...args); export class Channel { constructor(){ globalThis.recoveryHarness.channels.push(this); } }`,
  "@tauri-apps/api/webview": `export const getCurrentWebview = () => ({onDragDropEvent: async callback => { const app=globalThis.recoveryHarness; app.drop=callback; return () => app.unlistened++; }});`,
  "@tauri-apps/plugin-dialog": `export const open = args => globalThis.recoveryHarness.dialogOpen(args); export const confirm = (...args) => globalThis.recoveryHarness.confirm(...args); export const save = async () => null;`,
};
const hooks = registerHooks({
  resolve(specifier, context, next) {
    if (specifier in mocks) return { url: `recovery-test:${specifier}`, shortCircuit: true };
    if (specifier.startsWith(".") && context.parentURL?.startsWith(rootUrl.href)) {
      const url = new URL(specifier, context.parentURL);
      for (const extension of ["", ".ts", ".tsx"]) { const candidate = new URL(url.href + extension); if (existsSync(candidate)) return { url: candidate.href, shortCircuit: true }; }
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith("recovery-test:")) return { format: "module", source: mocks[url.slice(14)], shortCircuit: true };
    if (transformed.has(url)) return { format: "module", source: transformed.get(url)!, shortCircuit: true };
    if (url.endsWith(".css")) return { format: "module", source: "", shortCircuit: true };
    return next(url, context);
  },
});
const { default: App } = await import(new URL("App.tsx", rootUrl).href);
const library = { root: "/owned/library", name: "Library", packages: [], folders: [], assetCount: 0 };
const summary = { entries: 7, files: 4, directories: 3, logicalBytes: 1234, externalReferenceRecords: null, externalCoverage: "metadataOnly" };
const limits = { maxLogicalBytes: 2199023255552, maxEntries: 250000, maxDepth: 64, maxRelativePathBytes: 4096, maxComponentBytes: 255, maxManifestBytes: 134217728, maxRetainedBytes: 268435456, preparationDeadlineMs: 1800000, runDeadlineMs: 86400000, chunkBytes: 65536 };
const ready = (request: Request, kind = request.args.request.kind) => ({ requestId: request.args.requestId, kind, status: "ready", destinationPath: `/owned/result-${request.args.requestId}`, summary, limits });
const published = (request: Request, kind: string, extras: any = {}) => ({ requestId: request.args.requestId, kind, status: "published", destinationPath: "/owned/published", summary, durability: "synced", warning: null, ...extras });
function last<T>(items: T[]): T { return items[items.length - 1]; }
class Harness {
  private contexts = new Map<string, { slots: any[]; cursor: number }>(); context = "app";
  dirty = true; private pending: (() => void)[] = []; private visited = new Set<string>(); private disposed = false;
  tree: Element | null = null; private timers = new Map<number, () => void>(); private timerId = 0;
  channels: any[] = []; picks: Request[] = []; prepares: Request[] = []; authorizations: Request[] = []; consumes: Request[] = []; cancellations: Request[] = [];
  confirmations: Request[] = []; openDialogs: Request[] = []; openLibraries: Request[] = [];
  commands: string[] = []; deferredCancel = false; initialLibrary: any = library; initialError: string | null = null; recoveries: Request[] = []; runs: Request[] = []; recoveryCancels: Request[] = [];
  drop: (event: any) => void = () => {}; unlistened = 0;
  afterRender: (() => void) | null = null;
  private deferred(list: Request[], args: any) { return new Promise((resolve, reject) => list.push({ args, resolve, reject })); }
  invoke = (command: string, args: any) => {
    this.commands.push(command);
    if (command === "prepare_library_recovery") return this.deferred(this.recoveries, args);
    if (command === "run_library_recovery") return this.deferred(this.runs, args);
    if (command === "cancel_library_recovery") { const result = this.deferred(this.recoveryCancels, args); if (!this.deferredCancel) last(this.recoveryCancels).resolve({ requestId: args.requestId, acknowledged: true }); return result; }
    if (command === "select_import_source") return this.deferred(this.picks, args);
    if (command === "cancel_prepared_import") return Promise.resolve();
    if (command === "get_library") return this.initialError ? Promise.reject(this.initialError) : Promise.resolve(this.initialLibrary);
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
    if (typeof node.type === "function" && node.type.name === "RecoveryDialog") return this.render(`recovery:${node.props.scopeId}`, () => node.type(node.props));
    return { ...node, props: { ...node.props, children: this.expand(node.props.children) } }; }
  async settle() { globals.recoveryHarness = this; globals.window = { setTimeout: (callback: () => void) => { const id = ++this.timerId; this.timers.set(id, callback); return id; }, clearTimeout: (id: number) => this.timers.delete(id) };
    for (let turn = 0; turn < 40; turn++) { if (this.dirty && !this.disposed) { this.dirty = false; this.visited.clear(); this.tree = this.expand(this.render("app", App)); this.afterRender?.();
        for (const [key, state] of this.contexts) if (!this.visited.has(key)) { for (const slot of state.slots) if (typeof slot?.cleanup === "function") slot.cleanup(); this.contexts.delete(key); }
        for (const effect of this.pending.splice(0)) effect(); }
      await Promise.resolve(); }
    if (!this.disposed) assert.equal(this.dirty, false, "Render did not settle"); }
  elements(node: any): Element[] { return Array.isArray(node) ? node.flatMap((child) => this.elements(child)) : node && typeof node === "object" && "props" in node ? [node, ...this.elements(node.props.children)] : []; }
  text(node: any): string { return Array.isArray(node) ? node.map((child) => this.text(child)).join(" ") : node && typeof node === "object" ? this.text(node.props?.children) : typeof node === "number" || typeof node === "string" ? String(node) : ""; }
  button(label: string) { const button = this.elements(this.tree).find((node) => node.type === "button" && (node.props["aria-label"] ?? this.text(node).trim()) === label); assert.ok(button, `Missing ${label}`); return button; }
  async click(label: string) { const button = this.button(label); assert.notEqual(button.props.disabled, true, `${label} disabled`); button.props.onClick(); await this.settle(); }
  async mount() { await this.settle(); for (const [id, callback] of this.timers) { this.timers.delete(id); callback(); } await this.settle(); }
  async pick(path = "/owned/source", isZip = false) { await this.click("Paket importieren"); this.picks[this.picks.length - 1].resolve({ path, isZip }); await this.settle(); }
  async prepare(kind: "backup" | "restore" = "backup") { await this.click(kind === "backup" ? "Bibliothek sichern" : "Bibliothek wiederherstellen"); await this.prepareAgain(kind); }
  async prepareAgain(kind: "backup" | "restore" = "backup") { await this.click(this.elements(this.tree).some(node => node.type === "button" && this.text(node).trim() === "Erneut vorbereiten") ? "Erneut vorbereiten" : "Vorbereiten");
    if (kind === "restore") { last(this.openDialogs).resolve("/owned/artifact"); await this.settle(); }
    last(this.openDialogs).resolve("/owned/destination"); await this.settle(); }
  async ready() { last(this.recoveries).resolve(ready(last(this.recoveries))); await this.settle(); }
  ack(index = this.recoveryCancels.length - 1) { const cancel = this.recoveryCancels[index]; cancel.resolve({ requestId: cancel.args.requestId, acknowledged: true }); }
  dispose() { if (this.disposed) return; this.disposed = true; for (const state of this.contexts.values()) for (const slot of state.slots) if (typeof slot?.cleanup === "function") slot.cleanup(); this.tree = null; }
}
test.after(() => hooks.deregister());

test("initial and failed-open screens expose explicit restore without requiring a live library or automatically preparing", async () => {
  for (const failed of [false, true]) {
    const app = new Harness(); app.initialLibrary = null; app.initialError = failed ? "Catalog unavailable" : null;
    try {
      await app.mount();
      assert.equal(app.elements(app.tree).some(node => node.type === "button" && app.text(node).trim() === "Bibliothek sichern"), false);
      await app.click("Bibliothek wiederherstellen");
      assert.match(app.text(app.tree), /Bibliothek aus Backup wiederherstellen/);
      assert.equal(app.recoveries.length, 0); assert.equal(app.openDialogs.length, 0);
      await app.click("Vorbereiten");
      assert.equal(app.openDialogs[0].args.directory, true);
      app.openDialogs[0].resolve("/owned/backup"); await app.settle();
      app.openDialogs[1].resolve("/owned/restore-parent"); await app.settle();
      assert.equal(app.recoveries[0].args.root, null);
      assert.deepEqual(app.recoveries[0].args.request, { kind: "restore", artifactRoot: "/owned/backup", destinationParent: "/owned/restore-parent" });
      assert.equal(typeof app.recoveries[0].args.requestId, "string");
      assert.equal(app.runs.length, 0);
    } finally { app.dispose(); }
  }
});

test("backup preparation reviews exact generated destination, unknown external coverage and limits before single-use token Start", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.prepare();
    const prep = app.recoveries[0]; assert.equal(prep.args.root, library.root);
    assert.deepEqual(prep.args.request, { kind: "backup", destinationParent: "/owned/destination" });
    await app.ready(); const text = app.text(app.tree);
    assert.match(text, /Anzahl unbekannt/); assert.match(text, /nur Katalogmetadaten/); assert.match(text, /mehrfach gelesen/); assert.match(text, /nicht fortgesetzt/); assert.match(text, /250\.000/); assert.match(text, /30\s+Minuten/); assert.match(text, /24\s+Stunden/);
    assert.match(text, new RegExp(`result-${prep.args.requestId}`)); assert.equal(app.runs.length, 0);
    const start = app.button("Starten"); start.props.onClick(); start.props.onClick(); await app.settle();
    assert.equal(app.runs.length, 1); assert.deepEqual(Object.keys(app.runs[0].args).sort(), ["onProgress", "requestId"]);
    assert.equal(app.runs[0].args.requestId, prep.args.requestId);
  } finally { app.dispose(); }
});

test("progress is per phase with honest unknown totals and rejects wrong request, kind and late channels", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.prepare(); await app.ready(); await app.click("Starten");
    const run = app.runs[0], channel = run.args.onProgress;
    const progress = { requestId: run.args.requestId, kind: "backup", phase: "verifyingCopy", entriesDone: 2, entriesTotal: null, bytesDone: 900, bytesTotal: null, elapsedMs: 1, currentRelativePath: "textures/check.png" };
    channel.onmessage(progress); await app.settle();
    assert.match(app.text(app.tree), /Kopie überprüfen/); assert.match(app.text(app.tree), /2\s+\/\s+unbekannt/);
    assert.equal(app.elements(app.tree).find(node => node.type === "progress")!.props.value, undefined);
    channel.onmessage({ ...progress, requestId: "stale", currentRelativePath: "WRONG-ID" }); channel.onmessage({ ...progress, kind: "restore", currentRelativePath: "WRONG-KIND" }); await app.settle();
    assert.doesNotMatch(app.text(app.tree), /WRONG/);
    channel.onmessage({ ...progress, phase: "verifyingSource", entriesDone: 0, entriesTotal: 7, bytesDone: 0, bytesTotal: 1234 }); await app.settle();
    assert.match(app.text(app.tree), /Quelle erneut überprüfen/); assert.match(app.text(app.tree), /0\s+\/\s+7/);
    run.resolve(published(run, "backup")); await app.settle(); channel.onmessage({ ...progress, currentRelativePath: "LATE" }); await app.settle();
    assert.match(app.text(app.tree), /Backup erstellt/); assert.doesNotMatch(app.text(app.tree), /LATE/);
  } finally { app.dispose(); }
});

test("unavailable and cancelled preparation never offer Start or imply an eligible backup", async () => {
  for (const status of ["unavailable", "cancelled"]) {
    const app = new Harness();
    try {
      await app.mount(); await app.prepare(); const prep = app.recoveries[0];
      prep.resolve({ requestId: prep.args.requestId, kind: "backup", status, reason: "unsupported", message: "WAL is active" }); await app.settle();
      assert.match(app.text(app.tree), status === "unavailable" ? /Vorbereitung nicht verfügbar.*WAL is active/ : /Vorbereitung abgebrochen/);
      assert.equal(app.elements(app.tree).some(node => node.type === "button" && app.text(node).trim() === "Starten"), false);
      assert.equal(app.runs.length, 0);
    } finally { app.dispose(); }
  }
});

test("Stop during unresolved prepare waits for operation and ACK in either order and disposes late ready owner", async () => {
  for (const ackFirst of [false, true]) {
    const app = new Harness(); app.deferredCancel = true;
    try {
      await app.mount(); await app.prepare(); const prep = app.recoveries[0];
      await app.click("Stoppen"); assert.equal(app.recoveryCancels[0].args.requestId, prep.args.requestId);
      if (ackFirst) { app.ack(0); await app.settle(); }
      else { prep.resolve(ready(prep)); await app.settle(); }
      assert.equal(app.button("Vorbereiten").props.disabled, true); assert.equal(app.button("Paket importieren").props.disabled, true);
      if (ackFirst) prep.resolve(ready(prep)); else app.ack(0);
      await app.settle(); assert.equal(app.recoveryCancels.length, 2, "late ready requires post-return disposal");
      assert.equal(app.button("Vorbereiten").props.disabled, true);
      app.ack(1); await app.settle(); assert.equal(app.button("Vorbereiten").props.disabled, false); assert.equal(app.runs.length, 0);
      await app.click("Schließen"); assert.equal(app.button("Paket importieren").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("Stop during Run retains late committed publication and waits for both original result and ACK", async () => {
  for (const ackFirst of [false, true]) {
    const app = new Harness(); app.deferredCancel = true;
    try {
      await app.mount(); await app.prepare(); await app.ready(); await app.click("Starten"); const run = app.runs[0];
      await app.click("Stoppen");
      if (ackFirst) { app.ack(0); await app.settle(); } else { run.resolve(published(run, "backup", { durability: "uncertain", warning: "directory sync failed" })); await app.settle(); }
      assert.equal(app.button("Erneut vorbereiten").props.disabled, true);
      if (ackFirst) run.resolve(published(run, "backup", { durability: "uncertain", warning: "directory sync failed" })); else app.ack(0);
      await app.settle(); assert.match(app.text(app.tree), /Backup erstellt/); assert.match(app.text(app.tree), /directory sync failed/); assert.match(app.text(app.tree), /Zielordner veröffentlicht/);
      assert.equal(app.button("Erneut vorbereiten").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("restore publishes without auto-open and explicit Open uses the ordinary library command at returned path", async () => {
  const app = new Harness(); app.initialLibrary = null;
  try {
    await app.mount(); await app.prepare("restore"); await app.ready(); await app.click("Starten"); const run = app.runs[0];
    run.resolve(published(run, "restore")); await app.settle();
    assert.equal(app.openLibraries.length, 0); assert.match(app.text(app.tree), /keine Zusicherung bei Stromausfall/);
    await app.click("Wiederhergestellte Bibliothek öffnen");
    assert.deepEqual(app.openLibraries[0].args, { root: "/owned/published" }); assert.equal(app.openDialogs.length, 2);
    app.openLibraries[0].resolve({ ...library, root: "/owned/published" }); await app.settle();
    assert.match(app.text(app.tree), /\/owned\/published/); assert.equal(app.elements(app.tree).some(node => node.type === "dialog"), false);
  } finally { app.dispose(); }
});

test("failed retained residues remain visible under Stop and during fresh preparation", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.prepare(); await app.ready(); await app.click("Starten"); const run = app.runs[0];
    await app.click("Stoppen"); run.resolve({ requestId: run.args.requestId, kind: "backup", status: "failed", reason: "cleanupIncomplete", message: "owned leaf changed", cleanup: "retained", residuePath: "/owned/residue" }); await app.settle();
    assert.match(app.text(app.tree), /owned leaf changed/); assert.match(app.text(app.tree), /\/owned\/residue/);
    await app.prepareAgain(); assert.notEqual(app.recoveries[1].args.requestId, app.recoveries[0].args.requestId);
    assert.match(app.text(app.tree), /\/owned\/residue/); assert.match(app.text(app.tree), /owned leaf changed/);
  } finally { app.dispose(); }
});

test("double fresh preparation while disposing the old ready UUID cannot attach two requests", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.prepare(); await app.ready();
    const button = app.button("Erneut vorbereiten"); button.props.onClick(); button.props.onClick(); await app.settle();
    assert.equal(app.recoveryCancels.length, 1); app.ack(0); await app.settle();
    assert.equal(app.openDialogs.length, 2, "one initial picker and one new picker only");
    app.openDialogs[1].resolve("/owned/next-parent"); await app.settle();
    assert.equal(app.recoveries.length, 2);
  } finally { app.dispose(); }
});

test("unavailable re-preparation after ready releases its consumed cancellation owner and permits a new preparation", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.prepare(); await app.ready(); await app.prepareAgain();
    const prep = app.recoveries[1]; prep.resolve({ requestId: prep.args.requestId, kind: "backup", status: "unavailable", reason: "sourceChanged", message: "source moved" }); await app.settle();
    assert.match(app.text(app.tree), /source moved/); assert.equal(app.button("Erneut vorbereiten").props.disabled, false);
  } finally { app.dispose(); }
});

test("Stop or close during unresolved native pickers cannot prepare, including after unmount", async () => {
  for (const kind of ["backup", "restore"] as const) for (const disposal of ["stop", "close", "unmount"]) {
    const app = new Harness();
    try {
      await app.mount(); await app.click(kind === "backup" ? "Bibliothek sichern" : "Bibliothek wiederherstellen"); await app.click("Vorbereiten");
      if (disposal === "unmount") app.dispose(); else await app.click(disposal === "close" ? "Schließen" : "Stoppen");
      assert.equal(app.recoveryCancels.length, 0, "there is no native request during source selection");
      app.openDialogs[0].resolve("/owned/late-selection"); await app.settle();
      assert.equal(app.recoveries.length, 0); assert.equal(app.openDialogs.length, 1); assert.equal(app.runs.length, 0);
      if (disposal === "close") assert.equal(app.button("Paket importieren").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("unmount and close while prepare is unresolved discard late responses and deduplicate outstanding cancellation", async () => {
  for (const close of [false, true]) for (const reject of [false, true]) {
    const app = new Harness(); app.deferredCancel = true;
    try {
      await app.mount(); await app.prepare(); const prep = app.recoveries[0];
      if (close) await app.click("Schließen"); else app.dispose();
      assert.equal(app.recoveryCancels.length, 1);
      if (close) { assert.equal(app.button("Paket importieren").props.disabled, true); const modal = app.elements(app.tree).find(node => node.type === "dialog")!; let prevented = false; modal.props.onCancel({ preventDefault() { prevented = true; } }); await app.settle(); assert.equal(prevented, true); assert.equal(app.recoveryCancels.length, 1); }
      if (reject) prep.reject("late preparation error"); else prep.resolve(ready(prep));
      await app.settle(); app.ack(0); await app.settle();
      if (!reject) { assert.equal(app.recoveryCancels.length, 2); app.ack(1); await app.settle(); }
      assert.equal(app.runs.length, 0);
      if (close) assert.equal(app.button("Paket importieren").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("a failed or mismatched Stop acknowledgment is visible and keeps its native owner until explicit retry", async () => {
  for (const wrongAck of [false, true]) {
    const app = new Harness(); app.deferredCancel = true;
    try {
      await app.mount(); await app.prepare(); await app.ready(); await app.click("Starten"); const run = app.runs[0];
      await app.click("Stoppen");
      if (wrongAck) app.recoveryCancels[0].resolve({ requestId: "another-id", acknowledged: true }); else app.recoveryCancels[0].reject("cancellation transport failed");
      run.resolve({ requestId: run.args.requestId, kind: "backup", status: "cancelled", cleanup: "complete" }); await app.settle();
      assert.match(app.text(app.tree), /Abbruch nicht bestätigt/); assert.equal(app.button("Erneut vorbereiten").props.disabled, true);
      await app.click("Abbruch erneut versuchen"); app.ack(1); await app.settle();
      assert.equal(app.button("Erneut vorbereiten").props.disabled, false); assert.doesNotMatch(app.text(app.tree), /Abbruch nicht bestätigt/); assert.match(app.text(app.tree), /Vorgang abgebrochen/);
    } finally { app.dispose(); }
  }
});

test("closing a ready owner waits for cancellation ACK and cannot start or reprepare in between", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.prepare(); await app.ready(); const start = app.button("Starten");
    await app.click("Schließen"); start.props.onClick(); await app.settle(); assert.equal(app.runs.length, 0);
    assert.equal(app.button("Paket importieren").props.disabled, true); app.ack(0); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, false); assert.equal(app.elements(app.tree).some(node => node.type === "dialog"), false);
  } finally { app.dispose(); }
});

test("a late committed restore remains visible after close, with an explicit Open action and no automatic library switch", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.prepare("restore"); await app.ready(); await app.click("Starten"); const run = app.runs[0];
    await app.click("Schließen"); app.ack(0); await app.settle();
    run.resolve(published(run, "restore", { durability: "uncertain", warning: "post-rename release failed" })); await app.settle();
    assert.match(app.text(app.tree), /Bibliothek wiederhergestellt/); assert.match(app.text(app.tree), /post-rename release failed/);
    assert.equal(app.openLibraries.length, 0); assert.equal(app.button("Wiederhergestellte Bibliothek öffnen").props.disabled, false);
  } finally { app.dispose(); }
});

test("a prior library picker act finally cannot clear the newer recovery global busy owner after root replacement", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.click("Bibliothek öffnen"); app.openDialogs[0].resolve("/owned/B"); await app.settle();
    app.afterRender = () => { if (!app.text(app.tree).includes("Library B") || app.button("Bibliothek sichern").props.disabled) return; app.afterRender = null; app.button("Bibliothek sichern").props.onClick(); };
    app.openLibraries[0].resolve({ ...library, root: "/owned/B", name: "Library B" }); await app.settle();
    assert.equal(app.button("Paket importieren").props.disabled, true, "stale generic finally cannot release recovery");
    await app.click("Vorbereiten"); app.openDialogs[1].resolve("/owned/destination"); await app.settle(); assert.equal(app.recoveries[0].args.root, "/owned/B");
  } finally { app.dispose(); }
});

test("explicit restored-root opening and its older finally cannot clear a newer import/picker owner", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.prepare("restore"); await app.ready(); await app.click("Starten"); const run = app.runs[0];
    run.resolve(published(run, "restore")); await app.settle(); await app.click("Wiederhergestellte Bibliothek öffnen");
    app.afterRender = () => { if (!app.text(app.tree).includes("Restored library") || app.button("Paket importieren").props.disabled) return; app.afterRender = null; app.button("Paket importieren").props.onClick(); };
    app.openLibraries[0].resolve({ ...library, root: "/owned/published", name: "Restored library" }); await app.settle();
    assert.equal(app.picks.length, 1); assert.equal(app.button("Paket importieren").props.disabled, true, "old open finally cannot release newer import");
    const oldChannel = run.args.onProgress; oldChannel.onmessage({ requestId: run.args.requestId, kind: "restore", phase: "publishing", entriesDone: 1, entriesTotal: 1, bytesDone: 1, bytesTotal: 1, elapsedMs: 1, currentRelativePath: "STALE-RESTORE" }); await app.settle();
    assert.doesNotMatch(app.text(app.tree), /STALE-RESTORE/); assert.equal(app.button("Paket importieren").props.disabled, true);
  } finally { app.dispose(); }
});

test("same-path reopen and A to B to A reject old close callbacks, progress and prepared Start owners", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.prepare(); await app.ready();
    const oldClose = app.button("Schließen").props.onClick, oldStart = app.button("Starten").props.onClick, oldProgress = app.recoveries[0].args.onProgress;
    await app.click("Schließen");
    for (const root of [library.root, "/owned/B", library.root]) {
      await app.click("Bibliothek öffnen"); last(app.openDialogs).resolve(root); await app.settle(); last(app.openLibraries).resolve({ ...library, root }); await app.settle();
      await app.prepare(); const prep = last(app.recoveries); assert.equal(prep.args.root, root); assert.notEqual(prep.args.requestId, app.recoveries[0].args.requestId);
      oldClose(); oldStart(); oldProgress.onmessage({ requestId: app.recoveries[0].args.requestId, kind: "backup", phase: "preparing", entriesDone: 99, entriesTotal: 99, bytesDone: 9, bytesTotal: 9, elapsedMs: 1, currentRelativePath: "OLD-ROOT" }); await app.settle();
      assert.equal(app.button("Paket importieren").props.disabled, true); assert.doesNotMatch(app.text(app.tree), /OLD-ROOT/); assert.equal(app.runs.length, 0);
      prep.resolve(ready(prep)); await app.settle(); await app.click("Schließen");
    }
  } finally { app.dispose(); }
});

test("run transport rejection defensively cancels the single-use owner before another preparation or close", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.prepare(); await app.ready(); await app.click("Starten"); const run = app.runs[0];
    run.reject("run transport failed"); await app.settle();
    assert.match(app.text(app.tree), /run transport failed/); assert.equal(app.recoveryCancels.length, 1);
    assert.equal(app.recoveryCancels[0].args.requestId, run.args.requestId); assert.equal(app.button("Erneut vorbereiten").props.disabled, true);
    app.ack(0); await app.settle(); assert.equal(app.button("Erneut vorbereiten").props.disabled, false);
  } finally { app.dispose(); }
});

test("Stop or close during old ready-owner disposal cannot launch a late fresh picker", async () => {
  for (const close of [false, true]) {
    const app = new Harness(); app.deferredCancel = true;
    try {
      await app.mount(); await app.prepare(); await app.ready(); await app.click("Erneut vorbereiten");
      if (close) await app.click("Schließen"); else await app.click("Stoppen");
      await app.settle(); app.ack(0); await app.settle();
      assert.equal(app.openDialogs.length, 1, "no fresh destination picker after Stop"); assert.equal(app.recoveries.length, 1);
      if (close) assert.equal(app.button("Paket importieren").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("a run rejection after close remains visible and survives a fresh preparation", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.prepare(); await app.ready(); await app.click("Starten");
    await app.click("Schließen"); app.runs[0].reject("delivery outcome unknown"); await app.settle();
    assert.match(app.text(app.tree), /delivery outcome unknown/); assert.equal(app.button("Erneut vorbereiten").props.disabled, false);
    await app.prepareAgain(); assert.match(app.text(app.tree), /delivery outcome unknown/);
  } finally { app.dispose(); }
});

test("explicit Open of a previous restored result disposes a newly ready request before opening that library", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.prepare("restore"); await app.ready(); await app.click("Starten");
    app.runs[0].resolve(published(app.runs[0], "restore")); await app.settle();
    await app.prepareAgain("restore"); await app.ready(); const id = app.recoveries[1].args.requestId;
    await app.click("Wiederhergestellte Bibliothek öffnen");
    assert.equal(app.openLibraries.length, 0, "new prepared owner must be disposed before root switch");
    assert.equal(last(app.recoveryCancels).args.requestId, id); app.ack(); await app.settle();
    assert.deepEqual(app.openLibraries[0].args, { root: "/owned/published" }); assert.equal(app.runs.length, 1);
  } finally { app.dispose(); }
});

test("Close revokes explicit Open while its prepared-owner cancellation ACK is outstanding", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.mount(); await app.prepare("restore"); await app.ready(); await app.click("Starten");
    app.runs[0].resolve(published(app.runs[0], "restore")); await app.settle(); await app.prepareAgain("restore"); await app.ready();
    await app.click("Wiederhergestellte Bibliothek öffnen"); await app.click("Schließen");
    assert.equal(app.recoveryCancels.length, 1); app.ack(); await app.settle(); assert.equal(app.openLibraries.length, 0);
    assert.match(app.text(app.tree), /Bibliothek wiederhergestellt/); assert.equal(app.button("Wiederhergestellte Bibliothek öffnen").props.disabled, false);
  } finally { app.dispose(); }
});

test("native outcome status remains authoritative for cancelled and non-retained failed runs", async () => {
  for (const cleanup of ["complete", "notNeeded"]) {
    const app = new Harness();
    try {
      await app.mount(); await app.prepare(); await app.ready(); await app.click("Starten"); const run = app.runs[0];
      run.resolve(cleanup === "complete" ? { requestId: run.args.requestId, kind: "backup", status: "cancelled", cleanup } : { requestId: run.args.requestId, kind: "backup", status: "failed", reason: "busy", message: "writer is busy", cleanup, residuePath: null }); await app.settle();
      assert.match(app.text(app.tree), cleanup === "complete" ? /Vorgang abgebrochen\. Zwischendateien aufgeräumt/ : /writer is busy/);
      assert.doesNotMatch(app.text(app.tree), /Backup erstellt/); assert.equal(app.button("Erneut vorbereiten").props.disabled, false);
    } finally { app.dispose(); }
  }
});

test("each native operation owns its progress Channel across ready, Start and a fresh preparation", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.prepare(); const prepare = app.recoveries[0]; await app.ready(); await app.click("Starten"); const run = app.runs[0];
    const copy = { requestId: run.args.requestId, kind: "backup", phase: "copying", entriesDone: 3, entriesTotal: 7, bytesDone: 800, bytesTotal: 1234, elapsedMs: 10, currentRelativePath: "CURRENT-COPY" };
    run.args.onProgress.onmessage(copy); await app.settle(); assert.match(app.text(app.tree), /CURRENT-COPY/);
    prepare.args.onProgress.onmessage({ ...copy, phase: "preparing", entriesDone: 0, bytesDone: 0, currentRelativePath: "STALE-PREPARE-AFTER-START" }); await app.settle();
    assert.match(app.text(app.tree), /CURRENT-COPY/); assert.doesNotMatch(app.text(app.tree), /STALE-PREPARE-AFTER-START/);
    run.resolve(published(run, "backup")); await app.settle(); await app.prepareAgain(); const next = app.recoveries[1];
    const inventory = { ...copy, requestId: next.args.requestId, phase: "preparing", entriesDone: 1, bytesDone: 4, currentRelativePath: "CURRENT-INVENTORY" };
    next.args.onProgress.onmessage(inventory); run.args.onProgress.onmessage({ ...copy, currentRelativePath: "STALE-RUN-AFTER-REPREPARE" }); prepare.args.onProgress.onmessage({ ...copy, currentRelativePath: "STALE-OLDER-PREPARE" }); await app.settle();
    assert.match(app.text(app.tree), /CURRENT-INVENTORY/); assert.doesNotMatch(app.text(app.tree), /STALE-RUN-AFTER-REPREPARE|STALE-OLDER-PREPARE/);
  } finally { app.dispose(); }
});
