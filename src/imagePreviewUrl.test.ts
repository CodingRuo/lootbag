import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { decodeTgaUrl, imagePreviewUrl, loadPreviewImage } from "./imagePreviewUrl.ts";
import { decodeTga } from "./tgaDecode.ts";

const red = Uint8Array.from([0,0,2,0,0,0,0,0,0,0,0,0,1,0,1,0,32,40,0,0,255,255]);
const dataUrl = (data: Uint8Array) => `data:image/x-tga;base64,${Buffer.from(data).toString("base64")}#snapshot=test`;

function workerBoundary(t: TestContext, stallFirst = false, failure?: "error" | "messageerror") {
  const original = Object.getOwnPropertyDescriptor(globalThis, "Worker");
  let started = 0, active = 0, maxActive = 0;
  let posted!: () => void;
  const firstPost = new Promise<void>((resolve) => { posted = resolve; });
  class BoundaryWorker {
    onmessage: ((event: { data: unknown }) => void) | null = null;
    onerror: (() => void) | null = null;
    onmessageerror: (() => void) | null = null;
    stopped = false;
    constructor() { started++; active++; maxActive = Math.max(maxActive, active); }
    postMessage(buffer: ArrayBuffer) {
      posted();
      if (stallFirst && started === 1) return;
      queueMicrotask(() => {
        if (this.stopped) return;
        if (failure && started === 1) { if (failure === "error") this.onerror?.(); else this.onmessageerror?.(); return; }
        try {
          const decoded = decodeTga(buffer);
          this.onmessage?.({ data: { width: decoded.width, height: decoded.height, buffer: decoded.data.buffer } });
        } catch (reason) { this.onmessage?.({ data: { error: (reason as Error).message } }); }
      });
    }
    terminate() { if (!this.stopped) { this.stopped = true; active--; } }
  }
  Object.defineProperty(globalThis, "Worker", { configurable: true, value: BoundaryWorker });
  t.after(() => { if (original) Object.defineProperty(globalThis, "Worker", original); else Reflect.deleteProperty(globalThis, "Worker"); });
  return { firstPost, counts: () => ({ started, active, maxActive }) };
}

test("worker-boundary cancellation releases the decoder owner for the next valid image", async (t) => {
  const boundary = workerBoundary(t, true);
  const controller = new AbortController();
  const cancelled = decodeTgaUrl(dataUrl(red), controller.signal);
  const rejection = assert.rejects(cancelled, { name: "AbortError" });
  await boundary.firstPost;
  const next = decodeTgaUrl(dataUrl(red), new AbortController().signal);
  controller.abort();
  await rejection;
  const decoded = await next;
  assert.deepEqual([...new Uint8Array(decoded.buffer)], [255,0,0,255]);
  assert.deepEqual(boundary.counts(), { started: 2, active: 0, maxActive: 1 });
});

test("malformed worker input fails visibly without stranding queued valid input", async (t) => {
  const boundary = workerBoundary(t);
  const broken = decodeTgaUrl(dataUrl(red.subarray(0, 18)), new AbortController().signal);
  const valid = decodeTgaUrl(dataUrl(red), new AbortController().signal);
  await assert.rejects(broken, /unvollständig/);
  assert.deepEqual([...new Uint8Array((await valid).buffer)], [255,0,0,255]);
  assert.deepEqual(boundary.counts(), { started: 2, active: 0, maxActive: 1 });
});

test("queued cancellation never fetches or starts the abandoned decoder", async (t) => {
  const boundary = workerBoundary(t, true);
  const first = new AbortController(), second = new AbortController();
  const pending = decodeTgaUrl(dataUrl(red), first.signal);
  const pendingError = assert.rejects(pending, { name: "AbortError" });
  await boundary.firstPost;
  const queued = decodeTgaUrl(dataUrl(red), second.signal);
  const queuedError = assert.rejects(queued, { name: "AbortError" });
  second.abort(); first.abort();
  await Promise.all([pendingError, queuedError]);
  await decodeTgaUrl(dataUrl(red), new AbortController().signal);
  assert.deepEqual(boundary.counts(), { started: 2, active: 0, maxActive: 1 });
});

