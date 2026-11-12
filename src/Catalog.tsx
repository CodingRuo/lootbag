import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";
import AnnotationEditor from "./AnnotationEditor";
import { ImageThumbnail } from "./ImagePreview";
import type { ModelExportSelection } from "./ModelPreview";
import ModelThumbnail, { createCatalogModelThumbnails } from "./ModelThumbnail";
import { assetKey } from "./imageWorkflows";
import { TrashActions } from "./Trash";
import { folderPath, type ProjectFolder } from "./ProjectFolders";
import { catalogSelection, nextCatalogIndex, type CatalogSelection, type SelectionMode } from "./catalogSelection";
import type { CatalogQuery } from "./useCatalogQuery.ts";
import type { useAudioAnalysis } from "./useAudioAnalysis.ts";
import { EMPTY_AUDIO_FILTERS, Icon, PAGE_SIZE, bytes, number, type Act, type Asset, type AssetKind, type AssetPackage, type AudioFilters, type LibraryInfo, type Notice, type Setter } from "./ui.tsx";

type BulkAction = { kind: "addTags"; tags: string[] } | { kind: "addToFolder"; folderId: number } | { kind: "trash" };
type BulkResult = { assetCount: number; fileCount: number };

// Larger models are only parsed on demand in the inspector; BLEND thumbnails have their own 64 MiB cap.
const MAX_MODEL_THUMBNAIL_BYTES = 32 * 1024 * 1024;

// Catalog column: search, category/filter/view tools, audio analysis, bulk actions, results and paging.
export default function Catalog({ library, query, audio, selected, setSelected, selection, setSelection, setSelectedFileId, setModelSelection, filteredPackage, filteredFolderPath, projectFolders, actionsDisabled, act, setLibrary, setRevision, setNotice, trashItem, relinkSource, importPackage }: {
  library: LibraryInfo;
  query: CatalogQuery;
  audio: ReturnType<typeof useAudioAnalysis>;
  selected: Asset | null;
  setSelected: Setter<Asset | null>;
  selection: CatalogSelection;
  setSelection: Setter<CatalogSelection>;
  setSelectedFileId: Setter<number | null>;
  setModelSelection: Setter<ModelExportSelection | null>;
  filteredPackage: AssetPackage | undefined;
  filteredFolderPath: string | null;
  projectFolders: ProjectFolder[];
  actionsDisabled: boolean;
  act: Act;
  setLibrary: Setter<LibraryInfo | null>;
  setRevision: Setter<number>;
  setNotice: Setter<Notice | null>;
  trashItem: (kind: "package" | "file", id: number, name: string) => void;
  relinkSource: (sourcePackage: AssetPackage) => void;
  importPackage: () => void;
}) {
  const { text, setText, packageId, folderId, originalFolder, setOriginalFolder, format, setFormat, kind, setKind, workflow, setWorkflow, tag, setTag,
    audioFilters, setAudioFilters, offset, setOffset, filters, page, queryLoading, queryError, audioFilterCount, audioFilterError,
    audioResultsStale, setAudioResultsStale, changeAudioFilters, clearFilters, filterPackage } = query;
  const { audioAnalysis, prepareAudioAnalysis, startAudioAnalysis, stopAudioAnalysis, discardAudioAnalysis } = audio;
  const [filtersOpen, setFiltersOpen] = useState(false);
  const [catalogView, setCatalogView] = useState<"cards" | "list">("cards");
  const [modelThumbnails] = useState(createCatalogModelThumbnails);
  const thumbnailScope = library?.root ?? "";
  const [bulkTags, setBulkTags] = useState("");
  const [bulkFolderId, setBulkFolderId] = useState<number | null>(null);
  const activeFilterCount = [originalFolder, format, tag].filter((value) => value !== null).length + audioFilterCount;
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

  useEffect(() => { modelThumbnails.reset(thumbnailScope); }, [modelThumbnails, thumbnailScope]);
  useEffect(() => () => modelThumbnails.dispose(), [modelThumbnails]);

  function refreshAudioResults() {
    setAudioResultsStale(false);
    setSelected(null);
    setSelectedFileId(null);
    setModelSelection(null);
    setRevision((value) => value + 1);
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

  return (
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
  );
}
