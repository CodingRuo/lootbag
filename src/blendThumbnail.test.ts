import assert from "node:assert/strict";
import test from "node:test";
import { readBlendThumbnail } from "./blendThumbnail.ts";

// Literal format1 little-endian64 blocks; TEST RGBA rows are stored bottom-up.
function block(code: string, payload: Uint8Array) {
  const bytes = new Uint8Array(32 + payload.length);
  bytes.set(Buffer.from(code));
  new DataView(bytes.buffer).setBigInt64(16, BigInt(payload.length), true);
  bytes.set(payload, 32);
  return bytes;
}
function fixture(payload = Uint8Array.from([2,0,0,0,2,0,0,0, 0,0,255,255, 0,255,255,0, 255,0,0,255, 255,0,255,128])) {
  return Uint8Array.from([...Buffer.from("BLENDER17-01v0502"), ...block("REND", Uint8Array.from([1,2,3,4])), ...block("TEST", payload)]);
}

test("a qualified saved Blender thumbnail presents top-left RGBA without editing original bytes", () => {
  const bytes = fixture(), original = bytes.slice();
  const result = readBlendThumbnail(bytes.buffer);
  assert.equal(result.width, 2); assert.equal(result.height, 2);
  assert.deepEqual([...new Uint8Array(result.buffer)], [255,0,0,255, 255,0,255,128, 0,0,255,255, 0,255,255,0]);
  assert.deepEqual(bytes, original);
});

test("compressed, legacy, other endian and version headers remain explicitly unsupported", () => {
  for (const bytes of [Uint8Array.from([31,139,8]), Uint8Array.from([40,181,47,253])]) assert.throws(() => readBlendThumbnail(bytes.buffer), /komprimiert/i);
  for (const header of ["BLENDER-v300", "BLENDER17-01V0502", "BLENDER17-01v0503", "BLENDER17-00v0502"]) {
    const bytes = fixture(); bytes.set(Buffer.from(header));
    assert.throws(() => readBlendThumbnail(bytes.buffer), /unterstützt/);
  }
  assert.throws(() => readBlendThumbnail(Uint8Array.from([66,76]).buffer), /unvollständig/);
});

test("malformed signed block and TEST bounds fail before pixel allocation", () => {
  for (const length of [-1n, 0x7fffffffffffffffn, 7n, 100n]) {
    const bytes = fixture(); new DataView(bytes.buffer).setBigInt64(17 + 36 + 16, length, true);
    assert.throws(() => readBlendThumbnail(bytes.buffer), /Block|unvollständig/);
  }
  const bytes = fixture(); new DataView(bytes.buffer).setBigInt64(17 + 24, -1n, true);
  assert.throws(() => readBlendThumbnail(bytes.buffer), /Block/);
  for (const width of [0, -1, 2049, 2147483647]) {
    const bytes = fixture(); new DataView(bytes.buffer).setInt32(17 + 36 + 32, width, true);
    assert.throws(() => readBlendThumbnail(bytes.buffer), /Bildgröße/);
  }
  assert.throws(() => readBlendThumbnail(fixture().subarray(0, -1).slice().buffer), /unvollständig/);
});

test("only bounded leading REND/TEST blocks are scanned, never scene structures", () => {
  const noPreview = Uint8Array.from([...Buffer.from("BLENDER17-01v0502"), ...block("GLOB", fixture())]);
  assert.throws(() => readBlendThumbnail(noPreview.buffer), /Kein gespeichertes/);
  const many = Uint8Array.from([...Buffer.from("BLENDER17-01v0502"), ...new Uint8Array(256 * 32)]);
  for (let cursor = 17; cursor < many.length; cursor += 32) many.set(Buffer.from("REND"), cursor);
  const appended = Uint8Array.from([...many, ...block("TEST", Uint8Array.from([1,0,0,0,1,0,0,0,255,0,0,255]))]);
  assert.throws(() => readBlendThumbnail(appended.buffer), /Blocklimit/);
  const leading = Uint8Array.from([...Buffer.from("BLENDER17-01v0502"), ...block("REND", new Uint8Array(1024 * 1024)), ...block("TEST", Uint8Array.from([1,0,0,0,1,0,0,0,255,0,0,255]))]);
  assert.throws(() => readBlendThumbnail(leading.buffer), /Suchbereich/);
});
