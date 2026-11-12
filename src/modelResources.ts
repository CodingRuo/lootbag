import { maxBlendBytes } from "./blendThumbnail.ts";

export type ModelResource = { relativePath: string; path: string; sizeBytes: number };

export function resolveModelResource(relative: string, format: string, resources: ModelResource[]) {
  const exact = resources.find((item) => item.relativePath.normalize("NFC") === relative);
  if (exact) return { resource: exact, substituted: false };
  if (format !== "FBX") throw new Error(`Modell-Ressource fehlt oder ist nicht verfügbar: ${relative}`);

  // FBXLoader drops Windows directories; only an unambiguous package-local name is safe.
  const filename = relative.split("/").pop()!.toLowerCase();
  const basename = (item: ModelResource) => item.relativePath.split("/").pop()!.normalize("NFC").toLowerCase();
  let matches = resources.filter((item) => basename(item) === filename);
  if (matches.length > 1) throw new Error(`FBX-Textur ist nicht eindeutig: ${filename}`);
  if (matches.length === 1) return { resource: matches[0], substituted: false };

  // ponytail: PSD replacements are preview-only; source repair needs an explicit export design.
  if (filename.endsWith(".psd")) {
    const stem = filename.slice(0, -4);
    matches = resources.filter((item) => {
      const name = basename(item);
      return /\.(png|jpe?g|webp)$/.test(name) && name.replace(/\.[^.]+$/, "") === stem;
    });
    if (matches.length > 1) throw new Error(`FBX-Ersatztextur ist nicht eindeutig: ${filename}`);
    if (matches.length === 1) return { resource: matches[0], substituted: true };
  }
  if (/\.(psd|png|jpe?g|webp|tga|gif|ase|aseprite)$/.test(filename)) return { resource: null, substituted: false, missingTexture: relative };
  throw new Error(`Modell-Ressource fehlt oder ist nicht verfügbar: ${relative}`);
}

export function requiresModelBundle(file: { kind: string; format: string }) {
  return file.kind === "model" && file.format !== "BLEND";
}

export function blendEntryResource(snapshot: { format: string; entryPath: string; resources: ModelResource[]; declaredPaths: string[] }) {
  const entry = snapshot.resources[0];
  if (snapshot.format !== "BLEND" || snapshot.resources.length !== 1 || snapshot.declaredPaths.length || entry?.relativePath !== snapshot.entryPath) {
    throw new Error("Blender-Vorschau: Keine eindeutig vorbereitete Originaldatei verfügbar.");
  }
  if (!Number.isSafeInteger(entry.sizeBytes) || entry.sizeBytes < 0 || entry.sizeBytes > maxBlendBytes) throw new Error("Blender-Vorschau: Die Originaldatei überschreitet 64 MiB oder ihre Größe ist ungültig.");
  return entry;
}
