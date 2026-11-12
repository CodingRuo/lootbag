import { maxAsepriteBytes, type AsepriteAnimation } from "./asepriteDecode.ts";
import { maxBlendBytes } from "./blendThumbnail.ts";
import { maxTgaBytes } from "./tgaDecode.ts";
import { maxGifBytes, preflightGif } from "./gifPreflight.ts";

type PreviewFormat = "TGA" | "GIF" | "BLEND" | "ASEPRITE";
type Pixels = { width: number; height: number; buffer: ArrayBuffer };
type Job = { run: () => Promise<void>; cancel: () => void };
const queued: Job[] = [];
let decoding = false;

async function next() {
  if (decoding) return;
  const job = queued.shift();
  if (!job) return;
  decoding = true;
  try { await job.run(); } finally { decoding = false; void next(); }
}

async function readPreviewFile(url: string, signal: AbortSignal, format: PreviewFormat) {
  const limit = format === "TGA" ? maxTgaBytes : format === "BLEND" ? maxBlendBytes : format === "ASEPRITE" ? maxAsepriteBytes : maxGifBytes;
  const tooLarge = `${format}-Vorschau: Die Datei überschreitet ${limit / 1024 / 1024} MiB.`;
  const response = await fetch(url, { signal });
  if (!response.ok) throw new Error(`Die ${format}-Datei ist nicht verfügbar.`);
  if (Number(response.headers.get("content-length")) > limit) {
    await response.body?.cancel().catch(() => {});
    throw new Error(tooLarge);
  }
  if (!response.body) throw new Error(`Die ${format}-Vorschau benötigt begrenztes Streaming in dieser Ansicht.`);
  const reader = response.body.getReader();
  const abort = () => { void reader.cancel().catch(() => {}); };
  signal.addEventListener("abort", abort, { once: true });
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      signal.throwIfAborted();
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > limit) throw new Error(tooLarge);
      chunks.push(value);
    }
  } catch (reason) { await reader.cancel().catch(() => {}); throw reason; }
  finally { signal.removeEventListener("abort", abort); reader.releaseLock(); }
  signal.throwIfAborted();
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return bytes.buffer;
}

function previewJob<T>(url: string, signal: AbortSignal, format: PreviewFormat, useBytes: (bytes: ArrayBuffer, signal: AbortSignal) => Promise<T>): Promise<T> {
  signal.throwIfAborted();
  if (queued.length >= 48) return Promise.reject(new Error("Die Bildvorschau ist ausgelastet. Wähle das Bild erneut aus."));
  return new Promise((resolve, reject) => {
    const controller = new AbortController();
    const dispose = () => { signal.removeEventListener("abort", abort); clearTimeout(timer); };
    const abort = () => {
      controller.abort(signal.reason);
      const index = queued.indexOf(job);
      if (index >= 0) queued.splice(index, 1);
      dispose();
      reject(signal.reason ?? new DOMException("Abgebrochen", "AbortError"));
    };
    const timer = setTimeout(() => {
      controller.abort();
      const index = queued.indexOf(job);
      if (index >= 0) queued.splice(index, 1);
      dispose();
      reject(new Error(`${format}-Vorschau: Zeitlimit von 30 Sekunden überschritten.`));
    }, 30_000);
    const job: Job = { cancel: abort, run: async () => {
      try {
        const buffer = await readPreviewFile(url, controller.signal, format);
        controller.signal.throwIfAborted();
        const preview = await useBytes(buffer, controller.signal);
        controller.signal.throwIfAborted();
        resolve(preview);
      } catch (reason) { reject(reason); }
      finally { dispose(); }
    } };
    signal.addEventListener("abort", job.cancel, { once: true });
    queued.push(job);
    void next();
  });
}

async function workerOutput<T>(worker: Worker, message: unknown, buffer: ArrayBuffer, signal: AbortSignal, format: string): Promise<T> {
  let abort = () => {};
  try {
    signal.throwIfAborted();
    const result = await new Promise<T>((resolve, reject) => {
      abort = () => { worker.terminate(); reject(signal.reason ?? new DOMException("Abgebrochen", "AbortError")); };
      signal.addEventListener("abort", abort, { once: true });
      worker.onerror = () => reject(new Error(`Der ${format}-Decoder konnte in dieser Ansicht nicht gestartet werden.`));
      worker.onmessageerror = () => reject(new Error(`Die ${format}-Pixel konnten nicht übertragen werden.`));
      worker.onmessage = ({ data }: MessageEvent<T & { error?: string }>) => data.error ? reject(new Error(data.error)) : resolve(data);
      worker.postMessage(message, [buffer]);
    });
    signal.throwIfAborted();
    return result;
  } finally { signal.removeEventListener("abort", abort); worker.terminate(); }
}

