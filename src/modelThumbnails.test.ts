import assert from "node:assert/strict";
import test from "node:test";
import { createModelThumbnailQueue, previewScope, type ModelThumbnailResult } from "./modelThumbnails.ts";

const thumbnail = (id: number): ModelThumbnailResult => ({ url: `blob:${id}`, warning: null, error: null });
const flush = () => new Promise<void>((resolve) => setImmediate(resolve));
function deferred() {
  let resolve!: (result: ModelThumbnailResult) => void;
  const promise = new Promise<ModelThumbnailResult>((done) => { resolve = done; });
  return { promise, resolve };
}

test("thumbnails render one at a time and same-key subscribers share one job", async () => {
  const jobs = [deferred(), deferred()];
  const started: number[] = [];
  let active = 0;
  let peak = 0;
  const queue = createModelThumbnailQueue(async (id) => {
    started.push(id);
    peak = Math.max(peak, ++active);
    const result = await jobs[id - 1].promise;
    active--;
    return result;
  }, () => {}, () => {});
  const first: ModelThumbnailResult[] = [];
  const duplicate: ModelThumbnailResult[] = [];
  const second: ModelThumbnailResult[] = [];
  queue.request("library", "1", 1, (value) => first.push(value));
  queue.request("library", "1", 1, (value) => duplicate.push(value));
  queue.request("library", "2", 2, (value) => second.push(value));
  assert.deepEqual(started, [1]);
  jobs[0].resolve(thumbnail(1));
  await flush();
  assert.deepEqual(started, [1, 2]);
  assert.deepEqual(first, [thumbnail(1)]);
  assert.deepEqual(duplicate, first);
  jobs[1].resolve(thumbnail(2));
  await flush();
  assert.equal(peak, 1);
  assert.deepEqual(second, [thumbnail(2)]);
  queue.dispose();
});

test("cancelled queued jobs are skipped and active cancellation frees its late URL", async () => {
  const job = deferred();
  const started: number[] = [];
  const revoked: string[] = [];
  let signal: AbortSignal | null = null;
  const queue = createModelThumbnailQueue(async (id, abort) => {
    started.push(id);
    signal = abort;
    return job.promise;
  }, (url) => revoked.push(url), () => {});
  const observed: ModelThumbnailResult[] = [];
  const cancelActive = queue.request("library", "1", 1, (value) => observed.push(value));
  const cancelQueued = queue.request("library", "2", 2, (value) => observed.push(value));
  cancelQueued();
  cancelActive();
  assert.equal((signal as AbortSignal | null)?.aborted, true);
  job.resolve(thumbnail(1));
  await flush();
  assert.deepEqual(started, [1]);
  assert.deepEqual(observed, []);
  assert.deepEqual(revoked, ["blob:1"]);
  queue.dispose();
});

test("library switch suppresses stale results without overlapping the new job", async () => {
  const old = deferred();
  const started: number[] = [];
  const observed: ModelThumbnailResult[] = [];
  const revoked: string[] = [];
  const queue = createModelThumbnailQueue(async (id) => {
    started.push(id);
    return id === 1 ? old.promise : thumbnail(id);
  }, (url) => revoked.push(url), () => {});
  queue.request("old-library", "same-id", 1, (value) => observed.push(value));
  queue.request("new-library", "same-id", 2, (value) => observed.push(value));
  assert.deepEqual(started, [1]);
  old.resolve(thumbnail(1));
  await flush();
  assert.deepEqual(started, [1, 2]);
  assert.deepEqual(observed, [thumbnail(2)]);
  assert.deepEqual(revoked, ["blob:1"]);
  queue.reset("other-library");
  assert.deepEqual(revoked, ["blob:1", "blob:2"]);
  queue.dispose();
});

test("one failed model cannot stop following thumbnails and failure is cached", async () => {
  const started: number[] = [];
  const observed: ModelThumbnailResult[] = [];
  const queue = createModelThumbnailQueue(async (id) => {
    started.push(id);
    if (id === 1) throw new Error("Missing model buffer");
    return thumbnail(id);
  }, () => {}, () => {});
  queue.request("library", "1", 1, (value) => observed.push(value));
  queue.request("library", "2", 2, (value) => observed.push(value));
  await flush();
  queue.request("library", "1", 1, (value) => observed.push(value));
  assert.deepEqual(started, [1, 2]);
  assert.deepEqual(observed, [
    { url: null, warning: null, error: "Missing model buffer" }, thumbnail(2),
    { url: null, warning: null, error: "Missing model buffer" },
  ]);
  queue.dispose();
});

test("bounded cache preserves subscribed images, evicts unused URLs and can restart after disposal", async () => {
  const revoked: string[] = [];
  let disposed = 0;
  const queue = createModelThumbnailQueue(async (id) => thumbnail(id), (url) => revoked.push(url), () => { disposed++; }, 2);
  const cancelFirst = queue.request("library", "1", 1, () => {});
  await flush();
  const cancelSecond = queue.request("library", "2", 2, () => {});
  await flush();
  cancelSecond();
  queue.request("library", "3", 3, () => {});
  await flush();
  assert.deepEqual(revoked, ["blob:2"]);
  cancelFirst();
  queue.dispose();
  assert.deepEqual(revoked, ["blob:2", "blob:1", "blob:3"]);
  assert.equal(disposed, 1);
  const observed: ModelThumbnailResult[] = [];
  queue.request("library", "4", 4, (value) => observed.push(value));
  await flush();
  assert.deepEqual(observed, [thumbnail(4)]);
  queue.dispose();
});

test("a rescan's preview generation renders the same file key again", async () => {
  const rendered: number[] = [];
  const revoked: string[] = [];
  const queue = createModelThumbnailQueue(async (id) => { rendered.push(id); return thumbnail(rendered.length); }, (url) => revoked.push(url), () => {});
  const results: (string | null)[] = [];
  const key = "7:Fx/Hit.glb:3:true";
  queue.request(previewScope("/lib", 0), key, 7, (value) => results.push(value.url));
  await flush();
  queue.request(previewScope("/lib", 0), key, 7, (value) => results.push(value.url));
  await flush();
  assert.deepEqual(rendered, [7], "cached within one generation");
  queue.request(previewScope("/lib", 1), key, 7, (value) => results.push(value.url));
  await flush();
  assert.deepEqual(rendered, [7, 7]);
  assert.deepEqual(results, ["blob:1", "blob:1", "blob:2"]);
  assert.deepEqual(revoked, ["blob:1"], "the stale thumbnail is released");
  assert.notEqual(previewScope("/lib", 0), previewScope("/lib", 1));
  queue.dispose();
});
