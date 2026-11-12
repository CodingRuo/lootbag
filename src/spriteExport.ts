import { spriteSheetFrame, type SpriteSheetLayout } from "./imageWorkflows.ts";

export type SpriteExportMode = "frames" | "atlas" | "packed";
export const spriteExportBytes = 32 * 1024 * 1024;
type Rect = { x: number; y: number; width: number; height: number };
type Frame = { source: Rect; output: Rect; filename: string; trim?: Rect; empty?: boolean };
export type SpriteExportPlan = {
  width: number; height: number; layout: SpriteSheetLayout; fps: number; mode: SpriteExportMode;
  frames: Frame[]; outputs: Array<{ filename: string; width: number; height: number }>;
  packed?: { width: number; height: number; frames: number[][] };
};

export function spriteExportPlan(width: number, height: number, layout: SpriteSheetLayout, fps: number, mode: Exclude<SpriteExportMode, "packed">): SpriteExportPlan {
  const first = spriteSheetFrame(width, height, layout, 0);
  if (!first || !Number.isInteger(fps) || fps < 1 || fps > 60 || !["frames", "atlas"].includes(mode)) throw new Error("Spritesheet-Raster oder Bildrate ist ungültig.");
  if (first.count > 256) throw new Error("Ein Sprite-Export unterstützt maximal 256 Bilder.");
  const pixels = first.width * first.height * first.count;
  const outputWidth = mode === "atlas" ? first.width * layout.columns : first.width;
  const outputHeight = mode === "atlas" ? first.height * layout.rows : first.height;
  if (!Number.isSafeInteger(pixels) || pixels > 8 * 1024 * 1024 || outputWidth > 8192 || outputHeight > 8192) throw new Error("Sprite-Export: maximal 8 MiPixel und 8192 Pixel pro Ausgabekante.");
  const outputs = mode === "atlas" ? [{ filename: "atlas.png", width: outputWidth, height: outputHeight }] : Array.from({ length: first.count }, (_, i) => ({ filename: `frame-${String(i + 1).padStart(4, "0")}.png`, width: outputWidth, height: outputHeight }));
  const frames = Array.from({ length: first.count }, (_, i) => {
    const crop = spriteSheetFrame(width, height, layout, i)!;
    const source: Rect = { x: crop.x, y: crop.y, width: crop.width, height: crop.height };
    return { source, output: { x: mode === "atlas" ? i % layout.columns * crop.width : 0, y: mode === "atlas" ? Math.floor(i / layout.columns) * crop.height : 0, width: crop.width, height: crop.height }, filename: outputs[mode === "atlas" ? 0 : i].filename };
  });
  return { width, height, layout: { ...layout }, fps, mode, frames, outputs };
}

