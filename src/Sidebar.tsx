import type { RefObject } from "react";
import { TrashActions } from "./Trash";
import { folderPath, type ProjectFolder } from "./ProjectFolders";
import type { SavedSearch } from "./useCatalogQuery.ts";
import { errorMessage } from "./errorMessage.ts";
import { CATALOG_TYPES, Icon, number, type AssetPackage, type CatalogType, type LibraryInfo, type Notice, type Setter } from "./ui.tsx";

// Library heading, asset type, package and project folder filters, trash, library actions and storage location.
export default function Sidebar({ savedSearches, searchJson, applySavedSearch, deleteSavedSearch, library, traitsPending, catalogType, chooseType, packageId, folderId, projectFolders, actionsDisabled, filterPackage, filterFolder, trashItem, editPackageInfo, rescanPackage, watchPackage, refreshSources, setNotice, setShowFolderDialog, setShowTrash, trashGeneration, openLibrary, referenceFolder }: {
  savedSearches: SavedSearch[];
  searchJson: string;
  applySavedSearch: (saved: SavedSearch) => void;
  deleteSavedSearch: (id: number) => Promise<void>;
  catalogType: CatalogType;
  traitsPending: boolean;
  chooseType: (value: CatalogType) => void;
  library: LibraryInfo;
  packageId: number | null;
  folderId: number | null;
  projectFolders: ProjectFolder[];
  actionsDisabled: boolean;
  filterPackage: (id: number | null) => void;
  filterFolder: (id: number) => void;
  trashItem: (kind: "package" | "file", id: number, name: string) => void;
  editPackageInfo: (id: number) => void;
  rescanPackage: (id: number, name: string) => void;
  watchPackage: (item: AssetPackage) => void;
  refreshSources: () => void;
  setNotice: Setter<Notice | null>;
  setShowFolderDialog: Setter<boolean>;
  setShowTrash: Setter<boolean>;
  trashGeneration: RefObject<number>;
  openLibrary: () => void;
  referenceFolder: (zip?: boolean) => void;
}) {
  const allAssets = catalogType === "" && packageId === null && folderId === null;
  return (
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
        <div className="package-heading">
          <h3>TYPEN</h3>
        </div>
        <nav className="packages asset-types" aria-label="Asset-Typen">
          {CATALOG_TYPES.map((item) => {
            const active = item.value === "" ? allAssets : catalogType === item.value;
            const count = item.value === "" ? library.assetCount : library.kindCounts[item.value];
            const detecting = item.value === "animation" && traitsPending;
            return <button key={item.value || "all"} data-type={item.value} className={`package-item ${active ? "active" : ""}`} aria-pressed={active}
              title={item.value === "animation" ? "Reine Animationen ohne Mesh. Animierte Modelle stehen unter 3D-Modelle." : undefined}
              aria-label={`${item.label} · ${number.format(count)}${detecting ? " · werden erkannt" : ""}`}
              onClick={() => { if (item.value === "") filterPackage(null); chooseType(item.value); }}>
              <Icon name={item.icon} /><span>{item.label}</span><span className="count">{number.format(count)}{detecting && <span title="Animationen werden erkannt"> …</span>}</span>
            </button>;
          })}
        </nav>
        <div className="package-heading">
          <h3>GESPEICHERTE SUCHEN</h3>
          <span>{number.format(savedSearches.length)}</span>
        </div>
        <nav className="packages saved-searches" aria-label="Gespeicherte Suchen">
          {savedSearches.length ? savedSearches.map((item) => (
            <div key={item.id} className="saved-search-row">
              <button className={`package-item ${item.queryJson === searchJson ? "active" : ""}`} aria-pressed={item.queryJson === searchJson}
                title={item.name} onClick={() => applySavedSearch(item)}>
                <Icon name="search" /><span>{item.name}</span>
              </button>
              <button className="icon-button" aria-label={`Gespeicherte Suche ${item.name} löschen`} title="Löschen" disabled={actionsDisabled}
                onClick={() => deleteSavedSearch(item.id).catch((error: unknown) => setNotice({ tone: "error", message: errorMessage(error) }))}>
                <Icon name="trash" />
              </button>
            </div>
          )) : <p className="sidebar-empty">Suche oder Filter setzen und speichern.</p>}
        </nav>
        <div className="package-heading">
          <h3>PAKETE</h3>
          <span>{number.format(library.packages.length)}</span>
        </div>
        <nav className="packages" aria-label="Paketfilter">
          {library.packages.length ? (
            library.packages.map((item) => (
              <TrashActions key={item.id} className="package-row" label={`Paket ${item.name}`} disabled={actionsDisabled}
                onTrash={() => trashItem("package", item.id, item.name)} onEditInfo={() => editPackageInfo(item.id)}
                onRescan={() => rescanPackage(item.id, item.name)}
                onWatch={item.sourceKind === "external" && !item.zip ? () => watchPackage(item) : undefined} watched={item.watched}>
                <button
                  className={`package-item ${packageId === item.id ? "active" : ""}`}
                  aria-pressed={packageId === item.id}
                  aria-label={`${item.name} · ${number.format(item.assetCount)} Assets${item.sourceKind === "external" ? ` · ${item.zip ? "ZIP" : "Externe Quelle"} ${item.available ? "erreichbar" : "offline"}` : ""}`}
                  title={`${item.name}${item.sourceKind === "external" ? ` · Externe Quelle ${item.available ? "erreichbar" : "offline"}` : ""}`}
                  onClick={() => filterPackage(item.id)}
                >
                  <Icon name="folder" />
                  <span>{item.name}</span>
                  <span className="count">
                    {number.format(item.assetCount)}
                  </span>
                  {item.zip && <span className="package-zip-badge" aria-hidden="true">ZIP</span>}
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
          <Icon name="trash" /><span>Papierkorb</span>
        </button>
        <div className="storage">
          <Icon name="library" />
          <div>
            <strong>Speicherort</strong>
            <p title={library.root}>{library.name}</p>
            <div className="storage-actions">
              <button className="text-button" onClick={openLibrary} disabled={actionsDisabled}>Bibliothek öffnen</button>
              <button className="text-button" onClick={() => referenceFolder()} disabled={actionsDisabled}>Externen Ordner einbinden</button>
              <button className="text-button" onClick={() => referenceFolder(true)} disabled={actionsDisabled}
                title="Nutzt das Archiv an Ort und Stelle; Dateien werden erst bei Vorschau oder Export entpackt">ZIP einbinden</button>
            </div>
          </div>
        </div>
      </div>
    </aside>
  );
}
