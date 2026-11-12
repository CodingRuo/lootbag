import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { LoadingManager } from "three";
import { ImageTexturePreviewLoader } from "./TgaPreviewLoader.ts";
import { decodeTga } from "./tgaDecode.ts";

const red = Uint8Array.from([0,0,2,0,0,0,0,0,0,0,0,0,1,0,1,0,32,40,0,0,255,255]);
const transport = (bytes: Uint8Array) => `data:image/x-tga;base64,${Buffer.from(bytes).toString("base64")}#model-preview=owned`;

function workerBoundary(t: TestContext) {
  const original = Object.getOwnPropertyDescriptor(globalThis, "Worker");
  class WorkerBoundary {
    onmessage: ((event: { data: unknown }) => void) | null = null;
    stopped = false;
    postMessage(buffer: ArrayBuffer) {
      queueMicrotask(() => {
        if (this.stopped) return;
        try { const decoded = decodeTga(buffer); this.onmessage?.({ data: { width: decoded.width, height: decoded.height, buffer: decoded.data.buffer } }); }
        catch (reason) { this.onmessage?.({ data: { error: (reason as Error).message } }); }
      });
    }
    terminate() { this.stopped = true; }
  }
  Object.defineProperty(globalThis, "Worker", { configurable: true, value: WorkerBoundary });
  t.after(() => { if (original) Object.defineProperty(globalThis, "Worker", original); else Reflect.deleteProperty(globalThis, "Worker"); });
}

test("TGA handler captures FBX's temporary path and resolves exactly once before asynchronous pixels", async (t) => {
  workerBoundary(t);
  const events: string[] = [], resolved: string[] = [];
  const manager = new LoadingManager();
  const complete = new Promise<void>((resolve) => { manager.onLoad = () => { events.push("complete"); resolve(); }; });
  manager.onStart = () => events.push("start");
  manager.setURLModifier((url) => { resolved.push(url); return transport(red); });
  const loader = new ImageTexturePreviewLoader(manager, new AbortController().signal, "TGA");
  loader.setPath("https://model.invalid/owned/Textures/");
  const texture = loader.load("Checker.TGA", () => events.push("pixels"));
  loader.setPath("");
  assert.equal(texture.image.data, null);
  await complete;
  assert.deepEqual(resolved, ["https://model.invalid/owned/Textures/Checker.TGA"]);
  assert.deepEqual(events, ["start", "pixels", "complete"]);
  assert.deepEqual([...texture.image.data!], [255,0,0,255]);
  assert.equal(texture.flipY, true);
  assert.equal(texture.generateMipmaps, true);
  texture.dispose();
});

test("a bad TGA and a rejected package URL each balance manager errors and completion", async (t) => {
  workerBoundary(t);
  for (const rejectUrl of [false, true]) {
    let errors = 0, callbacks = 0;
    const manager = new LoadingManager();
    const complete = new Promise<void>((resolve) => { manager.onLoad = resolve; });
    manager.onError = () => errors++;
    manager.setURLModifier(() => { if (rejectUrl) throw new Error("Outside package"); return transport(red.subarray(0, 18)); });
    const texture = new ImageTexturePreviewLoader(manager, new AbortController().signal, "TGA").load("Checker.tga", () => assert.fail("Corrupt texture must not load"), undefined, () => callbacks++);
    await complete;
    assert.equal(errors, 1); assert.equal(callbacks, 1);
    assert.equal(texture.image.data, null);
  }
});

test("source and manager cancellation settle loader completion without publishing texture pixels", async (t) => {
  workerBoundary(t);
  for (const cancelSource of [false, true]) {
    const manager = new LoadingManager(), controller = new AbortController();
    let errors = 0;
    const complete = new Promise<void>((resolve) => { manager.onLoad = resolve; });
    manager.onError = () => errors++;
    manager.setURLModifier(() => transport(red));
    const texture = new ImageTexturePreviewLoader(manager, controller.signal, "TGA").load(".tga", () => assert.fail("Cancelled texture must not load"));
    if (cancelSource) controller.abort(); else manager.abort();
    await complete;
    assert.equal(errors, 1); assert.equal(texture.image.data, null);
  }
});
