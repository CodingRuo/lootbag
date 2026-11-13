import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { registerHooks } from "node:module";
import { fileURLToPath } from "node:url";
import { transformWithOxc } from "vite";

// Exercise App's real handlers and effects with a hook renderer and a fake desktop boundary.
type Element = { type: unknown; props: Record<string, any> };
type Request = { args: any; resolve: (page: any) => void; reject: (error: unknown) => void };
const sourceRoot = new URL("./", import.meta.url);
const transformed = new Map<string, string>();
for (const name of readdirSync(sourceRoot).filter((name) => name.endsWith(".tsx"))) {
  const url = new URL(name, sourceRoot);
  transformed.set(url.href, (await transformWithOxc(readFileSync(url, "utf8"), fileURLToPath(url))).code);
}
const keydownListeners = new Set<(event: any) => void>();
const globals = globalThis as typeof globalThis & { audioFilterHarness: Harness; window: any };
// The catalog's quick audition needs a media element and document listeners; neither plays in these tests.
Object.assign(globalThis, {
  Audio: class { src = ""; onended = null; async play() {} pause() {} removeAttribute() {} load() {} },
  document: { addEventListener(type: string, listener: (event: any) => void) { if (type === "keydown") keydownListeners.add(listener); },
    removeEventListener(_type: string, listener: (event: any) => void) { keydownListeners.delete(listener); } },
});
const mocks: Record<string, string> = {
  react: `export const useState = (...args) => globalThis.audioFilterHarness.useState(...args);
    export const useEffect = (...args) => globalThis.audioFilterHarness.useEffect(...args);
    export const useRef = (...args) => globalThis.audioFilterHarness.useRef(...args);
    export const useId = () => "test-id";`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({ type, props }); export const jsxs = jsx; export const Fragment = "fragment";`,
  "@tauri-apps/api/core": `export const isTauri = () => true; export const invoke = (...args) => globalThis.audioFilterHarness.invoke(...args); export const convertFileSrc = path => path; export class Channel {}`,
  "@tauri-apps/api/event": `export const listen = async (event, handler) => { (globalThis.audioFilterHarness.listeners[event] ??= []).push(handler); return () => {}; };`,
  "@tauri-apps/api/webview": `export const getCurrentWebview = () => ({ onDragDropEvent: async () => () => {} });`,
  "@tauri-apps/plugin-dialog": `export const open = async () => "/owned/reopened"; export const save = async () => null; export const confirm = async () => true;`,
};
const hooks = registerHooks({
  resolve(specifier, context, next) {
    if (specifier in mocks) return { url: `audio-test:${specifier}`, shortCircuit: true };
    if (specifier.startsWith(".") && context.parentURL?.startsWith(sourceRoot.href)) {
      const url = new URL(specifier, context.parentURL);
      for (const extension of ["", ".ts", ".tsx"]) {
        const candidate = new URL(url.href + extension);
        if (existsSync(candidate)) return { url: candidate.href, shortCircuit: true };
      }
    }
    return next(specifier, context);
  },
  load(url, context, next) {
    if (url.startsWith("audio-test:")) return { format: "module", source: mocks[url.slice(11)], shortCircuit: true };
    if (transformed.has(url)) return { format: "module", source: transformed.get(url)!, shortCircuit: true };
    if (url.endsWith(".css")) return { format: "module", source: "", shortCircuit: true };
    return next(url, context);
  },
});
const { default: App } = await import(new URL("App.tsx", sourceRoot).href);
const { default: AudioPreview } = await import(new URL("AudioPreview.tsx", sourceRoot).href);
const { default: ModelThumbnail } = await import(new URL("ModelThumbnail.tsx", sourceRoot).href);
const { PackageInfoDialog } = await import(new URL("PackageInfoDialog.tsx", sourceRoot).href);
// App composes its regions from these function components; the harness renders them inline so their controls stay queryable.
const regions = new Set(await Promise.all(["Header", "Onboarding", "Sidebar", "Catalog", "Inspector"]
  .map(async (name) => (await import(new URL(`${name}.tsx`, sourceRoot).href)).default)));

const library = { root: "/owned/library", name: "Owned library", packages: [{ id: 1, name: "Fixture", relativePath: "fixture", assetCount: 41, sourceKind: "managed", available: true, externalRoot: null, tags: [], info: { store: null, url: null, license: null, author: null, purchased: null, version: null, notes: null } }], folders: [], assetCount: 41, kindCounts: { sound: 41, image: 0, model: 0, font: 0, material: 0, spriteSequence: 0, animation: 0 } };
const matchingFile = { id: 11, originalFilename: "match.mp3", relativePath: "match.mp3", originalFolder: "", format: "MP3", kind: "sound", sizeBytes: 50, audio: { status: "ready", durationSeconds: 1, sampleRate: 44100, channels: 2, codec: "mp3", error: null }, available: true };
const alternateFile = { ...matchingFile, id: 12, originalFilename: "alternate.wav", format: "WAV", audio: { ...matchingFile.audio, durationSeconds: 9, channels: 1 } };
const asset = { id: 1, name: "Sound", kind: "sound", packageId: 1, packageName: "Fixture", originalFolder: "", files: [matchingFile, alternateFile], tags: [], folderIds: [] };
const page = (items: any[] = [asset], total = 141, offset = 0) => ({ items, total, offset, limit: 100 });

class Harness {
  private slots: any[] = [];
  private cursor = 0;
  private pending: (() => void)[] = [];
  private dirty = true;
  private timers = new Map<number, () => void>();
  private timerId = 0;
  tree: Element | null = null;
  requests: Request[] = [];
  get latestRequest() { return this.requests[this.requests.length - 1]; }
  prepareRequests: Request[] = [];
  populationRequests: Request[] = [];
  populationCancellations: string[] = [];
  populationCancelError: Error | null = null;
  analysisRequests: { resolve: (metadata: any) => void; reject: (error: unknown) => void }[] = [];
  component: () => Element = App;
  libraryInfo: any = library;
  packageInfoSaves: any[] = [];
  savedSearches: any[] = [];
  listeners: Record<string, ((event: any) => void)[]> = {};
  savedSearchCalls: [string, any][] = [];
  annotations: any[] = [];
  references: any[] = [];
  watchNotices: any[] = [];
  watchChecks = 0;
  watchCalls: any[] = [];
  focusListeners = new Set<() => void>();
  unityReference = false;
  audioTags: any = { status: "ready", title: null, artist: null, album: null, genre: null, message: null };
  commands: string[] = [];
  droppedHashes = 0;
  invoke = (command: string, args: any) => {
    this.commands.push(command);
    if (command === "prepare_audio_analysis") return new Promise((resolve, reject) => this.prepareRequests.push({ args, resolve, reject }));
    if (command === "run_audio_analysis") return new Promise((resolve, reject) => this.populationRequests.push({ args, resolve, reject }));
    if (command === "cancel_audio_analysis") {
      this.populationCancellations.push(args.requestId);
      return this.populationCancelError ? Promise.reject(this.populationCancelError) : Promise.resolve();
    }
    if (command === "query_catalog") return new Promise((resolve, reject) => this.requests.push({ args, resolve, reject }));
    if (command === "get_library") return Promise.resolve(this.libraryInfo);
    if (command === "saved_searches") return Promise.resolve(this.savedSearches);
    if (command === "save_search" || command === "delete_saved_search") {
      this.savedSearchCalls.push([command, args]);
      return Promise.resolve(command === "save_search" ? { id: 9, name: args.name.trim(), queryJson: args.queryJson } : undefined);
    }
    if (command === "reference_zip") { this.references.push(args); return Promise.resolve({ ...library.packages[0], id: 2, name: "Forest", zip: true, unity: this.unityReference }); }
    if (command === "check_watched_sources") { this.watchChecks++; return Promise.resolve(this.watchNotices.splice(0)); }
    if (command === "set_watched") { this.watchCalls.push(args); return Promise.resolve(); }
    if (command === "annotate_asset") { this.annotations.push(args); return Promise.resolve(); }
    if (command === "set_package_info") { this.packageInfoSaves.push(args); return Promise.resolve(); }
    if (command === "open_library") return Promise.resolve({ ...library, root: "/owned/reopened" });
    if (command === "catalog_filters") return Promise.resolve({ folders: ["ambience"], formats: ["MP3", "WAV", "OGG", "FLAC", "PNG", "OBJ"], tags: ["wind"], duplicates: 3 });
    if (command === "asset_duplicates") return Promise.resolve({ copies: args.assetId === 1 ? [{ fileId: 31, assetId: 2, packageName: "Other Pack", path: "sfx/match.mp3" }] : [], dropped: this.droppedHashes });
    if (command === "verify_duplicates") return Promise.resolve(this.droppedHashes);
    if (command === "analyze_audio") return new Promise((resolve, reject) => this.analysisRequests.push({ resolve, reject }));
    if (command === "resolve_preview") return Promise.resolve("/owned/sound.wav");
    if (command === "audio_waveform") return Promise.resolve({ durationSeconds: 1, peaks: [] });
    if (command === "audio_tags") return Promise.resolve(this.audioTags);
    if (command === "cancel_audio_waveform" || command === "cancel_audio_tags") return Promise.resolve(true);
    throw new Error(`Unexpected desktop operation: ${command}`);
  };
  useState(initial: any) {
    const index = this.cursor++;
    if (!(index in this.slots)) this.slots[index] = typeof initial === "function" ? initial() : initial;
    return [this.slots[index], (value: any) => {
      const next = typeof value === "function" ? value(this.slots[index]) : value;
      if (!Object.is(next, this.slots[index])) { this.slots[index] = next; this.dirty = true; }
    }];
  }
  useRef(initial: any) {
    const index = this.cursor++;
    if (!(index in this.slots)) this.slots[index] = { current: initial };
    return this.slots[index];
  }
  useEffect(effect: () => void | (() => void), dependencies: unknown[]) {
    const index = this.cursor++;
    const previous = this.slots[index];
    if (!previous || dependencies.some((value, i) => !Object.is(value, previous.dependencies[i]))) {
      this.pending.push(() => { previous?.cleanup?.(); this.slots[index] = { dependencies, cleanup: effect() }; });
    }
  }
  async settle() {
    globals.audioFilterHarness = this;
    globals.window = { setTimeout: (callback: () => void) => { const id = ++this.timerId; this.timers.set(id, callback); return id; }, clearTimeout: (id: number) => this.timers.delete(id),
      addEventListener: (type: string, listener: () => void) => { if (type === "focus") this.focusListeners.add(listener); },
      removeEventListener: (_type: string, listener: () => void) => this.focusListeners.delete(listener) };
    for (let turn = 0; turn < 20; turn++) {
      if (this.dirty) {
        this.dirty = false; this.cursor = 0; this.tree = this.expand(this.component());
        for (const effect of this.pending.splice(0)) effect();
      }
      await Promise.resolve(); await Promise.resolve();
      if (!this.dirty && this.pending.length === 0) return;
    }
    throw new Error("Render did not settle");
  }
  async runTimers() {
    for (const [id, callback] of this.timers) { this.timers.delete(id); callback(); }
    await this.settle();
  }
  expand(node: any): any {
    if (Array.isArray(node)) return node.map((item) => this.expand(item));
    if (!node || typeof node !== "object" || !("props" in node)) return node;
    if (regions.has(node.type)) return this.expand(node.type(node.props));
    return node.props.children === undefined ? node : { ...node, props: { ...node.props, children: this.expand(node.props.children) } };
  }
  elements(node: any): Element[] {
    if (Array.isArray(node)) return node.flatMap((item) => this.elements(item));
    if (!node || typeof node !== "object" || !("props" in node)) return [];
    return [node, ...this.elements(node.props.children)];
  }
  find(predicate: (node: Element) => boolean) {
    const found = this.elements(this.tree).find(predicate);
    assert.ok(found, "Expected rendered control was absent");
    return found;
  }
  control(label: string) { return this.find((node) => node.props["aria-label"] === label); }
  text(node: any): string {
    if (Array.isArray(node)) return node.map((item) => this.text(item)).join(" ");
    return node && typeof node === "object" ? this.text(node.props?.children) : typeof node === "string" || typeof node === "number" ? String(node) : "";
  }
  button(label: string) { return this.find((node) => node.type === "button" && this.text(node) === label); }
  async change(label: string, value: string) { this.control(label).props.onChange({ target: { value } }); await this.settle(); }
  async chooseType(value: string) { this.find((node) => node.type === "button" && node.props["data-type"] === value).props.onClick(); await this.settle(); }
  async click(label: string) { this.button(label).props.onClick(); await this.settle(); }
  async mount() { await this.settle(); await this.runTimers(); this.requests[0].resolve(page()); await this.settle(); }
  dispose() { for (const slot of this.slots) slot?.cleanup?.(); }
}

test.after(() => hooks.deregister());

test("saved audio criteria select Sounds, reset paging, and reach the desktop query", async () => {
  const app = new Harness();
  try {
    await app.mount();
    app.control("Nächste Seite").props.onClick(); await app.settle(); await app.runTimers();
    app.requests[1].resolve(page([asset], 141, 100)); await app.settle();
    await app.change("Nach Audiokanälen filtern", "stereo");
    assert.equal(app.find((node) => node.props["data-type"] === "sound").props["aria-pressed"], true);
    await app.runTimers();
    assert.deepEqual(app.latestRequest?.args.audioCriteria, { minDurationSeconds: null, maxDurationSeconds: null, channels: "stereo", analysis: "any" });
    assert.equal(app.latestRequest?.args.query.offset, 0);
    assert.equal(app.find((node) => node.props.className === "filter-count").props.children, 1);
  } finally { app.dispose(); }
});

test("duration bounds and analysis state use saved values, preserve unknown, and stop invalid queries", async () => {
  const app = new Harness();
  try {
    await app.mount();
    await app.change("Minimale Audiodauer in Sekunden", "0");
    await app.change("Maximale Audiodauer in Sekunden", "2.5");
    await app.change("Nach Audioanalysestatus filtern", "ready");
    await app.runTimers();
    assert.deepEqual(app.latestRequest?.args.audioCriteria, { minDurationSeconds: 0, maxDurationSeconds: 2.5, channels: "any", analysis: "ready" });
    assert.match(app.text(app.tree), /Gespeicherte Audioanalyse/);
    assert.match(app.text(app.tree), /Unbekannte Werte/);
    const count = app.requests.length;
    for (const value of ["-1", "Infinity", "3"]) {
      await app.change("Minimale Audiodauer in Sekunden", value);
      await app.runTimers();
      assert.equal(app.requests.length, count, `Invalid duration ${value} must not query`);
      assert.match(app.text(app.tree), /Dauer.*(?:nichtnegative|Minimum)/);
    }
    await app.change("Minimale Audiodauer in Sekunden", "");
    await app.change("Maximale Audiodauer in Sekunden", "");
    await app.change("Nach Audioanalysestatus filtern", "notAnalysed");
    await app.runTimers();
    assert.deepEqual(app.latestRequest?.args.audioCriteria, { minDurationSeconds: null, maxDurationSeconds: null, channels: "any", analysis: "notAnalysed" });
    assert.equal(app.find((node) => node.props.className === "filter-count").props.children, 1);
  } finally { app.dispose(); }
});

test("audio-filtered inspection defaults to the qualifying variant and allows a deliberate alternate until scope changes", async () => {
  const app = new Harness();
  try {
    await app.mount();
    assert.equal(app.find((node) => node.type === AudioPreview).props.file.id, alternateFile.id, "Unfiltered WAV preference stays intact");
    app.find((node) => node.type === "select" && node.props.value === alternateFile.id).props.onChange({ target: { value: String(alternateFile.id) } });
    await app.settle();
    await app.change("Nach Audiokanälen filtern", "stereo");
    assert.equal(app.elements(app.tree).some((node) => node.type === AudioPreview), false, "Previous-scope audio must not open during debounce");
    await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    assert.equal(app.find((node) => node.type === AudioPreview).props.file.id, matchingFile.id);
    assert.match(app.text(app.tree), /Gefundene Variante/);
    const variants = app.find((node) => node.type === "select" && node.props.value === matchingFile.id);
    assert.equal(variants.props.children.length, 2, "Every variant remains selectable");
    variants.props.onChange({ target: { value: String(alternateFile.id) } }); await app.settle();
    assert.equal(app.find((node) => node.type === AudioPreview).props.file.id, alternateFile.id);
    assert.match(app.text(app.tree), /Andere Variante.*Audiofilter/);
    assert.equal(app.elements(app.tree).some((node) => node.props["aria-label"] === "Katalogauswahl"), false, "Bulk bar stays hidden without a selection");
    app.control("Sound auswählen · alle Varianten").props.onClick({ shiftKey: false }); await app.settle();
    assert.match(app.text(app.tree), /1\s+ausgewählt/);
    await app.change("Assets durchsuchen", "wind"); await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    assert.equal(app.find((node) => node.type === AudioPreview).props.file.id, matchingFile.id);
    assert.doesNotMatch(app.text(app.tree), /ausgewählt/);
  } finally { app.dispose(); }
});

test("non-sound categories, clear filters, package and library changes visibly reset all audio criteria", async () => {
  const app = new Harness();
  try {
    await app.mount();
    await app.chooseType("material");
    await app.change("Nach Audiokanälen filtern", "multi"); await app.runTimers();
    assert.equal(app.latestRequest?.args.query.kind, "sound");
    assert.equal(app.latestRequest?.args.query.workflow, null);
    for (const category of ["image", "model", "material", "spriteSequence", ""]) {
      await app.change("Nach Audiokanälen filtern", "stereo");
      await app.chooseType(category); await app.runTimers();
      assert.equal(app.control("Nach Audiokanälen filtern").props.value, "any");
      assert.equal(app.latestRequest?.args.audioCriteria, null);
    }
    await app.change("Minimale Audiodauer in Sekunden", "1");
    await app.change("Maximale Audiodauer in Sekunden", "3");
    await app.change("Nach Audioanalysestatus filtern", "error");
    await app.click("Filter zurücksetzen"); await app.runTimers();
    assert.equal(app.control("Minimale Audiodauer in Sekunden").props.value, "");
    assert.equal(app.control("Maximale Audiodauer in Sekunden").props.value, "");
    assert.equal(app.control("Nach Audioanalysestatus filtern").props.value, "any");
    assert.equal(app.latestRequest?.args.audioCriteria, null);
    await app.change("Nach Audiokanälen filtern", "mono");
    app.control("Fixture · 41 Assets").props.onClick(); await app.settle(); await app.runTimers();
    assert.equal(app.latestRequest?.args.query.packageId, 1);
    assert.equal(app.latestRequest?.args.query.kind, "sound", "Choosing a package keeps the asset type");
    assert.equal(app.latestRequest?.args.audioCriteria, null);
    await app.change("Nach Audiokanälen filtern", "mono");
    app.find((node) => node.type === "button" && app.text(node).trim() === "Bibliothek öffnen").props.onClick();
    await app.settle(); await app.runTimers();
    assert.ok(app.find((node) => node.props.title === "/owned/reopened"), "Storage shows the reopened library path as tooltip");
    assert.equal(app.latestRequest?.args.query.packageId, null);
    assert.equal(app.latestRequest?.args.audioCriteria, null);
    assert.equal(app.control("Nach Audiokanälen filtern").props.value, "any");
  } finally { app.dispose(); }
});

test("inspector analysis only marks saved filter results stale until the user explicitly refreshes", async () => {
  const app = new Harness();
  try {
    await app.mount();
    await app.change("Nach Audioanalysestatus filtern", "notAnalysed"); await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    const count = app.requests.length;
    app.find((node) => node.type === AudioPreview).props.onAnalysisComplete();
    await app.settle(); await app.runTimers();
    assert.equal(app.requests.length, count, "Analysis completion must never refresh the catalog automatically");
    assert.match(app.text(app.tree), /Analyse gespeichert.*aktualisieren/);
    assert.equal(app.find((node) => node.type === AudioPreview).props.file.id, matchingFile.id);
    await app.click("Audiofilter aktualisieren");
    assert.equal(app.elements(app.tree).some((node) => node.type === AudioPreview), false, "Refresh waits for the actual new representative");
    await app.runTimers();
    assert.equal(app.requests.length, count + 1);
    assert.equal(app.latestRequest?.args.audioCriteria.analysis, "notAnalysed");
    assert.doesNotMatch(app.text(app.tree), /Analyse gespeichert/);
  } finally { app.dispose(); }
});

test("AudioPreview reports a saved ready or failed analysis once without repeating analysis on callback renders", async () => {
  for (const status of ["ready", "error"]) {
    const preview = new Harness();
    let completions = 0;
    preview.component = () => AudioPreview({ file: { ...matchingFile, audio: null }, onAnalysisComplete: () => { completions++; } });
    try {
      await preview.settle();
      preview.analysisRequests[0].resolve({ ...matchingFile.audio, status, error: status === "error" ? "Owned fixture failure" : null });
      await preview.settle();
      assert.equal(completions, 1);
      assert.equal(preview.analysisRequests.length, 1, "A new callback identity cannot restart analysis");
    } finally { preview.dispose(); }
  }
});

test("AudioPreview reuses saved metadata without re-analyzing on open", async () => {
  const preview = new Harness();
  preview.component = () => AudioPreview({ file: matchingFile });
  try {
    await preview.settle();
    assert.equal(preview.analysisRequests.length, 0);
  } finally { preview.dispose(); }
});

test("late catalog successes and failures cannot replace results from the current audio scope", async () => {
  const app = new Harness();
  try {
    await app.mount();
    await app.change("Nach Audiokanälen filtern", "stereo"); await app.runTimers();
    const oldSuccess = app.latestRequest!;
    await app.change("Nach Audiokanälen filtern", "mono"); await app.runTimers();
    const currentAsset = { ...asset, id: 2, name: "Current mono", files: [alternateFile, matchingFile] };
    app.latestRequest!.resolve(page([currentAsset], 1)); await app.settle();
    oldSuccess.resolve(page([asset], 99)); await app.settle();
    assert.equal(app.find((node) => node.type === AudioPreview).props.file.id, alternateFile.id);
    assert.match(app.text(app.control("Asset-Katalog")), /1 Asset im Katalog/);
    await app.change("Assets durchsuchen", "outdated"); await app.runTimers();
    const oldFailure = app.latestRequest!;
    await app.change("Assets durchsuchen", "current"); await app.runTimers();
    app.latestRequest!.resolve(page([currentAsset], 1)); await app.settle();
    oldFailure.reject(new Error("Obsolete scope failure")); await app.settle();
    assert.doesNotMatch(app.text(app.tree), /Obsolete scope failure|Katalog nicht verfügbar/);
    assert.equal(app.find((node) => node.type === AudioPreview).props.file.id, alternateFile.id);
  } finally { app.dispose(); }
});

test("an unmounted audio preview cannot report a late analysis as a current filter change", async () => {
  const preview = new Harness();
  let completions = 0;
  preview.component = () => AudioPreview({ file: { ...matchingFile, audio: null }, onAnalysisComplete: () => { completions++; } });
  await preview.settle();
  preview.dispose();
  preview.analysisRequests[0].resolve(matchingFile.audio); await preview.settle();
  assert.equal(completions, 0);
});

test("entering audio criteria from a visual category clears its format, while existing Sound format filters compose", async () => {
  const app = new Harness();
  try {
    await app.mount();
    for (const category of ["image", "model", "material", "spriteSequence"]) {
      await app.chooseType(category);
      await app.change("Nach Dateityp filtern", category === "model" ? "OBJ" : "PNG");
      await app.change("Nach Audiokanälen filtern", "stereo"); await app.runTimers();
      assert.equal(app.latestRequest.args.query.kind, "sound");
      assert.equal(app.latestRequest.args.query.workflow, null);
      assert.equal(app.latestRequest.args.query.format, null, "A visual format cannot silently exclude every sound");
      assert.equal(app.control("Nach Dateityp filtern").props.value, "");
    }
    await app.change("Nach Dateityp filtern", "WAV");
    await app.change("Nach Audiokanälen filtern", "mono"); await app.runTimers();
    assert.equal(app.latestRequest.args.query.format, "WAV");
    assert.equal(app.latestRequest.args.audioCriteria.channels, "mono");
  } finally { app.dispose(); }
});

test("sound format filters chosen in All Assets compose with newly activated audio criteria", async () => {
  const app = new Harness();
  try {
    await app.mount();
    for (const format of ["WAV", "MP3", "OGG", "FLAC"]) {
      await app.chooseType("");
      await app.change("Nach Dateityp filtern", format);
      await app.change("Nach Audiokanälen filtern", "stereo"); await app.runTimers();
      assert.equal(app.latestRequest.args.query.kind, "sound");
      assert.equal(app.latestRequest.args.query.format, format, "An already selected sound format must remain part of the query");
      assert.equal(app.control("Nach Dateityp filtern").props.value, format);
      assert.equal(app.latestRequest.args.audioCriteria.channels, "stereo");
    }
  } finally { app.dispose(); }
});


test("sound analysis explicitly prepares every matching file variant with the current search filters across pages", async () => {
  const app = new Harness();
  try {
    await app.mount();
    assert.equal(app.prepareRequests.length, 0, "Opening a library must never start a scan");
    await app.chooseType("sound");
    app.control("Fixture · 41 Assets").props.onClick(); await app.settle();
    await app.chooseType("sound");
    await app.change("Assets durchsuchen", "wind");
    await app.change("Nach Originalordner filtern", "ambience");
    await app.change("Nach Dateityp filtern", "WAV");
    await app.change("Nach Tag filtern", "wind");
    await app.change("Nach Audioanalysestatus filtern", "notAnalysed");
    await app.runTimers(); app.latestRequest!.resolve(page()); await app.settle();
    app.control("Nächste Seite").props.onClick(); await app.settle(); await app.runTimers();
    app.latestRequest!.resolve(page([asset], 141, 100)); await app.settle();
    const catalogRequests = app.requests.length;
    await app.click("Analyse vorbereiten");
    assert.match(app.text(app.tree), /Suchbereich wird geprüft/);
    assert.deepEqual(app.prepareRequests[0].args.query, {
      text: "wind", packageId: 1, folderId: null, originalFolder: "ambience", format: "WAV", kind: "sound", workflow: null, tag: "wind", offset: 0, limit: 100,
    });
    assert.equal(app.prepareRequests[0].args.audioCriteria.analysis, "notAnalysed");
    const requestId = app.prepareRequests[0].args.requestId;
    app.prepareRequests[0].resolve({ requestId, total: 87 }); await app.settle();
    assert.match(app.text(app.tree), /87\s+passende Dateivarianten/);
    assert.match(app.text(app.tree), /Paket: Fixture.*Suche: „wind“.*Originalordner: ambience.*Dateityp: WAV.*Tag: wind/);
    assert.match(app.text(app.tree), /alle Katalogseiten/);
    assert.match(app.text(app.tree), /Audiofilter: nicht analysiert/);
    assert.equal(app.populationRequests.length, 0, "Preparation is reviewable and cannot start work");
    app.control("Sound auswählen · alle Varianten").props.onClick({ shiftKey: false }); await app.settle();
    assert.match(app.text(app.tree), /87\s+passende Dateivarianten/, "Selection is unrelated to the frozen search scope");
    assert.equal(app.populationCancellations.length, 0);
    await app.click("Vorbereitung verwerfen");
    assert.deepEqual(app.populationCancellations, [requestId]);
    assert.doesNotMatch(app.text(app.tree), /87\s+passende Dateivarianten/);
    assert.equal(app.requests.length, catalogRequests, "Preparation must not implicitly refresh the catalogue");
  } finally { app.dispose(); }
});


test("a prepared sound job reports progress and retains partial measurements on stop until explicit refresh", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.chooseType("sound"); await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    await app.click("Analyse vorbereiten");
    const requestId = app.prepareRequests[0].args.requestId;
    app.prepareRequests[0].resolve({ requestId, total: 87 }); await app.settle();
    const catalogRequests = app.requests.length;
    await app.click("Analyse starten");
    assert.equal(app.populationRequests[0].args.requestId, requestId);
    const channel = app.populationRequests[0].args.progress;
    channel.onmessage({ requestId, total: 87, processed: 5, ready: 3, errors: 1, skipped: 1, cancelled: false });
    await app.settle();
    assert.equal(app.control("Fortschritt der Sound-Analyse").props.value, 5);
    assert.equal(app.control("Fortschritt der Sound-Analyse").props.max, 87);
    assert.match(app.text(app.tree), /3 analysiert.*1 Analysefehler.*1 übersprungen/);
    assert.equal(app.find((node) => node.type === "button" && /^(Paket|Ordner) importieren$/.test(app.text(node).trim())).props.disabled, true, "File mutations stay inside the existing operation boundary");
    assert.equal(app.control("Assets durchsuchen").props.disabled, undefined, "Reading and search remain usable");
    await app.click("Analyse abbrechen");
    assert.deepEqual(app.populationCancellations, [requestId]);
    assert.match(app.text(app.tree), /Wird abgebrochen/);
    app.populationRequests[0].resolve({ requestId, total: 87, processed: 6, ready: 4, errors: 1, skipped: 1, cancelled: true });
    await app.settle(); await app.runTimers();
    assert.match(app.text(app.tree), /Abgebrochen.*6.*87/);
    assert.match(app.text(app.tree), /Bereits gespeicherte Analysen bleiben erhalten/);
    assert.equal(app.find((node) => node.type === "button" && /^(Paket|Ordner) importieren$/.test(app.text(node).trim())).props.disabled, false);
    assert.equal(app.requests.length, catalogRequests, "Completed analysis must not trigger an implicit catalogue scan");
    await app.click("Analyseergebnisse aktualisieren"); await app.runTimers();
    assert.equal(app.requests.length, catalogRequests + 1);
    assert.equal(app.latestRequest!.args.query.kind, "sound");
    assert.doesNotMatch(app.text(app.tree), /Abgebrochen.*6.*87/);
    channel.onmessage({ requestId, total: 87, processed: 87, ready: 87, errors: 0, skipped: 0, cancelled: false }); await app.settle();
    assert.doesNotMatch(app.text(app.tree), /87 analysiert/, "A completed old channel cannot publish after refresh");
  } finally { app.dispose(); }
});


test("obsolete sound preparations cannot publish after search replacement or library change", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.chooseType("sound");
    await app.click("Analyse vorbereiten"); const old = app.prepareRequests[0];
    await app.change("Assets durchsuchen", "new scope");
    assert.ok(app.populationCancellations.includes(old.args.requestId));
    await app.click("Analyse vorbereiten"); const current = app.prepareRequests[1];
    current.resolve({ requestId: current.args.requestId, total: 12 }); await app.settle();
    old.resolve({ requestId: old.args.requestId, total: 999 }); await app.settle();
    assert.match(app.text(app.tree), /12\s+passende Dateivarianten/);
    assert.doesNotMatch(app.text(app.tree), /999/);
    await app.click("Vorbereitung verwerfen");
    await app.click("Analyse vorbereiten"); const beforeRoot = app.prepareRequests[2];
    app.find((node) => node.type === "button" && app.text(node).trim() === "Bibliothek öffnen").props.onClick(); await app.settle();
    assert.ok(app.populationCancellations.includes(beforeRoot.args.requestId));
    await app.chooseType("sound");
    await app.click("Analyse vorbereiten"); const afterRoot = app.prepareRequests[3];
    afterRoot.resolve({ requestId: afterRoot.args.requestId, total: 4 }); await app.settle();
    beforeRoot.reject(new Error("Obsolete preparation failure")); await app.settle();
    assert.match(app.text(app.tree), /4\s+passende Dateivarianten/);
    assert.doesNotMatch(app.text(app.tree), /Obsolete preparation failure/);
  } finally { app.dispose(); }
});

test("a changed search cancels sound work and discards its late progress, success and failure", async () => {
  for (const completion of ["success", "failure"]) {
    const app = new Harness();
    try {
      await app.mount(); await app.chooseType("sound");
      await app.click("Analyse vorbereiten"); const prepared = app.prepareRequests[0];
      prepared.resolve({ requestId: prepared.args.requestId, total: 9 }); await app.settle();
      await app.click("Analyse starten"); const running = app.populationRequests[0];
      await app.change("Nach Tag filtern", "wind");
      assert.ok(app.populationCancellations.includes(prepared.args.requestId));
      running.args.progress.onmessage({ requestId: prepared.args.requestId, total: 9, processed: 9, ready: 9, errors: 0, skipped: 0, cancelled: true });
      if (completion === "success") running.resolve({ requestId: prepared.args.requestId, total: 9, processed: 9, ready: 9, errors: 0, skipped: 0, cancelled: true });
      else running.reject(new Error("Obsolete run failure"));
      await app.settle();
      assert.doesNotMatch(app.text(app.tree), /9 analysiert|Abgebrochen|Obsolete run failure/);
      assert.equal(app.elements(app.tree).some((node) => node.props["aria-label"] === "Fortschritt der Sound-Analyse"), false);
      assert.equal(app.find((node) => node.type === "button" && /^(Paket|Ordner) importieren$/.test(app.text(node).trim())).props.disabled, false);
      await app.click("Analyse vorbereiten"); const replacement = app.prepareRequests[1];
      replacement.resolve({ requestId: replacement.args.requestId, total: 2 }); await app.settle();
      running.args.progress.onmessage({ requestId: prepared.args.requestId, total: 9, processed: 9, ready: 9, errors: 0, skipped: 0, cancelled: false }); await app.settle();
      assert.match(app.text(app.tree), /2\s+passende Dateivarianten/);
      assert.doesNotMatch(app.text(app.tree), /9 analysiert/);
    } finally { app.dispose(); }
  }
});

test("paging and inspector variant changes preserve the complete prepared sound job", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.chooseType("sound"); await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    await app.click("Analyse vorbereiten"); const prepared = app.prepareRequests[0];
    prepared.resolve({ requestId: prepared.args.requestId, total: 82 }); await app.settle();
    app.find((node) => node.type === "select" && node.props.value === alternateFile.id).props.onChange({ target: { value: String(matchingFile.id) } }); await app.settle();
    app.control("Nächste Seite").props.onClick(); await app.settle(); await app.runTimers();
    app.latestRequest!.resolve(page([asset], 141, 100)); await app.settle();
    assert.match(app.text(app.tree), /82\s+passende Dateivarianten/);
    assert.deepEqual(app.populationCancellations, []);
    await app.click("Analyse starten");
    assert.equal(app.populationRequests[0].args.requestId, prepared.args.requestId);
    app.populationRequests[0].resolve({ requestId: prepared.args.requestId, total: 82, processed: 82, ready: 82, errors: 0, skipped: 0, cancelled: false }); await app.settle();
    assert.match(app.text(app.tree), /Analyse abgeschlossen.*82 von 82/);
  } finally { app.dispose(); }
});

test("empty and rejected sound scopes are visible and cannot start a run", async () => {
  const app = new Harness();
  try {
    await app.mount();
    assert.equal(app.elements(app.tree).some((node) => node.props["aria-label"] === "Sounds im Suchbereich analysieren"), false);
    await app.chooseType("sound");
    await app.click("Analyse vorbereiten");
    app.prepareRequests[0].resolve({ requestId: app.prepareRequests[0].args.requestId, total: 0 }); await app.settle();
    assert.equal(app.button("Analyse starten").props.disabled, true);
    await app.click("Analyse starten"); assert.equal(app.populationRequests.length, 0);
    assert.match(app.text(app.tree), /Keine passenden Sounds/);
    await app.click("Vorbereitung verwerfen");
    await app.change("Minimale Audiodauer in Sekunden", "-1");
    assert.equal(app.button("Analyse vorbereiten").props.disabled, true);
    await app.click("Analyse vorbereiten"); assert.equal(app.prepareRequests.length, 1);
    await app.change("Minimale Audiodauer in Sekunden", "");
    await app.click("Analyse vorbereiten");
    app.prepareRequests[1].reject(new Error("Mehr als 100.000 Dateivarianten: Suchbereich eingrenzen.")); await app.settle();
    assert.match(app.text(app.tree), /100.000 Dateivarianten.*eingrenzen/);
    assert.equal(app.populationRequests.length, 0);
    assert.equal(app.button("Analyse vorbereiten").props.disabled, false);
  } finally { app.dispose(); }
});

test("failed sound cancellation stays retryable and failed runs preserve a refreshable partial summary", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.chooseType("sound"); await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    await app.click("Analyse vorbereiten"); const prepared = app.prepareRequests[0];
    prepared.resolve({ requestId: prepared.args.requestId, total: 5 }); await app.settle();
    await app.click("Analyse starten"); const running = app.populationRequests[0];
    running.args.progress.onmessage({ requestId: prepared.args.requestId, total: 5, processed: 2, ready: 2, errors: 0, skipped: 0, cancelled: false }); await app.settle();
    app.populationCancelError = new Error("Owned cancellation failure");
    await app.click("Analyse abbrechen");
    assert.match(app.text(app.tree), /Abbruch nicht möglich: Owned cancellation failure/);
    assert.equal(app.button("Analyse abbrechen").props.disabled, false);
    const catalogRequests = app.requests.length;
    running.reject(new Error("Owned run failure")); await app.settle();
    assert.match(app.text(app.tree), /Owned run failure/);
    assert.match(app.text(app.tree), /2 analysiert/);
    assert.match(app.text(app.tree), /Bereits gespeicherte Analysen bleiben erhalten/);
    assert.equal(app.requests.length, catalogRequests);
    running.args.progress.onmessage({ requestId: prepared.args.requestId, total: 5, processed: 5, ready: 5, errors: 0, skipped: 0, cancelled: false }); await app.settle();
    assert.doesNotMatch(app.text(app.tree), /5 analysiert/);
    await app.click("Analyseergebnisse aktualisieren"); await app.runTimers();
    assert.equal(app.requests.length, catalogRequests + 1);
  } finally { app.dispose(); }
});

test("unmount cancels pending sound work and never publishes late boundary replies", async () => {
  const app = new Harness();
  await app.mount(); await app.chooseType("sound");
  await app.click("Analyse vorbereiten"); const prepared = app.prepareRequests[0];
  prepared.resolve({ requestId: prepared.args.requestId, total: 3 }); await app.settle();
  await app.click("Analyse starten"); const running = app.populationRequests[0];
  app.dispose();
  assert.ok(app.populationCancellations.includes(prepared.args.requestId));
  running.args.progress.onmessage({ requestId: prepared.args.requestId, total: 3, processed: 3, ready: 3, errors: 0, skipped: 0, cancelled: false });
  running.resolve({ requestId: prepared.args.requestId, total: 3, processed: 3, ready: 3, errors: 0, skipped: 0, cancelled: false }); await app.settle();
  assert.doesNotMatch(app.text(app.tree), /3 analysiert|Analyse abgeschlossen/);
});

test("the Animationen type queries animation kind and its animation-only cards skip model thumbnails", async () => {
  const app = new Harness();
  try {
    await app.mount();
    await app.chooseType("animation"); await app.runTimers();
    assert.equal(app.latestRequest?.args.query.kind, "animation");
    assert.match(app.text(app.find((node) => node.type === "h1")), /Animationen/);
    const modelFile = { ...matchingFile, id: 21, originalFilename: "run.fbx", relativePath: "run.fbx", format: "FBX", kind: "model", audio: null };
    const animation = { ...asset, id: 2, name: "Run", kind: "model", files: [{ ...modelFile, modelTraits: { hasMesh: false, hasAnimation: true } }] };
    const mixed = { ...asset, id: 3, name: "Hero", kind: "model", files: [{ ...modelFile, id: 22, modelTraits: { hasMesh: true, hasAnimation: true } }] };
    app.latestRequest!.resolve(page([animation, mixed], 2)); await app.settle();
    const thumbnails = app.elements(app.tree).filter((node) => node.type === ModelThumbnail);
    assert.deepEqual(thumbnails.map((node) => node.props.fileId), [22], "Only the mixed model requests a WebGL thumbnail");
    assert.match(app.text(app.tree), /FBX · Animation ·/);
    assert.match(app.text(app.tree), /FBX · animiert ·/);
  } finally { app.dispose(); }
});

test("AudioPreview hides empty embedded tags and lists only populated ones", async () => {
  for (const [tags, shown] of [[{}, []], [{ title: "Wind", genre: "Ambience" }, ["Titel", "Genre"]]] as const) {
    const preview = new Harness();
    preview.audioTags = { status: "ready", title: null, artist: null, album: null, genre: null, message: null, ...tags };
    preview.component = () => AudioPreview({ file: matchingFile });
    try {
      await preview.settle();
      const labels = preview.elements(preview.tree).filter((node) => node.type === "dt").map((node) => preview.text(node));
      assert.equal(/Eingebettete Audio-Tags/.test(preview.text(preview.tree)), shown.length > 0);
      assert.deepEqual(labels.filter((label) => ["Titel", "Künstler", "Album", "Genre"].includes(label)), shown);
    } finally { preview.dispose(); }
  }
});

test("the origin tab hints at a missing licence, edits package info and then shows shop, licence and link", async () => {
  const app = new Harness();
  try {
    await app.mount();
    assert.match(app.text(app.tree), /Keine Lizenz hinterlegt/);
    await app.click("Paketinfo bearbeiten…");
    const dialog = app.find((node) => node.type === PackageInfoDialog);
    assert.equal(dialog.props.pkg.id, 1);
    const info = { store: "Kenney", url: "https://kenney.nl", license: "CC0", author: "", purchased: "", version: "", notes: "" };
    app.libraryInfo = { ...library, packages: [{ ...library.packages[0], info: { ...library.packages[0].info, store: "Kenney", url: "https://kenney.nl", license: "CC0" } }] };
    dialog.props.onSave(info); await app.settle();
    assert.deepEqual(app.packageInfoSaves, [{ packageId: 1, info: { ...info, author: null, purchased: null, version: null, notes: null } }]);
    assert.equal(app.elements(app.tree).some((node) => node.type === PackageInfoDialog), false, "The dialog closes after saving");
    const text = app.text(app.tree);
    assert.doesNotMatch(text, /Keine Lizenz hinterlegt/);
    assert.match(text, /Shop\s+Kenney/);
    assert.match(text, /Lizenz\s+CC0/);
    assert.match(text, /https:\/\/kenney\.nl\s+Link kopieren/);
  } finally { app.dispose(); }
});

test("duplicates show a card chip, filter the catalog and list selectable copies in the origin tab", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.runTimers();
    app.requests[0].resolve(page([{ ...asset, duplicates: 1 }])); await app.settle();
    assert.match(app.text(app.find((node) => String(node.props.className).includes("duplicate-chip"))), /^2\s*×$/);
    const filter = app.elements(app.find((node) => node.props.className === "checkbox-filter")).find((node) => node.type === "input")!;
    filter.props.onChange({ target: { checked: true } }); await app.settle(); await app.runTimers();
    assert.equal(app.latestRequest?.args.query.duplicates, true);
    assert.equal(app.latestRequest?.args.query.offset, 0);
    assert.match(app.text(app.tree), /Duplikate \(\s*3\s*\)/);
    assert.equal(app.find((node) => node.props.className === "filter-count").props.children, 1);
    app.latestRequest!.resolve(page([{ ...asset, duplicates: 1 }], 1)); await app.settle();
    assert.match(app.text(app.tree), /Gleicher Inhalt \(Stand der letzten Prüfung\):/);
    assert.match(app.find((node) => String(node.props.className).includes("duplicate-chip")).props.title, /^Gleicher Inhalt \(Stand der letzten Prüfung\) in 1 weiteren Datei$/);
    const copy = app.find((node) => node.type === "button" && /Other Pack\s+·\s+sfx\/match\.mp3/.test(app.text(node)));
    copy.props.onClick(); await app.settle();
    assert.deepEqual(app.latestRequest?.args.query, { assetId: 2, offset: 0, limit: 1 });
    const other = { ...asset, id: 2, name: "Other Sound", packageName: "Other Pack", files: [{ ...matchingFile, id: 31 }], duplicates: 1 };
    app.latestRequest!.resolve(page([other], 1)); await app.settle();
    assert.equal(app.text(app.find((node) => node.type === "h2" && /Other Sound/.test(app.text(node)))), "Other Sound");
    assert.equal(app.find((node) => node.type === AudioPreview).props.file.id, 31);
  } finally { app.dispose(); }
});

test("turning the duplicate filter on verifies duplicates before querying and refreshes counts after a drop", async () => {
  for (const dropped of [0, 2]) {
    const app = new Harness();
    app.droppedHashes = dropped;
    try {
      await app.mount();
      const filters = app.commands.filter((command) => command === "catalog_filters").length;
      app.commands.length = 0;
      const filter = app.elements(app.find((node) => node.props.className === "checkbox-filter")).find((node) => node.type === "input")!;
      await filter.props.onChange({ target: { checked: true } }); await app.settle(); await app.runTimers();
      assert.deepEqual(app.commands.filter((command) => command !== "catalog_filters"), ["verify_duplicates", "query_catalog"]);
      assert.equal(app.latestRequest?.args.query.duplicates, true);
      assert.equal(app.commands.filter((command) => command === "catalog_filters").length, dropped ? 1 : 0, `dropped ${dropped}`);
      assert.ok(filters > 0);
    } finally { app.dispose(); }
  }
});

test("a saved duplicate search verifies duplicates and switching the filter off stays off", async () => {
  const app = new Harness();
  const stored = { text: "", kind: null, workflow: null, packageId: null, folderId: null, originalFolder: null, format: null, tag: null, duplicates: true,
    audioFilters: { minDurationSeconds: "", maxDurationSeconds: "", channels: "any", analysis: "any" } };
  app.savedSearches = [{ id: 7, name: "Doppelte", queryJson: JSON.stringify(stored) }];
  try {
    await app.mount();
    app.commands.length = 0;
    app.find((node) => node.type === "button" && node.props.title === "Doppelte").props.onClick(); await app.settle(); await app.runTimers();
    assert.ok(app.commands.includes("verify_duplicates"), "applying a saved duplicate search runs the check");
    assert.equal(app.latestRequest?.args.query.duplicates, true);
    const filter = () => app.elements(app.find((node) => node.props.className === "checkbox-filter")).find((node) => node.type === "input")!;
    filter().props.onChange({ target: { checked: false } }); filter().props.onChange({ target: { checked: true } }); filter().props.onChange({ target: { checked: false } });
    await app.settle(); await app.runTimers();
    assert.equal(filter().props.checked, false, "the last choice wins");
    assert.notEqual(app.latestRequest?.args.query.duplicates, true);
  } finally { app.dispose(); }
});

test("a copy list check that dropped changed hashes refreshes the Sounds results and counts", async () => {
  const app = new Harness();
  app.droppedHashes = 1;
  try {
    await app.mount(); await app.chooseType("sound"); await app.runTimers();
    app.commands.length = 0;
    app.latestRequest!.resolve(page([{ ...asset, duplicates: 1 }])); await app.settle(); await app.runTimers();
    assert.deepEqual(app.commands.filter((command) => command.includes("duplicates") || command === "query_catalog" || command === "catalog_filters").slice(0, 3),
      ["asset_duplicates", "catalog_filters", "query_catalog"]);
    assert.equal(app.latestRequest!.args.query.kind, "sound");
  } finally { app.dispose(); }
});

// Saved searches, the search shortcut and auto-tags share the App harness above.
test("saved searches apply every stored filter, can be saved from the search box and deleted", async () => {
  const app = new Harness();
  const stored = { text: "barr nuk", kind: "model", workflow: null, packageId: 1, folderId: null, originalFolder: null, format: null, tag: null, duplicates: true,
    audioFilters: { minDurationSeconds: "", maxDurationSeconds: "", channels: "any", analysis: "any" } };
  app.savedSearches = [{ id: 5, name: "Fässer", queryJson: JSON.stringify(stored) }];
  try {
    await app.mount();
    assert.equal(app.elements(app.tree).some((node) => node.type === "button" && app.text(node).trim() === "Suche speichern"), false, "Nothing to save without a search");
    const saved = () => app.find((node) => node.type === "button" && node.props.title === "Fässer");
    assert.equal(saved().props["aria-pressed"], false);
    saved().props.onClick(); await app.settle(); await app.runTimers();
    const { query } = app.latestRequest!.args;
    assert.deepEqual([query.text, query.kind, query.packageId, query.duplicates, query.offset], ["barr nuk", "model", 1, true, 0]);
    assert.equal(app.control("Assets durchsuchen").props.value, "barr nuk");
    assert.equal(saved().props["aria-pressed"], true);

    await app.change("Assets durchsuchen", "bite");
    await app.click("Suche speichern");
    assert.equal(app.control("Name der gespeicherten Suche").props.value, "bite");
    await app.change("Name der gespeicherten Suche", "Zombies");
    app.find((node) => node.type === "form" && node.props.className === "save-search").props.onSubmit({ preventDefault() {} });
    await app.settle();
    const [command, args] = app.savedSearchCalls[0];
    assert.equal(command, "save_search");
    assert.equal(args.name, "Zombies");
    assert.equal(JSON.parse(args.queryJson).text, "bite");
    assert.ok(app.find((node) => node.type === "button" && node.props.title === "Zombies"));

    app.control("Gespeicherte Suche Fässer löschen").props.onClick(); await app.settle();
    assert.deepEqual(app.savedSearchCalls[1], ["delete_saved_search", { id: 5 }]);
    assert.equal(app.elements(app.tree).some((node) => node.props.title === "Fässer"), false);
  } finally { app.dispose(); }
});

test("slash and Command-F focus the search box, but slash keeps working inside text fields", async () => {
  const app = new Harness();
  try {
    await app.mount();
    let focused = 0;
    app.control("Assets durchsuchen").props.ref.current = { focus: () => focused++, select() {} };
    const press = (key: string, tagName = "DIV", metaKey = false) => {
      let prevented = false;
      for (const listener of keydownListeners) listener({ key, metaKey, ctrlKey: false, target: { tagName }, preventDefault: () => { prevented = true; } });
      return prevented;
    };
    assert.equal(press("/"), true);
    assert.equal(focused, 1);
    assert.equal(press("/", "INPUT"), false, "Typing a slash in a field stays text");
    assert.equal(press("f", "INPUT", true), true);
    assert.equal(focused, 2);
    assert.equal(press("a"), false);
  } finally { app.dispose(); }
});

test("inspector offers auto-tags the user does not have and adopts one into the user tags", async () => {
  const app = new Harness();
  try {
    await app.settle(); await app.runTimers();
    app.requests[0].resolve(page([{ ...asset, tags: ["Wind"], autoTags: ["wind", "river"] }])); await app.settle();
    assert.match(app.text(app.control("Auto-Tags")), /river/);
    assert.equal(app.elements(app.tree).some((node) => node.props["aria-label"] === "Auto-Tag wind übernehmen"), false);
    app.control("Auto-Tag river übernehmen").props.onClick(); await app.settle();
    assert.deepEqual(app.annotations, [{ assetId: 1, name: "Sound", tags: ["Wind", "river"] }]);
  } finally { app.dispose(); }
});

test("a ZIP package shows its badge and archive entry, and ZIP einbinden references the chosen archive", async () => {
  const app = new Harness();
  const zipPackage = { ...library.packages[0], relativePath: "zip/k1", sourceKind: "external", externalRoot: "/Downloads/Forest.zip", zip: true };
  app.libraryInfo = { ...library, packages: [zipPackage] };
  try {
    await app.settle(); await app.runTimers();
    app.requests[0].resolve(page([{ ...asset, files: [{ ...matchingFile, relativePath: "zip/k1/Sounds/match.mp3" }] }])); await app.settle();
    assert.equal(app.text(app.find((node) => node.props.className === "package-zip-badge")), "ZIP");
    assert.match(app.text(app.tree), /Aus ZIP\s+\/Downloads\/Forest\.zip\/Sounds\/match\.mp3/);
    await app.click("ZIP einbinden");
    assert.deepEqual(app.references, [{ source: "/owned/reopened" }]);
    await app.settle();
    assert.match(app.text(app.tree), /Forest als ZIP eingebunden/);
  } finally { app.dispose(); }
});

test("external folders can be watched; checks run on open and focus and report changes", async () => {
  const app = new Harness();
  const folder = { ...library.packages[0], sourceKind: "external", externalRoot: "/owned/External", zip: false, watched: false };
  app.libraryInfo = { ...library, packages: [folder] };
  try {
    await app.mount();
    assert.equal(app.watchChecks, 1, "checked on open");
    const actions = app.find((node) => node.props.label === `Paket ${folder.name}`);
    assert.equal(actions.props.watched, false);
    actions.props.onWatch(); await app.settle();
    assert.deepEqual(app.watchCalls, [{ packageId: folder.id, watched: true }]);
    app.watchNotices = [{ packageId: folder.id, message: "Neue Dateien in Pack: 2 neu.", summary: { added: 2, changed: 0, missing: 0, ambiguous: [] } }];
    for (const listener of app.focusListeners) listener();
    await app.settle(); await app.settle();
    assert.equal(app.watchChecks, 2, "checked on focus");
    assert.match(app.text(app.tree), /Neue Dateien in Pack: 2 neu\./);
  } finally { app.dispose(); }
});

test("a Unity package shows its own badge and labels and is referenced like a ZIP", async () => {
  const app = new Harness();
  const unityPackage = { ...library.packages[0], relativePath: "unity/k1", sourceKind: "external", externalRoot: "/Assets/Forest.unitypackage", zip: true, unity: true };
  app.libraryInfo = { ...library, packages: [unityPackage] };
  app.unityReference = true;
  try {
    await app.settle(); await app.runTimers();
    app.requests[0].resolve(page([{ ...asset, files: [{ ...matchingFile, relativePath: "unity/k1/Assets/Sounds/match.mp3" }] }])); await app.settle();
    assert.equal(app.text(app.find((node) => node.props.className === "package-zip-badge")), "Unity");
    assert.match(app.text(app.tree), /Aus Unity-Paket\s+\/Assets\/Forest\.unitypackage\/Assets\/Sounds\/match\.mp3/);
    await app.click("Unity-Paket einbinden");
    assert.deepEqual(app.references, [{ source: "/owned/reopened" }]);
    await app.settle();
    assert.match(app.text(app.tree), /Forest als Unity-Paket eingebunden/);
  } finally { app.dispose(); }
});

test("finished duplicate and auto-tag passes refresh sound results unless a sound analysis is running", async () => {
  const app = new Harness();
  const finish = async (event: string) => {
    for (const handler of app.listeners[event] ?? []) handler({ payload: { done: 3, remaining: 0 } });
    await app.settle(); await app.runTimers();
  };
  try {
    await app.mount(); await app.chooseType("sound"); await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    let requests = app.requests.length;
    await finish("file-hashes-progress");
    assert.equal(app.requests.length, requests + 1);
    assert.equal(app.latestRequest!.args.query.kind, "sound");
    app.latestRequest!.resolve(page()); await app.settle();

    await app.click("Analyse vorbereiten");
    const requestId = app.prepareRequests[0].args.requestId;
    app.prepareRequests[0].resolve({ requestId, total: 87 }); await app.settle();
    await app.click("Analyse starten");
    requests = app.requests.length;
    await finish("auto-tags-progress");
    assert.equal(app.requests.length, requests, "A running sound analysis is not discarded");
    assert.deepEqual(app.populationCancellations, []);
  } finally { app.dispose(); }
});

test("a hashing run that only dropped changed files' hashes refreshes the catalog", async () => {
  const app = new Harness();
  try {
    await app.mount(); await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    const requests = app.requests.length;
    for (const handler of app.listeners["file-hashes-progress"] ?? []) handler({ payload: { done: 0, remaining: 0, invalidated: 2 } });
    await app.settle(); await app.runTimers();
    assert.equal(app.requests.length, requests + 1);
  } finally { app.dispose(); }
});

test("passes finishing while a sound analysis is prepared or running refresh once it is idle", async () => {
  const app = new Harness();
  const finish = async (event: string) => {
    for (const handler of app.listeners[event] ?? []) handler({ payload: { done: 3, remaining: 0 } });
    await app.settle(); await app.runTimers();
  };
  try {
    await app.mount(); await app.chooseType("sound"); await app.runTimers();
    app.latestRequest!.resolve(page()); await app.settle();
    await app.click("Analyse vorbereiten");
    const requestId = app.prepareRequests[0].args.requestId;
    app.prepareRequests[0].resolve({ requestId, total: 87 }); await app.settle();
    const requests = app.requests.length;
    await finish("file-hashes-progress");
    assert.equal(app.requests.length, requests, "A prepared sound analysis is not discarded");
    assert.match(app.text(app.tree), /87\s+passende Dateivarianten/);
    await app.click("Analyse starten");
    await finish("auto-tags-progress");
    assert.equal(app.requests.length, requests);
    assert.deepEqual(app.populationCancellations, []);
    app.populationRequests[0].resolve({ requestId, total: 87, processed: 87, ready: 87, errors: 0, skipped: 0, cancelled: false });
    await app.settle(); await app.runTimers();
    assert.equal(app.requests.length, requests + 1, "The deferred refresh runs once the analysis finished");
    await app.runTimers();
    assert.equal(app.requests.length, requests + 1, "and only once");
  } finally { app.dispose(); }
});

test("saved searches with malformed JSON or wrongly typed fields cannot break the catalog", async () => {
  const app = new Harness();
  app.savedSearches = [
    { id: 5, name: "Kaputt", queryJson: "{nope" },
    { id: 6, name: "Seltsam", queryJson: JSON.stringify({ text: {}, kind: "weapon", packageId: "1", folderId: 1.5, format: 3, duplicates: "yes",
      audioFilters: { minDurationSeconds: 2, maxDurationSeconds: "9", channels: "5.1", analysis: "ready" } }) },
  ];
  try {
    await app.mount();
    app.find((node) => node.type === "button" && node.props.title === "Kaputt").props.onClick(); await app.settle();
    assert.match(app.text(app.tree), /gespeicherte Suche „Kaputt“ ist nicht lesbar/);
    app.find((node) => node.type === "button" && node.props.title === "Seltsam").props.onClick(); await app.settle(); await app.runTimers();
    const { query, audioCriteria } = app.latestRequest!.args;
    assert.deepEqual([query.text, query.kind, query.packageId, query.folderId, query.format, query.duplicates], ["", null, null, null, null, false]);
    assert.deepEqual(audioCriteria, { minDurationSeconds: null, maxDurationSeconds: 9, channels: "any", analysis: "ready" });
  } finally { app.dispose(); }
});
