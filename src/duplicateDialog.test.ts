import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import { transformWithOxc } from "vite";

// Exercise the actual dialog's hooks, handlers and Channel at the desktop boundary.
type Element = { type: unknown; props: Record<string, any> };
type Request = { args: any; resolve: (result: any) => void; reject: (error: unknown) => void };
const source = new URL("./DuplicateDialog.tsx", import.meta.url);
const transformed = (await transformWithOxc(readFileSync(source, "utf8"), fileURLToPath(source))).code;
const globals = globalThis as typeof globalThis & { duplicateHarness: Harness };
const mocks: Record<string, string> = {
  react: `export const useState = (...args) => globalThis.duplicateHarness.useState(...args); export const useRef = (...args) => globalThis.duplicateHarness.useRef(...args); export const useEffect = (...args) => globalThis.duplicateHarness.useEffect(...args);`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({ type, props }); export const jsxs = jsx; export const Fragment = "fragment";`,
  "@tauri-apps/api/core": `export const invoke = (...args) => globalThis.duplicateHarness.invoke(...args); export class Channel { constructor() { globalThis.duplicateHarness.channels.push(this); } }`,
};
const hooks = registerHooks({
  resolve(specifier, context, next) { return specifier in mocks ? { url: `duplicate-test:${specifier}`, shortCircuit: true } : next(specifier, context); },
  load(url, context, next) {
    if (url.startsWith("duplicate-test:")) return { format: "module", source: mocks[url.slice(15)], shortCircuit: true };
    if (url === source.href) return { format: "module", source: transformed, shortCircuit: true };
    return next(url, context);
  },
});
const { DuplicateDialog } = await import(source.href);
const complete = { complete: true, cancelled: false, startedAtMs: 0, finishedAtMs: 1000, total: 90, processed: 90, bytesRead: 180,
  groups: [{ sha256: "a".repeat(64), sizeBytes: 3, members: [11, 12], sharedSource: false }],
  files: [
    { fileId: 11, assetId: 1, packageId: 7, packageName: "River Pack", originalFilename: "river.wav", relativePath: "water/river.wav", format: "WAV", sizeBytes: 3, status: "checked", sha256: "a".repeat(64), reason: null },
    { fileId: 12, assetId: 2, packageId: 8, packageName: "Other Pack", originalFilename: "A001.ogg", relativePath: "ambience/A001.ogg", format: "OGG", sizeBytes: 3, status: "checked", sha256: "a".repeat(64), reason: null },
  ], issues: [] };

class Harness {
  slots: any[] = []; cursor = 0; dirty = true; pending: (() => void)[] = [];
  root = "/owned/library"; selectedPackage: { id: number; name: string } | null = { id: 7, name: "River Pack" };
  closed = 0; tree: Element | null = null; channels: any[] = [];
  prepares: Request[] = []; runs: Request[] = []; cancels: Request[] = []; deferredCancel = false;
  invoke = (command: string, args: any) => {
    if (command === "prepare_duplicate_check") return new Promise((resolve, reject) => this.prepares.push({ args, resolve, reject }));
    if (command === "run_duplicate_check") return new Promise((resolve, reject) => this.runs.push({ args, resolve, reject }));
    if (command === "cancel_duplicate_check") return new Promise((resolve, reject) => { this.cancels.push({ args, resolve, reject }); if (!this.deferredCancel) resolve(undefined); });
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
    globals.duplicateHarness = this;
    for (let turn = 0; turn < 30; turn++) {
      if (this.dirty) { this.dirty = false; this.cursor = 0; this.tree = DuplicateDialog({ root: this.root, selectedPackage: this.selectedPackage, formatBytes: (bytes: number) => `${bytes} bytes`, onClose: () => { this.closed++; } }); for (const effect of this.pending.splice(0)) effect(); }
      await Promise.resolve(); await Promise.resolve();
    }
    assert.equal(this.dirty, false, "Render did not settle");
  }
  elements(node: any): Element[] { if (Array.isArray(node)) return node.flatMap((item) => this.elements(item)); return !node || typeof node !== "object" || !("props" in node) ? [] : [node, ...this.elements(node.props.children)]; }
  text(node: any): string { return Array.isArray(node) ? node.map((item) => this.text(item)).join(" ") : node && typeof node === "object" ? this.text(node.props?.children) : typeof node === "string" || typeof node === "number" ? String(node) : ""; }
  button(label: string) { const button = this.elements(this.tree).find((node) => node.type === "button" && (node.props["aria-label"] ?? this.text(node)) === label); assert.ok(button, `Missing ${label}`); return button; }
  async click(label: string) { const button = this.button(label); assert.notEqual(button.props.disabled, true, `${label} is disabled`); button.props.onClick(); await this.settle(); }
  async scope(value: string) { const select = this.elements(this.tree).find((node) => node.type === "select"); assert.ok(select); assert.notEqual(select.props.disabled, true); select.props.onChange({ target: { value } }); await this.settle(); }
  async ready(total = 90) { await this.click("Umfang vorbereiten"); const prepared = this.prepares[this.prepares.length - 1]; prepared.resolve({ requestId: prepared.args.requestId, total }); await this.settle(); }
  dispose() { for (const slot of this.slots) slot?.cleanup?.(); }
}
test.after(() => hooks.deregister());

test("explicit prepare freezes the complete variant scope before Start and displays exact confirmed file provenance", async () => {
  const app = new Harness();
  try {
    await app.settle(); assert.equal(app.prepares.length, 0); assert.equal(app.runs.length, 0);
    assert.match(app.text(app.tree), /alle Katalogseiten/);
    assert.match(app.text(app.tree), /Such.*Audiofilter.*nicht/);
    await app.ready();
    assert.equal(app.prepares[0].args.root, "/owned/library"); assert.equal(app.prepares[0].args.packageId, null);
    assert.match(app.text(app.tree), /90\s+Dateivarianten vorbereitet/); assert.equal(app.runs.length, 0);
    await app.click("Prüfung starten");
    assert.equal(app.runs[0].args.requestId, app.prepares[0].args.requestId);
    app.runs[0].resolve(complete); await app.settle();
    assert.match(app.text(app.tree), /Prüfung vollständig/);
    assert.match(app.text(app.tree), /water\/river.wav/); assert.match(app.text(app.tree), /Other Pack/);
    assert.match(app.text(app.tree), /ambience\/A001.ogg/); assert.match(app.text(app.tree), /3 bytes/);
    assert.match(app.text(app.tree), /keine.*Begleitdateien.*Lizenzen/);
  } finally { app.dispose(); }
});

test("selected package preparation is explicit and changing scope discards its frozen count and result", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.scope("package"); await app.ready(140);
    assert.equal(app.prepares[0].args.packageId, 7); assert.match(app.text(app.tree), /Paket: River Pack.*140\s+Dateivarianten vorbereitet/);
    await app.scope("all");
    assert.doesNotMatch(app.text(app.tree), /140\s+Dateivarianten vorbereitet/);
    assert.equal(app.cancels[0].args.requestId, app.prepares[0].args.requestId);
    await app.ready(90); assert.equal(app.prepares[1].args.packageId, null);
    assert.notEqual(app.prepares[1].args.requestId, app.prepares[0].args.requestId);
    await app.click("Prüfung starten"); app.runs[0].resolve(complete); await app.settle();
    await app.click("Umfang vorbereiten"); assert.doesNotMatch(app.text(app.tree), /Prüfung vollständig|water\/river.wav/);
  } finally { app.dispose(); }
});

test("progress is scoped and Stop retains ownership until run and cancellation acknowledgment both settle", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.settle(); await app.ready(); await app.click("Prüfung starten");
    app.channels[0].onmessage({ stage: "compare", total: 90, processed: 85, bytesRead: 170 }); await app.settle();
    assert.match(app.text(app.tree), /Bytevergleich.*85.*90.*170 bytes/);
    await app.click("Prüfung abbrechen");
    assert.equal(app.cancels[0].args.requestId, app.runs[0].args.requestId);
    app.runs[0].resolve({ ...complete, complete: false, cancelled: true, groups: [], files: [{ ...complete.files[0], status: "notChecked", sha256: null, reason: "Abgebrochen" }], issues: ["Nicht vollständig geprüft"] }); await app.settle();
    assert.equal(app.button("Umfang vorbereiten").props.disabled, true);
    assert.match(app.text(app.tree), /Prüfung abgebrochen.*unvollständig/);
    assert.doesNotMatch(app.text(app.tree), /Keine byte-identischen/);
    app.channels[0].onmessage({ stage: "finished", total: 999, processed: 999, bytesRead: 999 }); await app.settle();
    assert.doesNotMatch(app.text(app.tree), /999/);
    app.cancels[0].resolve(undefined); await app.settle();
    assert.equal(app.button("Umfang vorbereiten").props.disabled, false);
    await app.ready(); assert.notEqual(app.prepares[1].args.requestId, app.prepares[0].args.requestId);
    app.channels[0].onmessage({ stage: "finished", total: 999, processed: 999, bytesRead: 999 }); await app.settle();
    assert.doesNotMatch(app.text(app.tree), /999/);
  } finally { app.dispose(); }
});

test("incomplete or unavailable evidence keeps exact reasons and never becomes an empty successful check", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.ready(); await app.click("Prüfung starten");
    app.runs[0].resolve({ ...complete, complete: false, groups: [], processed: 2, bytesRead: 6,
      files: [{ ...complete.files[0], status: "unavailable", sha256: null, reason: "Quelle ist offline" }, { ...complete.files[1], status: "notChecked", sha256: null, reason: "Zeitlimit erreicht" }], issues: ["Quellprüfung unvollständig"] }); await app.settle();
    assert.match(app.text(app.tree), /Prüfung unvollständig/); assert.match(app.text(app.tree), /Quelle ist offline/);
    assert.match(app.text(app.tree), /Nicht geprüft.*Zeitlimit erreicht/); assert.match(app.text(app.tree), /Quellprüfung unvollständig/);
    assert.match(app.text(app.tree), /2.*90.*6 bytes/); assert.match(app.text(app.tree), /Beobachtungszeitraum/);
    assert.doesNotMatch(app.text(app.tree), /Keine byte-identischen/);
    await app.ready(0); await app.click("Prüfung starten");
    app.runs[1].resolve({ ...complete, total: 0, processed: 0, bytesRead: 0, groups: [], files: [] }); await app.settle();
    assert.match(app.text(app.tree), /Keine byte-identischen Dateien im vollständig geprüften Umfang/);
    assert.doesNotMatch(app.text(app.tree), /Quelle ist offline|Zeitlimit erreicht/);
  } finally { app.dispose(); }
});

test("groups, members, file evidence and reasons are paged with exact identities and shared-source caveat", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.ready(32); await app.click("Prüfung starten");
    const files = Array.from({ length: 32 }, (_, index) => ({ ...complete.files[0], fileId: 101 + index, assetId: 201 + index, originalFilename: `file-${101 + index}.wav`, relativePath: `owned/file-${101 + index}.wav` }));
    app.runs[0].resolve({ ...complete, total: 32, processed: 32, files,
      groups: [
        { ...complete.groups[0], members: [101,102,103,104,105,106,107,108,109,110,111,112], sharedSource: true },
        { ...complete.groups[0], sha256: "b".repeat(64), members: [113,114] },
        { ...complete.groups[0], sha256: "c".repeat(64), members: [115,116] },
        { ...complete.groups[0], sha256: "d".repeat(64), members: [117,118] },
        { ...complete.groups[0], sha256: "e".repeat(64), members: [119,120] },
        { ...complete.groups[0], sha256: "f".repeat(64), members: [121,122] },
        { ...complete.groups[0], sha256: "0".repeat(64), members: [123,124] },
      ], issues: Array.from({ length: 12 }, (_, index) => `Hinweis-${index + 1}`) }); await app.settle();
    assert.match(app.text(app.tree), /Gruppen.*1.*5.*7/);
    assert.throws(() => app.button("Gruppe 6 anzeigen"), /Missing/);
    const section = () => app.elements(app.tree).find((node) => node.props["aria-label"] === "Gruppenmitglieder")!;
    assert.match(app.text(section()), /Datei #.*101.*Asset #.*201/);
    assert.match(app.text(section()), /file-110.wav/); assert.doesNotMatch(app.text(section()), /file-111.wav/);
    assert.match(app.text(section()), /Gemeinsame Dateiidentität.*Hardlinks.*keine.*freigebbaren Speicher/);
    await app.click("Mitglieder: Weiter"); assert.match(app.text(section()), /file-111.wav.*file-112.wav/); assert.doesNotMatch(app.text(section()), /file-101.wav/);
    await app.click("Gruppen: Weiter"); assert.ok(app.button("Gruppe 6 anzeigen")); assert.match(app.text(section()), /file-121.wav.*file-122.wav/);
    const evidence = () => app.elements(app.tree).find((node) => node.props["aria-label"] === "Dateiprüfung")!;
    assert.match(app.text(evidence()), /file-120.wav/); assert.doesNotMatch(app.text(evidence()), /file-121.wav/);
    await app.click("Dateien: Weiter"); assert.match(app.text(evidence()), /file-121.wav.*file-132.wav/);
    const issues = () => app.elements(app.tree).find((node) => node.props["aria-label"] === "Prüfhinweise")!;
    assert.match(app.text(issues()), /Hinweis-10/); assert.doesNotMatch(app.text(issues()), /Hinweis-11/);
    await app.click("Hinweise: Weiter"); assert.match(app.text(issues()), /Hinweis-11.*Hinweis-12/);
  } finally { app.dispose(); }
});

test("preparation errors or mismatched replies release the actual owner and permit only a fresh explicit retry", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.click("Umfang vorbereiten");
    app.prepares[0].resolve({ requestId: "another-owner", total: 140 }); await app.settle();
    assert.match(app.text(app.tree), /gehört nicht zu dieser Anfrage/);
    assert.doesNotMatch(app.text(app.tree), /140.*Dateivarianten vorbereitet/);
    assert.equal(app.cancels[0]?.args.requestId, app.prepares[0].args.requestId);
    await app.click("Umfang vorbereiten"); app.prepares[1].reject("Auf dieser Plattform nicht verfügbar"); await app.settle();
    assert.match(app.text(app.tree), /Auf dieser Plattform nicht verfügbar/);
    assert.doesNotMatch(app.text(app.tree), /Prüfung vollständig|Keine byte-identischen/);
    assert.equal(app.button("Umfang vorbereiten").props.disabled, false);
    assert.equal(app.runs.length, 0);
  } finally { app.dispose(); }
});

test("Stop during unresolved prepare discards a late successful attachment and waits for both acknowledgments", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.settle(); await app.click("Umfang vorbereiten"); await app.click("Prüfung abbrechen");
    app.cancels[0].resolve(undefined); await app.settle();
    assert.equal(app.button("Umfang vorbereiten").props.disabled, true);
    app.prepares[0].resolve({ requestId: app.prepares[0].args.requestId, total: 100 }); await app.settle();
    assert.equal(app.cancels.length, 2); assert.equal(app.cancels[1].args.requestId, app.prepares[0].args.requestId);
    assert.equal(app.button("Umfang vorbereiten").props.disabled, true); assert.equal(app.runs.length, 0);
    assert.doesNotMatch(app.text(app.tree), /100.*Dateivarianten vorbereitet/);
    app.cancels[1].resolve(undefined); await app.settle();
    assert.equal(app.button("Umfang vorbereiten").props.disabled, false);
    await app.ready(90); assert.notEqual(app.prepares[1].args.requestId, app.prepares[0].args.requestId);
  } finally { app.dispose(); }
});

test("cancellation rejection remains visible and a completed run cannot race a newer retry", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.settle(); await app.ready(); await app.click("Prüfung starten"); await app.click("Prüfung abbrechen");
    app.cancels[0].reject("Abbruchkanal nicht erreichbar"); await app.settle();
    assert.match(app.text(app.tree), /Abbruch konnte nicht bestätigt werden.*Abbruchkanal nicht erreichbar/);
    assert.equal(app.button("Umfang vorbereiten").props.disabled, true);
    app.runs[0].reject("Prüfung beendet"); await app.settle();
    assert.match(app.text(app.tree), /Abbruchkanal nicht erreichbar/);
    assert.equal(app.button("Umfang vorbereiten").props.disabled, false);
    await app.ready(90); assert.doesNotMatch(app.text(app.tree), /Abbruchkanal nicht erreichbar/);
  } finally { app.dispose(); }
});

test("root or selected-package changes discard old preparation and reversed success or rejection cannot replace newer evidence", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.click("Umfang vorbereiten");
    app.root = "/owned/other"; app.dirty = true; await app.settle();
    assert.equal(app.cancels[0].args.requestId, app.prepares[0].args.requestId);
    await app.ready(42);
    app.prepares[0].resolve({ requestId: app.prepares[0].args.requestId, total: 999 }); await app.settle();
    assert.equal(app.cancels[1].args.requestId, app.prepares[0].args.requestId);
    assert.match(app.text(app.tree), /42\s+Dateivarianten vorbereitet/); assert.doesNotMatch(app.text(app.tree), /999/);
    await app.click("Prüfung starten");
    app.selectedPackage = { id: 8, name: "Other Pack" }; app.dirty = true; await app.settle();
    await app.ready(12); await app.click("Prüfung starten");
    app.channels[0].onmessage({ stage: "verify", total: 999, processed: 999, bytesRead: 999 }); app.runs[0].reject("Old root failure"); await app.settle();
    assert.doesNotMatch(app.text(app.tree), /999|Old root failure|Dateivarianten vorbereitet.*42/);
    app.runs[1].resolve({ ...complete, groups: [], total: 12, processed: 12 }); await app.settle();
    assert.match(app.text(app.tree), /Prüfung vollständig/);
  } finally { app.dispose(); }
});

test("close during prepare cancels both early and late attachment without accepting a late count", async () => {
  const app = new Harness();
  await app.settle(); await app.click("Umfang vorbereiten"); await app.click("Schließen");
  assert.equal(app.closed, 1); assert.equal(app.cancels[0].args.requestId, app.prepares[0].args.requestId);
  app.prepares[0].resolve({ requestId: app.prepares[0].args.requestId, total: 999 }); await app.settle();
  assert.equal(app.cancels[1].args.requestId, app.prepares[0].args.requestId); assert.doesNotMatch(app.text(app.tree), /999.*Dateivarianten vorbereitet/);
  app.dispose(); assert.equal(app.cancels.length, 2);
});

test("unmount during preparation or run ignores late rejection and progress and releases only its owner", async () => {
  for (const run of [false, true]) {
    const app = new Harness();
    await app.settle(); await app.click("Umfang vorbereiten");
    if (run) { app.prepares[0].resolve({ requestId: app.prepares[0].args.requestId, total: 90 }); await app.settle(); await app.click("Prüfung starten"); }
    app.dispose();
    if (run) { app.channels[0].onmessage({ stage: "finished", total: 999, processed: 999, bytesRead: 999 }); app.runs[0].reject("Disposed run failure"); }
    else app.prepares[0].reject("Disposed prepare failure");
    await app.settle(); assert.doesNotMatch(app.text(app.tree), /Disposed|999|Prüfung vollständig/);
    assert.equal(app.cancels.length, 1); assert.equal(app.cancels[0].args.requestId, app.prepares[0].args.requestId);
  }
});

test("rapid repeated Prepare dispatches only one owner before the disabled state renders", async () => {
  const app = new Harness();
  try {
    await app.settle(); const button = app.button("Umfang vorbereiten"); button.props.onClick(); button.props.onClick(); await app.settle();
    assert.equal(app.prepares.length, 1);
    app.prepares[0].resolve({ requestId: app.prepares[0].args.requestId, total: 90 }); await app.settle();
    const start = app.button("Prüfung starten"); start.props.onClick(); start.props.onClick(); await app.settle();
    assert.equal(app.runs.length, 1);
  } finally { app.dispose(); }
});

test("failed cancellation during prepare stays visible after the original preparation rejects", async () => {
  const app = new Harness(); app.deferredCancel = true;
  try {
    await app.settle(); await app.click("Umfang vorbereiten"); await app.click("Prüfung abbrechen");
    app.cancels[0].reject("Abbruchkanal nicht erreichbar"); await app.settle();
    assert.equal(app.button("Umfang vorbereiten").props.disabled, true);
    app.prepares[0].reject("Vorbereitung beendet"); await app.settle();
    assert.match(app.text(app.tree), /Abbruch konnte nicht bestätigt werden.*Abbruchkanal nicht erreichbar/);
    assert.match(app.text(app.tree), /Vorbereitung beendet/);
    assert.equal(app.button("Umfang vorbereiten").props.disabled, false);
  } finally { app.dispose(); }
});
