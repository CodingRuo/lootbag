import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import AnnotationEditor from "./AnnotationEditor";
import AudioPreview from "./AudioPreview";
import ImagePreview from "./ImagePreview";
import BlendPreview from "./BlendPreview.tsx";
import { requiresModelBundle } from "./modelResources.ts";
import ModelPreview, { type ModelExportSelection } from "./ModelPreview";
import ImageGroupPreview from "./ImageGroupPreview";
import { assetKey } from "./imageWorkflows";
import VariantEditor from "./VariantEditor";
import { TrashActions } from "./Trash";
import { FolderMembershipEditor, type ProjectFolder } from "./ProjectFolders";
import { nextInspectorTab, type InspectorTab } from "./inspectorTabs";
import type { CatalogQuery } from "./useCatalogQuery.ts";
import { Icon, bytes, type Act, type Asset, type AssetFile, type AssetPackage, type LibraryInfo, type Notice, type Setter } from "./ui.tsx";

type ExportResult =
  | { status: "cancelled" | "replacementRequired" }
  | { status: "exported"; path: string; sizeBytes: number };

// Detail aside for the selected asset: variants, preview/properties/origin tabs and export.
export default function Inspector({ library, query, selected, file, selectedPackage, projectFolders, modelSelection, setModelSelection, setSelectedFileId, actionsDisabled, act, setLibrary, setRevision, setNotice, trashItem, relinkSource }: {
  library: LibraryInfo;
  query: CatalogQuery;
  selected: Asset | null;
  file: AssetFile | null;
  selectedPackage: AssetPackage | undefined;
  projectFolders: ProjectFolder[];
  modelSelection: ModelExportSelection | null;
  setModelSelection: Setter<ModelExportSelection | null>;
  setSelectedFileId: Setter<number | null>;
  actionsDisabled: boolean;
  act: Act;
  setLibrary: Setter<LibraryInfo | null>;
  setRevision: Setter<number>;
  setNotice: Setter<Notice | null>;
  trashItem: (kind: "package" | "file", id: number, name: string) => void;
  relinkSource: (sourcePackage: AssetPackage) => void;
}) {
  const { audioFilterCount, queryLoading, queryError, focusAssetId, focusFileId, setAudioResultsStale } = query;
  const [inspectorTab, setInspectorTab] = useState<InspectorTab>("preview");
  useEffect(() => { setInspectorTab("preview"); }, [assetKey(selected)]);

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

  function saveAssetAnnotations(name: string | undefined, tags: string[]) {
    if (!selected || name === undefined) return;
    const assetId = selected.id;
    void act("Annotationen werden gespeichert …", async () => {
      await invoke("annotate_asset", { assetId, name, tags });
      setRevision((value) => value + 1);
      setNotice({ tone: "success", message: "Asset-Name und Tags gespeichert." });
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
        ? `${chosen.originalFilename} als eigenes Asset abgespalten.`
        : `${chosen.originalFilename} neu zugeordnet.` });
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

  return (
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
              ? "Gefundene Variante laut gespeicherter Analyse."
              : "Andere Variante: erfüllt die Audiofilter eventuell nicht."}</p>}
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
                    <p className="detail-hint">Wähle den verschobenen Originalordner.</p>
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
            <p>{selected.imageGroup ? "Kopiert die Originaldateien in einen neuen Ordner." : file.format === "BLEND" ? "Nur die .blend-Datei, ohne externe Texturen." : file.kind === "model" ? "Kopiert Modell und Ressourcen in einen neuen Ordner." : "Kopiert die Originaldatei unverändert."}</p>
          </div>
        </>
      ) : (
        <div className="detail-empty">
          <span className="sound-icon">
            <Icon name="sound" />
          </span>
          <h2>Ein Asset, alle Details.</h2>
          <p>Wähle ein Asset im Katalog aus.</p>
        </div>
      )}
    </aside>
  );
}