export async function encodeSpriteExport(image: HTMLImageElement, plan: SpriteExportPlan, source: { snapshotId: string; fileId: number; destination: string }, signal: AbortSignal) {
  signal.throwIfAborted();
  const canvas = document.createElement("canvas");
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  let rejectWait = (_reason: unknown) => {};
  const interrupted = new Promise<never>((_resolve, reject) => { rejectWait = reject; });
  const abort = () => rejectWait(signal.reason ?? new DOMException("Abgebrochen", "AbortError"));
  signal.addEventListener("abort", abort, { once: true });
  const timer = setTimeout(() => rejectWait(new Error("Sprite-Export: Encoding dauert länger als 30 Sekunden.")), 30_000);
  try {
    for (const output of plan.outputs) {
      signal.throwIfAborted();
      canvas.width = output.width; canvas.height = output.height;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("Sprite-Export: Bildfläche nicht verfügbar.");
      context.imageSmoothingEnabled = false;
      context.clearRect(0, 0, output.width, output.height);
      for (const frame of plan.frames.filter((frame) => frame.filename === output.filename)) {
        const { source: original, output: target } = frame;
        if (frame.empty) continue;
        const crop = frame.trim ? { x: original.x + frame.trim.x, y: original.y + frame.trim.y, width: frame.trim.width, height: frame.trim.height } : original;
        context.drawImage(image, crop.x, crop.y, crop.width, crop.height, target.x, target.y, target.width, target.height);
        if (plan.mode === "packed") {
          const { x, y, width: w, height: h } = crop, { x: dx, y: dy } = target;
          context.drawImage(image, x, y, 1, h, dx - 1, dy, 1, h);
          context.drawImage(image, x + w - 1, y, 1, h, dx + w, dy, 1, h);
          context.drawImage(image, x, y, w, 1, dx, dy - 1, w, 1);
          context.drawImage(image, x, y + h - 1, w, 1, dx, dy + h, w, 1);
          context.drawImage(image, x, y, 1, 1, dx - 1, dy - 1, 1, 1);
          context.drawImage(image, x + w - 1, y, 1, 1, dx + w, dy - 1, 1, 1);
          context.drawImage(image, x, y + h - 1, 1, 1, dx - 1, dy + h, 1, 1);
          context.drawImage(image, x + w - 1, y + h - 1, 1, 1, dx + w, dy + h, 1, 1);
        }
      }
      const blob = await Promise.race([new Promise<Blob>((resolve, reject) => {
        try { canvas.toBlob((blob) => { if (blob) resolve(blob); else reject(new Error("Sprite-Export: PNG konnte nicht erstellt werden.")); }, "image/png"); }
        catch (error) { reject(error); }
      }), interrupted]);
      signal.throwIfAborted();
      if (!blob.size || blob.size > spriteExportBytes - bytes) throw new Error("Sprite-Export: Die PNG-Ausgabe überschreitet 32 MiB.");
      const encoded = new Uint8Array(await Promise.race([blob.arrayBuffer(), interrupted]));
      signal.throwIfAborted();
      if (encoded.byteLength !== blob.size) throw new Error("Sprite-Export: PNG-Ausgabe unvollständig.");
      chunks.push(encoded); bytes += encoded.byteLength;
    }
    const header = new TextEncoder().encode(JSON.stringify({ ...source, width: plan.width, height: plan.height, layout: plan.layout, fps: plan.fps, mode: plan.mode, ...(plan.packed ? { packed: plan.packed } : {}), lengths: chunks.map((chunk) => chunk.length) }));
    if (header.length > 16 * 1024) throw new Error("Sprite-Export: Exportplan zu groß.");
    const payload = new Uint8Array(4 + header.length + bytes);
    new DataView(payload.buffer).setUint32(0, header.length, true);
    payload.set(header, 4);
    let offset = 4 + header.length;
    for (const chunk of chunks) { payload.set(chunk, offset); offset += chunk.length; }
    signal.throwIfAborted();
    return payload;
  } finally { clearTimeout(timer); signal.removeEventListener("abort", abort); canvas.width = canvas.height = 1; }
}

export async function deliverSpriteExport(image: HTMLImageElement, plan: SpriteExportPlan | (() => Promise<SpriteExportPlan>), source: { snapshotId: string; fileId: number; filename: string }, signal: AbortSignal, chooseParent: () => Promise<string | string[] | null>, publish: (payload: Uint8Array) => Promise<{ status: string; path?: string }>) {
  signal.throwIfAborted();
  const parent = await chooseParent();
  signal.throwIfAborted();
  if (!parent || typeof parent !== "string") return null;
  const prepared = typeof plan === "function" ? await plan() : plan;
  signal.throwIfAborted();
  const stem = Array.from(source.filename.replace(/\.[^.]+$/, "").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/^[. ]+|[. ]+$/g, "")).slice(0, 30).join("") || "Sprite";
  const destination = `${parent.replace(/[\\/]+$/g, "")}/${stem}-${prepared.mode}`;
  const bytes = await encodeSpriteExport(image, prepared, { snapshotId: source.snapshotId, fileId: source.fileId, destination }, signal);
  signal.throwIfAborted();
  const result = await publish(bytes);
  signal.throwIfAborted();
  return result.status === "exported" ? result.path ?? null : null;
}


