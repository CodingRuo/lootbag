import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { imagePreviewUrl } from "./imagePreviewUrl.ts";
import { readBlendThumbnail } from "./blendThumbnail.ts";

const blend = Uint8Array.from([...Buffer.from("BLENDER17-01v0502"), ...Buffer.from("TEST"), ...new Uint8Array(12), 24,0,0,0,0,0,0,0, ...new Uint8Array(8), 2,0,0,0,2,0,0,0, 0,0,255,255, 0,255,255,0, 255,0,0,255, 255,0,255,128]);
const transport = (bytes = blend) => `data:application/octet-stream;base64,${Buffer.from(bytes).toString("base64")}#model-preview=owned`;

function boundaries(t: TestContext, stall: "worker" | "png" | null = null) {
  const saved = ["Worker", "document", "ImageData"].map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  t.after(() => { for (const [name, descriptor] of saved) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
  let started!: () => void;
  const firstPost = new Promise<void>((resolve) => { started = resolve; });
  let encoding!: () => void;
  const firstPng = new Promise<void>((resolve) => { encoding = resolve; });
  let jobs = 0, active = 0, maxActive = 0;
  class WorkerBoundary {
    onmessage: ((event: { data: unknown }) => void) | null = null;
    onerror: (() => void) | null = null;
    onmessageerror: (() => void) | null = null;
    stopped = false;
    constructor(url: URL) { assert.match(url.pathname, /blendWorker\.ts$/); jobs++; active++; maxActive = Math.max(maxActive, active); }
    postMessage(buffer: ArrayBuffer) {
      started(); if (stall === "worker" && jobs === 1) return;
      queueMicrotask(() => { if (this.stopped) return; try { this.onmessage?.({ data: readBlendThumbnail(buffer) }); } catch (reason) { this.onmessage?.({ data: { error: (reason as Error).message } }); } });
    }
    terminate() { if (!this.stopped) { this.stopped = true; active--; } }
  }
  const pixels: number[][] = [], canvases: { width: number; height: number }[] = [], sizes: number[][] = [];
  Object.defineProperty(globalThis, "Worker", { configurable: true, value: WorkerBoundary });
  Object.defineProperty(globalThis, "ImageData", { configurable: true, value: class { constructor(data: Uint8ClampedArray) { pixels.push([...data]); } } });
  Object.defineProperty(globalThis, "document", { configurable: true, value: { createElement: () => {
    const canvas = { width: 0, height: 0, getContext: () => ({ putImageData: () => {}, drawImage: () => {} }),
      toBlob: (accept: (blob: Blob) => void) => { sizes.push([canvas.width, canvas.height]); encoding(); if (stall !== "png" || sizes.length > 1) accept(new Blob(["owned PNG proxy"], { type: "image/png" })); },
    }; canvases.push(canvas); return canvas;
  } } });
  return { firstPost, firstPng, pixels, canvases, sizes, counts: () => ({ jobs, active, maxActive }) };
}

test("saved Blender previews use the bounded worker and an owned downsampled PNG proxy", async (t) => {
  const boundary = boundaries(t);
  const proxy = await imagePreviewUrl(transport(), "Owned.BLEND", new AbortController().signal, 1);
  assert.equal((await fetch(proxy.url)).headers.get("content-type"), "image/png");
  assert.deepEqual(boundary.pixels, [[255,0,0,255,255,0,255,128,0,0,255,255,0,255,255,0]]);
  assert.deepEqual(boundary.sizes, [[1,1]]);
  assert.ok(boundary.canvases.every((canvas) => canvas.width === 0 && canvas.height === 0));
  assert.deepEqual(boundary.counts(), { jobs: 1, active: 0, maxActive: 1 });
  proxy.release(); await assert.rejects(fetch(proxy.url));
  assert.deepEqual(new Uint8Array(await (await fetch(transport())).arrayBuffer()), blend);
});

test("cancelled Blender workers and stalled PNG encoding release ownership for the next source", async (t) => {
  for (const stall of ["worker", "png"] as const) await t.test(stall, async (child) => {
    const boundary = boundaries(child, stall), controller = new AbortController();
    const pending = imagePreviewUrl(transport(), "Owned.blend", controller.signal);
    const failure = assert.rejects(pending, { name: "AbortError" });
    await (stall === "worker" ? boundary.firstPost : boundary.firstPng);
    const next = imagePreviewUrl(transport(), "Next.blend", new AbortController().signal);
    controller.abort(); await failure;
    const valid = await next;
    assert.equal((await fetch(valid.url)).headers.get("content-type"), "image/png");
    valid.release();
    assert.deepEqual(boundary.counts(), { jobs: 2, active: 0, maxActive: 1 });
    assert.ok(boundary.canvases.every((canvas) => canvas.width === 0 && canvas.height === 0));
  });
});

test("invalid saved Blender thumbnails do not strand a queued valid preview", async (t) => {
  const boundary = boundaries(t);
  const invalid = imagePreviewUrl(transport(blend.subarray(0, -1)), "Broken.blend", new AbortController().signal);
  const failure = assert.rejects(invalid, /unvollständig/);
  const next = imagePreviewUrl(transport(), "Owned.blend", new AbortController().signal);
  await failure; const valid = await next;
  valid.release();
  assert.deepEqual(boundary.counts(), { jobs: 2, active: 0, maxActive: 1 });
});

test("oversized Blender responses are cancelled before starting the worker", async (t) => {
  const boundary = boundaries(t);
  let cancelled = false;
  t.mock.method(globalThis, "fetch", async () => new Response(new ReadableStream({ cancel() { cancelled = true; } }), { headers: { "content-length": "67108865" } }));
  await assert.rejects(imagePreviewUrl("asset://localhost/large.blend", "Large.blend", new AbortController().signal), /64 MiB/);
  assert.equal(cancelled, true); assert.deepEqual(boundary.counts(), { jobs: 0, active: 0, maxActive: 0 });
});

test("the Blender worker timeout terminates the old source before handing off to valid input", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const boundary = boundaries(t, "worker");
  const pending = imagePreviewUrl(transport(), "Stalled.blend", new AbortController().signal);
  const failure = assert.rejects(pending, /Zeitlimit/);
  await boundary.firstPost;
  t.mock.timers.tick(30_000); await failure;
  const next = await imagePreviewUrl(transport(), "Owned.blend", new AbortController().signal);
  next.release();
  assert.deepEqual(boundary.counts(), { jobs: 2, active: 0, maxActive: 1 });
});
