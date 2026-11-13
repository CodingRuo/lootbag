import type { Dispatch, SetStateAction } from "react";
import { isTauri } from "@tauri-apps/api/core";
import type { AudioMetadata } from "./AudioPreview";
import type { ImageGroupSummary } from "./imageWorkflows";
import type { ProjectFolder } from "./ProjectFolders";
import type { FolderMember } from "./projectExport.ts";
import type { PackageInfo } from "./packageInfo.ts";

// Shared catalog types and tiny UI helpers used by App and its regions.
export type AssetPackage = {
  id: number;
  name: string;
  relativePath: string;
  assetCount: number;
  sourcePath?: string;
  sourceKind: "managed" | "external";
  available: boolean;
  externalRoot: string | null;
  // External source that is a ZIP archive used without extracting it.
  zip: boolean;
  // External folder watched for changes (opt-in).
  watched: boolean;
  tags: string[];
  info: PackageInfo;
};
// "<archive>/<entry>" of a file in a ZIP package; catalog paths are "<package path>/<entry>".
export const zipEntryPath = (source: AssetPackage, file: AssetFile) => `${source.externalRoot}/${file.relativePath.slice(source.relativePath.length + 1)}`;
export type AssetKind = "sound" | "image" | "model" | "font";
export type AssetFile = {
  id: number;
  originalFilename: string;
  relativePath: string;
  originalFolder: string;
  format: string;
  kind: AssetKind;
  sizeBytes: number;
  audio: AudioMetadata | null;
  available: boolean;
  // null until the background inspection has classified this model file.
  modelTraits?: { hasMesh: boolean | null; hasAnimation: boolean | null } | null;
};
// Animation-only model files: skeletal clips without any mesh.
export const isAnimationOnly = (file: AssetFile) => file.modelTraits?.hasAnimation === true && file.modelTraits.hasMesh === false;
export type Asset = {
  id: number;
  name: string;
  kind: AssetKind;
  packageId: number;
  packageName: string;
  originalFolder: string;
  files: AssetFile[];
  tags: string[];
  // Derived from file and folder names; shown muted in the inspector only.
  autoTags?: string[];
  folderIds: number[];
  folderMembers: FolderMember[];
  imageGroup?: ImageGroupSummary | null;
  // Active files in other assets with the same content as one of this asset's files.
  duplicates: number;
};
export type LibraryInfo = {
  root: string;
  name: string;
  packages: AssetPackage[];
  folders: ProjectFolder[];
  assetCount: number;
  kindCounts: KindCounts;
};
export type KindCounts = { sound: number; image: number; model: number; font: number; material: number; spriteSequence: number; animation: number };
// Sidebar asset types: "" is every asset; material and spriteSequence are grouped image workflows.
export type CatalogType = "" | keyof KindCounts;
// query_catalog kinds: "animation" selects animation-only model files (their asset kind stays "model").
export type QueryKind = AssetKind | "animation";
export const CATALOG_TYPES: { value: CatalogType; label: string; icon: keyof typeof icons }[] = [
  { value: "", label: "Alle Assets", icon: "library" },
  { value: "sound", label: "Sounds", icon: "sound" },
  { value: "image", label: "Bilder", icon: "image" },
  { value: "spriteSequence", label: "Sprite-Sequenzen", icon: "sprite" },
  { value: "material", label: "Materialien", icon: "material" },
  { value: "model", label: "3D-Modelle", icon: "model" },
  { value: "animation", label: "Animationen", icon: "animation" },
  { value: "font", label: "Schriften", icon: "font" },
];
export type CatalogPage = {
  items: Asset[];
  total: number;
  offset: number;
  limit: number;
};
export type AudioFilters = {
  minDurationSeconds: string;
  maxDurationSeconds: string;
  channels: "any" | "mono" | "stereo" | "multi";
  analysis: "any" | "notAnalysed" | "ready" | "error";
};
export const EMPTY_AUDIO_FILTERS: AudioFilters = { minDurationSeconds: "", maxDurationSeconds: "", channels: "any", analysis: "any" };

