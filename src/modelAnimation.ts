import { AnimationMixer, Bone, Box3, LoopOnce, LoopRepeat, Vector3, type AnimationAction, type AnimationClip, type Object3D } from "three";

export function modelPreviewBounds(model: Object3D) {
  const box = new Box3().setFromObject(model);
  if (box.isEmpty()) {
    const position = new Vector3();
    model.updateWorldMatrix(true, true);
    model.traverse((child) => { if (child instanceof Bone) box.expandByPoint(child.getWorldPosition(position)); });
  }
  return box;
}

export class ModelAnimation {
  private mixer: AnimationMixer;
  private root: Object3D;
  private action: AnimationAction;
  private clip: AnimationClip;
  readonly clips: AnimationClip[];
  private selected = 0;
  private loop = true;
  private speed = 1;

  constructor(root: Object3D, clips: AnimationClip[]) {
    if (!clips.length || clips.some((clip) => !Number.isFinite(clip.duration) || clip.duration <= 0 || !clip.tracks.length)) {
      throw new Error("Die Animation hat keine gültigen Tracks oder keine gültige Dauer.");
    }
    this.root = root;
    this.mixer = new AnimationMixer(root);
    this.clips = clips;
    this.clip = clips[0];
    this.action = this.mixer.clipAction(this.clip).setLoop(LoopRepeat, Infinity).play();
    this.action.clampWhenFinished = true;
    this.action.paused = true;
    this.mixer.update(0);
  }

  get state() {
    return {
      selected: this.selected, time: this.action.time, duration: this.clip.duration,
      playing: this.action.isRunning(), loop: this.loop, speed: this.speed,
    };
  }

  select(index: number) {
    if (!Number.isInteger(index) || !this.clips[index]) throw new Error("Der Animations-Clip ist nicht verfügbar.");
    this.mixer.stopAllAction();
    this.selected = index;
    this.clip = this.clips[index];
    this.action = this.mixer.clipAction(this.clip).reset().play();
    this.action.clampWhenFinished = true;
    this.setLoop(this.loop);
    this.setSpeed(this.speed);
    this.action.paused = true;
    this.mixer.update(0);
  }

  seek(time: number) {
    if (!Number.isFinite(time)) throw new Error("Die Animations-Zeit ist ungültig.");
    this.action.time = Math.max(0, Math.min(time, this.clip.duration));
    this.mixer.update(0);
  }

  play() {
    if (this.action.time >= this.clip.duration) this.seek(0);
    this.action.paused = false;
  }

  pause() {
    this.action.paused = true;
  }

  setSpeed(speed: number) {
    if (!Number.isFinite(speed) || speed <= 0 || speed > 4) throw new Error("Die Animations-Geschwindigkeit ist ungültig.");
    this.speed = speed;
    this.action.setEffectiveTimeScale(speed);
  }

  setLoop(loop: boolean) {
    this.loop = loop;
    this.action.setLoop(loop ? LoopRepeat : LoopOnce, loop ? Infinity : 1);
  }

  update(seconds: number) {
    this.mixer.update(seconds);
  }

  dispose() {
    this.mixer.stopAllAction();
    this.mixer.uncacheRoot(this.root);
  }
}
