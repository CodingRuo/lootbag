import assert from "node:assert/strict";
import test from "node:test";
import { loadFontFace, loadedFontCount, MAX_FONT_BYTES, MAX_LOADED_FONTS } from "./fontFace.ts";

function fakes(fail = false) {
  const added = new Set<unknown>();
  const fonts = { add: (face: unknown) => added.add(face), delete: (face: unknown) => added.delete(face) };
  const families: string[] = [];
  const create = (family: string) => { families.push(family); return { load: () => fail ? Promise.reject(new Error("bad")) : Promise.resolve() } as unknown as FontFace; };
  return { added, fonts, families, create };
}

test("fonts load under unique families and unload on release", async () => {
  const { added, fonts, families, create } = fakes();
  const a = await loadFontFace("asset://a.ttf", 10, fonts, create);
  const b = await loadFontFace("asset://a.ttf", 10, fonts, create);
  assert.notEqual(a.family, b.family);
  assert.deepEqual(families, [a.family, b.family]);
  assert.equal(added.size, 2);
  a.release(); a.release(); b.release();
  assert.equal(added.size, 0);
  assert.equal(loadedFontCount(), 0);
});

test("corrupt, oversized and excess fonts fail on their own", async () => {
  const broken = fakes(true);
  await assert.rejects(loadFontFace("asset://x.ttf", 10, broken.fonts, broken.create), /nicht lesbar/);
  assert.equal(loadedFontCount(), 0);
  const ok = fakes();
  await assert.rejects(loadFontFace("asset://x.ttf", MAX_FONT_BYTES + 1, ok.fonts, ok.create), /MiB/);
  const held = await Promise.all(Array.from({ length: MAX_LOADED_FONTS }, () => loadFontFace("asset://x.ttf", 10, ok.fonts, ok.create)));
  await assert.rejects(loadFontFace("asset://x.ttf", 10, ok.fonts, ok.create), /Zu viele/);
  held.forEach((font) => font.release());
  assert.equal(ok.added.size, 0);
});