function pixelJob<T>(url: string, signal: AbortSignal, format: "TGA" | "BLEND" | "ASEPRITE", usePixels: (pixels: Pixels, signal: AbortSignal) => Promise<T>) {
  return previewJob(url, signal, format, async (buffer, jobSignal) => {
    const worker = format === "ASEPRITE"
      ? new Worker(new URL("./asepriteWorker.ts", import.meta.url), { type: "module" })
      : format === "BLEND"
        ? new Worker(new URL("./blendWorker.ts", import.meta.url), { type: "module" })
        : new Worker(new URL("./tgaWorker.ts", import.meta.url), { type: "module" });
    const pixels = await workerOutput<Pixels>(worker, buffer, buffer, jobSignal, format);
    return usePixels(pixels, jobSignal);
  });
}

export function decodeAsepriteUrl(url: string, signal: AbortSignal) {
  return pixelJob(url, signal, "ASEPRITE", async (pixels) => pixels);
}

export async function loadAsepriteAnimation(url: string, signal: AbortSignal) {
  const animation = await previewJob(url, signal, "ASEPRITE", (buffer, jobSignal) => {
    const worker = new Worker(new URL("./asepriteWorker.ts", import.meta.url), { type: "module" });
    return workerOutput<AsepriteAnimation>(worker, { buffer, animation: true }, buffer, jobSignal, "Aseprite");
  });
  const release = () => { signal.removeEventListener("abort", release); animation.frames.length = 0; };
  signal.addEventListener("abort", release, { once: true });
  if (signal.aborted) { release(); signal.throwIfAborted(); }
  return { ...animation, release };
}

export function decodeTgaUrl(url: string, signal: AbortSignal) {
  return pixelJob(url, signal, "TGA", async (pixels) => pixels);
}

async function loadGifBytes(buffer: ArrayBuffer, signal: AbortSignal) {
  const info = preflightGif(buffer);
  signal.throwIfAborted();
  const url = URL.createObjectURL(new Blob([buffer], { type: "image/gif" }));
  const preview = await loadImageUrl({ url, release: () => URL.revokeObjectURL(url) }, "GIF", signal, 16 * 1024 * 1024, info);
  const release = () => { signal.removeEventListener("abort", release); preview.release(); };
  signal.addEventListener("abort", release, { once: true });
  if (signal.aborted) { release(); signal.throwIfAborted(); }
  return { ...preview, ...info, release };
}

export async function loadGifAnimation(url: string, signal: AbortSignal) {
  const preview = await previewJob(url, signal, "GIF", (buffer, jobSignal) => loadGifBytes(buffer, jobSignal));
  const release = () => { signal.removeEventListener("abort", release); preview.release(); };
  signal.addEventListener("abort", release, { once: true });
  if (signal.aborted) { release(); signal.throwIfAborted(); }
  return { ...preview, release };
}

export function decodeGifUrl(url: string, signal: AbortSignal) {
  return previewJob(url, signal, "GIF", async (buffer, jobSignal) => {
    const preview = await loadGifBytes(buffer, jobSignal);
    const canvas = document.createElement("canvas");
    canvas.width = preview.width; canvas.height = preview.height;
    try {
      const context = canvas.getContext("2d");
      if (!context) throw new Error("Die GIF-Bildvorschau ist nicht verfügbar.");
      // Canvas image sources use the animation's default (first composite) image.
      context.drawImage(preview.image, 0, 0);
      const data = context.getImageData(0, 0, preview.width, preview.height).data;
      return { width: preview.width, height: preview.height, buffer: data.buffer as ArrayBuffer };
    } finally { preview.release(); canvas.width = canvas.height = 0; }
  });
}

