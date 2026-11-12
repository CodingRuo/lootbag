import assert from "node:assert/strict";
import test from "node:test";
import { AnimationClip, Bone, Group, Object3D, SkeletonHelper, Vector3, VectorKeyframeTrack } from "three";
import { ModelAnimation, modelPreviewBounds } from "./modelAnimation.ts";

function animatedModel() {
  const model = new Group();
  const target = new Object3D();
  target.name = "Moving";
  model.add(target);
  const clips = [new AnimationClip("Move", 2, [
    new VectorKeyframeTrack("Moving.position", [0, 2], [0, 0, 0, 4, 0, 0]),
  ])];
  return { model, target, clips };
}

test("a paused clip can seek to its middle and exact endpoint without advancing", () => {
  const { model, target, clips } = animatedModel();
  const animation = new ModelAnimation(model, clips);
  animation.seek(1);
  assert.equal(target.position.x, 2);
  assert.equal(animation.state.playing, false);
  animation.update(0.5);
  assert.equal(target.position.x, 2);
  animation.seek(2);
  assert.equal(target.position.x, 4);
  assert.equal(animation.state.time, 2);
  animation.dispose();
});

test("playback advances at the selected speed, pauses and loops", () => {
  const { model, target, clips } = animatedModel();
  const animation = new ModelAnimation(model, clips);
  animation.setSpeed(2);
  animation.play();
  animation.update(0.25);
  assert.equal(target.position.x, 1);
  animation.pause();
  animation.update(0.5);
  assert.equal(target.position.x, 1);
  animation.play();
  animation.update(1);
  assert.equal(target.position.x, 1);
  assert.equal(animation.state.time, 0.5);
  assert.equal(animation.state.playing, true);
  animation.dispose();
});

test("a non-looping clip holds its final pose, stops and can play again", () => {
  const { model, target, clips } = animatedModel();
  const animation = new ModelAnimation(model, clips);
  animation.setLoop(false);
  animation.play();
  animation.update(3);
  assert.equal(target.position.x, 4);
  assert.equal(animation.state.time, 2);
  assert.equal(animation.state.playing, false);
  animation.play();
  assert.equal(target.position.x, 0);
  animation.update(0.5);
  assert.equal(target.position.x, 1);
  animation.dispose();
});

test("switching clips restores unanimated properties and resets the selected pose", () => {
  const { model, target, clips } = animatedModel();
  clips.push(new AnimationClip("Grow", 1, [
    new VectorKeyframeTrack("Moving.scale", [0, 1], [1, 1, 1, 2, 2, 2]),
  ]));
  const animation = new ModelAnimation(model, clips);
  animation.seek(1);
  assert.equal(target.position.x, 2);
  animation.select(1);
  assert.equal(target.position.x, 0);
  assert.equal(animation.state.selected, 1);
  assert.equal(animation.state.time, 0);
  assert.equal(animation.state.playing, false);
  animation.seek(0.5);
  assert.equal(target.scale.x, 1.5);
  animation.dispose();
  assert.equal(target.scale.x, 1);
  animation.update(1);
  assert.equal(target.scale.x, 1);
});

test("invalid clip indices, times and speeds cannot corrupt the active pose", () => {
  const { model, target, clips } = animatedModel();
  const animation = new ModelAnimation(model, clips);
  animation.seek(1);
  for (const index of [-1, 0.5, 2]) assert.throws(() => animation.select(index), /Clip/);
  assert.throws(() => animation.seek(NaN), /Zeit/);
  for (const speed of [0, -1, Infinity]) assert.throws(() => animation.setSpeed(speed), /Geschwindigkeit/);
  assert.equal(target.position.x, 2);
  animation.seek(-4);
  assert.equal(target.position.x, 0);
  animation.seek(4);
  assert.equal(target.position.x, 4);
  animation.dispose();
  assert.throws(() => new ModelAnimation(model, []), /Animation/);
  assert.throws(() => new ModelAnimation(model, [new AnimationClip("Invalid", 0, [])]), /Animation/);
});

test("a skeleton without a mesh has finite camera bounds and visible animated bone lines", () => {
  const model = new Group();
  const hip = new Bone();
  hip.name = "Hip";
  const tip = new Bone();
  tip.name = "Tip";
  tip.position.set(0, 2, 0);
  hip.add(tip);
  model.add(hip);
  const bounds = modelPreviewBounds(model);
  assert.deepEqual(bounds.min.toArray(), [0, 0, 0]);
  assert.deepEqual(bounds.max.toArray(), [0, 2, 0]);
  const helper = new SkeletonHelper(model);
  const animation = new ModelAnimation(model, [new AnimationClip("Bend", 2, [
    new VectorKeyframeTrack("Tip.position", [0, 2], [0, 2, 0, 2, 2, 0]),
  ])]);
  animation.seek(1);
  model.updateMatrixWorld(true);
  helper.updateMatrixWorld(true);
  assert.deepEqual(new Vector3().fromBufferAttribute(helper.geometry.getAttribute("position"), 0).toArray(), [1, 2, 0]);
  assert.deepEqual(modelPreviewBounds(model).max.toArray(), [1, 2, 0]);
  animation.dispose();
  helper.dispose();
});
