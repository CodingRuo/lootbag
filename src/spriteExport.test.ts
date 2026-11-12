import assert from "node:assert/strict";
import test from "node:test";
import { spriteExportPlan, encodeSpriteExport, deliverSpriteExport, packedSpriteExportPlan } from "./spriteExport.ts";

const layout = { columns: 3, rows: 2, left: 2, top: 3, right: 3, bottom: 2, gapX: 1, gapY: 2 };
test("sheet delivery uses literal source bounds and a tight atlas without gutters", () => {
  const plan = spriteExportPlan(25, 17, layout, 10, "atlas");
  assert.deepEqual(plan.frames.map(({ source, output }) => [source, output]), [
    [{ x: 2, y: 3, width: 6, height: 5 }, { x: 0, y: 0, width: 6, height: 5 }],
    [{ x: 9, y: 3, width: 6, height: 5 }, { x: 6, y: 0, width: 6, height: 5 }],
    [{ x: 16, y: 3, width: 6, height: 5 }, { x: 12, y: 0, width: 6, height: 5 }],
    [{ x: 2, y: 10, width: 6, height: 5 }, { x: 0, y: 5, width: 6, height: 5 }],
    [{ x: 9, y: 10, width: 6, height: 5 }, { x: 6, y: 5, width: 6, height: 5 }],
    [{ x: 16, y: 10, width: 6, height: 5 }, { x: 12, y: 5, width: 6, height: 5 }],
  ]);
  assert.deepEqual(plan.outputs, [{ filename: "atlas.png", width: 18, height: 10 }]);
  assert.equal(plan.fps, 10);
});

test("directory cancellation or source disposal cannot reach the native publisher", async () => {
  const controller = new AbortController(); let calls = 0;
  const plan = spriteExportPlan(25, 17, layout, 10, "atlas");
  const image = {} as HTMLImageElement;
  const source = { snapshotId: "owned", fileId: 9, filename: "sheet.png" };
  assert.equal(await deliverSpriteExport(image, plan, source, controller.signal, async () => null, async () => { calls++; return { status: "exported", path: "/project/result" }; }), null);
  await assert.rejects(deliverSpriteExport(image, plan, source, controller.signal, async () => { controller.abort(); return "/project"; }, async () => { calls++; return { status: "exported" }; }), /abort/i);
  assert.equal(calls, 0);
});

test("export caps reject consuming, fractional and oversized grids without limiting valid preview grids", () => {
  for (const bad of [{ ...layout, left: 25 }, { ...layout, gapX: 0.5 }, { ...layout, right: 2 }, { ...layout, rows: 0 }, { ...layout, columns: Number.MAX_SAFE_INTEGER }]) assert.throws(() => spriteExportPlan(25, 17, bad, 10, "frames"));
  for (const fps of [0, 61, 1.5, NaN, Infinity]) assert.throws(() => spriteExportPlan(25, 17, layout, fps, "atlas"));
  const zero = { columns: 1, rows: 1, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 };
  assert.throws(() => spriteExportPlan(257, 1, { ...zero, columns: 257 }, 8, "frames"), /256/);
  assert.throws(() => spriteExportPlan(8193, 1, zero, 8, "frames"), /8192/);
  assert.throws(() => spriteExportPlan(4096, 4096, { ...zero, columns: 2, rows: 2 }, 8, "atlas"), /MiPixel/);
  assert.deepEqual(spriteExportPlan(25, 17, layout, 10, "frames").frames.map((f) => f.filename), ["frame-0001.png", "frame-0002.png", "frame-0003.png", "frame-0004.png", "frame-0005.png", "frame-0006.png"]);
});