export async function packedSpriteExportPlan(image: HTMLImageElement, layout: SpriteSheetLayout, fps: number, signal: AbortSignal): Promise<SpriteExportPlan> {
  const plan = spriteExportPlan(image.naturalWidth, image.naturalHeight, layout, fps, "frames");
  const deadline = Date.now() + 30_000;
  const check = () => { signal.throwIfAborted(); if (Date.now() > deadline) throw new Error("Sprite-Export: Vorbereitung dauert länger als 30 Sekunden."); };
  const canvas = document.createElement("canvas");
  try {
    for (const frame of plan.frames) {
      check();
      const crop = frame.source;
      canvas.width = crop.width; canvas.height = crop.height;
      const context = canvas.getContext("2d");
      if (!context) throw new Error("Sprite-Export: Bildfläche nicht verfügbar.");
      context.imageSmoothingEnabled = false;
      context.clearRect(0, 0, crop.width, crop.height);
      context.drawImage(image, crop.x, crop.y, crop.width, crop.height, 0, 0, crop.width, crop.height);
      const pixels = context.getImageData(0, 0, crop.width, crop.height).data;
      let left = crop.width, top = crop.height, right = -1, bottom = -1;
      for (let start = 0; start < crop.width * crop.height; start += 65_536) {
        check();
        const end = Math.min(start + 65_536, crop.width * crop.height);
        for (let i = start; i < end; i++) if (pixels[i * 4 + 3]) {
          const x = i % crop.width, y = Math.floor(i / crop.width);
          left = Math.min(left, x); top = Math.min(top, y); right = Math.max(right, x); bottom = Math.max(bottom, y);
        }
        await new Promise<void>((resolve) => setTimeout(resolve, 0));
      }
      check();
      frame.empty = right < 0;
      frame.trim = frame.empty ? { x: 0, y: 0, width: 0, height: 0 } : { x: left, y: top, width: right - left + 1, height: bottom - top + 1 };
    }
    const rectangles = plan.frames.map((frame, index) => ({ index, width: Math.max(1, frame.trim!.width) + 2, height: Math.max(1, frame.trim!.height) + 2 }))
      .sort((a, b) => b.height - a.height || b.width - a.width || a.index - b.index);
    const largest = Math.max(...rectangles.map((r) => r.width));
    const area = rectangles.reduce((sum, r) => sum + r.width * r.height, 0);
    if (largest > 8192 || rectangles.some((r) => r.height > 8192) || area > 8 * 1024 * 1024) throw new Error("Sprite-Export: Gepackte Ausgabe überschreitet 8 MiPixel oder 8192 Pixel pro Kante.");
    const widths = new Set([largest, Math.max(largest, Math.ceil(Math.sqrt(area)))]);
    for (let width = 1; width <= 8192; width *= 2) if (width >= largest) widths.add(width);
    let best: { width: number; height: number; positions: Array<{ x: number; y: number }> } | null = null;
    // ponytail: Bounded shelf packing is deterministic, not optimal; upgrade only when real atlases cannot fit.
    for (const width of widths) {
      check();
      let x = 0, y = 0, rowHeight = 0, usedWidth = 0;
      const positions: Array<{ x: number; y: number }> = [];
      for (const r of rectangles) {
        if (x + r.width > width) { x = 0; y += rowHeight; rowHeight = 0; }
        positions[r.index] = { x: x + 1, y: y + 1 };
        x += r.width; rowHeight = Math.max(rowHeight, r.height); usedWidth = Math.max(usedWidth, x);
      }
      const height = y + rowHeight;
      const usedArea = usedWidth * height, edge = Math.max(usedWidth, height);
      const better = !best || usedArea < best.width * best.height || usedArea === best.width * best.height &&
        (edge < Math.max(best.width, best.height) || edge === Math.max(best.width, best.height) && usedWidth < best.width);
      if (height <= 8192 && usedArea <= 8 * 1024 * 1024 && better) best = { width: usedWidth, height, positions };
    }
    if (!best) throw new Error("Sprite-Export: Die Frames passen nicht in einen Atlas bis 8 MiPixel und 8192 Pixel pro Kante.");
    plan.mode = "packed";
    plan.outputs = [{ filename: "atlas.png", width: best.width, height: best.height }];
    for (let i = 0; i < plan.frames.length; i++) {
      const f = plan.frames[i];
      f.filename = "atlas.png";
      f.output = { ...best.positions[i], width: Math.max(1, f.trim!.width), height: Math.max(1, f.trim!.height) };
    }
    plan.packed = { width: best.width, height: best.height, frames: plan.frames.map((f) => [f.trim!.x, f.trim!.y, f.trim!.width, f.trim!.height, f.output.x, f.output.y]) };
    check();
    return plan;
  } finally { canvas.width = canvas.height = 1; }
}