test("oversize response headers cancel unread data before a decoder is started", async (t) => {
  const boundary = workerBoundary(t);
  let cancelled = false;
  t.mock.method(globalThis, "fetch", async () => new Response(new ReadableStream({ cancel() { cancelled = true; } }), { headers: { "content-length": "67108865" } }));
  await assert.rejects(decodeTgaUrl("https://fixture.invalid/oversize.tga", new AbortController().signal), /64 MiB/);
  assert.equal(cancelled, true);
  assert.equal(boundary.counts().started, 0);
});

test("ordinary image URLs keep their original transport and need no decoder", async () => {
  const preview = await imagePreviewUrl("asset://localhost/Stone.png", "Stone.png", new AbortController().signal);
  assert.equal(preview.url, "asset://localhost/Stone.png");
  preview.release();
});

test("cancelled PNG encoding releases the serial owner even when canvas never calls back", async (t) => {
  workerBoundary(t);
  const saved = ["document", "ImageData"].map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  t.after(() => { for (const [name, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
  let encoding!: () => void;
  const began = new Promise<void>((resolve) => { encoding = resolve; });
  Object.defineProperty(globalThis, "ImageData", { configurable: true, value: class { constructor(_pixels: Uint8ClampedArray, _width: number, _height: number) {} } });
  Object.defineProperty(globalThis, "document", { configurable: true, value: { createElement: () => ({ width: 0, height: 0, getContext: () => ({ putImageData: () => {} }), toBlob: () => encoding() }) } });
  const controller = new AbortController();
  const cancelled = imagePreviewUrl(dataUrl(red), "Checker.tga", controller.signal);
  const rejection = assert.rejects(cancelled, { name: "AbortError" });
  await began; controller.abort(); await rejection;
  const nextController = new AbortController();
  const next = decodeTgaUrl(dataUrl(red), nextController.signal);
  const advanced = await Promise.race([next.then(() => true), new Promise<false>((resolve) => setTimeout(() => resolve(false), 100))]);
  if (!advanced) { nextController.abort(); await assert.rejects(next, { name: "AbortError" }); }
  assert.equal(advanced, true);
});


test("decoded image dimensions honor the remaining group budget and release their URL on failure", async (t) => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "Image");
  let current = "";
  class ImageBoundary {
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    naturalWidth = 4096; naturalHeight = 4096;
    set src(value: string) { current = value; if (value) queueMicrotask(() => this.onload?.()); }
  }
  Object.defineProperty(globalThis, "Image", { configurable: true, value: ImageBoundary });
  t.after(() => { if (original) Object.defineProperty(globalThis, "Image", original); else Reflect.deleteProperty(globalThis, "Image"); });
  await assert.rejects(loadPreviewImage("asset://localhost/large.png", "large.png", new AbortController().signal, 1024), /Pixelbudget/);
  assert.equal(current, "");
});


test("worker startup and transfer failures release the owner for another valid decode", async (t) => {
  for (const failure of ["error", "messageerror"] as const) {
    await t.test(failure, async (child) => {
      const boundary = workerBoundary(child, false, failure);
      await assert.rejects(decodeTgaUrl(dataUrl(red), new AbortController().signal), /Decoder|übertragen/);
      assert.deepEqual([...new Uint8Array((await decodeTgaUrl(dataUrl(red), new AbortController().signal)).buffer)], [255,0,0,255]);
      assert.deepEqual(boundary.counts(), { started: 2, active: 0, maxActive: 1 });
    });
  }
});

test("the decoder timeout terminates a stalled worker and permits the next job", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const boundary = workerBoundary(t, true);
  const stalled = decodeTgaUrl(dataUrl(red), new AbortController().signal);
  const failure = assert.rejects(stalled, /Zeitlimit/);
  await boundary.firstPost;
  t.mock.timers.tick(30_000);
  await failure;
  assert.deepEqual([...new Uint8Array((await decodeTgaUrl(dataUrl(red), new AbortController().signal)).buffer)], [255,0,0,255]);
  assert.deepEqual(boundary.counts(), { started: 2, active: 0, maxActive: 1 });
});

test("the waiting queue is bounded and abandoned entries never start workers", async (t) => {
  const boundary = workerBoundary(t, true);
  const firstController = new AbortController();
  const first = decodeTgaUrl(dataUrl(red), firstController.signal);
  const firstError = assert.rejects(first, { name: "AbortError" });
  await boundary.firstPost;
  const controllers = Array.from({ length: 48 }, () => new AbortController());
  const rejections = controllers.map((controller) => assert.rejects(decodeTgaUrl(dataUrl(red), controller.signal), { name: "AbortError" }));
  await assert.rejects(decodeTgaUrl(dataUrl(red), new AbortController().signal), /ausgelastet/);
  for (const controller of controllers) controller.abort();
  firstController.abort();
  await Promise.all([firstError, ...rejections]);
  await decodeTgaUrl(dataUrl(red), new AbortController().signal);
  assert.deepEqual(boundary.counts(), { started: 2, active: 0, maxActive: 1 });
});

test("a lying content-length cannot allow an oversized stream through to allocation", async (t) => {
  const boundary = workerBoundary(t);
  let cancelled = false;
  t.mock.method(globalThis, "fetch", async () => new Response(new ReadableStream({
    start(controller) { controller.enqueue({ byteLength: 67108865 }); },
    cancel() { cancelled = true; },
  }), { headers: { "content-length": "1" } }));
  await assert.rejects(decodeTgaUrl("https://fixture.invalid/overflow.tga", new AbortController().signal), /64 MiB/);
  assert.equal(cancelled, true); assert.equal(boundary.counts().started, 0);
});

test("a job waiting behind a stalled decode gets its own full time limit", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const boundary = workerBoundary(t, true);
  const stalled = decodeTgaUrl(dataUrl(red), new AbortController().signal);
  const failure = assert.rejects(stalled, /Zeitlimit/);
  await boundary.firstPost;
  const waiting = decodeTgaUrl(dataUrl(red), new AbortController().signal);
  t.mock.timers.tick(30_000);
  await failure;
  assert.deepEqual([...new Uint8Array((await waiting).buffer)], [255,0,0,255]);
});

