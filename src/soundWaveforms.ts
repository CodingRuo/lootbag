import { createModelThumbnailQueue } from "./modelThumbnails.ts";

/** Card waveform bars (0–255 per bar, from `card_waveform`) as a small SVG image. */
export function waveformUrl(bars: number[]) {
  const rects = bars.map((level, index) => {
    const height = Math.max(2, level / 255 * 90);
    return `<rect x="${index * 4 + 0.5}" y="${(100 - height) / 2}" width="3" height="${height}"/>`;
  }).join("");
  return `data:image/svg+xml,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" width="192" height="96" viewBox="0 0 ${bars.length * 4} 100" preserveAspectRatio="none" fill="#91b9cf">${rects}</svg>`)}`;
}

// Same one-at-a-time, viewport-only queue as model thumbnails; data URLs need no revoking.
export const createSoundWaveformQueue = (load: (fileId: number) => Promise<number[]>) =>
  createModelThumbnailQueue(async (fileId) => ({ url: waveformUrl(await load(fileId)), warning: null, error: null }), () => {}, () => {});
