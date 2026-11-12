export const maxAsepriteBytes = 32 * 1024 * 1024;
const maxCanvasPixels = 4 * 1024 * 1024;
const maxRetainedPixels = 16 * 1024 * 1024;
const maxTouchedPixels = 64 * 1024 * 1024;
const metadataChunks = new Set([0x0004, 0x0011, 0x2018, 0x2019, 0x2020, 0x2022]);
type Layer = { visible: boolean; opacity: number };
type Cel = { layer: number; x: number; y: number; opacity: number; width: number; height: number; input: Uint8Array<ArrayBuffer>; compressed?: boolean; image?: Cel; pixels?: Uint8Array<ArrayBuffer> };
type Frame = { durationMs: number; cels: Map<number, Cel> };
export type AsepriteAnimation = { width: number; height: number; frames: { durationMs: number; buffer: ArrayBuffer }[] };
function fail(reason: string): never { throw new Error(`Aseprite-Vorschau: ${reason}`); }
const un8 = (a: number, b: number) => Math.floor((a * b + 127) / 255);

function preflight(buffer: ArrayBuffer) {
  if (buffer.byteLength > maxAsepriteBytes) fail("Die Datei überschreitet 32 MiB.");
  const bytes = new Uint8Array(buffer), view = new DataView(buffer);
  if (bytes.length < 128) fail("Der Dateikopf ist unvollständig.");
  if (view.getUint32(0, true) !== bytes.length || view.getUint16(4, true) !== 0xa5e0) fail("Ungültige Dateigröße oder Dateikennung.");
  const width = view.getUint16(8, true), height = view.getUint16(10, true), count = view.getUint16(6, true), flags = view.getUint32(14, true);
  if (view.getUint16(12, true) !== 32) fail("Nur 32-Bit-RGBA wird unterstützt; Indexed und Graustufen sind noch nicht verfügbar.");
  if (!width || !height || width > 4096 || height > 4096 || width * height > maxCanvasPixels) fail("Ungültige Bildgröße; maximal 4096 Pixel pro Seite und 4 MiPixel.");
  if (!count || count > 256 || width * height * count > maxRetainedPixels) fail("Frame-Budget überschritten: maximal 256 Frames und zusammen 16 MiPixel.");
  if (flags & ~7 || bytes[34] && bytes[35] && bytes[34] !== bytes[35]) fail("Unbekannte Dateiflags oder nicht quadratische Pixel werden noch nicht unterstützt.");
  const layers: Layer[] = [], frames: Frame[] = [], images: Cel[] = [];
  let cursor = 128, chunks = 0, decodedPixels = 0, touchedPixels = 0, layersClosed = false;
  for (let frameIndex = 0; frameIndex < count; frameIndex++) {
    if (cursor + 16 > bytes.length) fail("Der Frame-Kopf ist unvollständig.");
    const size = view.getUint32(cursor, true), end = cursor + size;
    if (size < 16 || end > bytes.length || view.getUint16(cursor + 4, true) !== 0xf1fa) fail("Ungültiger oder unvollständiger Frame.");
    const chunkCount = view.getUint32(cursor + 12, true) || view.getUint16(cursor + 6, true);
    chunks += chunkCount; if (chunks > 100000) fail("Chunk-Budget von 100000 überschritten.");
    const durationMs = view.getUint16(cursor + 8, true) || view.getUint16(18, true);
    if (!durationMs) fail("Keine positive Frame-Dauer vorhanden.");
    const frame: Frame = { durationMs, cels: new Map() }; frames.push(frame); cursor += 16;
    for (let index = 0; index < chunkCount; index++) {
      if (cursor + 6 > end) fail("Der Chunk-Kopf ist unvollständig.");
      const size = view.getUint32(cursor, true), type = view.getUint16(cursor + 4, true), next = cursor + size, body = cursor + 6;
      if (size < 6 || next > end) fail("Ungültiger oder unvollständiger Chunk.");
      const need = (length: number) => { if (body + length > next) fail("Chunk-Daten sind unvollständig."); };
      if (type === 0x2004) {
        need(18);
        const layerFlags = view.getUint16(body, true), nameBytes = view.getUint16(body + 16, true);
        if (frameIndex || layersClosed || layers.length >= 64) fail("Layer werden nur einmal vor den Cels im ersten Frame unterstützt; maximal 64 Layer.");
        if (layerFlags & ~127 || layerFlags & (8 | 64) || view.getUint16(body + 2, true) || view.getUint16(body + 4, true) || view.getUint16(body + 10, true)) fail("Nur flache normale Layer ohne Gruppen, Tilemaps, Referenz-/Hintergrundlayer oder andere Blend-Modi werden unterstützt.");
        if (nameBytes > 4096 || body + 18 + nameBytes + (flags & 4 ? 16 : 0) !== next) fail("Ungültiger Layer-Name oder UUID-Bereich.");
        layers.push({ visible: Boolean(layerFlags & 1), opacity: flags & 1 ? bytes[body + 12] : 255 });
      } else if (type === 0x2005) {
        need(16); layersClosed = true;
        const layer = view.getUint16(body, true), celType = view.getUint16(body + 7, true);
        if (layer >= layers.length || frame.cels.has(layer)) fail("Cel-Layer fehlt oder enthält mehrere Cels im selben Frame.");
        if (view.getInt16(body + 9, true) !== 0) fail("Cel-Z-Reihenfolge wird noch nicht unterstützt.");
        const cel: Cel = { layer, x: view.getInt16(body + 2, true), y: view.getInt16(body + 4, true), opacity: bytes[body + 6], width: 0, height: 0, input: bytes.subarray(0, 0) };
        if (celType === 1) {
          need(18); if (body + 18 !== next) fail("Ungültige verknüpfte Cel-Daten.");
          const targetFrame = view.getUint16(body + 16, true), target = frames[targetFrame]?.cels.get(layer);
          if (targetFrame >= frameIndex || !target) fail("Cel-Verknüpfung fehlt, zeigt vorwärts oder ist zyklisch.");
          const image = target.image ?? target; cel.image = image; cel.width = image.width; cel.height = image.height;
        } else if (celType === 0 || celType === 2) {
          need(20); cel.width = view.getUint16(body + 16, true); cel.height = view.getUint16(body + 18, true);
          if (!cel.width || !cel.height || cel.width > 4096 || cel.height > 4096 || cel.width * cel.height > maxCanvasPixels) fail("Ungültige Cel-Bildgröße oder Cel-Pixelbudget überschritten.");
          decodedPixels += cel.width * cel.height; if (decodedPixels > maxRetainedPixels) fail("Dekodierte Cel-Pixel überschreiten zusammen 16 MiPixel.");
          cel.input = bytes.subarray(body + 20, next);
          cel.compressed = celType === 2;
          if (cel.compressed ? !cel.input.length : cel.input.length !== cel.width * cel.height * 4) fail("Cel-Pixel sind unvollständig oder enthalten überzählige Daten.");
          images.push(cel);
        } else fail("Dieser Cel-Typ wird noch nicht unterstützt.");
        touchedPixels += cel.width * cel.height;
        if (touchedPixels > maxTouchedPixels) fail("Kompositions-Arbeitsbudget von 64 MiPixel überschritten.");
        frame.cels.set(layer, cel);
      } else if (type === 0x2007) {
        need(16);
        if (size !== 22 || view.getUint16(body, true) > 1 || view.getUint16(body + 2, true)) fail("ICC oder spezielle Gamma-Farbprofile werden noch nicht unterstützt.");
      } else if (!metadataChunks.has(type)) fail(`Render-Semantik von Chunk 0x${type.toString(16)} wird noch nicht unterstützt.`);
      cursor = next;
    }
    if (cursor !== end) fail("Frame-Ende stimmt nicht mit seinen Chunks überein.");
  }
  if (cursor !== bytes.length || !layers.length) fail("Dateiende oder Layer-Aufbau ist ungültig.");
  return { width, height, frames, layers, images };
}

