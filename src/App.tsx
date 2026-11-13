import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import type { ModelExportSelection } from "./ModelPreview";
import { TrashDialog, type TrashEntry } from "./Trash";
import { FolderDialog, folderPath, type ProjectFolder } from "./ProjectFolders";
import { PackageInfoDialog } from "./PackageInfoDialog";
import { savePackageInfo, type PackageInfo } from "./packageInfo.ts";
import type { CatalogSelection } from "./catalogSelection";
import { errorMessage } from "./errorMessage.ts";
import { ZIP_FILTER, native, number, type Asset, type AssetPackage, type LibraryInfo, type Notice } from "./ui.tsx";
import { useCatalogQuery } from "./useCatalogQuery.ts";
import { useAudioAnalysis } from "./useAudioAnalysis.ts";
import { useImportQueue } from "./useImportQueue.ts";
import Header from "./Header";
import Onboarding from "./Onboarding";
import Sidebar from "./Sidebar";
import Catalog from "./Catalog";
import Inspector from "./Inspector";

type RescanSummary = { added: number; changed: number; missing: number; ambiguous: string[] };
type EmptiedTrash = { packages: number; files: number; diskErrors: string[] };


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
  const [showFolderDialog, setShowFolderDialog] = useState(false);
  const [infoPackageId, setInfoPackageId] = useState<number | null>(null);
  const infoPackage = library?.packages.find((item) => item.id === infoPackageId);
  const [revision, setRevision] = useState(0);
  const [traitsPending, setTraitsPending] = useState(false);
  const [selected, setSelected] = useState<Asset | null>(null);
  const [selection, setSelection] = useState<CatalogSelection>({ keys: [], anchor: null });
  const operationRunning = useRef(false);
  const operationGeneration = useRef(0);
  const mounted = useRef(true);
  const currentLibraryRoot = useRef(library?.root);
  currentLibraryRoot.current = library?.root;
  const [showTrash, setShowTrash] = useState(false);
  const trashGeneration = useRef(0);
  const renderedTrashScope = trashGeneration.current;
  const [selectedFileId, setSelectedFileId] = useState<number | null>(null);
  const [modelSelection, setModelSelection] = useState<ModelExportSelection | null>(null);
  const query = useCatalogQuery({ library, revision, refreshCatalog: () => refreshCatalog(), setSelected, setSelection, setSelectedFileId, setModelSelection, setNotice });
  const { setText, packageId, setPackageId, folderId, setFolderId, format, setOffset, setPage, focusAssetId, focusFileId, audioFilterCount, clearFilters } = query;
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
  const audio = useAudioAnalysis({ library, query, revision, actionsDisabled, operationRunning, act, setNotice });
  // A revision bump would discard prepared or in-flight sound work, so background passes defer theirs until it is idle.
  const audioPhase = audio.audioAnalysis?.phase;
  const audioBusy = useRef(false);
  const refreshAfterAudio = useRef(false);
  audioBusy.current = audioPhase === "preparing" || audioPhase === "prepared" || audioPhase === "running";
  function refreshCatalog() {
    if (audioBusy.current) refreshAfterAudio.current = true;
    else setRevision((value) => value + 1);
  }
  useEffect(() => {
    if (audioBusy.current || !refreshAfterAudio.current) return;
    refreshAfterAudio.current = false;
    setRevision((value) => value + 1);
  }, [audioPhase]);
  const imports = useImportQueue({ library, actionsDisabled, mounted, operationRunning, operationGeneration, currentLibraryRoot, setBusy, setNotice, setLibrary, setRevision });

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

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

  // Background model inspection: counts refresh at most once per second, the page once it finishes.
  const catalogKind = useRef(query.kind);
  catalogKind.current = query.kind;
  useEffect(() => {
    if (!native) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    let refreshed = 0;
    listen<{ done: number; remaining: number }>("model-traits-progress", ({ payload }) => {
      if (!active) return;
      setTraitsPending(payload.remaining > 0);
      if (!payload.done || (payload.remaining > 0 && Date.now() - refreshed < 1000)) return;
      refreshed = Date.now();
      invoke<LibraryInfo | null>("get_library")
        .then((info) => { if (active && info && info.root === currentLibraryRoot.current) setLibrary(info); })
        .catch(() => {});
      // Sound results cannot change, and a revision bump would discard a running sound analysis.
      if (payload.remaining === 0 && catalogKind.current !== "sound") setRevision((value) => value + 1);
    }).then((stop) => { if (active) unlisten = stop; else stop(); }).catch(() => {});
    // Duplicate counts, auto-tags and the filter change only once their pass has finished.
    const stops: (() => void)[] = [];
    for (const event of ["file-hashes-progress", "auto-tags-progress"]) {
      // A hashing run may only drop hashes of changed files (`invalidated`), which changes duplicates too.
      listen<{ done: number; remaining: number; invalidated?: number }>(event, ({ payload }) => {
        if (!active || !(payload.done || payload.invalidated) || payload.remaining !== 0) return;
        refreshCatalog();
      }).then((stop) => { if (active) stops.push(stop); else stop(); }).catch(() => {});
    }
    return () => { active = false; unlisten?.(); stops.forEach((stop) => stop()); };
  }, []);

  useEffect(() => { setModelSelection(null); }, [library?.root, file?.id, file?.available, selected?.imageGroup?.workflow, selectedPackage?.externalRoot]);

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

  function useLibrary(info: LibraryInfo) {
    trashGeneration.current++;
    imports.resetImport();
    currentLibraryRoot.current = info.root;
    operationGeneration.current++; operationRunning.current = false; setBusy(null);
    setSelected(null);
    setSelection({ keys: [], anchor: null });
    setSelectedFileId(null);
    setModelSelection(null);
    setShowTrash(false);
    setShowFolderDialog(false);
    setInfoPackageId(null);
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

  function referenceFolder(zip = false) {
    void act("Externe Quelle wird eingelesen …", async () => {
      const source = await open(zip ? { multiple: false, filters: ZIP_FILTER, title: "ZIP-Paket ohne Entpacken einbinden" }
        : { directory: true, multiple: false, title: "Externen Asset-Ordner einbinden" });
      if (!source) return;
      const referenced = await invoke<AssetPackage>(zip ? "reference_zip" : "reference_folder", { source });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setPackageId(referenced.id);
      setFolderId(null);
      clearFilters();
      setText("");
      setOffset(0);
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: `${referenced.name} ${zip ? "als ZIP" : "extern"} eingebunden · ${number.format(referenced.assetCount)} Assets.` });
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
      const source = await open(sourcePackage.zip ? { multiple: false, filters: ZIP_FILTER, title: "Verschobenes ursprüngliches ZIP auswählen" }
        : { directory: true, multiple: false, title: "Verschobenen ursprünglichen Quellordner auswählen" });
      if (!source) return;
      await invoke<AssetPackage>("relink_source", { packageId: sourcePackage.id, source });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: `${sourcePackage.name} neu verknüpft.` });
    });
  }

  function rescanPackage(id: number, name: string) {
    void act("Paket wird neu eingelesen …", async () => {
      const summary = await invoke<RescanSummary>("rescan_package", { packageId: id });
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      const ambiguous = summary.ambiguous.length ? ` · Mehrdeutige Gruppen: ${summary.ambiguous.join(", ")}` : "";
      setNotice({ tone: "success", message: `${name} neu eingelesen · ${number.format(summary.added)} neu, ${number.format(summary.changed)} geändert, ${number.format(summary.missing)} fehlen${ambiguous}.` });
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
      clearFilters(true);
      setOffset(0);
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: `${name} in den Papierkorb verschoben.` });
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

  function editPackageInfo(id: number) {
    setNotice(null);
    setInfoPackageId(id);
  }

  function saveInfo(id: number, form: Record<keyof PackageInfo, string>) {
    void act("Paketinfo wird gespeichert …", async () => {
      await savePackageInfo(invoke, id, form);
      setLibrary(await invoke<LibraryInfo>("get_library"));
      setInfoPackageId(null);
      setNotice({ tone: "success", message: "Paketinfo gespeichert." });
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

  return (
    <div className={`app-shell ${library ? "editor-shell" : ""}`}>
      <Header library={library} actionsDisabled={actionsDisabled} busy={busy} notice={notice} setNotice={setNotice}
        dropHover={imports.dropHover} importActive={imports.importActive} importProgress={imports.importProgress} cancelling={imports.cancelling}
        cancelImport={imports.cancelImport} importPackage={imports.importPackage} />
      {!library ? (
        <Onboarding actionsDisabled={actionsDisabled} createLibrary={createLibrary} openLibrary={openLibrary} />
      ) : (
        <div className="workspace">
          <Sidebar savedSearches={query.savedSearches} searchJson={query.searchJson} applySavedSearch={query.applySavedSearch} deleteSavedSearch={query.deleteSavedSearch} library={library} traitsPending={traitsPending} catalogType={query.workflow ?? query.kind ?? ""} chooseType={query.chooseType} openLibrary={openLibrary} referenceFolder={referenceFolder} packageId={packageId} folderId={folderId} projectFolders={projectFolders} actionsDisabled={actionsDisabled}
            filterPackage={query.filterPackage} filterFolder={query.filterFolder} trashItem={trashItem} editPackageInfo={editPackageInfo} rescanPackage={rescanPackage} refreshSources={refreshSources}
            setNotice={setNotice} setShowFolderDialog={setShowFolderDialog} setShowTrash={setShowTrash} trashGeneration={trashGeneration} />
          <Catalog library={library} query={query} audio={audio} selected={selected} setSelected={setSelected} selection={selection} setSelection={setSelection}
            setSelectedFileId={setSelectedFileId} setModelSelection={setModelSelection} filteredPackage={filteredPackage} filteredFolderPath={filteredFolderPath}
            projectFolders={projectFolders} actionsDisabled={actionsDisabled} act={act} setLibrary={setLibrary} setRevision={setRevision} setNotice={setNotice}
            trashItem={trashItem} relinkSource={relinkSource} importPackage={imports.importPackage} />
          <Inspector library={library} query={query} selected={selected} setSelected={setSelected} file={file} selectedPackage={selectedPackage} projectFolders={projectFolders}
            modelSelection={modelSelection} setModelSelection={setModelSelection} setSelectedFileId={setSelectedFileId} actionsDisabled={actionsDisabled}
            act={act} setLibrary={setLibrary} setRevision={setRevision} refreshCatalog={refreshCatalog} setNotice={setNotice} trashItem={trashItem} relinkSource={relinkSource} editPackageInfo={editPackageInfo} />
        </div>
      )}
      {library && showFolderDialog && (
        <FolderDialog folders={projectFolders} parentId={folderId} disabled={actionsDisabled} notice={notice}
          onCreate={createFolder} onClose={() => setShowFolderDialog(false)} />
      )}
      {infoPackage && (
        <PackageInfoDialog key={infoPackage.id} pkg={infoPackage} disabled={actionsDisabled} notice={notice}
          onSave={(form) => saveInfo(infoPackage.id, form)} onClose={() => setInfoPackageId(null)} />
      )}
      {library && showTrash && (
        <TrashDialog root={library.root} revision={revision} disabled={actionsDisabled} notice={notice}
          onClose={() => closeTrash(renderedTrashScope)} onRestore={restoreTrash} onEmpty={emptyTrash} />
      )}
    </div>
  );
}