export type AudioAnalysisProgress = {
  requestId: string;
  total: number;
  processed: number;
  ready: number;
  errors: number;
  skipped: number;
  cancelled: boolean;
};
export type AudioAnalysis = {
  requestId: string;
  phase: "preparing" | "prepared" | "running" | "completed" | "failed";
  progress: AudioAnalysisProgress | null;
  stopping: boolean;
  total: number;
  message: string | null;
};
export type Notice = { tone: "error" | "success"; message: string };
export type Setter<T> = Dispatch<SetStateAction<T>>;
export type Act = (label: string, operation: (isCurrent: () => boolean) => Promise<void>) => Promise<void>;

export const PAGE_SIZE = 100;
export const native = isTauri();
// macOS has one native picker for folder or ZIP; other systems pick them with separate buttons.
export const mac = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent);
export const ZIP_FILTER = [{ name: "ZIP-Archiv", extensions: ["zip"] }];
export const number = new Intl.NumberFormat("de-DE");
const icons = {
  folder: <path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3Z" />,
  plus: <path d="M12 5v14M5 12h14" />,
  search: (
    <>
      <circle cx="10.5" cy="10.5" r="6.5" />
      <path d="m16 16 5 5" />
    </>
  ),
  sound: (
    <>
      <path d="M4 10h4l5-4v12l-5-4H4Z" />
      <path d="M17 9a5 5 0 0 1 0 6M19 6a9 9 0 0 1 0 12" />
    </>
  ),
  image: <><rect x="3" y="3" width="18" height="18" rx="2" /><circle cx="8" cy="8" r="1.5" /><path d="m3 17 6-6 4 4 3-3 5 5" /></>,
  model: <><path d="m12 3 9 5v8l-9 5-9-5V8Z" /><path d="m3 8 9 5 9-5M12 13v8" /></>,
  material: <><path d="m12 3 9 4.5-9 4.5-9-4.5Z" /><path d="m3 12 9 4.5 9-4.5M3 16.5 12 21l9-4.5" /></>,
  animation: <><circle cx="15" cy="4.5" r="1.6" /><path d="M6 11.5 9.5 8h4l2.5 3h3M13.5 8 11 14l3 3-1 4M11 14l-2 4-4 1" /></>,
  font: <path d="M4 20 10 4h1l6 16M6.5 14h8M17 20h4" />,
  trash: <><path d="M4 7h16M10 11v6M14 11v6" /><path d="m6 7 1 13h10l1-13M9 7V4h6v3" /></>,
  sprite: <><rect x="3" y="5" width="18" height="14" rx="2" /><path d="M9 5v14M15 5v14" /></>,
  play: <path d="M7 4.5v15l12-7.5Z" />,
  pause: <path d="M8 5v14M16 5v14" />,
  grid: <><rect x="3" y="3" width="7" height="7" rx="1" /><rect x="14" y="3" width="7" height="7" rx="1" /><rect x="3" y="14" width="7" height="7" rx="1" /><rect x="14" y="14" width="7" height="7" rx="1" /></>,
  list: <path d="M3 5h2m4 0h12M3 12h2m4 0h12M3 19h2m4 0h12" />,
  filter: <><path d="M3 6h18M3 12h18M3 18h18" /><path d="M8 3v6m8 0v6m-8 0v6" /></>,
  download: <path d="M12 3v12m-5-5 5 5 5-5M4 16v5h16v-5" />,
  arrow: <path d="M5 12h14m-6-6 6 6-6 6" />,
  back: <path d="m14 6-6 6 6 6" />,
  next: <path d="m10 6 6 6-6 6" />,
  first: <path d="M5 5v14m12-13-6 6 6 6" />,
  last: <path d="M19 5v14M7 6l6 6-6 6" />,
  close: <path d="m6 6 12 12M6 18 18 6" />,
  library: (
    <>
      <rect x="3" y="4" width="5" height="16" rx="1" />
      <rect x="10" y="4" width="5" height="16" rx="1" />
      <path d="m17 5 3-1 3 15-3 1Z" />
    </>
  ),
};

export function Icon({ name }: { name: keyof typeof icons }) {
  return (
    <svg
      width="20"
      height="20"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.65"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {icons[name]}
    </svg>
  );
}

export function bytes(value: number) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(units.length - 1, Math.max(0, Math.floor(Math.log(value || 1) / Math.log(1024))));
  return `${new Intl.NumberFormat("de-DE", { maximumFractionDigits: 1 }).format(value / 1024 ** index)} ${units[index]}`;
}
