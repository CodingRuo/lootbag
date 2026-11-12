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
const globals = globalThis as typeof globalThis & { audioFilterHarness: Harness; window: any };
const mocks: Record<string, string> = {
  react: `export const useState = (...args) => globalThis.audioFilterHarness.useState(...args);
    export const useEffect = (...args) => globalThis.audioFilterHarness.useEffect(...args);
    export const useRef = (...args) => globalThis.audioFilterHarness.useRef(...args);
    export const useId = () => "test-id";`,
  "react/jsx-runtime": `export const jsx = (type, props) => ({ type, props }); export const jsxs = jsx; export const Fragment = "fragment";`,
  "@tauri-apps/api/core": `export const isTauri = () => true; export const invoke = (...args) => globalThis.audioFilterHarness.invoke(...args); export const convertFileSrc = path => path; export class Channel {}`,
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
// App composes its regions from these function components; the harness renders them inline so their controls stay queryable.
const regions = new Set(await Promise.all(["Header", "Onboarding", "Sidebar", "Catalog", "Inspector"]
  .map(async (name) => (await import(new URL(`${name}.tsx`, sourceRoot).href)).default)));

const library = { root: "/owned/library", name: "Owned library", packages: [{ id: 1, name: "Fixture", relativePath: "fixture", assetCount: 41, sourceKind: "managed", available: true, externalRoot: null, tags: [] }], folders: [], assetCount: 41, kindCounts: { sound: 41, image: 0, model: 0, material: 0, spriteSequence: 0 } };
const matchingFile = { id: 11, originalFilename: "match.mp3", relativePath: "match.mp3", originalFolder: "", format: "MP3", kind: "sound", sizeBytes: 50, audio: { status: "ready", durationSeconds: 1, sampleRate: 44100, channels: 2, codec: "mp3", error: null }, available: true };
const alternateFile = { ...matchingFile, id: 12, originalFilename: "alternate.wav", format: "WAV", audio: { ...matchingFile.audio, durationSeconds: 9, channels: 1 } };
const asset = { id: 1, name: "Sound", kind: "sound", packageId: 1, packageName: "Fixture", originalFolder: "", files: [matchingFile, alternateFile], tags: [], folderIds: [] };
const page = (items = [asset], total = 141, offset = 0) => ({ items, total, offset, limit: 100 });

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
  invoke = (command: string, args: any) => {
    if (command === "prepare_audio_analysis") return new Promise((resolve, reject) => this.prepareRequests.push({ args, resolve, reject }));
    if (command === "run_audio_analysis") return new Promise((resolve, reject) => this.populationRequests.push({ args, resolve, reject }));
    if (command === "cancel_audio_analysis") {
      this.populationCancellations.push(args.requestId);
      return this.populationCancelError ? Promise.reject(this.populationCancelError) : Promise.resolve();
    }
    if (command === "query_catalog") return new Promise((resolve, reject) => this.requests.push({ args, resolve, reject }));
    if (command === "get_library") return Promise.resolve(library);
    if (command === "open_library") return Promise.resolve({ ...library, root: "/owned/reopened" });
    if (command === "catalog_filters") return Promise.resolve({ folders: ["ambience"], formats: ["MP3", "WAV", "OGG", "FLAC", "PNG", "OBJ"], tags: ["wind"] });
    if (command === "analyze_audio") return new Promise((resolve, reject) => this.analysisRequests.push({ resolve, reject }));
    if (command === "resolve_preview") return Promise.resolve("/owned/sound.wav");
    if (command === "audio_waveform") return Promise.resolve({ durationSeconds: 1, peaks: [] });
    if (command === "audio_tags") return Promise.resolve({ status: "ready", title: null, artist: null, album: null, genre: null, message: null });
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
    globals.window = { setTimeout: (callback: () => void) => { const id = ++this.timerId; this.timers.set(id, callback); return id; }, clearTimeout: (id: number) => this.timers.delete(id) };
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
    assert.equal(app.find((node) => node.type === "button" && app.text(node).trim() === "Paket importieren").props.disabled, true, "File mutations stay inside the existing operation boundary");
    assert.equal(app.control("Assets durchsuchen").props.disabled, undefined, "Reading and search remain usable");
    await app.click("Analyse abbrechen");
    assert.deepEqual(app.populationCancellations, [requestId]);
    assert.match(app.text(app.tree), /Wird abgebrochen/);
    app.populationRequests[0].resolve({ requestId, total: 87, processed: 6, ready: 4, errors: 1, skipped: 1, cancelled: true });
    await app.settle(); await app.runTimers();
    assert.match(app.text(app.tree), /Abgebrochen.*6.*87/);
    assert.match(app.text(app.tree), /Bereits gespeicherte Analysen bleiben erhalten/);
    assert.equal(app.find((node) => node.type === "button" && app.text(node).trim() === "Paket importieren").props.disabled, false);
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
      assert.equal(app.find((node) => node.type === "button" && app.text(node).trim() === "Paket importieren").props.disabled, false);
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
