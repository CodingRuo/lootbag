import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { assetKey, type ImageWorkflow } from "./imageWorkflows";
import type { CatalogSelection } from "./catalogSelection";
import type { ModelExportSelection } from "./ModelPreview";
import { errorMessage } from "./errorMessage.ts";
import { EMPTY_AUDIO_FILTERS, PAGE_SIZE, type Asset, type AudioFilters, type CatalogPage, type CatalogType, type LibraryInfo, type Notice, type QueryKind, type Setter } from "./ui.tsx";

type CatalogFilters = {
  folders: string[];
  formats: string[];
  tags: string[];
  duplicates: number;
};
export type CatalogQuery = ReturnType<typeof useCatalogQuery>;
export type SavedSearch = { id: number; name: string; queryJson: string };

// Catalog query state, its debounced page loading and the filter choices for the active scope.
export function useCatalogQuery({ library, revision, setSelected, setSelection, setSelectedFileId, setModelSelection, setNotice }: {
  library: LibraryInfo | null;
  revision: number;
  setSelected: Setter<Asset | null>;
  setSelection: Setter<CatalogSelection>;
  setSelectedFileId: Setter<number | null>;
  setModelSelection: Setter<ModelExportSelection | null>;
  setNotice: Setter<Notice | null>;
}) {
  const [text, setText] = useState("");
  const [packageId, setPackageId] = useState<number | null>(null);
  const [folderId, setFolderId] = useState<number | null>(null);
  const [originalFolder, setOriginalFolder] = useState<string | null>(null);
  const [format, setFormat] = useState<string | null>(null);
  const [kind, setKind] = useState<QueryKind | null>(null);
  const [workflow, setWorkflow] = useState<ImageWorkflow | null>(null);
  const [audioResultsStale, setAudioResultsStale] = useState(false);
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
  const [duplicates, setDuplicates] = useState(false);
  const [filters, setFilters] = useState<CatalogFilters>({
    folders: [],
    formats: [],
    tags: [],
    duplicates: 0,
  });
  const [offset, setOffset] = useState(0);
  const [page, setPage] = useState<CatalogPage | null>(null);
  const [queryLoading, setQueryLoading] = useState(false);
  const [queryError, setQueryError] = useState<string | null>(null);
  const [savedSearches, setSavedSearches] = useState<SavedSearch[]>([]);
  // Fixed key order, so the active saved search is recognised by string equality.
  const searchJson = JSON.stringify({ text, kind, workflow, packageId, folderId, originalFolder, format, tag, duplicates, audioFilters });
  const searchActive = searchJson !== JSON.stringify({ text: "", kind: null, workflow: null, packageId: null, folderId: null, originalFolder: null, format: null, tag: null, duplicates: false, audioFilters: EMPTY_AUDIO_FILTERS });
  const focusAssetId = useRef<number | null>(null);
  const focusFileId = useRef<number | null>(null);
  const selectionScope = JSON.stringify([library?.root, text, packageId, folderId, originalFolder, format, kind, workflow, tag, duplicates, audioFilters, offset]);

  useEffect(() => {
    setSelection({ keys: [], anchor: null });
    setSelectedFileId(null);
    setAudioResultsStale(false);
  }, [selectionScope]);

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
          duplicates,
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
  }, [library?.root, text, packageId, folderId, originalFolder, format, kind, workflow, tag, duplicates, audioFilters, offset, revision]);

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

  useEffect(() => {
    if (!library) return;
    let active = true;
    invoke<SavedSearch[]>("saved_searches")
      .then((result) => { if (active) setSavedSearches(result); })
      .catch((error: unknown) => { if (active) setNotice({ tone: "error", message: errorMessage(error) }); });
    return () => { active = false; };
  }, [library?.root]);

  async function saveSearch(name: string) {
    const saved = await invoke<SavedSearch>("save_search", { name, queryJson: searchJson });
    setSavedSearches((list) => [...list.filter((item) => item.id !== saved.id), saved].sort((a, b) => a.name.localeCompare(b.name, "de")));
  }

  async function deleteSavedSearch(id: number) {
    await invoke("delete_saved_search", { id });
    setSavedSearches((list) => list.filter((item) => item.id !== id));
  }

  // Stored JSON is untrusted: unusable searches get a notice, fields of the wrong type their default.
  function applySavedSearch(saved: SavedSearch) {
    let value: unknown;
    try { value = JSON.parse(saved.queryJson); } catch { value = null; }
    if (!isRecord(value)) {
      setNotice({ tone: "error", message: `Die gespeicherte Suche „${saved.name}“ ist nicht lesbar.` });
      return;
    }
    const audio = isRecord(value.audioFilters) ? value.audioFilters : {};
    setText(str(value.text) ?? ""); setKind(oneOf(value.kind, ["sound", "image", "model", "animation"] as const));
    setWorkflow(oneOf(value.workflow, ["material", "spriteSequence"] as const));
    setPackageId(id(value.packageId)); setFolderId(id(value.folderId)); setOriginalFolder(str(value.originalFolder));
    setFormat(str(value.format)); setTag(str(value.tag)); setDuplicates(value.duplicates === true);
    setAudioFilters({
      minDurationSeconds: str(audio.minDurationSeconds) ?? "", maxDurationSeconds: str(audio.maxDurationSeconds) ?? "",
      channels: oneOf(audio.channels, ["any", "mono", "stereo", "multi"] as const) ?? "any",
      analysis: oneOf(audio.analysis, ["any", "notAnalysed", "ready", "error"] as const) ?? "any",
    });
    setOffset(0); setSelected(null); setSelectedFileId(null); setModelSelection(null);
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

  // The asset type is navigation, so package, folder and "Filter zurücksetzen" keep it (keepType).
  function clearFilters(keepType = false) {
    setAudioFilters(EMPTY_AUDIO_FILTERS);
    if (!keepType) {
      setKind(null);
      setWorkflow(null);
    }
    setOriginalFolder(null);
    setFormat(null);
    setTag(null);
    setDuplicates(false);
  }

  function filterFolder(id: number) {
    setPackageId(null);
    setFolderId(id);
    clearFilters(true);
    setOffset(0);
  }

  function filterPackage(id: number | null) {
    setFolderId(null);
    setPackageId(id);
    clearFilters(true);
    setOffset(0);
  }

  function chooseType(value: CatalogType) {
    const grouped = value === "material" || value === "spriteSequence";
    setWorkflow(grouped ? value : null);
    setKind(grouped ? "image" : value as QueryKind || null);
    if (value !== "sound") setAudioFilters(EMPTY_AUDIO_FILTERS);
    setFormat(null); setOffset(0); setSelected(null); setSelectedFileId(null); setModelSelection(null);
  }

  return {
    text, setText, packageId, setPackageId, folderId, setFolderId, originalFolder, setOriginalFolder, format, setFormat,
    kind, setKind, workflow, setWorkflow, tag, setTag, duplicates, setDuplicates, audioFilters, setAudioFilters, offset, setOffset, filters,
    page, setPage, queryLoading, queryError, focusAssetId, focusFileId, audioFilterCount, audioCriteria, audioFilterError,
    audioResultsStale, setAudioResultsStale, changeAudioFilters, clearFilters, filterFolder, filterPackage, chooseType,
    savedSearches, searchJson, searchActive, saveSearch, deleteSavedSearch, applySavedSearch,
  };
}

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const str = (value: unknown) => typeof value === "string" ? value : null;
const id = (value: unknown) => Number.isSafeInteger(value) ? value as number : null;
const oneOf = <T extends string>(value: unknown, options: readonly T[]) => options.includes(value as T) ? value as T : null;
