export const maxGifBytes = 32 * 1024 * 1024;

/** Validate container work and decoded canvas budgets before browser-native GIF decoding. */
export function preflightGif(buffer: ArrayBuffer) {
  const bytes = new Uint8Array(buffer);
  const fail = (message: string): never => { throw new Error(`GIF-Vorschau: ${message}`); };
  if (bytes.length > maxGifBytes) fail("Die Datei überschreitet 32 MiB.");
  let cursor = 0, blocks = 0;
  const take = (count: number) => {
    if (cursor + count > bytes.length) fail("Die Datei ist unvollständig.");
    const start = cursor; cursor += count; return start;
  };
  const byte = () => bytes[take(1)];
  const word = () => { const offset = take(2); return bytes[offset] | bytes[offset + 1] << 8; };
  const text = (count: number) => String.fromCharCode(...bytes.subarray(take(count), cursor));
  const subBlocks = () => {
    let count = 0;
    while (true) {
      if (++blocks > 100_000) fail("Zu viele Datenblöcke.");
      const size = byte();
      if (!size) return count;
      take(size); count += size;
    }
  };
  const signature = text(6);
  if (signature !== "GIF87a" && signature !== "GIF89a") fail("Kein unterstützter GIF-Header.");
  const width = word(), height = word(), screen = byte(), background = byte(); byte();
  if (!width || !height || width > 8192 || height > 8192 || width * height > 16 * 1024 * 1024) fail("Ungültige Bildgröße oder Pixelbudget überschritten.");
  const globalColors = screen & 128 ? 2 ** ((screen & 7) + 1) : 0;
  if (globalColors) { take(globalColors * 3); if (background >= globalColors) fail("Ungültige Hintergrundfarbe."); }
  let frames = 0, transparent: number | null = null, control = false;
  while (true) {
    if (++blocks > 100_000) fail("Zu viele Datenblöcke.");
    const marker = byte();
    if (marker === 0x3b) {
      if (!frames || control || cursor !== bytes.length) fail("Ungültiges Dateiende.");
      return { width, height, frames, animated: frames > 1 };
    }
    if (marker === 0x21) {
      const label = byte();
      if (label === 0xf9) {
        if (control || byte() !== 4) fail("Ungültiger Animationsblock.");
        const flags = byte(); word(); const color = byte();
        if (byte() !== 0 || flags & 0xe0 || flags & 2 || (flags >> 2 & 7) > 3) fail("Diese Animationssteuerung wird nicht unterstützt.");
        transparent = flags & 1 ? color : null; control = true;
      } else if (label === 0xff) {
        if (byte() !== 11) fail("Ungültiger Anwendungsblock.");
        take(11); subBlocks();
      } else if (label === 0xfe) subBlocks();
      else fail("Diese GIF-Erweiterung wird nicht unterstützt.");
      continue;
    }
    if (marker !== 0x2c) fail("Ungültiger Bildblock.");
    const left = word(), top = word(), frameWidth = word(), frameHeight = word(), flags = byte();
    if (!frameWidth || !frameHeight || left + frameWidth > width || top + frameHeight > height || flags & 0x18) fail("Ungültige Frame-Grenzen.");
    const colors = flags & 128 ? 2 ** ((flags & 7) + 1) : globalColors;
    if (flags & 128) take(colors * 3);
    if (!colors || transparent !== null && transparent >= colors) fail("Ungültige Farbpalette.");
    const codeSize = byte();
    if (codeSize < 2 || codeSize > 8 || !subBlocks()) fail("Ungültige Bilddaten.");
    frames++;
    if (frames > 256 || width * height * frames > 32 * 1024 * 1024) fail("Animationsbudget überschritten: maximal 256 Frames und 32 Megapixel über alle Bildflächen.");
    transparent = null; control = false;
  }
}