export async function imagePreviewUrl(url: string, filename: string, signal: AbortSignal, maxSide?: number) {
  signal.throwIfAborted();
  if (!/\.(tga|gif|blend|ase|aseprite)$/i.test(filename)) return { url, release: () => {} };
  const encode = async (canvas: HTMLCanvasElement, jobSignal: AbortSignal) => {
    jobSignal.throwIfAborted();
    let scaled: HTMLCanvasElement | null = null;
    try {
      if (maxSide && Math.max(canvas.width, canvas.height) > maxSide) {
        scaled = document.createElement("canvas");
        const ratio = maxSide / Math.max(canvas.width, canvas.height);
        scaled.width = Math.max(1, Math.round(canvas.width * ratio)); scaled.height = Math.max(1, Math.round(canvas.height * ratio));
        const target = scaled.getContext("2d");
        if (!target) throw new Error("Die Miniaturvorschau ist nicht verfügbar.");
        target.drawImage(canvas, 0, 0, scaled.width, scaled.height);
      }
      const blob = await new Promise<Blob>((resolve, reject) => {
        const abort = () => reject(jobSignal.reason ?? new DOMException("Abgebrochen", "AbortError"));
        jobSignal.addEventListener("abort", abort, { once: true });
        (scaled ?? canvas).toBlob((value) => {
          jobSignal.removeEventListener("abort", abort);
          value ? resolve(value) : reject(new Error("Die Bildvorschau konnte nicht erstellt werden."));
        }, "image/png");
      });
      jobSignal.throwIfAborted();
      return blob;
    } finally { canvas.width = canvas.height = 0; if (scaled) scaled.width = scaled.height = 0; }
  };
  const blob = /\.gif$/i.test(filename)
    ? await previewJob(url, signal, "GIF", async (buffer, jobSignal) => {
      const preview = await loadGifBytes(buffer, jobSignal);
      const canvas = document.createElement("canvas");
      canvas.width = preview.width; canvas.height = preview.height;
      try {
        const context = canvas.getContext("2d");
        if (!context) throw new Error("Die GIF-Bildvorschau ist nicht verfügbar.");
        context.drawImage(preview.image, 0, 0);
        return await encode(canvas, jobSignal);
      } finally { preview.release(); canvas.width = canvas.height = 0; }
    })
    : await pixelJob(url, signal, /\.(ase|aseprite)$/i.test(filename) ? "ASEPRITE" : /\.blend$/i.test(filename) ? "BLEND" : "TGA", async (pixels, jobSignal) => {
      const canvas = document.createElement("canvas");
      canvas.width = pixels.width; canvas.height = pixels.height;
      try {
        const context = canvas.getContext("2d");
        if (!context) throw new Error("Die Bildvorschau ist nicht verfügbar.");
        context.putImageData(new ImageData(new Uint8ClampedArray(pixels.buffer), pixels.width, pixels.height), 0, 0);
        return await encode(canvas, jobSignal);
      } finally { canvas.width = canvas.height = 0; }
    });
  signal.throwIfAborted();
  const preview = URL.createObjectURL(blob);
  return { url: preview, release: () => URL.revokeObjectURL(preview) };
}

export async function loadPreviewImage(url: string, filename: string, signal: AbortSignal, pixelBudget = 32 * 1024 * 1024) {
  const preview = await imagePreviewUrl(url, filename, signal);
  return loadImageUrl(preview, filename, signal, pixelBudget);
}

async function loadImageUrl(preview: { url: string; release: () => void }, filename: string, signal: AbortSignal, pixelBudget: number, expected?: { width: number; height: number }) {
  const image = new Image();
  const release = () => { image.onload = image.onerror = null; image.src = ""; preview.release(); };
  try {
    await new Promise<void>((resolve, reject) => {
      const finish = (reason?: Error | DOMException) => {
        clearTimeout(timer); signal.removeEventListener("abort", abort);
        image.onload = image.onerror = null;
        reason ? reject(reason) : resolve();
      };
      const abort = () => finish(new DOMException("Abgebrochen", "AbortError"));
      const timer = setTimeout(() => finish(new Error(`Die Bildvorschau überschreitet das Zeitlimit: ${filename}`)), 15_000);
      image.onload = () => {
        const pixels = image.naturalWidth * image.naturalHeight;
        finish(!pixels || pixels > pixelBudget || expected && (image.naturalWidth !== expected.width || image.naturalHeight !== expected.height)
          ? new Error(`Pixelbudget der Bildvorschau überschritten oder Bildgröße ungültig: ${filename}`) : undefined);
      };
      image.onerror = () => finish(new Error(`Das Bild kann nicht angezeigt werden: ${filename}`));
      signal.addEventListener("abort", abort, { once: true });
      if (signal.aborted) abort(); else image.src = preview.url;
    });
    signal.throwIfAborted();
    return { image, release };
  } catch (reason) { release(); throw reason; }
}
