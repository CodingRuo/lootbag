import assert from "node:assert/strict";
import test from "node:test";
import { decodeTga } from "./tgaDecode.ts";

function tga(type: number, depth: number, width: number, height: number, descriptor: number, pixels: number[]) {
  const header = [0, 0, type, 0, 0, 0, 0, 0, 0, 0, 0, 0, width & 255, width >> 8, height & 255, height >> 8, depth, descriptor];
  return Uint8Array.from([...header, ...pixels]).buffer;
}

test("TGA checker preserves independent color, orientation and alpha expectations", () => {
  const decoded = decodeTga(tga(2, 32, 2, 2, 8, [255,0,0,255, 255,255,255,0, 0,0,255,255, 0,255,0,128]));
  assert.equal(decoded.width, 2);
  assert.equal(decoded.height, 2);
  assert.deepEqual([...decoded.data], [255,0,0,255, 0,255,0,128, 0,0,255,255, 255,255,255,0]);
});

test("RGBX padding stays opaque rather than hiding a valid TGA image", () => {
  const decoded = decodeTga(tga(2, 32, 1, 1, 32, [0, 0, 255, 0]));
  assert.deepEqual([...decoded.data], [255, 0, 0, 255]);
});

test("a TGA footer cannot substitute for missing raster bytes", () => {
  const header = tga(2, 32, 1, 1, 40, []);
  const footer = new Uint8Array(26);
  footer.set(new TextEncoder().encode("TRUEVISION-XFILE.\0"), 8);
  const truncated = new Uint8Array(18 + footer.length);
  truncated.set(new Uint8Array(header)); truncated.set(footer, 18);
  assert.throws(() => decodeTga(truncated.buffer), /unvollständig/);
});

test("raw and RLE origin flags produce the same independent checker", () => {
  const expected = [255,0,0,255, 0,255,0,128, 0,0,255,255, 255,255,255,0];
  const topLeft = [0,0,255,255, 0,255,0,128, 255,0,0,255, 255,255,255,0];
  const bottomRight = [255,255,255,0, 255,0,0,255, 0,255,0,128, 0,0,255,255];
  const topRight = [0,255,0,128, 0,0,255,255, 255,255,255,0, 255,0,0,255];
  for (const [descriptor, pixels] of [[40, topLeft], [24, bottomRight], [56, topRight]] as const) {
    assert.deepEqual([...decodeTga(tga(10, 32, 2, 2, descriptor, [3, ...pixels])).data], expected);
  }
  assert.deepEqual([...decodeTga(tga(10, 24, 2, 1, 32, [129, 0, 0, 255])).data], [255,0,0,255, 255,0,0,255]);
});

test("supported grayscale and indexed images retain their literal pixels", () => {
  assert.deepEqual([...decodeTga(tga(3, 8, 2, 1, 32, [0, 127])).data], [0,0,0,255, 127,127,127,255]);
  assert.deepEqual([...decodeTga(tga(11, 16, 2, 1, 40, [1, 90, 64, 200, 255])).data], [90,90,90,64, 200,200,200,255]);
  for (const type of [1, 9]) {
    const indexed = new Uint8Array(tga(type, 8, 2, 1, 32, [0,0,255, 0,255,0, ...(type === 9 ? [1,0,1] : [0,1])]));
    indexed[1] = 1; indexed[5] = 2; indexed[7] = 24;
    assert.deepEqual([...decodeTga(indexed.buffer).data], [255,0,0,255, 0,255,0,255]);
  }
});

test("invalid, truncated and tiny declared oversize TGAs fail before decoder allocation", () => {
  const invalid: ArrayBuffer[] = [
    new ArrayBuffer(0), tga(2, 32, 8192, 8192, 40, []), tga(2, 32, 1, 1, 40, [0,0,255]),
    tga(10, 32, 1, 1, 40, [129,0,0,255,255]), tga(10, 32, 1, 1, 40, [128,0,0]),
    tga(10, 24, 2, 1, 32, [1,0,0,255]), tga(2, 16, 1, 1, 33, [0,0]),
    tga(2, 24, 1, 1, 96, [0,0,255]), tga(3, 16, 1, 1, 32, [0,0]),
  ];
  for (const input of invalid) assert.throws(() => decodeTga(input), /TGA-Vorschau/);
  const indexed = new Uint8Array(tga(1, 8, 1, 1, 32, [0,0,255,1]));
  indexed[1] = 1; indexed[5] = 1; indexed[7] = 24;
  assert.throws(() => decodeTga(indexed.buffer), /Farbindex/);
  indexed[3] = 1;
  assert.throws(() => decodeTga(indexed.buffer), /Farbpalette/);
  indexed[3] = 0; indexed[5] = 2;
  assert.throws(() => decodeTga(indexed.buffer), /unvollständig/);
  const identified = new Uint8Array(tga(2, 24, 1, 1, 32, [0,0,255]));
  identified[0] = 255;
  assert.throws(() => decodeTga(identified.buffer), /Bildkennung/);
});

test("zero-offset v2 footer is accepted but unsupported alpha extension semantics fail visibly", () => {
  const original = new Uint8Array(tga(2, 32, 1, 1, 40, [0,0,255,128]));
  const input = new Uint8Array(original.length + 26); input.set(original);
  input.set(new TextEncoder().encode("TRUEVISION-XFILE.\0"), input.length - 18);
  assert.deepEqual([...decodeTga(input.buffer).data], [255,0,0,128]);
  input[original.length] = 18;
  assert.throws(() => decodeTga(input.buffer), /Erweiterungsbereiche/);
});
