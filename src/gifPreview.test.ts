import assert from "node:assert/strict";
import test, { type TestContext } from "node:test";
import { decodeGifUrl, imagePreviewUrl, loadGifAnimation } from "./imagePreviewUrl.ts";
import { loadModel } from "./loadModel.ts";
import { DataTexture, Mesh, MeshStandardMaterial, LoadingManager } from "three";
import { ImageTexturePreviewLoader } from "./TgaPreviewLoader.ts";

const gif = Uint8Array.from(Buffer.from("R0lGODlhAQABAIAAAP8AAAAAACH5BAAAAAAALAAAAAABAAEAAAICRAEAOw==", "base64"));
const transport = (bytes = gif) => `data:image/gif;base64,${Buffer.from(bytes).toString("base64")}#snapshot=owned`;

function nativeBoundary(t: TestContext, stallFirst = false, afterLoad?: () => void) {
  const descriptors = ["Image", "document"].map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  t.after(() => { for (const [name, descriptor] of descriptors) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
  const images: ImageBoundary[] = [];
  let started!: () => void;
  const firstLoad = new Promise<void>((resolve) => { started = resolve; });
  class ImageBoundary {
    onload: (() => void) | null = null;
    onerror: (() => void) | null = null;
    naturalWidth = 1; naturalHeight = 1; value = "";
    constructor() { images.push(this); }
    set src(value: string) { this.value = value; if (value) { started(); if (!stallFirst || images.length > 1) queueMicrotask(() => { this.onload?.(); afterLoad?.(); }); } }
    get src() { return this.value; }
  }
  Object.defineProperty(globalThis, "Image", { configurable: true, value: ImageBoundary });
  Object.defineProperty(globalThis, "document", { configurable: true, value: { createElement: () => ({
    width: 0, height: 0,
    getContext: () => ({ drawImage: () => {}, getImageData: () => ({ data: Uint8ClampedArray.from([255,0,0,255]) }) }),
    toBlob: (accept: (blob: Blob) => void) => accept(new Blob(["static png"], { type: "image/png" })),
  }) } });
  return { images, firstLoad };
}

test("validated GIFs expose native animation and a separate first-composite proxy without changing original bytes", async (t) => {
  const native = nativeBoundary(t);
  const animation = await loadGifAnimation(transport(), new AbortController().signal);
  assert.equal(animation.frames, 1);
  assert.deepEqual(new Uint8Array(await (await fetch(animation.image.src)).arrayBuffer()), gif);
  animation.release(); assert.equal(native.images[0].src, "");
  const pixels = await decodeGifUrl(transport(), new AbortController().signal);
  assert.deepEqual([...new Uint8Array(pixels.buffer)], [255,0,0,255]);
  const proxy = await imagePreviewUrl(transport(), "Checker.GIF", new AbortController().signal, 512);
  assert.equal((await fetch(proxy.url)).headers.get("content-type"), "image/png");
  proxy.release();
  await assert.rejects(fetch(proxy.url));
});

test("an aborted native GIF load clears its blob and releases the serial owner for valid input", async (t) => {
  const native = nativeBoundary(t, true);
  const controller = new AbortController();
  const pending = loadGifAnimation(transport(), controller.signal);
  const rejected = assert.rejects(pending, { name: "AbortError" });
  await native.firstLoad;
  const abandonedUrl = native.images[0].src;
  const next = loadGifAnimation(transport(), new AbortController().signal);
  controller.abort(); await rejected;
  assert.equal(native.images[0].src, "");
  await assert.rejects(fetch(abandonedUrl));
  (await next).release();
});

test("source cancellation during native handoff or after delivery never retains animation sources", async (t) => {
  for (const depth of [0, 1, 2, 3, 4]) await t.test(`handoff ${depth}`, async (child) => {
    const controller = new AbortController();
    let stop = () => controller.abort();
    for (let index = 0; index < depth; index++) { const previous = stop; stop = () => queueMicrotask(previous); }
    const native = nativeBoundary(child, false, stop);
    const urls: string[] = [];
    const create = URL.createObjectURL;
    child.mock.method(URL, "createObjectURL", (blob: Blob) => { const url = create(blob); urls.push(url); return url; });
    try { (await loadGifAnimation(transport(), controller.signal)).release(); }
    catch (reason) { assert.equal((reason as Error).name, "AbortError"); }
    await new Promise<void>((resolve) => queueMicrotask(resolve));
    assert.ok(native.images.every((image) => image.src === ""));
    for (const url of urls) await assert.rejects(fetch(url));
    const valid = await loadGifAnimation(transport(), new AbortController().signal);
    valid.release();
  });
});

test("broken containers and oversized encoded streams fail before the native image decoder", async (t) => {
  const native = nativeBoundary(t);
  await assert.rejects(loadGifAnimation(transport(gif.subarray(0, -1)), new AbortController().signal), /unvollständig/);
  assert.equal(native.images.length, 0);
  let cancelled = false;
  t.mock.method(globalThis, "fetch", async () => new Response(new ReadableStream({ start(controller) { controller.enqueue({ byteLength: 33554433 }); }, cancel() { cancelled = true; } }), { headers: { "content-length": "1" } }));
  await assert.rejects(loadGifAnimation("asset://localhost/large.gif", new AbortController().signal), /32 MiB/);
  assert.equal(cancelled, true); assert.equal(native.images.length, 0);
});

test("GIF texture handler preserves path, snapshot isolation and manager settlement", async (t) => {
  nativeBoundary(t);
  const manager = new LoadingManager(), controller = new AbortController();
  const resolved: string[] = [];
  let errors = 0;
  manager.onError = () => errors++;
  manager.setURLModifier((url) => { resolved.push(url); return transport(); });
  const complete = new Promise<void>((resolve) => { manager.onLoad = resolve; });
  const loader = new ImageTexturePreviewLoader(manager, controller.signal, "GIF");
  loader.setPath("https://model.invalid/owned/Textures/");
  const texture = loader.load("Checker.GIF"); loader.setPath("");
  await complete;
  assert.deepEqual(resolved, ["https://model.invalid/owned/Textures/Checker.GIF"]);
  assert.equal(errors, 0);
  assert.deepEqual([...texture.image.data!], [255,0,0,255]);
  texture.dispose();
});

test("a complete GIF-textured OBJ stays eligible for original bundle export", async (t) => {
  nativeBoundary(t);
  const descriptors = ["window", "ProgressEvent"].map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  t.after(() => { for (const [name, descriptor] of descriptors) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
  const originals: Record<string, string> = {
    "Model.obj": "mtllib Model.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Owned\nf 1 2 3\n",
    "Model.mtl": "newmtl Owned\nmap_Kd Textures/Checker.gif\n",
  };
  Object.defineProperty(globalThis, "ProgressEvent", { configurable: true, value: class { constructor(_name: string, _options: unknown) {} } });
  Object.defineProperty(globalThis, "window", { configurable: true, value: { __TAURI_INTERNALS__: { convertFileSrc: (path: string) => path.endsWith(".gif") ? transport() : `data:text/plain;base64,${Buffer.from(originals[path]).toString("base64")}` } } });
  const snapshot = { snapshotId: "owned-gif", fileId: 1, format: "OBJ", entryPath: "Model.obj", declaredPaths: ["Model.mtl", "Textures/Checker.gif"], errors: [], resources: ["Model.obj", "Model.mtl", "Textures/Checker.gif"].map((path) => ({ relativePath: path, path, sizeBytes: 1 })) };
  const result = await loadModel(snapshot, new AbortController().signal);
  assert.deepEqual(result.warnings, []); assert.deepEqual(result.replacements, []);
  assert.equal(result.staticGifTextures, true);
  assert.deepEqual(result.resourcePaths.sort(), ["Model.mtl", "Model.obj", "Textures/Checker.gif"]);
  const mesh = result.model.children[0] as Mesh;
  const texture = (mesh.material as MeshStandardMaterial).map as DataTexture;
  assert.deepEqual([...texture.image.data!], [255,0,0,255]);
  result.dispose();
});

test("explicit glTF GIF data and bufferView images use the guarded first-composite route", async (t) => {
  const native = nativeBoundary(t);
  const descriptors = ["window", "ProgressEvent", "self"].map((name) => [name, Object.getOwnPropertyDescriptor(globalThis, name)] as const);
  t.after(() => { for (const [name, descriptor] of descriptors) { if (descriptor) Object.defineProperty(globalThis, name, descriptor); else Reflect.deleteProperty(globalThis, name); } });
  Object.defineProperty(globalThis, "ProgressEvent", { configurable: true, value: class { constructor(_name: string, _options: unknown) {} } });
  Object.defineProperty(globalThis, "self", { configurable: true, value: globalThis });
  const positions = Buffer.alloc(36);
  [0,0,0, 1,0,0, 0,1,0].forEach((value, index) => positions.writeFloatLE(value, index * 4));
  for (const mime of [null, "image/gif", "image/GIF;version=89a"]) {
    const document = {
      asset: { version: "2.0" }, scene: 0, scenes: [{ nodes: [0] }], nodes: [{ mesh: 0 }],
      buffers: [{ byteLength: 36, uri: `data:application/octet-stream;base64,${positions.toString("base64")}` }, { byteLength: gif.length, uri: `data:application/octet-stream;base64,${Buffer.from(gif).toString("base64")}` }],
      bufferViews: [{ buffer: 0, byteLength: 36 }, { buffer: 1, byteLength: gif.length }],
      accessors: [{ bufferView: 0, componentType: 5126, count: 3, type: "VEC3", min: [0,0,0], max: [1,1,0] }],
      images: [mime ? { bufferView: 1, mimeType: mime } : { uri: transport() }],
      textures: [{ source: 0 }], materials: [{ pbrMetallicRoughness: { baseColorTexture: { index: 0 } } }],
      meshes: [{ primitives: [{ attributes: { POSITION: 0 }, material: 0 }] }],
    };
    Object.defineProperty(globalThis, "window", { configurable: true, value: { __TAURI_INTERNALS__: { convertFileSrc: () => `data:model/gltf+json;base64,${Buffer.from(JSON.stringify(document)).toString("base64")}` } } });
    const model = await loadModel({ snapshotId: `gif-gltf-${mime}`, fileId: 1, format: "GLTF", entryPath: "Model.gltf", declaredPaths: [], errors: [], resources: [{ relativePath: "Model.gltf", path: "Model.gltf", sizeBytes: 1 }] }, new AbortController().signal);
    assert.deepEqual(model.warnings, []);
    assert.equal(model.staticGifTextures, true);
    assert.deepEqual(model.resourcePaths, ["Model.gltf"]);
    const textures: DataTexture[] = [];
    model.model.traverse((object) => { if (object instanceof Mesh) textures.push((object.material as MeshStandardMaterial).map as DataTexture); });
    assert.ok(textures[0] instanceof DataTexture);
    assert.deepEqual([...textures[0].image.data!], [255,0,0,255]);
    model.dispose();
    if (mime) {
      // Tiny actual bytes with a large declared view prove rejection before native GIF decoding/copying.
      document.bufferViews[1].byteLength = 33_554_433;
      document.buffers[1].byteLength = 33_554_433;
      const decodedBefore = native.images.length;
      await assert.rejects(loadModel({ snapshotId: `oversize-gif-${mime}`, fileId: 1, format: "GLTF", entryPath: "Model.gltf", declaredPaths: [], errors: [], resources: [{ relativePath: "Model.gltf", path: "Model.gltf", sizeBytes: 1 }] }, new AbortController().signal), /32 MiB/);
      assert.equal(native.images.length, decodedBefore);
    }
  }
});

test("native GIF timeout clears source ownership and allows the following image to decode", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const native = nativeBoundary(t, true);
  const pending = loadGifAnimation(transport(), new AbortController().signal);
  const failure = assert.rejects(pending, /Zeitlimit/);
  await native.firstLoad;
  const url = native.images[0].src;
  t.mock.timers.tick(15_000);
  await failure;
  assert.equal(native.images[0].src, "");
  await assert.rejects(fetch(url));
  (await loadGifAnimation(transport(), new AbortController().signal)).release();
});

test("native GIF decode errors recover without publishing partial model texture pixels", async (t) => {
  const native = nativeBoundary(t, true);
  const manager = new LoadingManager();
  manager.setURLModifier(() => transport());
  let errors = 0;
  manager.onError = () => errors++;
  const complete = new Promise<void>((resolve) => { manager.onLoad = resolve; });
  const texture = new ImageTexturePreviewLoader(manager, new AbortController().signal, "GIF").load("Owned.gif");
  await native.firstLoad;
  native.images[0].onerror?.();
  await complete;
  assert.equal(errors, 1); assert.equal(texture.image.data, null); assert.equal(native.images[0].src, "");
  const next = await decodeGifUrl(transport(), new AbortController().signal);
  assert.deepEqual([...new Uint8Array(next.buffer)], [255,0,0,255]);
});
