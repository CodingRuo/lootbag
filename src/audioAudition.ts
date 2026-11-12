export type AudioLoop = { start: number; end: number };
type Player = Pick<HTMLAudioElement, "currentTime" | "duration" | "paused" | "ended">;

export function resolveAudioLoop(duration: number, start: string, end: string, explicit?: AudioLoop): AudioLoop {
  if (!explicit && (!start.trim() || !end.trim())) throw new Error("Bitte Loop-Anfang und -Ende eingeben.");
  const loop = explicit ?? { start: Number(start), end: Number(end) };
  validateAudioLoop(loop, duration);
  return loop;
}

export function validateAudioLoop(loop: AudioLoop, duration: number) {
  if (!Number.isFinite(duration) || duration <= 0 || !Number.isFinite(loop.start)
    || !Number.isFinite(loop.end) || loop.start < 0 || loop.end > duration || loop.start >= loop.end) {
    throw new Error("Loop-Anfang und -Ende müssen innerhalb des Sounds liegen; das Ende muss nach dem Anfang liegen.");
  }
}

export function seekAudio(player: Player, position: number) {
  if (!Number.isFinite(position) || !Number.isFinite(player.duration) || player.duration <= 0) {
    throw new Error("Die Abspielposition ist ungültig.");
  }
  player.currentTime = Math.max(0, Math.min(position, player.duration));
}

export function repeatAudioLoop(player: Player, loop: AudioLoop): boolean {
  validateAudioLoop(loop, player.duration);
  if (player.paused && !player.ended) return false;
  if (player.currentTime >= loop.end || player.currentTime < loop.start || player.ended) {
    const restart = player.ended;
    seekAudio(player, loop.start);
    return restart;
  }
  return false;
}
