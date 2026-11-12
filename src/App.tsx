import { useEffect, useRef, useState } from "react";
import { Channel, invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import AnnotationEditor from "./AnnotationEditor";
import AudioPreview, { type AudioMetadata } from "./AudioPreview";
import ImagePreview, { ImageThumbnail } from "./ImagePreview";
import BlendPreview from "./BlendPreview.tsx";
import { requiresModelBundle } from "./modelResources.ts";
import ModelPreview, { type ModelExportSelection } from "./ModelPreview";
import ModelThumbnail, { createCatalogModelThumbnails } from "./ModelThumbnail";
import ImageGroupPreview from "./ImageGroupPreview";
import { assetKey, type ImageGroupSummary, type ImageWorkflow } from "./imageWorkflows";
import VariantEditor from "./VariantEditor";
import { TrashActions, TrashDialog, type TrashEntry } from "./Trash";
import { FolderDialog, FolderMembershipEditor, folderPath, type ProjectFolder } from "./ProjectFolders";
import { nextInspectorTab, type InspectorTab } from "./inspectorTabs";
import { catalogSelection, nextCatalogIndex, type CatalogSelection, type SelectionMode } from "./catalogSelection";
import { errorMessage } from "./errorMessage.ts";

type AssetPackage = {
  id: number;
  name: string;
  relativePath: string;
  assetCount: number;
  sourcePath?: string;
  sourceKind: "managed" | "external";
  available: boolean;
  externalRoot: string | null;
  tags: string[];
};
type AssetKind = "sound" | "image" | "model";
type AssetFile = {
  id: number;
  originalFilename: string;
  relativePath: string;
  originalFolder: string;
  format: string;
  kind: AssetKind;
  sizeBytes: number;
  audio: AudioMetadata | null;
  available: boolean;
};
type Asset = {
  id: number;
  name: string;
  kind: AssetKind;
  packageId: number;
  packageName: string;
  originalFolder: string;
  files: AssetFile[];
  tags: string[];
  folderIds: number[];
  imageGroup?: ImageGroupSummary | null;
};
type LibraryInfo = {
  root: string;
  name: string;
  packages: AssetPackage[];
  folders: ProjectFolder[];
  assetCount: number;
};
type CatalogPage = {
  items: Asset[];
  total: number;
  offset: number;
  limit: number;
};
type AudioFilters = {
  minDurationSeconds: string;
  maxDurationSeconds: string;
  channels: "any" | "mono" | "stereo" | "multi";
  analysis: "any" | "notAnalysed" | "ready" | "error";
};
const EMPTY_AUDIO_FILTERS: AudioFilters = { minDurationSeconds: "", maxDurationSeconds: "", channels: "any", analysis: "any" };

type AudioAnalysisProgress = {
  requestId: string;
  total: number;
  processed: number;
  ready: number;
  errors: number;
  skipped: number;
  cancelled: boolean;
};
type AudioAnalysis = {
  requestId: string;
  phase: "preparing" | "prepared" | "running" | "completed" | "failed";
  progress: AudioAnalysisProgress | null;
  stopping: boolean;
  total: number;
  message: string | null;
};

type CatalogFilters = {
  folders: string[];
  formats: string[];
  tags: string[];
};
type ExportResult =
  | { status: "cancelled" | "replacementRequired" }
  | { status: "exported"; path: string; sizeBytes: number };
type ZipEstimate = {
  uncompressedBytes: number;
  requiredBytes: number;
  availableBytes: number;
  entryCount: number;
  hasSpace: boolean;
};
type ImportSource = { path: string; isZip: boolean };
type ImportProgress = {
  stage: "preparing" | "copying" | "cataloging" | "completed" | "cancelled" | "failed";
  completedBytes: number;
  totalBytes: number;
  completedEntries: number;
  totalEntries: number;
  currentPath: string | null;
};
type ImportOwner = { root: string; generation: number; stopped: boolean; imported: number; skipped: number; cancellationError: string | null };
type EmptiedTrash = { packages: number; files: number; diskErrors: string[] };
type Notice = { tone: "error" | "success"; message: string };
type BulkAction = { kind: "addTags"; tags: string[] } | { kind: "addToFolder"; folderId: number } | { kind: "trash" };
type BulkResult = { assetCount: number; fileCount: number };

const PAGE_SIZE = 40;
// Larger models are only parsed on demand in the inspector; BLEND thumbnails have their own 64 MiB cap.
const MAX_MODEL_THUMBNAIL_BYTES = 32 * 1024 * 1024;
const native = isTauri();
const number = new Intl.NumberFormat("de-DE");
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

function Icon({ name }: { name: keyof typeof icons }) {
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

function bytes(value: number) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  const index = Math.min(units.length - 1, Math.max(0, Math.floor(Math.log(value || 1) / Math.log(1024))));
  return `${new Intl.NumberFormat("de-DE", { maximumFractionDigits: 1 }).format(value / 1024 ** index)} ${units[index]}`;
}


export default function App() {
  const [library, setLibrary] = useState<LibraryInfo | null>(null);
  const [busy, setBusy] = useState<string | null>(
    native ? "Bibliothek wird geladen …" : null,
  );
  const [notice, setNotice] = useState<Notice | null>(
    native
      ? null
      : {
          tone: "error",
          message:
            "Starte die Desktop-App, um deine lokale Bibliothek zu verwenden.",
        },
  );
  const [importProgress, setImportProgress] = useState<ImportProgress | null>(null);
  const [importActive, setImportActive] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [text, setText] = useState("");
  const [packageId, setPackageId] = useState<number | null>(null);
  const [folderId, setFolderId] = useState<number | null>(null);
  const [showFolderDialog, setShowFolderDialog] = useState(false);
  const [originalFolder, setOriginalFolder] = useState<string | null>(null);
  const [format, setFormat] = useState<string | null>(null);
  const [kind, setKind] = useState<AssetKind | null>(null);
  const [workflow, setWorkflow] = useState<ImageWorkflow | null>(null);
  const [audioResultsStale, setAudioResultsStale] = useState(false);
  const [audioAnalysis, setAudioAnalysis] = useState<AudioAnalysis | null>(null);
  const audioAnalysisOwner = useRef<{ requestId: string; scope: string } | null>(null);
  const [audioFilters, setAudioFilters] = useState(EMPTY_AUDIO_FILTERS);
  const audioFilterCount = [audioFilters.minDurationSeconds !== "", audioFilters.maxDurationSeconds !== "", audioFilters.channels !== "any", audioFilters.analysis !== "any"].filter(Boolean).length;
  const audioCriteria = audioFilterCount ? {
    minDurationSeconds: audioFilters.minDurationSeconds === "" ? null : Number(audioFilters.minDurationSeconds),
    maxDurationSeconds: audioFilters.maxDurationSeconds === "" ? null : Number(audioFilters.maxDurationSeconds),
    channels: audioFilters.channels,
    analysis: audioFilters.analysis,
  } : null;
  const audioFilterError = audioCriteria && [audioCriteria.minDurationSeconds, audioCriteria.maxDurationSeconds].some((value) => value !== null && (!Number.isFinite(value) || value < 0))
    ? "Dauer muss eine endliche, nichtnegative Zahl sein."
    : audioCriteria?.minDurationSeconds != null && audioCriteria.maxDurationSeconds != null && audioCriteria.minDurationSeconds > audioCriteria.maxDurationSeconds
      ? "Dauer: Minimum darf nicht größer als Maximum sein." : null;
  const [tag, setTag] = useState<string | null>(null);
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [catalogView, setCatalogView] = useState<"cards" | "list">("cards");
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("preview");
  const [filters, setFilters] = useState<CatalogFilters>({
    folders: [],
    formats: [],
    tags: [],
  });
  const [offset, setOffset] = useState(0);
  const [revision, setRevision] = useState(0);
  const [modelThumbnails] = useState(createCatalogModelThumbnails);
  const thumbnailScope = library?.root ?? "";
  const [page, setPage] = useState<CatalogPage | null>(null);
  const [queryLoading, setQueryLoading] = useState(false);
  const [queryError, setQueryError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Asset | null>(null);
  const [selection, setSelection] = useState<CatalogSelection>({ keys: [], anchor: null });
  const [bulkTags, setBulkTags] = useState("");
  const [bulkFolderId, setBulkFolderId] = useState<number | null>(null);
  const [dropHover, setDropHover] = useState(false);
  const dropImport = useRef<(paths: string[]) => void>(() => {});
  const operationRunning = useRef(false);
  const operationGeneration = useRef(0);
  const importOwner = useRef<ImportOwner | null>(null);
  const mounted = useRef(true);
  const currentLibraryRoot = useRef(library?.root);
  currentLibraryRoot.current = library?.root;
  const [showTrash, setShowTrash] = useState(false);
  const trashGeneration = useRef(0);
  const renderedTrashScope = trashGeneration.current;
  const [selectedFileId, setSelectedFileId] = useState<number | null>(null);
  const [modelSelection, setModelSelection] = useState<ModelExportSelection | null>(null);
  const focusAssetId = useRef<number | null>(null);
  const focusFileId = useRef<number | null>(null);
  const file = selected?.files.find((item) => item.id === selected?.imageGroup?.fileId)
    ?? selected?.files.find((item) => item.id === selectedFileId)
    ?? (audioFilterCount ? selected?.files[0] : selected?.files.find((item) => item.format === (format ?? "WAV")))
    ?? selected?.files[0] ?? null;
  const selectedPackage = library?.packages.find(
    (item) => item.id === selected?.packageId,
  );
  const filteredPackage = library?.packages.find(
    (item) => item.id === packageId,
  );
  const projectFolders = (library?.folders ?? []).slice().sort((a, b) =>
    folderPath(a.id, library?.folders ?? []).localeCompare(folderPath(b.id, library?.folders ?? []), "de"));
  const filteredFolderPath = folderId === null ? null : folderPath(folderId, projectFolders);
  const actionsDisabled = !native || busy !== null;
  const activeFilterCount = [originalFolder, format, tag].filter((value) => value !== null).length + audioFilterCount;
  const selectionScope = JSON.stringify([library?.root, text, packageId, folderId, originalFolder, format, kind, workflow, tag, audioFilters, offset]);
  const audioAnalysisScope = JSON.stringify([library?.root, text, packageId, folderId, originalFolder, format, kind, workflow, tag, audioFilters, revision]);
  const currentAudioAnalysisScope = useRef(audioAnalysisScope);
  currentAudioAnalysisScope.current = audioAnalysisScope;
  const audioAnalysisScopeLabel = [
    filteredFolderPath ? `Projektordner: ${filteredFolderPath} (mit Unterordnern)` : filteredPackage ? `Paket: ${filteredPackage.name}` : "Gesamte Bibliothek",
    text && `Suche: „${text}“`,
    originalFolder !== null && `Originalordner: ${originalFolder || "Paketwurzel"}`,
    format && `Dateityp: ${format}`,
    tag && `Tag: ${tag}`,
    audioFilterCount > 0 && `Audiofilter: ${[
      audioFilters.minDurationSeconds !== "" && `ab ${audioFilters.minDurationSeconds} s`,
      audioFilters.maxDurationSeconds !== "" && `bis ${audioFilters.maxDurationSeconds} s`,
      audioFilters.channels !== "any" && ({ mono: "Mono", stereo: "Stereo", multi: "mehr als 2 Kanäle" }[audioFilters.channels]),
      audioFilters.analysis !== "any" && ({ notAnalysed: "nicht analysiert", ready: "Analyse vorhanden", error: "Analysefehler" }[audioFilters.analysis]),
    ].filter(Boolean).join(", ")}`,
  ].filter(Boolean).join(" · ");
  const visibleKeys = (page?.items ?? []).map(assetKey);
  const selectedAssets = (page?.items ?? []).filter((asset) => selection.keys.includes(assetKey(asset)));
  const bulkDisabled = actionsDisabled || queryLoading || audioFilterError !== null || selectedAssets.length === 0;

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      const owner = importOwner.current; importOwner.current = null;
      if (owner) abandonImport(owner);
    };
  }, []);

  useEffect(() => {
    setAudioAnalysis(null);
    return () => {
      const owner = audioAnalysisOwner.current;
      if (owner?.scope === audioAnalysisScope) {
        audioAnalysisOwner.current = null;
        void invoke("cancel_audio_analysis", { requestId: owner.requestId }).catch(() => {});
      }
    };
  }, [audioAnalysisScope]);

  useEffect(() => {
    setSelection({ keys: [], anchor: null });
    setSelectedFileId(null);
    setAudioResultsStale(false);
  }, [selectionScope]);
  useEffect(() => {
    if (!native) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (!active) return;
      setDropHover(event.payload.type === "enter" || event.payload.type === "over");
      if (event.payload.type === "drop") dropImport.current(event.payload.paths);
    }).then((stop) => { if (active) unlisten = stop; else stop(); })
      .catch((error: unknown) => { if (active) setNotice({ tone: "error", message: `Datei-Drop nicht verfügbar: ${errorMessage(error)}` }); });
    return () => { active = false; unlisten?.(); };
  }, []);

  useEffect(() => { modelThumbnails.reset(thumbnailScope); }, [modelThumbnails, thumbnailScope]);
  useEffect(() => () => modelThumbnails.dispose(), [modelThumbnails]);

  useEffect(() => {
    if (!native) return;
    let active = true;
    invoke<LibraryInfo | null>("get_library")
      .then((info) => {
        if (active) setLibrary(info);
      })
      .catch((error: unknown) => {
        if (active) setNotice({ tone: "error", message: errorMessage(error) });
      })
      .finally(() => {
        if (active) setBusy(null);
      });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    if (!library) return;
    if (audioFilterError) {
      setQueryLoading(false);
      setQueryError(audioFilterError);
      return;
    }
    let active = true;
    setQueryLoading(true);
    setQueryError(null);
    const timer = window.setTimeout(() => {
      invoke<CatalogPage>("query_catalog", {
        audioCriteria,
        query: {
          text,
          packageId,
          folderId,
          originalFolder,
          format,
          kind,
          workflow,
          tag,
          offset,
          limit: PAGE_SIZE,
        },
      })
        .then((result) => {
          if (!active) return;
          setPage(result);
          setOffset(result.offset);
          setSelection((previous) => ({ keys: previous.keys.filter((key) => result.items.some((item) => assetKey(item) === key)), anchor: previous.anchor }));
          const focused = focusAssetId.current;
          const focusedFile = focusFileId.current;
          focusAssetId.current = null;
          focusFileId.current = null;
          setSelected(
            (previous) =>
              result.items.find((item) => focusedFile !== null && (item.imageGroup ? item.imageGroup.fileId === focusedFile : item.files.some((entry) => entry.id === focusedFile))) ??
              result.items.find((item) => item.id === focused) ??
              result.items.find((item) => assetKey(item) === assetKey(previous)) ??
              result.items[0] ??
              null,
          );
        })
        .catch((error: unknown) => {
          if (active) setQueryError(errorMessage(error));
        })
        .finally(() => {
          if (active) setQueryLoading(false);
        });
    }, 160);
    return () => {
      active = false;
      window.clearTimeout(timer);
    };
  }, [library?.root, text, packageId, folderId, originalFolder, format, kind, workflow, tag, audioFilters, offset, revision]);

  useEffect(() => { setModelSelection(null); }, [library?.root, file?.id, file?.available, selected?.imageGroup?.workflow, selectedPackage?.externalRoot]);
  useEffect(() => { setInspectorTab("preview"); }, [assetKey(selected)]);

  useEffect(() => {
    if (!library) return;
    let active = true;
    invoke<CatalogFilters>("catalog_filters", { packageId })
      .then((result) => {
        if (active) setFilters(result);
      })
      .catch((error: unknown) => {
        if (active) setNotice({ tone: "error", message: errorMessage(error) });
      });
    return () => {
      active = false;
    };
  }, [library?.root, packageId, revision]);

  async function act(label: string, operation: (isCurrent: () => boolean) => Promise<void>) {
    if (actionsDisabled || operationRunning.current) return;
    const generation = ++operationGeneration.current;
    const isCurrent = () => mounted.current && operationGeneration.current === generation;
    operationRunning.current = true;
    setBusy(label);
    setNotice(null);
    try {
      await operation(isCurrent);
    } catch (error: unknown) {
      if (isCurrent()) setNotice({ tone: "error", message: errorMessage(error) });
    } finally {
      if (isCurrent()) { operationRunning.current = false; setBusy(null); }
    }
  }

  function ownsAudioAnalysis(owner: { requestId: string; scope: string }) {
    return audioAnalysisOwner.current === owner && currentAudioAnalysisScope.current === owner.scope;
  }

  async function prepareAudioAnalysis() {
    if (!library || kind !== "sound" || actionsDisabled || audioFilterError || operationRunning.current) return;
    const previous = audioAnalysisOwner.current;
    if (previous) void invoke("cancel_audio_analysis", { requestId: previous.requestId }).catch(() => {});
    const owner = { requestId: crypto.randomUUID(), scope: audioAnalysisScope };
    audioAnalysisOwner.current = owner;
    setAudioAnalysis({ requestId: owner.requestId, phase: "preparing", total: 0, progress: null, stopping: false, message: null });
    try {
      const prepared = await invoke<{ requestId: string; total: number }>("prepare_audio_analysis", {
        requestId: owner.requestId,
        audioCriteria,
        query: { text, packageId, folderId, originalFolder, format, kind, workflow, tag, offset: 0, limit: PAGE_SIZE },
      });
      if (!ownsAudioAnalysis(owner)) return;
      if (prepared.requestId !== owner.requestId) throw new Error("Die Analysevorbereitung gehört nicht zum aktuellen Auftrag.");
      setAudioAnalysis({ requestId: owner.requestId, phase: "prepared", total: prepared.total, progress: null, stopping: false, message: null });
    } catch (error: unknown) {
      if (ownsAudioAnalysis(owner)) setAudioAnalysis({ requestId: owner.requestId, phase: "failed", total: 0, progress: null, stopping: false, message: errorMessage(error) });
    }
  }

  function startAudioAnalysis() {
    const owner = audioAnalysisOwner.current;
    const prepared = audioAnalysis;
    if (!owner || !ownsAudioAnalysis(owner) || prepared?.phase !== "prepared" || prepared.requestId !== owner.requestId || prepared.total === 0) return;
    void act("Sounds im Suchbereich werden analysiert …", async () => {
      const initial: AudioAnalysisProgress = { requestId: owner.requestId, total: prepared.total, processed: 0, ready: 0, errors: 0, skipped: 0, cancelled: false };
      setAudioAnalysis({ ...prepared, phase: "running", progress: initial });
      const progress = new Channel<AudioAnalysisProgress>();
      let receiving = true;
      progress.onmessage = (update) => {
        if (receiving && ownsAudioAnalysis(owner) && update.requestId === owner.requestId) {
          setAudioAnalysis((previous) => previous?.requestId === owner.requestId ? { ...previous, progress: update } : previous);
        }
      };
      try {
        const result = await invoke<AudioAnalysisProgress>("run_audio_analysis", { requestId: owner.requestId, progress });
        if (!ownsAudioAnalysis(owner)) return;
        if (result.requestId !== owner.requestId) throw new Error("Das Analyseergebnis gehört nicht zum aktuellen Auftrag.");
        setAudioAnalysis({ ...prepared, phase: "completed", progress: result, stopping: false });
        if (result.ready + result.errors > 0) setAudioResultsStale(true);
      } catch (error: unknown) {
        if (ownsAudioAnalysis(owner)) {
          setAudioAnalysis((previous) => previous ? { ...previous, phase: "failed", stopping: false, message: errorMessage(error) } : previous);
          setAudioResultsStale(true);
        }
      } finally {
        receiving = false;
      }
    });
  }

  async function stopAudioAnalysis() {
    const owner = audioAnalysisOwner.current;
    if (!owner || !ownsAudioAnalysis(owner) || audioAnalysis?.phase !== "running" || audioAnalysis.stopping) return;
    setAudioAnalysis((previous) => previous ? { ...previous, stopping: true, message: null } : previous);
    try {
      await invoke("cancel_audio_analysis", { requestId: owner.requestId });
    } catch (error: unknown) {
      if (ownsAudioAnalysis(owner)) setAudioAnalysis((previous) => previous?.phase === "running" ? { ...previous, stopping: false, message: `Abbruch nicht möglich: ${errorMessage(error)}` } : previous);
    }
  }

  function refreshAudioResults() {
    setAudioResultsStale(false);
    setSelected(null);
    setSelectedFileId(null);
    setModelSelection(null);
    setRevision((value) => value + 1);
  }

  async function discardAudioAnalysis() {
    const owner = audioAnalysisOwner.current;
    if (!owner) return;
    audioAnalysisOwner.current = null;
    setAudioAnalysis(null);
    try {
      await invoke("cancel_audio_analysis", { requestId: owner.requestId });
    } catch (error: unknown) {
      if (currentAudioAnalysisScope.current === owner.scope && audioAnalysisOwner.current === null) setNotice({ tone: "error", message: errorMessage(error) });
    }
  }

  function useLibrary(info: LibraryInfo) {
    trashGeneration.current++;
    const owner = importOwner.current; importOwner.current = null;
    if (owner) abandonImport(owner);
    currentLibraryRoot.current = info.root;
    operationGeneration.current++; operationRunning.current = false; setBusy(null);
    setImportProgress(null); setImportActive(false); setCancelling(false);
    setSelected(null);
    setSelection({ keys: [], anchor: null });
    setSelectedFileId(null);
    setModelSelection(null);
    setShowTrash(false);
    setShowFolderDialog(false);
    setFolderId(null);
    focusAssetId.current = null;
    focusFileId.current = null;
    setPage(null);
    setText("");
    setPackageId(null);
    clearFilters();
    setOffset(0);
    setLibrary(info);
    setRevision((value) => value + 1);
  }

  function createLibrary(chooseLocation: boolean) {
    void act("Bibliothek wird angelegt …", async (isCurrent) => {
      const root = chooseLocation
        ? await open({
            directory: true,
            multiple: false,
            title: "Leeren Ordner für die Bibliothek auswählen",
          })
        : null;
      if (!isCurrent() || (chooseLocation && !root)) return;
      const info = await invoke<LibraryInfo>("create_library", { root });
      if (isCurrent()) useLibrary(info);
    });
  }

  function openLibrary() {
    void act("Bibliothek wird geöffnet …", async (isCurrent) => {
      const root = await open({
        directory: true,
        multiple: false,
        title: "Bestehende Bibliothek öffnen",
      });
      if (!isCurrent() || !root) return;
      const info = await invoke<LibraryInfo>("open_library", { root });
      if (isCurrent()) useLibrary(info);
    });
  }

  function importPackage() {
    beginImports("Paket zum Importieren auswählen …", async () => {
      const selected = await invoke<ImportSource | null>("select_import_source");
      return selected ? [selected] : [];
    });
  }

  function ownsImport(owner: ImportOwner) {
    return mounted.current && importOwner.current === owner && operationGeneration.current === owner.generation
      && currentLibraryRoot.current === owner.root;
  }
  function abandonImport(owner: ImportOwner) {
    owner.stopped = true;
    void invoke<boolean>("cancel_import").catch(() => {});
  }
  function beginImports(label: string, sources: () => Promise<ImportSource[]>) {
    if (!library || actionsDisabled || operationRunning.current) return;
    const owner: ImportOwner = { root: library.root, generation: ++operationGeneration.current, stopped: false, imported: 0, skipped: 0, cancellationError: null };
    importOwner.current = owner; operationRunning.current = true; setBusy(label); setNotice(null); setImportActive(true); setCancelling(false);
    void (async () => {
      try {
        const selected = await sources();
        if (!ownsImport(owner)) return;
        if (owner.stopped) { setNotice({ tone: "success", message: "Import abgebrochen. Es wurde kein Paket importiert." }); return; }
        for (const source of selected) {
          if (!ownsImport(owner) || owner.stopped) break;
          try { await importSource(owner, source); }
          catch (error: unknown) { throw new Error(`${source.path}: ${errorMessage(error)}`); }
        }
        if (ownsImport(owner) && owner.stopped) setNotice({ tone: owner.cancellationError ? "error" : "success", message: `Import abgebrochen. ${owner.imported} importierte Pakete bleiben erhalten.${owner.cancellationError ? ` Abbruchbestätigung fehlgeschlagen: ${owner.cancellationError}` : ""}` });
        else if (ownsImport(owner) && (selected.length > 1 || owner.skipped)) setNotice({ tone: "success", message: `${owner.imported} Pakete importiert${owner.skipped ? ` · ${owner.skipped} ganze Pakete übersprungen` : ""}.` });
      } catch (error: unknown) {
        if (ownsImport(owner)) setNotice({ tone: "error", message: `${errorMessage(error)}${owner.cancellationError ? ` Abbruchbestätigung fehlgeschlagen: ${owner.cancellationError}` : ""}${owner.imported ? ` ${owner.imported} vorherige Pakete bleiben importiert.` : ""} Verbleibende Quellen wurden nicht importiert.` });
      } finally {
        if (ownsImport(owner)) { importOwner.current = null; operationRunning.current = false; setBusy(null); setImportActive(false); setImportProgress(null); setCancelling(false); }
      }
    })();
  }
  async function importSource(owner: ImportOwner, source: ImportSource) {
    if (source.isZip) {
      setBusy("ZIP-Paket wird geprüft …");
      const estimate = await invoke<ZipEstimate>("estimate_zip", { source: source.path });
      if (!ownsImport(owner) || owner.stopped) return;
      if (!estimate.hasSpace) throw new Error(`Nicht genug Speicherplatz: ${bytes(estimate.requiredBytes)} mit Reserve benötigt, ${bytes(estimate.availableBytes)} verfügbar.`);
      setBusy("ZIP-Paket wartet auf die Speicherbestätigung …");
      const proceed = await confirm(
        `${number.format(estimate.entryCount)} ZIP-Einträge werden vollständig entpackt.\n\nInhalt: ${bytes(estimate.uncompressedBytes)}\nMit Speicherreserve: ${bytes(estimate.requiredBytes)}\nAm Bibliotheksort verfügbar: ${bytes(estimate.availableBytes)}\n\nDas Originalarchiv bleibt erhalten.`,
        { title: "ZIP-Paket importieren?", okLabel: "Importieren", cancelLabel: "Abbrechen" },
      );
      if (!ownsImport(owner) || owner.stopped) return;
      if (!proceed) { owner.skipped++; return; }
    }
    setBusy("Paket wird in die Bibliothek kopiert …");
    const onProgress = new Channel<ImportProgress>();
    onProgress.onmessage = (value) => {
      // A cancel that arrived before the native import registered its control is re-sent once progress starts.
      if (owner.stopped) void invoke<boolean>("cancel_import").catch(() => {});
      else if (ownsImport(owner)) setImportProgress(value);
    };
    try {
      const imported = await invoke<AssetPackage>(source.isZip ? "import_zip" : "import_folder", { source: source.path, onProgress });
      if (!ownsImport(owner)) return;
      owner.imported++;
      const info = await invoke<LibraryInfo>("get_library");
      if (!ownsImport(owner)) return;
      if (info.root !== owner.root) throw new Error("Die aktive Bibliothek wurde geändert.");
      setLibrary(info); setRevision((value) => value + 1);
      setNotice({ tone: "success", message: `${imported.name} importiert · ${number.format(imported.assetCount)} ${imported.assetCount === 1 ? "Asset" : "Assets"}.` });
    } catch (error: unknown) {
      if (owner.stopped) return;
      throw error;
    } finally {
      if (ownsImport(owner)) setImportProgress(null);
    }
  }

  dropImport.current = (paths) => {
    if (!library || actionsDisabled || operationRunning.current) {
      setNotice({ tone: "error", message: library ? "Bitte den laufenden Vorgang abwarten und danach erneut ablegen." : "Bitte zuerst eine Bibliothek öffnen oder anlegen." });
      return;
    }
    beginImports("Abgelegte Importquellen werden geprüft …", () => invoke<ImportSource[]>("dropped_import_sources", { sources: paths }));
  };

  async function cancelImport() {
    const owner = importOwner.current;
    if (!owner || !ownsImport(owner) || owner.stopped) return;
    owner.stopped = true; setCancelling(true);
    try {
      await invoke<boolean>("cancel_import");
      owner.cancellationError = null;
    } catch (error: unknown) {
      owner.cancellationError = errorMessage(error);
      if (ownsImport(owner)) setCancelling(false);
    }
  }

  function exportFile() {
    if (!file) return;
    const selectedFile = file;
    const selection = modelSelection;
    if (requiresModelBundle(selectedFile) || selected?.imageGroup) {
      if (!selection || selection.fileId !== selectedFile.id) return;
      const group = selected?.imageGroup;
      const label = group?.workflow === "material" ? "Material-Set" : group ? "Sprite-Sequenz" : "Modellpaket";
      void act(`${label} wird exportiert …`, async () => {
        const parent = await open({ directory: true, multiple: false, title: `Projektordner für ${label} auswählen` });
        if (!parent) return;
        const stem = (group?.name ?? selectedFile.originalFilename.replace(/\.[^.]+$/, "")).replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/[. ]+$/g, "").slice(0, 128) || "Asset";
        const suffix = group?.workflow === "material" ? "material" : group ? "sprites" : "model";
        const result = await invoke<ExportResult>("export_model", {
          snapshotId: selection.snapshotId,
          fileId: selectedFile.id,
          resourcePaths: selection.resourcePaths,
          destination: `${parent.replace(/[\\/]+$/g, "")}/${stem}-${suffix}`,
        });
        if (result.status === "exported") setNotice({ tone: "success", message: `${label} exportiert: ${result.path}` });
      });
      return;
    }
    void act("Asset wird exportiert …", async () => {
      const destination = await save({
        title: selectedFile.format === "BLEND" ? "Nur die Blender-Originaldatei exportieren" : "Asset in dein Spielprojekt exportieren",
        defaultPath: selectedFile.originalFilename,
        filters: [{ name: `${selectedFile.format}-Datei`, extensions: selectedFile.format === "JPEG" ? ["jpg", "jpeg"] : selectedFile.format === "ASEPRITE" ? ["ase", "aseprite"] : [selectedFile.format.toLowerCase()] }],
      });
      let result = await invoke<ExportResult>("export_asset", {
        fileId: selectedFile.id,
        destination,
        overwrite: false,
      });
      if (result.status === "replacementRequired") {
        const replace = await confirm(
          `Am gewählten Ziel existiert bereits eine Datei. Möchtest du sie durch ${selectedFile.originalFilename} ersetzen?`,
          {
            title: "Datei ersetzen?",
            kind: "warning",
            okLabel: "Ersetzen",
            cancelLabel: "Abbrechen",
          },
        );
        if (!replace) return;
        result = await invoke<ExportResult>("export_asset", {
          fileId: selectedFile.id,
          destination,
          overwrite: true,
        });
      }
      if (result.status === "exported")
        setNotice({ tone: "success", message: `Exportiert: ${result.path}` });
    });
  }

  function referenceFolder() {
    void act("Externe Quelle wird eingelesen …", async () => {
      const source = await open({ directory: true, multiple: false, title: "Externen Asset-Ordner einbinden" });
      if (!source) return;
      const referenced = await invoke<AssetPackage>("reference_folder", { source });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setPackageId(referenced.id);
      setFolderId(null);
      clearFilters();
      setText("");
      setOffset(0);
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: `${referenced.name} extern eingebunden · ${number.format(referenced.assetCount)} Assets. Die Dateien bleiben im Quellordner.` });
    });
  }

  function refreshSources() {
    void act("Quellen werden geprüft …", async () => {
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
    });
  }

  function relinkSource(sourcePackage: AssetPackage) {
    void act("Quelle wird neu verknüpft …", async () => {
      const source = await open({ directory: true, multiple: false, title: "Verschobenen ursprünglichen Quellordner auswählen" });
      if (!source) return;
      await invoke<AssetPackage>("relink_source", { packageId: sourcePackage.id, source });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: `${sourcePackage.name} neu verknüpft. Katalog und Annotationen bleiben erhalten.` });
    });
  }

  function changeAudioFilters(next: AudioFilters) {
    setAudioFilters(next);
    setSelected(null);
    setSelectedFileId(null);
    setModelSelection(null);
    setOffset(0);
    if (next.minDurationSeconds !== "" || next.maxDurationSeconds !== "" || next.channels !== "any" || next.analysis !== "any") {
      if (format !== null && !["WAV", "MP3", "OGG", "FLAC"].includes(format)) setFormat(null);
      setKind("sound");
      setWorkflow(null);
    }
  }

  function clearFilters() {
    setAudioFilters(EMPTY_AUDIO_FILTERS);
    setKind(null);
    setWorkflow(null);
    setOriginalFolder(null);
    setFormat(null);
    setTag(null);
  }

  function saveAssetAnnotations(name: string | undefined, tags: string[]) {
    if (!selected || name === undefined) return;
    const assetId = selected.id;
    void act("Annotationen werden gespeichert …", async () => {
      await invoke("annotate_asset", { assetId, name, tags });
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: "Asset-Name und Tags gespeichert." });
    });
  }

  function savePackageAnnotations(_name: string | undefined, tags: string[]) {
    if (!filteredPackage) return;
    const packageId = filteredPackage.id;
    void act("Paket-Tags werden gespeichert …", async () => {
      await invoke("annotate_package", { packageId, tags });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: "Paket-Tags gespeichert." });
    });
  }

  function correctGrouping(targetAssetId: number | null) {
    if (!file) return;
    const chosen = file;
    void act("Variantenzuordnung wird gespeichert …", async () => {
      let assetId: number;
      if (targetAssetId === null) {
        assetId = await invoke<number>("split_variant", { fileId: chosen.id });
      } else {
        await invoke("assign_variant", { fileId: chosen.id, targetAssetId });
        assetId = targetAssetId;
      }
      focusAssetId.current = assetId;
      focusFileId.current = chosen.id;
      setSelectedFileId(chosen.id);
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: targetAssetId === null
        ? `${chosen.originalFilename} als eigenes Asset abgespalten. Name, Tags und Ordnerverknüpfungen übernommen.`
        : `${chosen.originalFilename} neu zugeordnet. Originalpfad, Tags und Ordnerverknüpfungen bleiben erhalten.` });
    });
  }

  function trashItem(kind: "package" | "file", id: number, name: string) {
    void act("Eintrag wird in den Papierkorb verschoben …", async () => {
      await invoke(kind === "package" ? "trash_package" : "trash_file", kind === "package" ? { packageId: id } : { fileId: id });
      setSelected(null);
      setSelectedFileId(null);
      setPage(null);
      focusAssetId.current = null;
      focusFileId.current = null;
      if (kind === "package" && packageId === id) setPackageId(null);
      clearFilters();
      setOffset(0);
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: `${name} im internen Papierkorb. Dateien bleiben gespeichert und belegen weiterhin Speicherplatz; Originale und Archive bleiben erhalten.` });
    });
  }

  function restoreTrash(entry: TrashEntry) {
    void act("Eintrag wird wiederhergestellt …", async () => {
      await invoke("restore_trash", { trashId: entry.id });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: `${entry.name} wiederhergestellt.` });
    });
  }

  function closeTrash(scope: number) {
    if (trashGeneration.current !== scope) return;
    trashGeneration.current++;
    setShowTrash(false);
  }

  function emptyTrash(entries: TrashEntry[]) {
    if (!entries.length) return;
    void act("Papierkorb wird geleert …", async (isCurrent) => {
      const packages = entries.filter((entry) => entry.fileId === null).length;
      const names = entries.slice(0, 8).map((entry) => `• ${entry.name}`).join("\n");
      const proceed = await confirm(
        `${number.format(packages)} Pakete und ${number.format(entries.length - packages)} Dateien endgültig löschen?\n\n${names}${entries.length > 8 ? `\n… und ${entries.length - 8} weitere` : ""}\n\nImportierte Paketdaten werden von der Festplatte gelöscht. Bei externen Paketen werden nur die Katalogeinträge entfernt; die Originale bleiben unverändert. Annotationen und Projektzuordnungen gehen verloren. Dies kann nicht rückgängig gemacht werden.`,
        { title: "Papierkorb leeren?", kind: "warning", okLabel: "Endgültig löschen", cancelLabel: "Abbrechen" },
      );
      if (!proceed || !isCurrent()) return;
      const result = await invoke<EmptiedTrash>("empty_trash");
      const info = await invoke<LibraryInfo>("get_library");
      if (!isCurrent()) return;
      setLibrary(info);
      setRevision((value) => value + 1);
      setNotice(result.diskErrors.length
        ? { tone: "error", message: `Papierkorb geleert, aber ${result.diskErrors.length} Pfade konnten nicht von der Festplatte gelöscht werden: ${result.diskErrors.join("; ")}` }
        : { tone: "success", message: `Papierkorb geleert: ${number.format(result.packages)} Pakete und ${number.format(result.files)} Dateien endgültig gelöscht.` });
    });
  }

  function createFolder(name: string, parentId: number | null) {
    void act("Projektordner wird angelegt …", async () => {
      await invoke<ProjectFolder>("create_folder", { name, parentId });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setShowFolderDialog(false);
      setNotice({ tone: "success", message: "Projektordner angelegt." });
    });
  }

  function changeFolderMembership(id: number, add: boolean) {
    if (!selected) return;
    const assetId = selected.id;
    void act("Ordnerzuordnung wird gespeichert …", async () => {
      await invoke(add ? "add_to_folder" : "remove_from_folder", { folderId: id, assetId });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: add ? "Asset dem Projektordner hinzugefügt." : "Ordnerverknüpfung entfernt." });
    });
  }

  function chooseAsset(asset: Asset, mode: SelectionMode) {
    if (queryLoading) return;
    setSelected(asset);
    setSelectedFileId(null);
    setSelection((previous) => catalogSelection(previous, visibleKeys, assetKey(asset), mode));
  }

  function bulkChange(action: BulkAction) {
    if (bulkDisabled) return;
    const chosen = selectedAssets.slice();
    void act("Sammelaktion wird gespeichert …", async () => {
      if (action.kind === "trash") {
        const names = chosen.slice(0, 6).map((asset) => `• ${asset.imageGroup?.name ?? asset.name}`).join("\n");
        const proceed = await confirm(`${chosen.length} ausgewählte Katalogeinträge mit allen Formatvarianten in den internen Papierkorb verschieben?\n\n${names}${chosen.length > 6 ? `\n… und ${chosen.length - 6} weitere` : ""}\n\nMaterialien und Sprite-Sequenzen umfassen alle zugehörigen Maps bzw. Frames. Dateien und Projektzuordnungen bleiben für die Wiederherstellung erhalten.`,
          { title: "Auswahl in Papierkorb verschieben?", kind: "warning", okLabel: "Verschieben", cancelLabel: "Abbrechen" });
        if (!proceed) return;
      }
      const result = await invoke<BulkResult>("bulk_assets", {
        selection: chosen.map((asset) => asset.imageGroup
          ? { kind: "imageGroup", fileId: asset.imageGroup.fileId, workflow: asset.imageGroup.workflow }
          : { kind: "asset", assetId: asset.id }),
        action,
      });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      if (action.kind === "trash") setSelection({ keys: [], anchor: null });
      if (action.kind === "addTags") setBulkTags("");
      setNotice({ tone: "success", message: `${number.format(result.assetCount)} Assets${action.kind === "trash" ? ` mit ${number.format(result.fileCount)} Dateien im internen Papierkorb. Wiederherstellung über die Seitenleiste möglich; Speicher bleibt belegt.` : action.kind === "addTags" ? ": Tags ergänzt. Vorhandene Namen und Tags bleiben erhalten." : " dem Projektordner hinzugefügt. Vorhandene Zuordnungen bleiben erhalten."}` });
    });
  }

  function filterFolder(id: number) {
    setPackageId(null);
    setFolderId(id);
    clearFilters();
    setOffset(0);
  }

  function filterPackage(id: number | null) {
    setFolderId(null);
    setPackageId(id);
    clearFilters();
    setOffset(0);
  }

  return (
    <div className={`app-shell ${library ? "editor-shell" : ""}`}>
      {dropHover && <div className="native-drop-hint" role="status">{library && !actionsDisabled ? "Ordner oder ZIP-Pakete hier zum Importieren ablegen" : library ? "Bitte den laufenden Vorgang abwarten" : "Bitte zuerst eine Bibliothek öffnen"}</div>}
      <header className="topbar">
        <div className="brand">
          <span className="brand-mark" aria-hidden="true">
            <i />
            <i />
            <i />
            <i />
            <i />
          </span>
          <div>
            <strong>Game Asset Library</strong>
            <span>Deine Assets. Dein nächstes Spiel.</span>
          </div>
        </div>
        {library ? (
          <div className="topbar-actions">
            <button className="button subtle" onClick={referenceFolder} disabled={actionsDisabled}>
              <Icon name="folder" />
              Externen Ordner einbinden
            </button>
            <button
              className="button subtle"
              onClick={openLibrary}
              disabled={actionsDisabled}
            >
              <Icon name="folder" />
              Bibliothek öffnen
            </button>
            <button
              className="button primary"
              onClick={importPackage}
              disabled={actionsDisabled}
              title="Ordner oder ZIP-Datei importieren"
            >
              <Icon name="plus" />
              Paket importieren
            </button>
          </div>
        ) : (
          <span className="local-badge">
            <span />
            Lokal gespeichert
          </span>
        )}
      </header>

      {(busy || notice) && (
        <div
          className={`notice ${notice?.tone ?? "working"}`}
          role={notice?.tone === "error" ? "alert" : "status"}
        >
          {busy ? (
            <>
              <span className="spinner" />
              {busy}
            </>
          ) : (
            <>
              <span className="notice-message">{notice?.message}</span>
              <button
                className="icon-button"
                onClick={() => setNotice(null)}
                aria-label="Meldung schließen"
              >
                <Icon name="close" />
              </button>
            </>
          )}
        </div>
      )}

      {importActive && !importProgress && (
        <section className="import-progress" aria-label="Paketprüfung" aria-live="polite">
          <div className="import-progress-summary">
            <strong>{cancelling ? "Abbruch angefordert …" : busy}</strong>
            <button className="button outlined" onClick={() => void cancelImport()} disabled={cancelling}>{cancelling ? "Abbruch angefordert …" : "Import abbrechen"}</button>
          </div>
        </section>
      )}
      {importProgress && (
        <section className="import-progress" aria-label="Paketimport" aria-live="polite">
          <div className="import-progress-summary">
            <strong>{importProgress.stage === "preparing" ? "Import wird vorbereitet …" : importProgress.stage === "cataloging" ? "Katalog wird gespeichert …" : "Paketdateien werden kopiert …"}</strong>
            <span>{bytes(importProgress.completedBytes)} / {bytes(importProgress.totalBytes)} · {number.format(importProgress.completedEntries)} / {number.format(importProgress.totalEntries)} Einträge</span>
            <button className="button outlined" onClick={() => void cancelImport()} disabled={cancelling || ["completed", "cancelled", "failed"].includes(importProgress.stage)}>
              {cancelling ? "Abbruch angefordert …" : "Import abbrechen"}
            </button>
          </div>
          <progress max={importProgress.totalBytes || 1} value={importProgress.totalBytes ? Math.min(importProgress.completedBytes, importProgress.totalBytes) : undefined} aria-label="Kopierfortschritt" />
          {importProgress.currentPath && <p title={importProgress.currentPath}>{importProgress.currentPath}</p>}
        </section>
      )}

      {!library ? (
        <main className="onboarding">
          <section className="welcome">
            <p className="eyebrow">
              <span />
              DEIN LOKALER ASSET-FUNDUS
            </p>
            <h1>
              Von der Sammlung
              <br />
              ins nächste <em>Spiel.</em>
            </h1>
            <p className="intro">
              Importiere Ordner oder ZIP-Pakete. Finde passende Sounds,
              Sprites und 3D-Modelle und nimm sie direkt in dein Projekt mit.
            </p>
            <div className="workflow" aria-label="Dein Arbeitsablauf">
              <div>
                <span>01</span>
                <strong>Pakete sammeln</strong>
                <p>Originalordner und Begleitdateien bleiben zusammen.</p>
              </div>
              <div>
                <span>02</span>
                <strong>Assets finden</strong>
                <p>Paket, Herkunft und Dateiname im Blick.</p>
              </div>
              <div>
                <span>03</span>
                <strong>Im Spiel verwenden</strong>
                <p>Originaldatei ins eigene Projekt exportieren.</p>
              </div>
            </div>
          </section>
          <section className="setup-card" aria-labelledby="setup-title">
            <div className="setup-illustration" aria-hidden="true">
              <div className="folder-back" />
              <div className="folder-front">
                <span className="sound-bars">
                  <i />
                  <i />
                  <i />
                  <i />
                  <i />
                  <i />
                  <i />
                </span>
              </div>
              <span className="illustration-dot" />
            </div>
            <p className="eyebrow">ERSTER SCHRITT</p>
            <h2 id="setup-title">Deine eigene Bibliothek</h2>
            <p>
              Dein Katalog und deine importierten Pakete werden gemeinsam an
              einem Ort gespeichert.
            </p>
            <button
              className="button primary full"
              onClick={() => createLibrary(false)}
              disabled={actionsDisabled}
            >
              Standardbibliothek anlegen
              <Icon name="arrow" />
            </button>
            <p className="storage-hint">
              Im Benutzerordner unter „Game Asset Library“
            </p>
            <button
              className="button outlined full"
              onClick={() => createLibrary(true)}
              disabled={actionsDisabled}
            >
              <Icon name="folder" />
              Eigenen Speicherort wählen
            </button>
            <div className="setup-open">
              <span>Schon eine Bibliothek vorhanden?</span>
              <button
                className="text-button"
                onClick={openLibrary}
                disabled={actionsDisabled}
              >
                Bibliothek öffnen
                <Icon name="arrow" />
              </button>
            </div>
          </section>
        </main>
      ) : (
        <div className="workspace">
          <aside className="sidebar" aria-label="Bibliothek, Pakete und Projektordner">
            <div className="library-heading">
              <span className="eyebrow">BIBLIOTHEK</span>
              <h2 title={library.name}>{library.name}</h2>
              <span className="local-badge">
                <span />
                Lokal verfügbar
              </span>
            </div>
            <div className="sidebar-navigation">
              <button
                className={`package-item all-sounds ${packageId === null && folderId === null ? "active" : ""}`}
                aria-pressed={packageId === null && folderId === null}
                onClick={() => filterPackage(null)}
              >
                <Icon name="library" />
                <span>Alle Assets</span>
                <span className="count">{number.format(library.assetCount)}</span>
              </button>
              <div className="package-heading">
                <h3>PAKETE</h3>
                <span>{number.format(library.packages.length)}</span>
              </div>
              <nav className="packages" aria-label="Paketfilter">
                {library.packages.length ? (
                  library.packages.map((item) => (
                    <TrashActions key={item.id} className="package-row" label={`Paket ${item.name}`} disabled={actionsDisabled}
                      onTrash={() => trashItem("package", item.id, item.name)}>
                      <button
                        className={`package-item ${packageId === item.id ? "active" : ""}`}
                        aria-pressed={packageId === item.id}
                        aria-label={`${item.name} · ${number.format(item.assetCount)} Assets${item.sourceKind === "external" ? ` · Externe Quelle ${item.available ? "erreichbar" : "offline"}` : ""}`}
                        title={`${item.name}${item.sourceKind === "external" ? ` · Externe Quelle ${item.available ? "erreichbar" : "offline"}` : ""}`}
                        onClick={() => filterPackage(item.id)}
                      >
                        <Icon name="folder" />
                        <span>{item.name}</span>
                        <span className="count">
                          {number.format(item.assetCount)}
                        </span>
                        {item.sourceKind === "external" && <span className={`package-source-status ${item.available ? "" : "unavailable"}`} aria-hidden="true">↗</span>}
                      </button>
                    </TrashActions>
                  ))
                ) : (
                  <p className="sidebar-empty">
                    Dein erstes Paket
                    <br />
                    findet hier seinen Platz.
                  </p>
                )}
              </nav>
              {library.packages.some((item) => item.sourceKind === "external") && (
                <button className="text-button check-sources" onClick={refreshSources} disabled={actionsDisabled}>Quellen prüfen</button>
              )}
              <div className="package-heading project-folder-heading">
                <h3>PROJEKTORDNER</h3>
                <button className="icon-button" aria-label="Projektordner anlegen" disabled={actionsDisabled}
                  onClick={() => { setNotice(null); setShowFolderDialog(true); }}><Icon name="plus" /></button>
              </div>
              <nav className="packages project-folders" aria-label="Projektordnerfilter">
                {projectFolders.length ? projectFolders.map((item) => {
                  const path = folderPath(item.id, projectFolders);
                  return <button key={item.id} className={`package-item ${folderId === item.id ? "active" : ""}`}
                    aria-pressed={folderId === item.id} title={`${path} · einschließlich Unterordner`} onClick={() => filterFolder(item.id)}>
                    <Icon name="folder" /><span>{path}</span><span className="count">{number.format(item.assetCount)}</span>
                  </button>;
                }) : <p className="sidebar-empty">Assets für ein Projekt sammeln.</p>}
              </nav>
            </div>
            <div className="sidebar-footer">
              <button className="package-item trash-link" onClick={() => { trashGeneration.current++; setNotice(null); setShowTrash(true); }} disabled={actionsDisabled}>
                <Icon name="folder" /><span>Interner Papierkorb</span>
              </button>
              <div className="storage">
                <Icon name="library" />
                <div>
                  <strong>Speicherort</strong>
                  <p title={library.root}>{library.root}</p>
                </div>
              </div>
            </div>
          </aside>

          <main className="catalog" aria-label="Asset-Katalog">
            <div className="catalog-heading">
              <div>
                <p className="eyebrow">ASSET-KATALOG</p>
                <h1 title={filteredFolderPath ?? filteredPackage?.name}>
                  {filteredFolderPath ?? filteredPackage?.name ?? "Alle Assets"}
                </h1>
              </div>
              <span className="format-badge">{kind === "sound" ? "SOUNDS" : kind === "image" ? "BILDER" : kind === "model" ? "3D" : "ASSETS"}</span>
            </div>
            <div className="search-box">
              <Icon name="search" />
              <input
                type="search"
                value={text}
                onChange={(event) => {
                  setText(event.target.value);
                  setOffset(0);
                }}
                aria-label="Assets durchsuchen"
                placeholder="Name, Tags, Datei, Paket oder Originalordner suchen …"
              />
            </div>
            <div className="catalog-tools">
              <label className="category-control">
                <span className="visually-hidden">Kategorie</span>
                <select aria-label="Nach Asset-Kategorie filtern" value={workflow ?? kind ?? ""} onChange={(event) => {
                  const value = event.target.value;
                  const grouped = value === "material" || value === "spriteSequence";
                  setWorkflow(grouped ? value : null); setKind(grouped ? "image" : value as AssetKind || null);
                  if (value !== "sound") setAudioFilters(EMPTY_AUDIO_FILTERS);
                  setFormat(null); setOffset(0); setSelected(null); setSelectedFileId(null); setModelSelection(null);
                }}>
                  <option value="">Alle Assets</option>
                  <option value="sound">Sounds</option>
                  <option value="image">Bilder</option>
                  <option value="material">Materialien</option>
                  <option value="spriteSequence">Sprite-Sequenzen</option>
                  <option value="model">3D-Modelle</option>
                </select>
              </label>
              <button className={`button outlined filter-toggle ${activeFilterCount ? "active" : ""}`} aria-expanded={filtersOpen}
                aria-controls="catalog-filter-panel" onClick={() => setFiltersOpen((current) => !current)}>
                <Icon name="filter" />Filter{activeFilterCount > 0 && <span className="filter-count">{activeFilterCount}</span>}
              </button>
              <div className="view-switch" role="group" aria-label="Katalogansicht">
                <button className="icon-button" aria-label="Kartenansicht" title="Kartenansicht" aria-pressed={catalogView === "cards"}
                  onClick={() => setCatalogView("cards")}><Icon name="grid" /></button>
                <button className="icon-button" aria-label="Listenansicht" title="Listenansicht" aria-pressed={catalogView === "list"}
                  onClick={() => setCatalogView("list")}><Icon name="list" /></button>
              </div>
            </div>
            <div id="catalog-filter-panel" className="catalog-filter-panel" hidden={!filtersOpen}>
              {filteredPackage?.sourceKind === "external" && (
                <section className="source-controls" aria-label="Externe Quelle">
                  <div>
                    <strong className={filteredPackage.available ? "" : "unavailable"}>Externe Quelle · {filteredPackage.available ? "erreichbar" : "nicht verfügbar"}</strong>
                    <p>{filteredPackage.externalRoot}</p>
                  </div>
                  <button className="text-button" onClick={() => relinkSource(filteredPackage)} disabled={actionsDisabled}>Quelle neu verknüpfen</button>
                </section>
              )}
              <div className="catalog-filters" aria-label="Katalogfilter">
                <label>
                  Originalordner
                  <select
                    aria-label="Nach Originalordner filtern"
                    value={originalFolder === "" ? "/" : originalFolder ?? ""}
                    onChange={(event) => {
                      const value = event.target.value;
                      setOriginalFolder(value === "/" ? "" : value || null);
                      setOffset(0);
                    }}
                  >
                    <option value="">Alle Ordner</option>
                    {filters.folders.map((folder) => (
                      <option key={folder} value={folder || "/"}>
                        {folder || "Paketwurzel"}
                      </option>
                    ))}
                  </select>
                </label>
                <label>
                  Dateityp
                  <select
                    aria-label="Nach Dateityp filtern"
                    value={format ?? ""}
                    onChange={(event) => {
                      setFormat(event.target.value || null);
                      setOffset(0);
                    }}
                  >
                    <option value="">Alle Typen</option>
                    {filters.formats.map((item) => <option key={item} value={item}>{item}</option>)}
                  </select>
                </label>
                <label>
                  Tag
                  <select
                    aria-label="Nach Tag filtern"
                    value={tag ?? ""}
                    onChange={(event) => {
                      setTag(event.target.value || null);
                      setOffset(0);
                    }}
                  >
                    <option value="">Alle Tags</option>
                    {filters.tags.map((item) => <option key={item} value={item}>{item}</option>)}
                  </select>
                </label>
              </div>
              <section className="audio-filter-section" aria-label="Gespeicherte Audioanalyse">
                <h3>Gespeicherte Audioanalyse</h3>
                <p className="detail-hint">Neue Importe sind zunächst nicht analysiert. Die Vorschau liest Eigenschaften. Unbekannte Werte sind keine Nullwerte und passen nicht auf Dauer- oder Kanalgrenzen. Gespeicherte Werte können bei geänderten Quelldateien veraltet sein.</p>
                <div className="catalog-filters">
                  <label>Dauer ab (Sekunden)
                    <input type="number" min="0" step="any" aria-label="Minimale Audiodauer in Sekunden" value={audioFilters.minDurationSeconds} onChange={(event) => changeAudioFilters({ ...audioFilters, minDurationSeconds: event.target.value })} />
                  </label>
                  <label>Dauer bis (Sekunden)
                    <input type="number" min="0" step="any" aria-label="Maximale Audiodauer in Sekunden" value={audioFilters.maxDurationSeconds} onChange={(event) => changeAudioFilters({ ...audioFilters, maxDurationSeconds: event.target.value })} />
                  </label>
                  <label>Kanäle
                    <select aria-label="Nach Audiokanälen filtern" value={audioFilters.channels} onChange={(event) => changeAudioFilters({ ...audioFilters, channels: event.target.value as AudioFilters["channels"] })}>
                      <option value="any">Alle Kanäle</option>
                      <option value="mono">Mono (1)</option>
                      <option value="stereo">Stereo (2)</option>
                      <option value="multi">Mehr als 2</option>
                    </select>
                  </label>
                  <label>Analysestatus
                    <select aria-label="Nach Audioanalysestatus filtern" value={audioFilters.analysis} onChange={(event) => changeAudioFilters({ ...audioFilters, analysis: event.target.value as AudioFilters["analysis"] })}>
                      <option value="any">Jeder Status</option>
                      <option value="notAnalysed">Nicht analysiert</option>
                      <option value="ready">Analyse vorhanden</option>
                      <option value="error">Analysefehler</option>
                    </select>
                  </label>
                </div>
                {audioFilterError && <p className="inline-error" role="alert">{audioFilterError}</p>}
              </section>
              {filteredPackage && (
                <details className="package-annotations">
                  <summary>
                    Paket-Tags bearbeiten
                    {filteredPackage.tags.length ? ` · ${filteredPackage.tags.join(", ")}` : ""}
                  </summary>
                  <AnnotationEditor
                    tags={filteredPackage.tags}
                    label="Paket annotieren"
                    disabled={actionsDisabled}
                    onSave={savePackageAnnotations}
                  />
                </details>
              )}
            </div>
            {audioFilterCount > 0 && <div className="audio-filter-refresh">
              <p className="detail-hint" role="status">{audioResultsStale
                ? "Analyse gespeichert. Audiofilter aktualisieren, um die Treffer neu zu prüfen."
                : "Treffer beruhen auf gespeicherten Analysewerten."}</p>
              <button className="text-button" disabled={actionsDisabled || queryLoading || audioFilterError !== null} onClick={refreshAudioResults}>Audiofilter aktualisieren</button>
            </div>}
            {kind === "sound" && <details className="audio-analysis" aria-label="Sounds im Suchbereich analysieren" open={audioAnalysis !== null}>
              <summary>Sounds im Suchbereich analysieren</summary>
              <p className="detail-hint">{audioAnalysisScopeLabel}</p>
              <p className="detail-hint">Alle passenden Sound-Dateivarianten, über alle Katalogseiten. Gespeicherte Dauer-, Kanal- und Statusfilter gelten für die Startliste. Entferne Dauer- und Kanalgrenzen und wähle „Jeder Status“ oder „Nicht analysiert“, um unanalysierte Sounds einzubeziehen.</p>
              {audioAnalysis?.phase === "preparing" && <p role="status">Suchbereich wird geprüft …</p>}
              {audioAnalysis?.phase === "prepared" && <p role="status"><strong>{number.format(audioAnalysis.total)} passende Dateivarianten</strong> · Startliste festgehalten.{audioAnalysis.total === 0 && " Keine passenden Sounds. Ändere den Suchbereich oder die gespeicherten Audiofilter."}</p>}
              {audioAnalysis?.progress && <>
                <p role="status">{`${audioAnalysis.phase === "completed" ? audioAnalysis.progress.cancelled ? "Abgebrochen" : "Analyse abgeschlossen" : "Sound-Analyse"} · ${number.format(audioAnalysis.progress.processed)} von ${number.format(audioAnalysis.progress.total)} Dateivarianten`}</p>
                <progress max={audioAnalysis.progress.total || 1} value={audioAnalysis.progress.processed} aria-label="Fortschritt der Sound-Analyse" />
                <p className="detail-hint">{`${number.format(audioAnalysis.progress.ready)} analysiert · ${number.format(audioAnalysis.progress.errors)} Analysefehler · ${number.format(audioAnalysis.progress.skipped)} übersprungen`}</p>
                <p className="detail-hint">Bereits gespeicherte Analysen bleiben erhalten. Unverfügbare oder geänderte Quelldateien werden übersprungen. Aktualisiere die Liste ausdrücklich, um gespeicherte Werte in den Treffern zu sehen.</p>
              </>}
              {audioAnalysis?.message && <p className="inline-error" role="alert">{audioAnalysis.message}</p>}
              <div className="audio-analysis-actions">
                {(!audioAnalysis || audioAnalysis.phase === "failed" || audioAnalysis.phase === "completed") && <button className="button outlined" disabled={actionsDisabled || audioFilterError !== null} onClick={() => void prepareAudioAnalysis()}>Analyse vorbereiten</button>}
                {audioAnalysis?.phase === "prepared" && <button className="button primary" disabled={actionsDisabled || audioAnalysis.total === 0} onClick={startAudioAnalysis}>Analyse starten</button>}
                {audioAnalysis?.phase === "running" && <button className="button outlined" disabled={audioAnalysis.stopping} onClick={() => void stopAudioAnalysis()}>{audioAnalysis.stopping ? "Wird abgebrochen …" : "Analyse abbrechen"}</button>}
                {(audioAnalysis?.phase === "completed" || audioAnalysis?.phase === "failed") && <button className="text-button" disabled={actionsDisabled || queryLoading || audioFilterError !== null} onClick={refreshAudioResults}>Analyseergebnisse aktualisieren</button>}
                {(audioAnalysis?.phase === "preparing" || audioAnalysis?.phase === "prepared") && <button className="text-button" onClick={() => void discardAudioAnalysis()}>Vorbereitung verwerfen</button>}
              </div>
            </details>}
            <div className="catalog-summary" role="status">
              <span>
                {queryLoading
                  ? "Katalog wird durchsucht …"
                  : `${number.format(page?.total ?? 0)} ${(page?.total ?? 0) === 1 ? "Asset" : "Assets"}${text ? " gefunden" : " im Katalog"}`}
              </span>
              {(activeFilterCount > 0 || kind !== null) ? <button className="text-button" onClick={() => {
                clearFilters(); setOffset(0);
              }}>Filter zurücksetzen</button> : <span>Nach Name</span>}
            </div>
            {page?.items.length ? <section className="bulk-toolbar" aria-label="Katalogauswahl">
              <div className="selection-summary">
                <strong role="status">{selectedAssets.length} ausgewählt</strong>
                <button className="text-button" disabled={actionsDisabled || queryLoading} onClick={() => setSelection({ keys: visibleKeys, anchor: visibleKeys[0] ?? null })}>Diese Seite auswählen</button>
                <button className="text-button" disabled={!selectedAssets.length || actionsDisabled} onClick={() => setSelection({ keys: [], anchor: null })}>Auswahl aufheben</button>
              </div>
              <details className="bulk-controls">
                <summary>Sammelaktionen</summary>
                <p id="selection-help" className="detail-hint">Auswahl nur auf dieser Seite. ⌘/Strg-Klick ergänzt, Umschalt wählt einen Bereich. Tags und Ordner betreffen alle ausgewählten Assets; Materialien und Sprite-Sequenzen umfassen alle Maps bzw. Frames. Papierkorb umfasst alle Formatvarianten.</p>
                <form onSubmit={(event) => { event.preventDefault(); bulkChange({ kind: "addTags", tags: bulkTags.split(",").map((value) => value.trim()).filter(Boolean) }); }}>
                  <label>Tags ergänzen<input value={bulkTags} onChange={(event) => setBulkTags(event.target.value)} disabled={bulkDisabled} placeholder="z. B. salzburg, ambience" /></label>
                  <button className="button outlined" disabled={bulkDisabled || !bulkTags.trim()}>Tags ergänzen</button>
                </form>
                <form onSubmit={(event) => { event.preventDefault(); if (bulkFolderId !== null) bulkChange({ kind: "addToFolder", folderId: bulkFolderId }); }}>
                  <label>Zum Projektordner hinzufügen<select value={bulkFolderId ?? ""} onChange={(event) => setBulkFolderId(event.target.value ? Number(event.target.value) : null)} disabled={bulkDisabled}>
                    <option value="">Ordner auswählen</option>
                    {projectFolders.map((folder) => <option key={folder.id} value={folder.id}>{folderPath(folder.id, projectFolders)}</option>)}
                  </select></label>
                  <button className="button outlined" disabled={bulkDisabled || bulkFolderId === null}>Hinzufügen</button>
                </form>
                <button className="button outlined" disabled={bulkDisabled} onClick={() => bulkChange({ kind: "trash" })}>Auswahl in Papierkorb</button>
              </details>
            </section> : null}
            {queryError ? (
              <div className="empty-state" role="alert">
                <Icon name="folder" />
                <h2>{audioFilterError ? "Audiofilter prüfen" : "Katalog nicht verfügbar"}</h2>
                <p>{queryError}</p>
                <button
                  className="button outlined"
                  onClick={() => setRevision((value) => value + 1)}
                >
                  Erneut versuchen
                </button>
              </div>
            ) : !page && queryLoading ? (
              <div className="empty-state">
                <span className="spinner" />
                <p>Katalog wird geladen …</p>
              </div>
            ) : page?.items.length ? (
              <div className="catalog-scroll" aria-busy={queryLoading}>
                <div className={`asset-grid ${catalogView}`} role="list" aria-label={catalogView === "cards" ? "Asset-Karten" : "Asset-Liste"}>
                  {page.items.map((asset, index) => {
                    const firstFile = asset.files.find((item) => item.id === asset.imageGroup?.fileId) ?? asset.files[0];
                    const folders = Array.from(new Set(asset.files.map((item) => item.originalFolder || "Paketwurzel"))).join(", ");
                    const checked = selection.keys.includes(assetKey(asset));
                    return <div role="listitem" key={assetKey(asset)} className={`asset-item ${assetKey(selected) === assetKey(asset) ? "selected" : ""} ${checked ? "bulk-selected" : ""}`}>
                      <input type="checkbox" className="asset-checkbox" checked={checked} onChange={() => {}} disabled={actionsDisabled || queryLoading}
                        aria-label={`${asset.imageGroup?.name ?? asset.name} auswählen${asset.imageGroup ? " · gesamte Bildgruppe" : " · alle Varianten"}`}
                        onClick={(event) => chooseAsset(asset, event.shiftKey ? (event.metaKey || event.ctrlKey ? "addRange" : "range") : "toggle")} />
                      <TrashActions label={`Datei ${firstFile.originalFilename}`} disabled={actionsDisabled}
                        onTrash={() => trashItem("file", firstFile.id, firstFile.originalFilename)}>
                        <button className="asset-select" disabled={queryLoading}
                          onClick={(event) => chooseAsset(asset, event.shiftKey ? (event.metaKey || event.ctrlKey ? "addRange" : "range") : event.metaKey || event.ctrlKey ? "toggle" : "single")}
                          onKeyDown={(event) => {
                            if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "a") {
                              event.preventDefault(); setSelection({ keys: visibleKeys, anchor: visibleKeys[0] ?? null }); return;
                            }
                            if (event.key === "Escape") { setSelection({ keys: [], anchor: null }); return; }
                            const grid = event.currentTarget.closest(".asset-grid");
                            if (!grid) return;
                            const columns = catalogView === "list" ? 1 : Math.max(1, getComputedStyle(grid).gridTemplateColumns.split(" ").length);
                            const next = nextCatalogIndex(index, page.items.length, event.key, columns);
                            if (next === null) return;
                            event.preventDefault();
                            grid.querySelectorAll<HTMLButtonElement>(".asset-select")[next]?.focus();
                            if (!event.metaKey && !event.ctrlKey) chooseAsset(page.items[next], event.shiftKey ? "range" : "single");
                          }}
                          aria-pressed={assetKey(selected) === assetKey(asset)}>
                          <span className={`asset-poster ${asset.kind === "image" ? "transparency-background" : ""}`}>
                            {asset.kind === "image" ? <ImageThumbnail key={`${library.root}:${firstFile.id}:${firstFile.available}`} fileId={firstFile.id} available={firstFile.available} filename={firstFile.originalFilename} /> : asset.kind === "model" && (firstFile.format === "BLEND" || firstFile.sizeBytes <= MAX_MODEL_THUMBNAIL_BYTES) ? <ModelThumbnail queue={modelThumbnails} scope={thumbnailScope} fileId={firstFile.id} path={firstFile.relativePath} sizeBytes={firstFile.sizeBytes} format={firstFile.format} available={firstFile.available} fallback={<Icon name="model" />} /> : <Icon name={asset.kind} />}
                            <span className="asset-format">{asset.imageGroup ? asset.imageGroup.workflow === "material" ? "MATERIAL" : "SPRITES" : firstFile.format}</span>
                          </span>
                          <span className="asset-card-copy">
                            <strong>{asset.imageGroup?.name ?? asset.name}</strong>
                            <span className="asset-filename">{firstFile.originalFilename}</span>
                            <span className="asset-context" title={`${asset.packageName} · ${folders}`}>{asset.packageName} · {folders}</span>
                            <span className="asset-facts">{asset.imageGroup ? `${asset.imageGroup.fileCount} ${asset.imageGroup.workflow === "material" ? "Maps" : "Frames"}` : <>{bytes(firstFile.sizeBytes)}{asset.files.length > 1 && ` · ${asset.files.length} Varianten`}</>}</span>
                            {asset.imageGroup?.errors.length ? <span className="unavailable">Zuordnung prüfen</span> : null}
                            {!firstFile.available && <span className="unavailable">Nicht verfügbar</span>}
                          </span>
                        </button>
                      </TrashActions>
                    </div>;
                  })}
                </div>
              </div>
            ) : (
              <div className="empty-state">
                <span className="empty-icon">
                  <Icon name={library.assetCount === 0 ? "folder" : "search"} />
                </span>
                <h2>
                  {library.assetCount === 0
                    ? "Platz für dein erstes Asset."
                    : "Kein passendes Asset gefunden."}
                </h2>
                <p>
                  {library.assetCount === 0
                    ? "Importiere einen Ordner oder ein ZIP-Paket mit Sounds, Bildern oder 3D-Modellen. Die ursprüngliche Struktur und Begleitdateien bleiben erhalten."
                    : "Versuche einen anderen Suchbegriff oder passe die Filter an."}
                </p>
                {library.assetCount === 0 ? (
                  <button
                    className="button primary"
                    onClick={importPackage}
                    disabled={actionsDisabled}
                  >
                    <Icon name="plus" />
                    Erstes Paket importieren
                  </button>
                ) : (
                  (text || packageId !== null || folderId !== null || originalFolder !== null || format || kind || tag) && (
                    <button
                      className="button outlined"
                      onClick={() => {
                        setText("");
                        filterPackage(null);
                      }}
                    >
                      Suche zurücksetzen
                    </button>
                  )
                )}
              </div>
            )}
            <footer className="pagination">
              <span>
                {page && page.total > 0
                  ? `${number.format(page.offset + 1)}–${number.format(Math.min(page.offset + page.items.length, page.total))} von ${number.format(page.total)}`
                  : "0 Assets"}
              </span>
              <div>
                <span title={`${PAGE_SIZE} Assets pro Seite`}>
                  Seite {number.format(Math.floor(offset / PAGE_SIZE) + 1)} / {number.format(Math.max(1, Math.ceil((page?.total ?? 0) / PAGE_SIZE)))}
                </span>
                <button
                  className="icon-button"
                  aria-label="Erste Seite"
                  disabled={queryLoading || offset === 0}
                  onClick={() => setOffset(0)}
                >
                  <Icon name="first" />
                </button>
                <button
                  className="icon-button"
                  aria-label="Vorherige Seite"
                  disabled={queryLoading || offset === 0}
                  onClick={() => setOffset(Math.max(0, offset - PAGE_SIZE))}
                >
                  <Icon name="back" />
                </button>
                <button
                  className="icon-button"
                  aria-label="Nächste Seite"
                  disabled={
                    queryLoading || !page || offset + PAGE_SIZE >= page.total
                  }
                  onClick={() => setOffset(offset + PAGE_SIZE)}
                >
                  <Icon name="next" />
                </button>
                <button
                  className="icon-button"
                  aria-label="Letzte Seite"
                  disabled={
                    queryLoading || !page || offset + PAGE_SIZE >= page.total
                  }
                  onClick={() => setOffset(Math.max(0, Math.ceil((page?.total ?? 0) / PAGE_SIZE) - 1) * PAGE_SIZE)}
                >
                  <Icon name="last" />
                </button>
              </div>
            </footer>
          </main>

          <aside className="detail" aria-label="Ausgewähltes Asset">
            {selected && file ? (
              <>
                <div className="detail-heading">
                  <TrashActions className="detail-file-actions" label={`Datei ${file.originalFilename}`} disabled={actionsDisabled}
                    onTrash={() => trashItem("file", file.id, file.originalFilename)}>
                    <div className="detail-title">
                      <h2>{selected.imageGroup?.name ?? selected.name}</h2>
                      <p>{file.originalFilename}</p>
                    </div>
                  </TrashActions>
                  <p className="detail-source">{selected.packageName} · {file.format}</p>
                  {!selected.imageGroup && selected.files.length > 1 && <section className="variants" aria-label="Formatvarianten">
                    <label>
                      Formatvariante für Vorschau und Export
                      <select aria-label="Formatvariante für Vorschau und Export" value={file.id} onChange={(event) => setSelectedFileId(Number(event.target.value))}>
                        {selected.files.map((item) => (
                          <option key={item.id} value={item.id}>{item.format} · {item.originalFilename} · {item.originalFolder || "Paketwurzel"}{audioFilterCount > 0 && item.id === selected.files[0]?.id ? " · Filtertreffer" : ""}</option>
                        ))}
                      </select>
                    </label>
                  </section>}
                  {audioFilterCount > 0 && !queryLoading && !queryError && <p className="detail-hint" role="status">{file.id === selected.files[0]?.id
                    ? "Gefundene Variante anhand gespeicherter Analysewerte."
                    : "Andere Variante: Diese Auswahl für Vorschau und Export muss die aktiven Audiofilter nicht erfüllen."}</p>}
                </div>
                <div className="inspector-tabs" role="tablist" aria-label="Asset-Details" onKeyDown={(event) => {
                  const current = (event.target as HTMLElement).getAttribute("data-inspector-tab") as InspectorTab | null;
                  if (!current) return;
                  const next = nextInspectorTab(current, event.key);
                  if (!next) return;
                  event.preventDefault();
                  const target = event.currentTarget.querySelector<HTMLButtonElement>(`[data-inspector-tab="${next}"]`);
                  target?.focus(); target?.click();
                }}>
                  <button id="preview-tab" data-inspector-tab="preview" role="tab" aria-selected={inspectorTab === "preview"} aria-controls="preview-panel"
                    tabIndex={inspectorTab === "preview" ? 0 : -1} onClick={() => setInspectorTab("preview")}>Vorschau</button>
                  <button id="properties-tab" data-inspector-tab="properties" role="tab" aria-selected={inspectorTab === "properties"} aria-controls="properties-panel"
                    tabIndex={inspectorTab === "properties" ? 0 : -1} onClick={() => setInspectorTab("properties")}>Eigenschaften</button>
                  <button id="origin-tab" data-inspector-tab="origin" role="tab" aria-selected={inspectorTab === "origin"} aria-controls="origin-panel"
                    tabIndex={inspectorTab === "origin" ? 0 : -1} onClick={() => setInspectorTab("origin")}>Herkunft</button>
                </div>
                <div className="inspector-scroll">
                  <section id="preview-panel" className="inspector-panel preview" role="tabpanel" aria-labelledby="preview-tab" hidden={inspectorTab !== "preview"} tabIndex={0}>
                    <div className="section-heading">
                      <h3>{selected.imageGroup ? selected.imageGroup.workflow === "material" ? "Material-Vorschau" : "Sprite-Animation" : file.kind === "sound" ? "Anhören" : file.kind === "image" ? file.format === "ASEPRITE" ? "Aseprite-Animation" : "Bild & Spritesheet" : file.format === "BLEND" ? "Gespeichertes Blender-Bild" : "3D-Vorschau"}</h3>
                      <span>{selected.imageGroup ? `${selected.imageGroup.fileCount} Originaldateien` : file.format === "BLEND" ? "Eingebettetes Bild" : "Originaldatei"}</span>
                    </div>
                    {selected.imageGroup ? <ImageGroupPreview
                      key={`${library.root}:${selected.imageGroup.workflow}:${file.id}:${file.available}:${selectedPackage?.externalRoot}`} fileId={file.id}
                      workflow={selected.imageGroup.workflow} visible={inspectorTab === "preview"} onReady={setModelSelection}
                    /> : file.format === "BLEND" ? inspectorTab === "preview" && <BlendPreview
                      key={`${library.root}:${file.id}:${file.available}:${selectedPackage?.externalRoot}`} file={file}
                    /> : file.kind === "model" ? <ModelPreview
                      key={`${library.root}:${file.id}:${file.available}:${selectedPackage?.externalRoot}`} file={file} onReady={setModelSelection}
                      {...{ visible: inspectorTab === "preview" }}
                    /> : inspectorTab === "preview" && (file.kind === "sound" ? <AudioPreview
                      key={`${library.root}:${file.id}:${file.available}:${selectedPackage?.externalRoot}`} file={file}
                      onAnalysisComplete={() => { if (audioFilterCount > 0) setAudioResultsStale(true); }}
                    /> : <ImagePreview
                      key={`${library.root}:${file.id}:${file.available}:${selectedPackage?.externalRoot}`} file={file}
                    />)}
                  </section>
                  <section id="properties-panel" className="inspector-panel" role="tabpanel" aria-labelledby="properties-tab" hidden={inspectorTab !== "properties"} tabIndex={0}>
                    <VariantEditor
                      key={`${library.root}:${selected.id}:${file.id}`}
                      assetId={selected.id}
                      packageId={selected.packageId}
                      kind={selected.kind}
                      fileCount={selected.files.length}
                      disabled={actionsDisabled}
                      onSplit={() => correctGrouping(null)}
                      onAssign={correctGrouping}
                    />
                    <section className="annotations" aria-labelledby="annotations-title">
                      <h3 id="annotations-title">Name & Tags</h3>
                      <AnnotationEditor
                        key={`${library.root}:${selected.id}`}
                        name={selected.name}
                        tags={selected.tags}
                        label="Asset annotieren"
                        disabled={actionsDisabled}
                        onSave={saveAssetAnnotations}
                      />
                      {selectedPackage?.tags.length ? (
                        <p className="detail-hint">Paket-Tags: {selectedPackage.tags.join(", ")}</p>
                      ) : null}
                    </section>
                    <FolderMembershipEditor key={`${library.root}:${selected.id}`} folders={projectFolders} folderIds={selected.folderIds}
                      disabled={actionsDisabled} onAdd={(id) => changeFolderMembership(id, true)} onRemove={(id) => changeFolderMembership(id, false)} />
                  </section>
                  <section id="origin-panel" className="inspector-panel" role="tabpanel" aria-labelledby="origin-tab" hidden={inspectorTab !== "origin"} tabIndex={0}>
                    <section className="provenance" aria-labelledby="origin-title">
                      <h3 id="origin-title">Herkunft & Datei</h3>
                      <dl>
                        <div>
                          <dt>Paket</dt>
                          <dd>{selected.packageName}</dd>
                        </div>
                        <div>
                          <dt>Originalordner</dt>
                          <dd>{file.originalFolder || "Paketwurzel"}</dd>
                        </div>
                        <div>
                          <dt>Originaldateiname</dt>
                          <dd>{file.originalFilename}</dd>
                        </div>
                        <div className="file-facts">
                          <div>
                            <dt>Format</dt>
                            <dd>{file.format}</dd>
                          </div>
                          <div>
                            <dt>Dateigröße</dt>
                            <dd>{bytes(file.sizeBytes)}</dd>
                          </div>
                        </div>
                        {selectedPackage?.sourcePath && (
                          <div>
                            <dt>{selectedPackage.sourceKind === "external" ? "Ursprüngliche Quelle" : "Importiert aus"}</dt>
                            <dd className="source-path">
                              {selectedPackage.sourcePath}
                            </dd>
                          </div>
                        )}
                        {selectedPackage?.sourceKind === "external" && (
                          <div>
                            <dt>Externer Quellordner</dt>
                            <dd className="source-path">{selectedPackage.externalRoot}</dd>
                            <dd className={file.available ? "" : "unavailable"}>{file.available ? "Datei erreichbar" : "Datei nicht verfügbar"}</dd>
                          </div>
                        )}
                      </dl>
                      {selectedPackage?.sourceKind === "external" && (
                        <>
                          <button className="text-button" onClick={() => relinkSource(selectedPackage)} disabled={actionsDisabled}>Quelle neu verknüpfen</button>
                          <p className="detail-hint">Wähle den verschobenen Originalordner. Eine Kopie auf einem anderen Laufwerk bitte als neue Quelle einbinden.</p>
                        </>
                      )}
                    </section>
                  </section>
                </div>
                <div className="export-area">
                  <button
                    className="button primary full"
                    onClick={exportFile}
                    disabled={actionsDisabled || !file.available || ((requiresModelBundle(file) || selected.imageGroup != null) && modelSelection?.fileId !== file.id)}
                  >
                    <Icon name="download" />
                    {file.format === "BLEND" ? "Blender-Original exportieren" : "In Projekt exportieren"}
                  </button>
                  <p>{selected.imageGroup ? "Ausgewählte Originaldateien werden unverändert in einen neuen Paketordner kopiert." : file.format === "BLEND" ? "Nur die .blend-Originaldatei wird unverändert kopiert. Externe Texturen und verknüpfte Bibliotheken werden nicht mitgeliefert." : file.kind === "model" ? "Modell und Ressourcen werden unverändert in einen neuen Modellordner kopiert." : "Die Originaldatei wird unverändert kopiert."}</p>
                </div>
              </>
            ) : (
              <div className="detail-empty">
                <span className="sound-icon">
                  <Icon name="sound" />
                </span>
                <h2>Ein Asset, alle Details.</h2>
                <p>
                  Wähle ein Asset im Katalog aus, um es anzusehen oder anzuhören
                  und seine Herkunft zu sehen.
                </p>
              </div>
            )}
          </aside>
        </div>
      )}
      {library && showFolderDialog && (
        <FolderDialog folders={projectFolders} parentId={folderId} disabled={actionsDisabled} notice={notice}
          onCreate={createFolder} onClose={() => setShowFolderDialog(false)} />
      )}
      {library && showTrash && (
        <TrashDialog root={library.root} revision={revision} disabled={actionsDisabled} notice={notice}
          onClose={() => closeTrash(renderedTrashScope)} onRestore={restoreTrash} onEmpty={emptyTrash} />
      )}
      {!library && (
        <footer className="onboarding-footer">
          <span>Deine Bibliothek bleibt bei dir.</span>
          <span>
            Pakete importieren · Assets ansehen · Originale exportieren
          </span>
        </footer>
      )}
    </div>
  );
}
