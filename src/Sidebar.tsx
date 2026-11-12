import type { RefObject } from "react";
import { TrashActions } from "./Trash";
import { folderPath, type ProjectFolder } from "./ProjectFolders";
import { Icon, number, type LibraryInfo, type Notice, type Setter } from "./ui.tsx";

// Library heading, package and project folder filters, trash entry and storage location.
export default function Sidebar({ library, packageId, folderId, projectFolders, actionsDisabled, filterPackage, filterFolder, trashItem, refreshSources, setNotice, setShowFolderDialog, setShowTrash, trashGeneration }: {
  library: LibraryInfo;
  packageId: number | null;
  folderId: number | null;
  projectFolders: ProjectFolder[];
  actionsDisabled: boolean;
  filterPackage: (id: number | null) => void;
  filterFolder: (id: number) => void;
  trashItem: (kind: "package" | "file", id: number, name: string) => void;
  refreshSources: () => void;
  setNotice: Setter<Notice | null>;
  setShowFolderDialog: Setter<boolean>;
  setShowTrash: Setter<boolean>;
  trashGeneration: RefObject<number>;
}) {
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
  );
}
