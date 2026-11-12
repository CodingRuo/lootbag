import assert from "node:assert/strict";
import test from "node:test";
import { preflightGif } from "./gifPreflight.ts";

// Literal GIF89a 1x1; pixels are decoded by the browser, never by this preflight.
const red = Uint8Array.from(Buffer.from("R0lGODlhAQABAIAAAP8AAAAAACH5BAEAAAAALAAAAAABAAEAAAICRAEAOw==", "base64"));

test("GIF preflight reports the complete logical canvas and frame budget before native decoding", () => {
  assert.deepEqual(preflightGif(red.buffer), { width: 1, height: 1, frames: 1, animated: false });
  const version87 = red.slice(); version87[4] = 55;
  assert.equal(preflightGif(version87.buffer).frames, 1);
});

test("malformed palettes, frame rectangles, controls and truncated chains never reach native decoding", () => {
  const broken = [red.subarray(0, 18), red.subarray(0, red.length - 1), Uint8Array.from([...red, 0])];
  for (const [offset, value] of [[0, 0], [6, 0], [28, 2], [35, 2], [38, 1], [25, 2], [22, 16], [26, 2]] as const) {
    const bytes = red.slice(); bytes[offset] = value; broken.push(bytes);
  }
  for (const bytes of broken) assert.throws(() => preflightGif(bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength)), /GIF-Vorschau/);
});

test("small frame rectangles cannot evade full logical canvas times frame-count limits", () => {
  const header = red.slice(0, 19); header[6] = header[8] = 0; header[7] = header[9] = 16;
  const frame = red.slice(27, -1);
  const bytes = Uint8Array.from([...header, ...frame, ...frame, ...frame, 59]);
  assert.throws(() => preflightGif(bytes.buffer), /Animationsbudget/);
  const tinyHeader = red.slice(0, 19);
  const tooMany = Uint8Array.from([...tinyHeader, ...Array.from({ length: 257 }, () => [...frame]).flat(), 59]);
  assert.throws(() => preflightGif(tooMany.buffer), /256/);
});
