import assert from "node:assert/strict";
import test from "node:test";
import { existsSync, readFileSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import { transformWithOxc } from "vite";

type Element = { type: any; props: any };
const sourceRoot = new URL("./", import.meta.url);
const globals = globalThis as any;
const mocks: Record<string, string> = {
  react: `export const useState = (...a) => globalThis.packedHarness.useState(...a); export const useEffect = (...a) => globalThis.packedHarness.useEffect(...a); export const useRef = (...a) => globalThis.packedHarness.useRef(...a);`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({type,props}); export const jsxs = jsx; export const Fragment = "fragment";`,
  "@tauri-apps/api/core": `export const invoke = (...a) => globalThis.packedHarness.invoke(...a); export const convertFileSrc = path => path;`,
  "@tauri-apps/plugin-dialog": `export const open = () => globalThis.packedHarness.choose();`,
};
const transformed = new Map<string, string>();
for (const name of ["ImagePreview", "GifPreview", "AsepritePreview"]) {
  const url = new URL(name + ".tsx", sourceRoot);
  transformed.set(url.href, (await transformWithOxc(readFileSync(url, "utf8"), fileURLToPath(url))).code);
}
const hooks = registerHooks({
  resolve(specifier, context, next) {
    if (specifier in mocks) return { url: "packed-test:" + specifier, shortCircuit: true };
    if (specifier.startsWith(".") && context.parentURL?.startsWith(sourceRoot.href)) for (const extension of ["", ".ts", ".tsx"]) {
      const url = new URL(specifier + extension, context.parentURL);
      if (existsSync(url)) return { url: url.href, shortCircuit: true };
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith("packed-test:")) return { format: "module", source: mocks[url.slice(12)], shortCircuit: true };
    if (transformed.has(url)) return { format: "module", source: transformed.get(url)!, shortCircuit: true };
    return next(url, context);
  },
});
const { default: ImagePreview } = await import(new URL("ImagePreview.tsx", sourceRoot).href);
const originalImage = Object.getOwnPropertyDescriptor(globalThis, "Image");
const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
test.after(() => { hooks.deregister(); for (const [key, descriptor] of [["Image", originalImage], ["document", originalDocument]] as const) { if (descriptor) Object.defineProperty(globalThis, key, descriptor); else Reflect.deleteProperty(globalThis, key); } });

class Harness {
  slots: any[] = []; cursor = 0; dirty = true; effects: Array<() => void> = [];
  file = { id: 9, originalFilename: "owned.png" }; tree: Element | null = null;
  picks: Array<(parent: string | null) => void> = [];
  published: Array<{ header: any; resolve: (result: any) => void; reject: (error: Error) => void }> = [];
  cancelled: any[] = []; canvases: any[] = []; stall = false; callbacks: Array<() => void> = [];
  choose = () => new Promise((resolve) => this.picks.push(resolve));
  invoke = (command: string, args: any) => {
    if (command === "sheet_resources") return Promise.resolve({ snapshotId: "sheet-" + args.fileId, fileId: args.fileId, errors: [], entryPath: "owned.png", resources: [{ relativePath: "owned.png", path: "/owned/owned.png" }] });
    if (command === "cancel_sprite_export") { this.cancelled.push(args); return Promise.resolve(); }
    if (command === "export_sprite") return new Promise((resolve, reject) => {
      const length = new DataView(args.buffer).getUint32(0, true);
      this.published.push({ header: JSON.parse(new TextDecoder().decode(args.subarray(4, 4 + length))), resolve, reject });
    });
    throw new Error("Unexpected native boundary " + command);
  };
  useState(initial: any) { const i = this.cursor++; if (!(i in this.slots)) this.slots[i] = initial; return [this.slots[i], (value: any) => { this.slots[i] = typeof value === "function" ? value(this.slots[i]) : value; this.dirty = true; }]; }
  useRef(initial: any) { const i = this.cursor++; return this.slots[i] ??= { current: initial }; }
  useEffect(effect: () => any, deps: any[]) { const i = this.cursor++, old = this.slots[i]; if (!old || deps.some((v, j) => !Object.is(v, old.deps[j]))) this.effects.push(() => { old?.cleanup?.(); this.slots[i] = { deps, cleanup: effect() }; }); }
  async settle() {
    globals.packedHarness = this;
    Object.defineProperty(globalThis, "Image", { configurable: true, value: class { naturalWidth = 12; naturalHeight = 4; onload: (() => void) | null = null; onerror = null; set src(value: string) { if (value) queueMicrotask(() => this.onload?.()); } } });
    Object.defineProperty(globalThis, "document", { configurable: true, value: { createElement: () => {
      const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: () => {}, getImageData: () => ({ data: new Uint8ClampedArray(canvas.width * canvas.height * 4) }) }), toBlob: (cb: (blob: Blob) => void) => { const finish = () => cb(new Blob(["owned PNG"])); if (this.stall) this.callbacks.push(finish); else finish(); } };
      this.canvases.push(canvas); return canvas;
    } } });
    for (let i = 0; i < 8; i++) {
      if (this.dirty) { this.dirty = false; this.cursor = 0; const element = ImagePreview({ file: this.file }); this.tree = element.type(element.props); for (const effect of this.effects.splice(0)) effect(); }
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
  }
  elements(node: any): Element[] { if (Array.isArray(node)) return node.flatMap((v) => this.elements(v)); if (!node || typeof node !== "object" || !("props" in node)) return []; return [node, ...this.elements(node.props.children)]; }
  text(node: any): string { if (Array.isArray(node)) return node.map((v) => this.text(v)).join(" "); return node && typeof node === "object" ? this.text(node.props?.children) : typeof node === "string" || typeof node === "number" ? String(node) : ""; }
  button(label: string) { const node = this.elements(this.tree).find((e) => e.type === "button" && this.text(e) === label); assert.ok(node, label + " absent"); return node; }
  control(label: string) { const node = this.elements(this.tree).find((e) => e.props["aria-label"] === label); assert.ok(node, label + " absent"); return node; }
  async click(label: string) { this.button(label).props.onClick(); await this.settle(); }
  async change(label: string, value: string) { this.control(label).props.onChange({ target: { value } }); await this.settle(); }
  dispose() { for (const slot of this.slots) slot?.cleanup?.(); }
}

test("the static sheet inspector exports a distinct packed artifact with frozen raster and FPS", async () => {
  const ui = new Harness();
  try {
    await ui.settle(); await ui.change("Spritesheet-Spalten", "3"); await ui.change("Spritesheet-FPS", "10");
    await ui.click("Gepackten Atlas exportieren");
    assert.equal(ui.elements(ui.tree).find((e) => e.type === "fieldset")?.props.disabled, true);
    assert.equal(ui.published.length, 0);
    ui.picks[0]("/project"); await ui.settle();
    assert.equal(ui.published.length, 1);
    assert.equal(ui.published[0].header.mode, "packed");
    assert.equal(ui.published[0].header.fps, 10); assert.equal(ui.published[0].header.packed.frames.length, 3);
    assert.equal(ui.published[0].header.destination, "/project/owned-packed");
    ui.published[0].resolve({ status: "exported", path: "/project/owned-packed" }); await ui.settle();
    assert.match(ui.text(ui.tree), /Sprite-Export gespeichert: \/project\/owned-packed/);
  } finally { for (const publication of ui.published) publication.resolve({ status: "cancelled" }); ui.dispose(); }
});


test("picker cancellation and Stop discard late packed work and permit a fresh retry", async () => {
  const ui = new Harness();
  try {
    await ui.settle(); await ui.click("Gepackten Atlas exportieren"); ui.picks[0](null); await ui.settle();
    assert.equal(ui.published.length, 0);
    await ui.click("Gepackten Atlas exportieren"); await ui.click("Export abbrechen");
    ui.picks[1]("/project"); await ui.settle(); assert.equal(ui.published.length, 0);
    assert.ok(ui.cancelled.some((c) => c.snapshotId === "sheet-9" && c.dispose === false));
    await ui.click("Gepackten Atlas exportieren"); ui.picks[2]("/project"); await ui.settle();
    assert.equal(ui.published.length, 1);
    ui.published[0].resolve({ status: "exported", path: "/project/owned-packed" }); await ui.settle();
    assert.equal(ui.elements(ui.tree).find((e) => e.type === "fieldset")?.props.disabled, false);
  } finally { for (const publication of ui.published) publication.resolve({ status: "cancelled" }); ui.dispose(); }
});

test("source disposal cancels a stalled PNG callback without publishing old packed pixels", async () => {
  const ui = new Harness();
  try {
    await ui.settle(); ui.stall = true; await ui.click("Gepackten Atlas exportieren"); ui.picks[0]("/project"); await ui.settle();
    assert.equal(ui.callbacks.length, 1); ui.dispose(); await ui.settle();
    ui.callbacks[0](); await ui.settle();
    assert.equal(ui.published.length, 0);
    assert.ok(ui.canvases.every((canvas) => canvas.width === 1 && canvas.height === 1));
    assert.ok(ui.cancelled.some((c) => c.snapshotId === "sheet-9" && c.dispose === true));
  } finally { for (const publication of ui.published) publication.resolve({ status: "cancelled" }); ui.dispose(); }
});

test("native rejection is visible and retry retains the selected frame layout", async () => {
  const ui = new Harness();
  try {
    await ui.settle(); await ui.change("Spritesheet-Spalten", "3");
    await ui.click("Gepackten Atlas exportieren"); ui.picks[0]("/project"); await ui.settle();
    ui.published[0].reject(new Error("Das Ziel existiert bereits")); await ui.settle();
    assert.match(ui.text(ui.tree), /Das Ziel existiert bereits/);
    await ui.click("Gepackten Atlas exportieren"); ui.picks[1]("/new-project"); await ui.settle();
    assert.equal(ui.published[1].header.layout.columns, 3);
    assert.equal(ui.published[1].header.packed.frames.length, 3);
    ui.published[1].resolve({ status: "exported", path: "/new-project/owned-packed" }); await ui.settle();
    await ui.change("Spritesheet-Spalten", "5");
    assert.equal(ui.button("Gepackten Atlas exportieren").props.disabled, true);
  } finally { for (const publication of ui.published) publication.resolve({ status: "cancelled" }); ui.dispose(); }
});


test("the inspector deadline cancels stalled encoding and shows an honest retained-export message", async (t) => {
  const nativeTimeout = globalThis.setTimeout; const timers: Array<() => void> = [];
  t.mock.method(globalThis, "setTimeout", ((callback: () => void, delay: number, ...args: unknown[]) => {
    if (delay === 30_000) { timers.push(callback); return nativeTimeout(() => {}, 60_000); }
    return nativeTimeout(callback, delay, ...args);
  }) as typeof setTimeout);
  const ui = new Harness(), ordinary = ui.invoke;
  const acknowledgments: Array<() => void> = [];
  ui.invoke = (command, args) => {
    if (command === "cancel_sprite_export" && !args.dispose) {
      ui.cancelled.push(args);
      return new Promise<void>((resolve) => acknowledgments.push(resolve));
    }
    return ordinary(command, args);
  };
  try {
    await ui.settle(); ui.stall = true; await ui.click("Gepackten Atlas exportieren"); ui.picks[0]("/project"); await ui.settle();
    assert.ok(timers.length >= 2); timers[0](); await ui.settle();
    assert.match(ui.text(ui.tree), /Zeitlimit von 30 Sekunden/);
    assert.match(ui.text(ui.tree), /bereits gespeicherter Export bleibt erhalten/);
    assert.equal(ui.published.length, 0);
    assert.ok(ui.cancelled.some((c) => c.snapshotId === "sheet-9" && c.dispose === false));
    ui.callbacks[0](); await ui.settle(); assert.equal(ui.published.length, 0);
    assert.equal(ui.elements(ui.tree).find((e) => e.type === "fieldset")?.props.disabled, true);
    await ui.click("Export abbrechen"); assert.equal(acknowledgments.length, 1);
    acknowledgments[0](); await ui.settle();
    assert.equal(ui.elements(ui.tree).find((e) => e.type === "fieldset")?.props.disabled, false);
  } finally { for (const resolve of acknowledgments) resolve(); ui.dispose(); }
});


test("Stop keeps the same sheet busy until its deduplicated native cancellation settles", async () => {
  const ui = new Harness(), ordinary = ui.invoke;
  const acknowledgments: Array<() => void> = [];
  ui.invoke = (command, args) => {
    if (command === "cancel_sprite_export" && !args.dispose) {
      ui.cancelled.push(args);
      return new Promise<void>((resolve) => acknowledgments.push(resolve));
    }
    return ordinary(command, args);
  };
  try {
    await ui.settle(); ui.stall = true;
    await ui.click("Gepackten Atlas exportieren"); ui.picks[0]("/project"); await ui.settle();
    await ui.click("Export abbrechen");
    assert.equal(ui.elements(ui.tree).find((e) => e.type === "fieldset")?.props.disabled, true);
    await ui.click("Export abbrechen"); await ui.click("Gepackten Atlas exportieren");
    assert.equal(acknowledgments.length, 1); assert.equal(ui.picks.length, 1); assert.equal(ui.published.length, 0);
    acknowledgments[0](); await ui.settle();
    assert.equal(ui.elements(ui.tree).find((e) => e.type === "fieldset")?.props.disabled, false);
    ui.stall = false; await ui.click("Gepackten Atlas exportieren"); ui.picks[1]("/retry"); await ui.settle();
    assert.equal(ui.published.length, 1);
    ui.published[0].resolve({ status: "exported", path: "/retry/owned-packed" }); await ui.settle();
    assert.match(ui.text(ui.tree), /Sprite-Export gespeichert: \/retry\/owned-packed/);
  } finally { for (const resolve of acknowledgments) resolve(); ui.dispose(); }
});


test("a failed cancellation is visible and retry waits for the old native export to finish", async () => {
  const ui = new Harness(), ordinary = ui.invoke;
  let rejectCancellation: (reason: Error) => void = () => {};
  ui.invoke = (command, args) => {
    if (command === "cancel_sprite_export" && !args.dispose) {
      ui.cancelled.push(args);
      return new Promise<void>((_resolve, reject) => { rejectCancellation = reject; });
    }
    return ordinary(command, args);
  };
  try {
    await ui.settle(); await ui.click("Gepackten Atlas exportieren"); ui.picks[0]("/project"); await ui.settle();
    assert.equal(ui.published.length, 1); await ui.click("Export abbrechen");
    rejectCancellation(new Error("Native cancellation unavailable")); await ui.settle();
    assert.match(ui.text(ui.tree), /Abbruch konnte nicht bestätigt werden: Native cancellation unavailable/);
    assert.equal(ui.elements(ui.tree).find((e) => e.type === "fieldset")?.props.disabled, true);
    await ui.click("Gepackten Atlas exportieren"); assert.equal(ui.picks.length, 1);
    ui.published[0].resolve({ status: "exported", path: "/project/owned-packed" }); await ui.settle();
    assert.equal(ui.elements(ui.tree).find((e) => e.type === "fieldset")?.props.disabled, false);
    assert.doesNotMatch(ui.text(ui.tree), /Sprite-Export gespeichert:/);
    assert.match(ui.text(ui.tree), /Prüfe den Zielordner/);
    await ui.click("Gepackten Atlas exportieren"); ui.picks[1]("/retry"); await ui.settle();
    ui.published[1].resolve({ status: "exported", path: "/retry/owned-packed" }); await ui.settle();
    assert.match(ui.text(ui.tree), /Sprite-Export gespeichert: \/retry\/owned-packed/);
  } finally { for (const publication of ui.published) publication.resolve({ status: "cancelled" }); ui.dispose(); }
});
