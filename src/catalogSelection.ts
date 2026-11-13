import { assetKey, type ImageGroupSummary, type ImageWorkflow } from "./imageWorkflows.ts";

export type CatalogSelection = { keys: string[]; anchor: string | null };
export type SelectionMode = "single" | "toggle" | "range" | "addRange";

export function catalogSelection(previous: CatalogSelection, visible: string[], key: string, mode: SelectionMode): CatalogSelection {
  const index = visible.indexOf(key);
  if (index < 0) return previous;
  const keys = previous.keys.filter((item) => visible.includes(item));
  const anchor = previous.anchor === null ? -1 : visible.indexOf(previous.anchor);
  if ((mode === "range" || mode === "addRange") && anchor >= 0) {
    const range = visible.slice(Math.min(anchor, index), Math.max(anchor, index) + 1);
    return { keys: mode === "addRange" ? Array.from(new Set([...keys, ...range])) : range, anchor: previous.anchor };
  }
  return {
    keys: mode === "toggle" ? keys.includes(key) ? keys.filter((item) => item !== key) : [...keys, key] : [key],
    anchor: key,
  };
}

export function nextCatalogIndex(index: number, length: number, key: string, columns: number): number | null {
  const steps: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -columns, ArrowDown: columns };
  if (key === "Home") return 0;
  if (key === "End") return Math.max(0, length - 1);
  return key in steps ? Math.max(0, Math.min(length - 1, index + steps[key])) : null;
}

export type CatalogView = "cards" | "list";
const catalogViewKey = (type: string) => `catalogView:${type || "all"}`;

// The chosen catalog view per sidebar type; storage may be missing or throw (private mode), cards stay the default.
export function storedCatalogView(type: string, storage?: Pick<Storage, "getItem">): CatalogView {
  try { return (storage ?? globalThis.localStorage)?.getItem(catalogViewKey(type)) === "list" ? "list" : "cards"; } catch { return "cards"; }
}

export function storeCatalogView(type: string, view: CatalogView, storage?: Pick<Storage, "setItem">) {
  try { (storage ?? globalThis.localStorage)?.setItem(catalogViewKey(type), view); } catch { /* the in-memory choice still applies */ }
}

export type DragSource = { fileId: number; workflow?: ImageWorkflow };
type DragAsset = { id: number; imageGroup?: ImageGroupSummary | null; files: { id: number }[] };

/** A card inside the selection drags every selected asset with all variants; any other card only its default file. */
export function dragSources(asset: DragAsset, selected: DragAsset[]): DragSource[] {
  const group = (item: DragAsset) => item.imageGroup ? [{ fileId: item.imageGroup.fileId, workflow: item.imageGroup.workflow }] : null;
  if (!selected.some((item) => assetKey(item) === assetKey(asset))) return group(asset) ?? [{ fileId: asset.files[0].id }];
  return selected.flatMap((item) => group(item) ?? item.files.map((file) => ({ fileId: file.id })));
}
