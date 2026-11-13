import { useEffect, useRef, useState, type RefObject } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { confirm, open } from "@tauri-apps/plugin-dialog";
import { draggingOut } from "./dragOut.ts";
import { errorMessage } from "./errorMessage.ts";
import { ZIP_FILTER, bytes, native, number, type AssetPackage, type LibraryInfo, type Notice, type Setter } from "./ui.tsx";

type ZipEstimate = {
  uncompressedBytes: number;
  requiredBytes: number;
  availableBytes: number;
  entryCount: number;
  hasSpace: boolean;
};
type ImportSource = { path: string; isZip: boolean };
export type ImportProgress = {
  stage: "preparing" | "copying" | "cataloging" | "completed" | "cancelled" | "failed";
  completedBytes: number;
  totalBytes: number;
  completedEntries: number;
  totalEntries: number;
  currentPath: string | null;
};
type ImportOwner = { root: string; generation: number; stopped: boolean; imported: number; skipped: number; cancellationError: string | null };

// Picked or dropped import sources run one after another with cancellable progress.
export function useImportQueue({ library, actionsDisabled, mounted, operationRunning, operationGeneration, currentLibraryRoot, setBusy, setNotice, setLibrary, setRevision }: {
  library: LibraryInfo | null;
  actionsDisabled: boolean;
  mounted: RefObject<boolean>;
  operationRunning: RefObject<boolean>;
  operationGeneration: RefObject<number>;
  currentLibraryRoot: RefObject<string | undefined>;
  setBusy: Setter<string | null>;
  setNotice: Setter<Notice | null>;
  setLibrary: Setter<LibraryInfo | null>;
  setRevision: Setter<number>;
}) {
  const [importProgress, setImportProgress] = useState<ImportProgress | null>(null);
  const [importActive, setImportActive] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [dropHover, setDropHover] = useState(false);
  const dropImport = useRef<(paths: string[]) => void>(() => {});
  const importOwner = useRef<ImportOwner | null>(null);

  useEffect(() => () => {
    const owner = importOwner.current; importOwner.current = null;
    if (owner) abandonImport(owner);
  }, []);

  useEffect(() => {
    if (!native) return;
    let active = true;
    let unlisten: (() => void) | undefined;
    getCurrentWebview().onDragDropEvent((event) => {
      if (!active || draggingOut()) return;
      setDropHover(event.payload.type === "enter" || event.payload.type === "over");
      if (event.payload.type === "drop") dropImport.current(event.payload.paths);
    }).then((stop) => { if (active) unlisten = stop; else stop(); })
      .catch((error: unknown) => { if (active) setNotice({ tone: "error", message: `Datei-Drop nicht verfügbar: ${errorMessage(error)}` }); });
    return () => { active = false; unlisten?.(); };
  }, []);

  /** `zip` undefined: macOS' combined native picker; otherwise the dialog plugin's folder or ZIP picker. */
  function importPackage(zip?: boolean) {
    beginImports("Paket zum Importieren auswählen …", async () => {
      if (zip === undefined) {
        const selected = await invoke<ImportSource | null>("select_import_source");
        return selected ? [selected] : [];
      }
      const path = await open(zip ? { multiple: false, filters: ZIP_FILTER, title: "ZIP-Paket importieren" }
        : { directory: true, multiple: false, title: "Ordner importieren" });
      return path ? [{ path, isZip: zip }] : [];
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

  function resetImport() {
    const owner = importOwner.current; importOwner.current = null;
    if (owner) abandonImport(owner);
    setImportProgress(null); setImportActive(false); setCancelling(false);
  }

  return { importProgress, importActive, cancelling, dropHover, importPackage, cancelImport, resetImport };
}
