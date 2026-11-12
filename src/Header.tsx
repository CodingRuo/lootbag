import type { ImportProgress } from "./useImportQueue.ts";
import { Icon, bytes, number, type LibraryInfo, type Notice, type Setter } from "./ui.tsx";

// Drop hint, top bar with the import action, and the notice/import progress banners.
export default function Header({ library, actionsDisabled, busy, notice, setNotice, dropHover, importActive, importProgress, cancelling, cancelImport, importPackage }: {
  library: LibraryInfo | null;
  actionsDisabled: boolean;
  busy: string | null;
  notice: Notice | null;
  setNotice: Setter<Notice | null>;
  dropHover: boolean;
  importActive: boolean;
  importProgress: ImportProgress | null;
  cancelling: boolean;
  cancelImport: () => Promise<void>;
  importPackage: () => void;
}) {
  return (
    <>
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
    </>
  );
}
