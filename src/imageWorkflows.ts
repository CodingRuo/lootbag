import type { ModelResource } from "./modelResources";

export type ImageWorkflow = "material" | "spriteSequence";
export type MaterialRole = "baseColor" | "normal" | "roughness" | "metallic" | "ao" | "height";
export const spriteFrameLimit = 256;
export const materialRoles: MaterialRole[] = ["baseColor", "normal", "roughness", "metallic", "ao", "height"];
export type ImageGroupSummary = { workflow: ImageWorkflow; fileId: number; name: string; fileCount: number; errors: string[] };
export const assetKey = (asset: { id: number; imageGroup?: ImageGroupSummary | null } | null) => asset?.imageGroup ? `${asset.imageGroup.workflow}:${asset.imageGroup.fileId}` : `asset:${asset?.id}`;
export type ImageGroupMember = {
  assetId: number;
  file: { id: number; originalFilename: string; available: boolean };
  role: MaterialRole | null;
  frameIndex: number | null;
  resourcePath: string;
};
export type ImageGroup = {
  workflow: ImageWorkflow;
  name: string;
  members: ImageGroupMember[];
  errors: string[];
  resources: { snapshotId: string; fileId: number; format: string; entryPath: string; declaredPaths: string[]; errors: string[]; resources: ModelResource[] };
};
export type MaterialChoices = Partial<Record<MaterialRole, string>>;

export function defaultMaterialChoices(group: ImageGroup): MaterialChoices {
  return Object.fromEntries(materialRoles.map((role) => {
    const members = group.members.filter((member) => member.role === role);
    return [role, members.length === 1 ? members[0].resourcePath : ""];
  }));
}

export function materialSelection(group: ImageGroup, choices: MaterialChoices): string[] {
  const paths: string[] = [];
  for (const role of materialRoles) {
    const members = group.members.filter((member) => member.role === role);
    const choice = choices[role] ?? "";
    if (members.length > 1 && !choice) throw new Error(`Für ${role} eine Map auswählen oder ausdrücklich weglassen.`);
    if (!choice || choice === "none") continue;
    const member = members.find((candidate) => candidate.resourcePath === choice);
    if (!member || !member.file.available || !group.resources.resources.some((resource) => resource.relativePath === choice)) {
      throw new Error(`Material-Map ist nicht verfügbar: ${choice}`);
    }
    paths.push(choice);
  }
  if (!paths.length) throw new Error("Mindestens eine Material-Map auswählen.");
  return paths;
}

export function prepareSpriteSequence<T extends { index: number; path: string; width: number; height: number }>(frames: T[]): T[] {
  if (frames.length > spriteFrameLimit) throw new Error(`Die Sprite-Vorschau unterstützt maximal ${spriteFrameLimit} Bilder pro Sequenz.`);
  if (frames.length < 2) throw new Error("Eine Sprite-Sequenz braucht mindestens zwei Bilder.");
  const ordered = frames.slice().sort((a, b) => a.index - b.index);
  const first = ordered[0];
  for (let index = 0; index < ordered.length; index++) {
    const frame = ordered[index];
    if (!Number.isSafeInteger(frame.index) || frame.index < 0) throw new Error(`Frame-Nummer ist nicht lesbar: ${frame.path}`);
    if (index > 0 && frame.index === ordered[index - 1].index) throw new Error(`Frame-Nummer ${frame.index} ist doppelt: ${frame.path}`);
    if (frame.width < 1 || frame.height < 1 || frame.width !== first.width || frame.height !== first.height) {
      throw new Error(`Unterschiedliche Bildgröße: ${frame.path} (${frame.width} × ${frame.height}, erwartet ${first.width} × ${first.height}).`);
    }
  }
  return ordered;
}

export function imageFit(width: number, height: number, viewportWidth: number, viewportHeight: number) {
  return Math.max(0.01, Math.min(1, (viewportWidth - 16) / width, (viewportHeight - 16) / height));
}

export type SpriteSheetLayout = {
  columns: number; rows: number;
  left: number; top: number; right: number; bottom: number;
  gapX: number; gapY: number;
};

export function spriteSheetFrame(imageWidth: number, imageHeight: number, layout: SpriteSheetLayout, index: number) {
  const { columns, rows, left, top, right, bottom, gapX, gapY } = layout;
  if (![imageWidth, imageHeight, columns, rows, left, top, right, bottom, gapX, gapY, index].every(Number.isSafeInteger)
    || imageWidth < 1 || imageHeight < 1 || columns < 1 || rows < 1
    || [left, top, right, bottom, gapX, gapY, index].some((value) => value < 0)) return null;
  const count = columns * rows;
  if (!Number.isSafeInteger(count) || index >= count) return null;
  const width = (imageWidth - left - right - (columns - 1) * gapX) / columns;
  const height = (imageHeight - top - bottom - (rows - 1) * gapY) / rows;
  if (!Number.isSafeInteger(width) || !Number.isSafeInteger(height) || width < 1 || height < 1) return null;
  const x = left + index % columns * (width + gapX);
  const y = top + Math.floor(index / columns) * (height + gapY);
  if (!Number.isSafeInteger(x) || !Number.isSafeInteger(y) || x + width > imageWidth - right || y + height > imageHeight - bottom) return null;
  return { x, y, width, height, count };
}