test("grid thumbnails downscale large PNGs and keep small ones at their original URL", async (t) => {
  const saved = ["document", "createImageBitmap"].map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  t.after(() => { for (const [name, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
  t.mock.method(globalThis, "fetch", async () => new Response(new Blob(["png"])));
  let size = { width: 2048, height: 1024 };
  const canvases: { width: number; height: number }[] = [];
  Object.defineProperty(globalThis, "createImageBitmap", { configurable: true, value: async () => ({ ...size, close: () => {} }) });
  Object.defineProperty(globalThis, "document", { configurable: true, value: { createElement: () => {
    const canvas = { width: 0, height: 0, getContext: () => ({ drawImage: () => canvases.push({ width: canvas.width, height: canvas.height }) }), toBlob: (done: (blob: Blob) => void) => done(new Blob(["small"])) };
    return canvas;
  } } });
  const large = await imagePreviewUrl("asset://localhost/Large.png", "Large.png", new AbortController().signal, 512);
  assert.match(large.url, /^blob:/);
  assert.deepEqual(canvases, [{ width: 512, height: 256 }]);
  large.release();
  size = { width: 300, height: 200 };
  const small = await imagePreviewUrl("asset://localhost/Small.png", "Small.png", new AbortController().signal, 512);
  assert.equal(small.url, "asset://localhost/Small.png");
});