test("PNG encoder failures, byte caps and disposal after encode release the canvas and permit recovery", async () => {
  const original = globalThis.document;
  let callback: ((blob: Blob | null) => void) | null = null;
  let encoding: "null" | "large" | "pending" | "valid" = "null";
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: () => {} }), toBlob: (cb: (blob: Blob | null) => void) => {
    if (encoding === "pending") callback = cb;
    else if (encoding === "null") cb(null);
    else if (encoding === "large") cb({ size: 32 * 1024 * 1024 + 1, arrayBuffer: () => { throw new Error("must not allocate"); } } as unknown as Blob);
    else cb(new Blob([new Uint8Array([1, 2, 3])]));
  } };
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  const plan = spriteExportPlan(25, 17, layout, 10, "atlas");
  const source = { snapshotId: "owned", fileId: 9, filename: "../sheet.png" };
  let published = 0;
  const publish = async () => { published++; return { status: "exported", path: "/project/result" }; };
  try {
    await assert.rejects(deliverSpriteExport({} as HTMLImageElement, plan, source, new AbortController().signal, async () => "/project", publish), /PNG/);
    encoding = "large";
    await assert.rejects(deliverSpriteExport({} as HTMLImageElement, plan, source, new AbortController().signal, async () => "/project", publish), /32 MiB/);
    encoding = "pending"; const cancelled = new AbortController();
    const pending = deliverSpriteExport({} as HTMLImageElement, plan, source, cancelled.signal, async () => "/project", publish);
    await Promise.resolve(); cancelled.abort(); await assert.rejects(pending, /abort/i);
    assert.equal(canvas.width, 1); assert.equal(canvas.height, 1); assert.equal(published, 0);
    (callback as unknown as (blob: Blob) => void)(new Blob([new Uint8Array([4])]));
    await Promise.resolve(); assert.equal(published, 0);
    encoding = "valid";
    assert.equal(await deliverSpriteExport({} as HTMLImageElement, plan, source, new AbortController().signal, async () => "/project", publish), "/project/result");
    assert.equal(published, 1);
  } finally { globalThis.document = original; }
});

test("disposal after byte conversion prevents publication and long Unicode names remain valid bounded UTF8", async () => {
  const original = globalThis.document; const controller = new AbortController(); let abortOnRead = true; let published = 0;
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: () => {} }), toBlob: (cb: (blob: Blob) => void) => cb({ size: 3, arrayBuffer: async () => { if (abortOnRead) controller.abort(); return new Uint8Array([1, 2, 3]).buffer; } } as Blob) };
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  try {
    const plan = spriteExportPlan(25, 17, layout, 10, "atlas");
    const source = { snapshotId: "owned", fileId: 9, filename: "🌊".repeat(120) + ".png" };
    const publish = async (payload: Uint8Array) => {
      published++;
      const length = new DataView(payload.buffer).getUint32(0, true);
      const header = JSON.parse(new TextDecoder().decode(payload.subarray(4, 4 + length)));
      assert.equal(header.destination, `/project/${"🌊".repeat(30)}-atlas`);
      assert.equal(new TextEncoder().encode(header.destination.split("/").pop()).length, 126);
      return { status: "exported", path: header.destination };
    };
    await assert.rejects(deliverSpriteExport({} as HTMLImageElement, plan, source, controller.signal, async () => "/project", publish), /abort/i);
    assert.equal(published, 0); assert.equal(canvas.width, 1);
    abortOnRead = false;
    await deliverSpriteExport({} as HTMLImageElement, plan, source, new AbortController().signal, async () => "/project", publish);
    assert.equal(published, 1);
  } finally { globalThis.document = original; }
});

test("frame delivery encodes natural-size crops, releases canvas, and passes only bounded binary outputs", async () => {
  const draws: unknown[][] = [];
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: (...args: unknown[]) => draws.push(args) }), toBlob: (callback: (blob: Blob) => void) => callback(new Blob([new Uint8Array([1, 2, 3])])) };
  const original = globalThis.document;
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  try {
    const plan = spriteExportPlan(25, 17, layout, 8, "frames");
    const image = {} as HTMLImageElement;
    const bytes = await encodeSpriteExport(image, plan, { snapshotId: "owned", fileId: 9, destination: "/project/sprites" }, new AbortController().signal);
    assert.deepEqual(draws.map((draw) => draw.slice(1)), [[2, 3, 6, 5, 0, 0, 6, 5], [9, 3, 6, 5, 0, 0, 6, 5], [16, 3, 6, 5, 0, 0, 6, 5], [2, 10, 6, 5, 0, 0, 6, 5], [9, 10, 6, 5, 0, 0, 6, 5], [16, 10, 6, 5, 0, 0, 6, 5]]);
    const jsonSize = new DataView(bytes.buffer).getUint32(0, true);
    const header = JSON.parse(new TextDecoder().decode(bytes.subarray(4, 4 + jsonSize)));
    assert.deepEqual(header.lengths, [3, 3, 3, 3, 3, 3]);
    assert.deepEqual(Array.from(bytes.subarray(4 + jsonSize)), Array(6).fill([1, 2, 3]).flat());
    assert.equal(canvas.width, 1); assert.equal(canvas.height, 1);
  } finally { globalThis.document = original; }
});


