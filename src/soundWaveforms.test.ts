import assert from "node:assert/strict";
import test from "node:test";
import { createSoundWaveformQueue, waveformUrl } from "./soundWaveforms.ts";
import type { ModelThumbnailResult } from "./modelThumbnails.ts";

test("bars become one rect each, silence keeps a visible line", () => {
  const svg = decodeURIComponent(waveformUrl([0, 255]).replace("data:image/svg+xml,", ""));
  assert.equal(svg.match(/<rect /g)?.length, 2);
  assert.match(svg, /height="2"/);
  assert.match(svg, /height="90"/);
});

test("a library switch drops queued waveform requests and failures stay visible", async () => {
  const started: number[] = [];
  let finish!: (bars: number[]) => void;
  const queue = createSoundWaveformQueue((fileId) => {
    started.push(fileId);
    return fileId === 3 ? Promise.reject("Audioformat nicht lesbar") : new Promise((resolve) => { finish = resolve; });
  });
  const results: ModelThumbnailResult[] = [];
  queue.request("first", "1", 1, (result) => results.push(result));
  queue.request("first", "2", 2, (result) => results.push(result));
  queue.request("second", "3", 3, (result) => results.push(result));
  finish([1]);
  await new Promise((resolve) => setImmediate(resolve));
  assert.deepEqual(started, [1, 3]);
  assert.deepEqual(results, [{ url: null, warning: null, error: "Audioformat nicht lesbar" }]);
  queue.dispose();
});