async function inflateCel(input: Uint8Array<ArrayBuffer>, expected: number, signal: AbortSignal) {
  if (typeof DecompressionStream === "undefined") fail("Standard-Zlib-Dekompression ist in dieser Ansicht nicht verfügbar.");
  signal.throwIfAborted();
  const decoder = (() => {
    try { return new DecompressionStream("deflate"); } catch { return fail("Standard-Zlib-Dekompression ist in dieser Ansicht nicht verfügbar."); }
  })();
  const controller = new AbortController();
  let offset = 0;
  const source = new ReadableStream<BufferSource>({ pull(stream) {
    if (offset === input.length) { stream.close(); return; }
    const end = Math.min(input.length, offset + 1024);
    stream.enqueue(input.subarray(offset, end)); offset = end;
  } });
  // ponytail: native decompressor memory is opaque; input/output/work and worker lifetime are capped.
  const pipe = source.pipeTo(decoder.writable, { signal: controller.signal });
  const pipeSettled = pipe.catch(() => {});
  const reader = decoder.readable.getReader();
  const abort = () => { controller.abort(signal.reason); void reader.cancel(signal.reason).catch(() => {}); };
  signal.addEventListener("abort", abort, { once: true });
  const pixels = new Uint8Array(expected);
  let written = 0;
  try {
    signal.throwIfAborted();
    while (true) {
      const { done, value } = await reader.read();
      signal.throwIfAborted();
      if (done) break;
      if (value.length > expected - written) fail("Zlib liefert mehr Pixel als die deklarierte Cel-Größe.");
      pixels.set(value, written); written += value.length;
    }
    await pipe; signal.throwIfAborted();
    if (written !== expected) fail("Die dekomprimierten Cel-Pixel sind unvollständig.");
    return pixels;
  } catch (reason) {
    signal.throwIfAborted();
    if (reason instanceof Error && reason.message.startsWith("Aseprite-Vorschau:")) throw reason;
    fail("Zlib-Daten sind beschädigt, unvollständig oder enthalten überzählige Daten.");
  } finally {
    signal.removeEventListener("abort", abort);
    controller.abort(); await reader.cancel().catch(() => {}); await pipeSettled; reader.releaseLock();
  }
}