test("packed sheets retain unequal trim offsets, empty frames and original timeline order", async () => {
  const original = globalThis.document;
  let cell = 0;
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: (...args: unknown[]) => { cell = Number(args[1]) / 4; }, getImageData: () => {
    const data = new Uint8ClampedArray(64);
    if (cell === 0) for (const p of [5, 6, 7]) data[p * 4 + 3] = 128;
    if (cell === 1) for (const p of [0, 4]) data[p * 4 + 3] = 255;
    return { data };
  } }) };
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  try {
    const plan = await packedSpriteExportPlan({ naturalWidth: 12, naturalHeight: 4 } as HTMLImageElement,
      { columns: 3, rows: 1, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 }, 10, new AbortController().signal);
    assert.equal(plan.mode, "packed");
    assert.deepEqual(plan.frames.map((f) => [f.trim, f.empty]), [
      [{ x: 1, y: 1, width: 3, height: 1 }, false],
      [{ x: 0, y: 0, width: 1, height: 2 }, false],
      [{ x: 0, y: 0, width: 0, height: 0 }, true],
    ]);
    assert.deepEqual(plan.frames.map((f) => f.output), [
      { x: 4, y: 1, width: 3, height: 1 },
      { x: 1, y: 1, width: 1, height: 2 },
      { x: 9, y: 1, width: 1, height: 1 },
    ]);
    assert.deepEqual(plan.outputs, [{ filename: "atlas.png", width: 11, height: 4 }]);
    assert.equal(canvas.width, 1); assert.equal(canvas.height, 1);
  } finally { globalThis.document = original; }
});


test("packed PNG output retains empty timeline slots and includes trim coordinates with extruded borders", async () => {
  const original = globalThis.document;
  const draws: number[][] = [];
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: (...args: unknown[]) => draws.push(args.slice(1) as number[]) }), toBlob: (cb: (blob: Blob) => void) => cb(new Blob([new Uint8Array([1, 2, 3])])) };
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  try {
    const plan = { width: 8, height: 4, layout: { columns: 2, rows: 1, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 }, fps: 10, mode: "packed" as const,
      frames: [
        { source: { x: 0, y: 0, width: 4, height: 4 }, trim: { x: 1, y: 1, width: 2, height: 2 }, empty: false, output: { x: 1, y: 1, width: 2, height: 2 }, filename: "atlas.png" },
        { source: { x: 4, y: 0, width: 4, height: 4 }, trim: { x: 0, y: 0, width: 0, height: 0 }, empty: true, output: { x: 5, y: 1, width: 1, height: 1 }, filename: "atlas.png" },
      ], outputs: [{ filename: "atlas.png", width: 7, height: 4 }], packed: { width: 7, height: 4, frames: [[1, 1, 2, 2, 1, 1], [0, 0, 0, 0, 5, 1]] } };
    const payload = await encodeSpriteExport({} as HTMLImageElement, plan, { snapshotId: "owned", fileId: 9, destination: "/project/packed" }, new AbortController().signal);
    const size = new DataView(payload.buffer).getUint32(0, true);
    assert.deepEqual(JSON.parse(new TextDecoder().decode(payload.subarray(4, 4 + size))).packed, plan.packed);
    assert.deepEqual(draws, [
      [1, 1, 2, 2, 1, 1, 2, 2], [1, 1, 1, 2, 0, 1, 1, 2], [2, 1, 1, 2, 3, 1, 1, 2],
      [1, 1, 2, 1, 1, 0, 2, 1], [1, 2, 2, 1, 1, 3, 2, 1],
      [1, 1, 1, 1, 0, 0, 1, 1], [2, 1, 1, 1, 3, 0, 1, 1], [1, 2, 1, 1, 0, 3, 1, 1], [2, 2, 1, 1, 3, 3, 1, 1],
    ]);
    assert.equal(canvas.width, 1); assert.equal(canvas.height, 1);
  } finally { globalThis.document = original; }
});


