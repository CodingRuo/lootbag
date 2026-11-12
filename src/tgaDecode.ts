import { TGALoader } from "three/addons/loaders/TGALoader.js";

export const maxTgaBytes = 64 * 1024 * 1024;
const maxPixels = 16 * 1024 * 1024;

// TGALoader tolerates missing bytes; validate before its dimension-driven allocations.
export function decodeTga(buffer: ArrayBuffer) {
  const data = new Uint8Array(buffer);
  const invalid = (detail: string): never => { throw new Error(`TGA-Vorschau: ${detail}`); };
  if (data.length < 18) invalid("Der Dateikopf ist unvollständig.");
  if (data.length > maxTgaBytes) invalid("Die Datei überschreitet 64 MiB.");
  const u16 = (offset: number) => data[offset] | data[offset + 1] << 8;
  const type = data[2];
  const indexed = type === 1 || type === 9;
  const gray = type === 3 || type === 11;
  if (![1, 2, 3, 9, 10, 11].includes(type)) invalid("Dieser Bildtyp wird nicht unterstützt.");
  const width = u16(12), height = u16(14), depth = data[16], descriptor = data[17];
  const pixels = width * height;
  if (!width || !height || width > 8192 || height > 8192 || pixels > maxPixels) invalid("Maximal 16 Megapixel und 8192 Pixel pro Seite sind erlaubt.");
  if (descriptor & 0xc0) invalid("Interleaved-Bilder werden nicht unterstützt.");
  const alpha = descriptor & 15;
  if (indexed ? depth !== 8 || alpha !== 0 : gray ? ![8, 16].includes(depth) || alpha !== (depth === 16 ? 8 : 0) : ![24, 32].includes(depth) || ![0, depth === 32 ? 8 : 0].includes(alpha)) {
    // ponytail: Installed TGALoader inverts 16-bit truecolor alpha; reject until a maintained decoder fixes it.
    invalid("Diese Farbtiefe oder Alpha-Kodierung wird nicht unterstützt.");
  }
  const paletteLength = u16(5);
  if (indexed ? data[1] !== 1 || u16(3) !== 0 || !paletteLength || paletteLength > 256 || data[7] !== 24 : data[1] !== 0 || u16(3) !== 0 || paletteLength !== 0 || data[7] !== 0) invalid("Die Farbpalette wird nicht unterstützt.");
  let payloadEnd = data.length;
  if (data.length >= 26 && new TextDecoder().decode(data.subarray(data.length - 18)) === "TRUEVISION-XFILE.\0") {
    payloadEnd -= 26;
    const footer = new DataView(buffer, payloadEnd, 26);
    // ponytail: Extension/developer areas can redefine alpha; reject until their semantics are supported.
    if (footer.getUint32(0, true) || footer.getUint32(4, true)) invalid("TGA-Erweiterungsbereiche werden noch nicht unterstützt.");
  }
  let offset = 18 + data[0] + (indexed ? paletteLength * 3 : 0);
  if (offset > payloadEnd) invalid("Bildkennung oder Farbpalette ist unvollständig.");
  const size = depth / 8;
  const check = (count: number) => {
    if (offset + count * size > payloadEnd) invalid("Die Pixeldaten sind unvollständig.");
    if (indexed) for (let index = offset; index < offset + count; index++) if (data[index] >= paletteLength) invalid("Ein Farbindex liegt außerhalb der Palette.");
    offset += count * size;
  };
  if (type < 9) check(pixels);
  else for (let remaining = pixels; remaining > 0;) {
    if (offset >= payloadEnd) invalid("Die RLE-Pixeldaten sind unvollständig.");
    const packet = data[offset++], count = (packet & 127) + 1;
    if (count > remaining) invalid("Ein RLE-Paket überschreitet die Bildgröße.");
    check(packet & 128 ? 1 : count);
    remaining -= count;
  }
  const decoded = new TGALoader().parse(buffer);
  if (!(decoded.data instanceof Uint8Array)) invalid("Der Decoder hat keine RGBA-Pixel geliefert.");
  const rgba = decoded.data as Uint8Array;
  if (decoded.width !== width || decoded.height !== height || rgba.length !== pixels * 4) invalid("Der Decoder hat eine ungültige Bildgröße geliefert.");
  if (!alpha) for (let index = 3; index < rgba.length; index += 4) rgba[index] = 255;
  return { width: decoded.width, height: decoded.height, data: rgba };
}