export async function decodeAseprite(buffer: ArrayBuffer, signal = new AbortController().signal, firstOnly = false): Promise<AsepriteAnimation> {
  signal.throwIfAborted();
  const file = preflight(buffer);
  for (const image of file.images) {
    signal.throwIfAborted();
    image.pixels = image.compressed ? await inflateCel(image.input, image.width * image.height * 4, signal) : image.input;
  }
  const frames: AsepriteAnimation["frames"] = [];
  for (const frame of firstOnly ? file.frames.slice(0, 1) : file.frames) {
    signal.throwIfAborted();
    const pixels = new Uint8Array(file.width * file.height * 4);
    for (const [layerIndex, layer] of file.layers.entries()) {
      const cel = frame.cels.get(layerIndex); if (!layer.visible || !cel) continue;
      const source = (cel.image ?? cel).pixels!, opacity = un8(cel.opacity, layer.opacity);
      for (let y = Math.max(0, -cel.y); y < Math.min(cel.height, file.height - cel.y); y++) {
        for (let x = Math.max(0, -cel.x); x < Math.min(cel.width, file.width - cel.x); x++) {
          const from = (y * cel.width + x) * 4, to = ((y + cel.y) * file.width + x + cel.x) * 4;
          const alpha = un8(source[from + 3], opacity), backdrop = pixels[to + 3];
          if (!backdrop) { pixels.set(source.subarray(from, from + 3), to); pixels[to + 3] = alpha; }
          else if (source[from + 3]) {
            const combined = alpha + backdrop - un8(alpha, backdrop);
            for (let channel = 0; channel < 3; channel++) pixels[to + channel] += Math.trunc((source[from + channel] - pixels[to + channel]) * alpha / combined);
            pixels[to + 3] = combined;
          }
        }
      }
    }
    frames.push({ durationMs: frame.durationMs, buffer: pixels.buffer });
  }
  return { width: file.width, height: file.height, frames };
}