test("stalled PNG encoding times out and releases the export for retry", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const original = globalThis.document;
  let stall = true;
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: () => {} }), toBlob: (cb: (blob: Blob) => void) => { if (!stall) cb(new Blob(["owned PNG"])); } };
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  try {
    const plan = spriteExportPlan(1, 1, { columns: 1, rows: 1, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 }, 10, "atlas");
    let failure: unknown;
    const pending = encodeSpriteExport({} as HTMLImageElement, plan, { snapshotId: "owned", fileId: 9, destination: "/project/atlas" }, new AbortController().signal).catch((e) => { failure = e; });
    t.mock.timers.tick(30000);
    for (let i = 0; i < 8; i++) await Promise.resolve();
    assert.match(String(failure), /30 Sekunden/);
    await pending; assert.equal(canvas.width, 1);
    stall = false;
    assert.ok((await encodeSpriteExport({} as HTMLImageElement, plan, { snapshotId: "owned", fileId: 9, destination: "/project/atlas" }, new AbortController().signal)).length);
  } finally { globalThis.document = original; }
});


test("packed pixel scanning releases an aborted source and oversized borders fail explicitly", async () => {
  const original = globalThis.document; const controller = new AbortController(); let cancel = true;
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: () => {}, getImageData: () => { if (cancel) controller.abort(); const data = new Uint8ClampedArray(canvas.width * canvas.height * 4); data.fill(255); return { data }; } }) };
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  const grid = { columns: 1, rows: 1, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 };
  try {
    await assert.rejects(packedSpriteExportPlan({ naturalWidth: 2, naturalHeight: 2 } as HTMLImageElement, grid, 10, controller.signal), /abort/i);
    assert.equal(canvas.width, 1); cancel = false;
    const recovered = await packedSpriteExportPlan({ naturalWidth: 2, naturalHeight: 2 } as HTMLImageElement, grid, 10, new AbortController().signal);
    assert.deepEqual(recovered.outputs, [{ filename: "atlas.png", width: 4, height: 4 }]);
    await assert.rejects(packedSpriteExportPlan({ naturalWidth: 8192, naturalHeight: 1 } as HTMLImageElement, grid, 10, new AbortController().signal), /8192/);
    assert.equal(canvas.width, 1);
  } finally { globalThis.document = original; }
});

test("a cancelled PNG byte conversion cannot retain or publish a stale packed export", async () => {
  const original = globalThis.document; const controller = new AbortController(); let started = () => {};
  const reading = new Promise<void>((resolve) => { started = resolve; });
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: () => {} }), toBlob: (cb: (blob: Blob) => void) => cb({ size: 3, arrayBuffer: () => { started(); return new Promise<ArrayBuffer>(() => {}); } } as Blob) };
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  try {
    const plan = spriteExportPlan(1, 1, { columns: 1, rows: 1, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 }, 10, "atlas");
    const pending = encodeSpriteExport({} as HTMLImageElement, plan, { snapshotId: "owned", fileId: 9, destination: "/project/atlas" }, controller.signal);
    const failure = assert.rejects(pending, /abort/i);
    await reading; controller.abort(); await failure; assert.equal(canvas.width, 1);
  } finally { globalThis.document = original; }
});


test("all 256 packed frames fit the bounded binary plan without losing timeline slots", async () => {
  const original = globalThis.document;
  const canvas = { width: 0, height: 0, getContext: () => ({ imageSmoothingEnabled: true, clearRect: () => {}, drawImage: () => {}, getImageData: () => ({ data: new Uint8ClampedArray(canvas.width * canvas.height * 4) }) }), toBlob: (cb: (blob: Blob) => void) => cb(new Blob(["PNG"])) };
  globalThis.document = { createElement: () => canvas } as unknown as Document;
  try {
    const plan = await packedSpriteExportPlan({ naturalWidth: 16, naturalHeight: 16 } as HTMLImageElement,
      { columns: 16, rows: 16, left: 0, top: 0, right: 0, bottom: 0, gapX: 0, gapY: 0 }, 60, new AbortController().signal);
    assert.equal(plan.frames.length, 256);
    assert.deepEqual(plan.outputs, [{ filename: "atlas.png", width: 48, height: 48 }]);
    const payload = await encodeSpriteExport({} as HTMLImageElement, plan, { snapshotId: "owned", fileId: 9, destination: "/project/packed" }, new AbortController().signal);
    const length = new DataView(payload.buffer).getUint32(0, true);
    assert.ok(length < 16 * 1024);
    assert.equal(JSON.parse(new TextDecoder().decode(payload.subarray(4, 4 + length))).packed.frames.length, 256);
    assert.equal(canvas.width, 1);
  } finally { globalThis.document = original; }
});
