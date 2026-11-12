import assert from "node:assert/strict";
import test from "node:test";
import { createCatalogAudition, formatDuration, stopAuditionOnPagePlay, repeatAudioLoop, resolveAudioLoop, seekAudio, validateAudioLoop } from "./audioAudition.ts";

test("a paused sound seeks safely and rejects invalid positions", () => {
  const player = { currentTime: 0, duration: 5, paused: true, ended: false };
  seekAudio(player, 2.5);
  assert.equal(player.currentTime, 2.5);
  seekAudio(player, 99);
  assert.equal(player.currentTime, 5);
  assert.throws(() => seekAudio(player, Number.NaN));
});

test("a selected sound loop repeats at its endpoint and preserves paused audition", () => {
  const player = { currentTime: 2, duration: 5, paused: false, ended: false };
  const loop = { start: 0.5, end: 2 };
  validateAudioLoop(loop, 5);
  assert.equal(repeatAudioLoop(player, loop), false);
  assert.equal(player.currentTime, 0.5);
  player.currentTime = 3;
  player.paused = true;
  repeatAudioLoop(player, loop);
  assert.equal(player.currentTime, 3);
  player.ended = true;
  assert.equal(repeatAudioLoop(player, loop), true);
  assert.equal(player.currentTime, 0.5);
  assert.throws(() => validateAudioLoop({ start: 2, end: 1 }, 5));
  assert.throws(() => validateAudioLoop({ start: 0, end: 6 }, 5));
  assert.throws(() => validateAudioLoop({ start: Number.NaN, end: 1 }, 5));
});

test("whole-sound repeat uses its explicit range even when region fields were cleared", () => {
  assert.deepEqual(resolveAudioLoop(5, "", "", { start: 0, end: 5 }), { start: 0, end: 5 });
  assert.throws(() => resolveAudioLoop(5, "", "2"));
  assert.throws(() => resolveAudioLoop(5, "0", ""));
  assert.deepEqual(resolveAudioLoop(5, "0.5", "2"), { start: 0.5, end: 2 });
});

test("catalog durations read as m:ss and unknown values stay hidden", () => {
  assert.equal(formatDuration(3.2), "0:03");
  assert.equal(formatDuration(0.3), "0:01");
  assert.equal(formatDuration(0), "0:00");
  assert.equal(formatDuration(75.6), "1:16");
  assert.equal(formatDuration(3725), "1:02:05");
  assert.equal(formatDuration(null), null);
  assert.equal(formatDuration(Number.NaN), null);
});

test("quick audition plays one sound at a time and a filter change stops it", async () => {
  const calls: string[] = [];
  const element = {
    src: "", onended: null as (() => void) | null,
    play: async () => { calls.push(`play ${element.src}`); },
    pause: () => { calls.push("pause"); },
    removeAttribute: () => { element.src = ""; },
    load: () => {},
  };
  let playing: number | null = null;
  const audition = createCatalogAudition(element as never, async (fileId) => `asset://${fileId}`, (fileId) => { playing = fileId; }, { querySelectorAll: () => [] } as never);
  await audition.toggle(1);
  assert.equal(playing, 1);
  await audition.toggle(2);
  assert.equal(playing, 2);
  assert.deepEqual(calls, ["pause", "play asset://1", "pause", "play asset://2"]);
  audition.stop(); // what the catalog does when the page or a filter changes
  assert.equal(playing, null);
  assert.equal(element.src, "");
  await audition.toggle(3);
  await audition.toggle(3);
  assert.equal(playing, null);
  // A slow lookup that a stop overtook never starts playing.
  const late = audition.toggle(4);
  audition.stop();
  await late;
  assert.equal(playing, null);
  assert.equal(calls[calls.length - 1], "pause");
  await audition.toggle(5);
  element.onended?.();
  assert.equal(audition.playing, null);
});

test("a finished sound releases its source when stopped", async () => {
  const element = { src: "", onended: null as (() => void) | null, play: async () => {}, pause: () => {}, removeAttribute: () => { element.src = ""; }, load: () => {} };
  const audition = createCatalogAudition(element as never, async (fileId) => `asset://${fileId}`, () => {}, { querySelectorAll: () => [] } as never);
  await audition.toggle(1);
  element.onended?.();
  assert.equal(element.src, "asset://1");
  audition.stop();
  assert.equal(element.src, "");
});

test("audition and page players take over from each other", async () => {
  const calls: string[] = [];
  const inspector = { pause: () => { calls.push("inspector pause"); } };
  const listeners = new Map<string, () => void>();
  const page = {
    querySelectorAll: (selector: string) => { calls.push(`query ${selector}`); return [inspector]; },
    addEventListener: (type: string, listener: () => void) => { listeners.set(type, listener); },
    removeEventListener: (type: string) => { listeners.delete(type); },
  };
  const element = { src: "", onended: null, play: async () => { calls.push("audition play"); }, pause: () => {}, removeAttribute: () => { element.src = ""; }, load: () => {} };
  let playing: number | null = null;
  const audition = createCatalogAudition(element as never, async (fileId) => `asset://${fileId}`, (fileId) => { playing = fileId; }, page as never);
  const detach = stopAuditionOnPagePlay(page as never, audition);
  await audition.toggle(1);
  assert.deepEqual(calls, ["query audio, video", "inspector pause", "audition play"]);
  listeners.get("play")?.(); // the inspector starts playing
  assert.equal(playing, null);
  assert.equal(element.src, "");
  detach();
  assert.equal(listeners.size, 0);
});
