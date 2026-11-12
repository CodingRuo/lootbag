export const maxBlendBytes = 64 * 1024 * 1024;
const maxLeadingBytes = 1024 * 1024;
const maxBlocks = 256;
const maxEdge = 2048;
const maxPixels = 4 * 1024 * 1024;

export function readBlendThumbnail(buffer: ArrayBuffer) {
  if (buffer.byteLength > maxBlendBytes) throw new Error("Blender-Vorschau: Die Datei überschreitet 64 MiB.");
  const bytes = new Uint8Array(buffer), view = new DataView(buffer);
  if (bytes[0] === 31 && bytes[1] === 139 || bytes[0] === 40 && bytes[1] === 181 && bytes[2] === 47 && bytes[3] === 253) {
    throw new Error("Blender-Vorschau: Komprimierte Dateien werden noch nicht unterstützt.");
  }
  if (bytes.length < 17) throw new Error("Blender-Vorschau: Der Dateikopf ist unvollständig.");
  // ponytail: only qualified raw format1/version502/little64; other layouts need fixtures first.
  const text = new TextDecoder();
  if (text.decode(bytes.subarray(0, 17)) !== "BLENDER17-01v0502") throw new Error("Blender-Vorschau: Nur unkomprimiertes Format 1, Version 502, Little-Endian/64-Bit wird unterstützt.");
  let cursor = 17;
  for (let index = 0; index < maxBlocks; index++) {
    if (cursor > maxLeadingBytes) throw new Error("Blender-Vorschau: Der begrenzte Suchbereich von 1 MiB wurde überschritten.");
    if (cursor + 32 > bytes.length) throw new Error("Blender-Vorschau: Der Blockkopf ist unvollständig oder das gespeicherte Bild fehlt.");
    const code = text.decode(bytes.subarray(cursor, cursor + 4));
    const size = view.getBigInt64(cursor + 16, true);
    if (size < 0n || size > BigInt(maxBlendBytes) || view.getInt32(cursor + 4, true) < 0 || view.getBigInt64(cursor + 24, true) < 0n) {
      throw new Error("Blender-Vorschau: Ungültige Blockgröße oder Blockdaten.");
    }
    cursor += 32;
    if (size > BigInt(bytes.length - cursor)) throw new Error("Blender-Vorschau: Der Block ist unvollständig.");
    if (code === "TEST") {
      if (size < 8n) throw new Error("Blender-Vorschau: Der TEST-Block ist unvollständig.");
      const width = view.getInt32(cursor, true), height = view.getInt32(cursor + 4, true);
      if (width < 1 || height < 1 || width > maxEdge || height > maxEdge || width * height > maxPixels) {
        throw new Error("Blender-Vorschau: Ungültige Bildgröße; maximal 2048 Pixel pro Seite und 4 MiPixel.");
      }
      const rowBytes = width * 4;
      if (BigInt(8 + rowBytes * height) > size) throw new Error("Blender-Vorschau: Die TEST-Pixel sind unvollständig.");
      const rgba = new Uint8Array(rowBytes * height);
      // Blender's TEST stores ImBuf byte pixels bottom-up; canvas uses top-down RGBA.
      for (let row = 0; row < height; row++) rgba.set(bytes.subarray(cursor + 8 + (height - 1 - row) * rowBytes, cursor + 8 + (height - row) * rowBytes), row * rowBytes);
      return { width, height, buffer: rgba.buffer };
    }
    if (code !== "REND") throw new Error("Blender-Vorschau: Kein gespeichertes Vorschaubild vor den Szenendaten vorhanden.");
    cursor += Number(size);
  }
  throw new Error("Blender-Vorschau: Blocklimit von 256 überschritten.");
}
