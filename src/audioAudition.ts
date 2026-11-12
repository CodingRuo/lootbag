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

// Catalog duration as m:ss (h:mm:ss from one hour); sub-second sounds show 0:01 rather than 0:00.
export function formatDuration(value: number | null | undefined) {
  if (value == null || !Number.isFinite(value) || value < 0) return null;
  const total = value > 0 ? Math.max(1, Math.round(value)) : 0;
  const pad = (part: number) => part.toString().padStart(2, "0");
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor(total / 60) % 60;
  return hours ? `${hours}:${pad(minutes)}:${pad(total % 60)}` : `${minutes}:${pad(total % 60)}`;
}

type AuditionElement = Pick<HTMLAudioElement, "src" | "pause" | "play" | "removeAttribute" | "load" | "onended">;
type Page = Pick<Document, "querySelectorAll" | "addEventListener" | "removeEventListener">;

// Quick catalog audition: one shared element, so starting a sound always stops the previous one,
// and pauses the page's own players (the audition element is detached, so they never see it play).
export function createCatalogAudition(element: AuditionElement, resolve: (fileId: number) => Promise<string>, onChange: (fileId: number | null) => void, page: Pick<Page, "querySelectorAll">) {
  let current: number | null = null;
  let request = 0;
  const set = (fileId: number | null) => { current = fileId; onChange(fileId); };
  element.onended = () => set(null);
  function stop() {
    request++;
    // Unconditional: a sound that ended already cleared `current` but still holds its source.
    element.pause();
    element.removeAttribute("src");
    element.load();
    if (current !== null) set(null);
  }
  async function toggle(fileId: number) {
    if (current === fileId) return stop();
    stop();
    const token = ++request;
    set(fileId);
    try {
      const src = await resolve(fileId);
      if (token !== request) return;
      element.src = src;
      page.querySelectorAll<HTMLMediaElement>("audio, video").forEach((media) => media.pause());
      await element.play();
    } catch (reason) {
      // A newer toggle or stop superseded this request; its abort is expected.
      if (token !== request) return;
      set(null);
      throw reason;
    }
  }
  return { toggle, stop, get playing() { return current; } };
}

// The reverse takeover: any player in the page starting stops the audition. Returns the cleanup.
export function stopAuditionOnPagePlay(page: Pick<Page, "addEventListener" | "removeEventListener">, audition: { stop(): void }) {
  const stop = () => audition.stop();
  page.addEventListener("play", stop, true);
  return () => page.removeEventListener("play", stop, true);
}
