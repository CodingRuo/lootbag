import assert from "node:assert/strict";
import test from "node:test";
import { repeatAudioLoop, resolveAudioLoop, seekAudio, validateAudioLoop } from "./audioAudition.ts";

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
