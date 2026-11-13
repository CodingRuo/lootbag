import assert from "node:assert/strict";
import test from "node:test";

(globalThis as any).window = { __TAURI_INTERNALS__: { convertFileSrc: (path: string) => `asset://localhost/${encodeURIComponent(path)}` } };
const { bumpPreviewGeneration, fileSrc } = await import("./fileSrc.ts");

test("a rescan changes every preview URL so the webview cannot serve cached bytes", () => {
  const before = fileSrc("/lib/a.png");
  assert.equal(before, "asset://localhost/%2Flib%2Fa.png");
  bumpPreviewGeneration();
  const after = fileSrc("/lib/a.png");
  assert.notEqual(after, before);
  assert.equal(new URL(after).pathname, new URL(before).pathname);
});
