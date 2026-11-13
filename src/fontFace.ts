// Every preview gets its own generated family, so fonts that share a name never collide.
export const MAX_FONT_BYTES = 32 * 1024 * 1024;
export const MAX_LOADED_FONTS = 64;
type FontSet = { add(face: FontFace): unknown; delete(face: FontFace): unknown };
type CreateFace = (family: string, source: string) => FontFace;

let generated = 0;
const loaded = new Set<FontFace>();

export async function loadFontFace(url: string, sizeBytes: number,
  fonts: FontSet = document.fonts, create: CreateFace = (family, source) => new FontFace(family, source)) {
  if (sizeBytes > MAX_FONT_BYTES) throw new Error(`Schrift größer als ${MAX_FONT_BYTES / 1024 / 1024} MiB.`);
  if (loaded.size >= MAX_LOADED_FONTS) throw new Error("Zu viele Schriften gleichzeitig geladen.");
  const family = `lootbag-font-${++generated}`;
  const face = create(family, `url(${JSON.stringify(url)})`);
  loaded.add(face);
  const release = () => { if (loaded.delete(face)) fonts.delete(face); };
  try { await face.load(); } catch { release(); throw new Error("Schrift nicht lesbar."); }
  fonts.add(face);
  return { family, release };
}

export const loadedFontCount = () => loaded.size;
