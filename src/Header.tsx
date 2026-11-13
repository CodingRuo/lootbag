import type { ImportProgress } from "./useImportQueue.ts";
import { Icon, bytes, mac, number, type LibraryInfo, type Notice, type Setter } from "./ui.tsx";

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
  importPackage: (zip?: boolean) => void;
}) {
  return (
    <>
      {dropHover && <div className="native-drop-hint" role="status">{library && !actionsDisabled ? "Ordner, ZIP- oder Unity-Pakete hier zum Importieren ablegen" : library ? "Bitte den laufenden Vorgang abwarten" : "Bitte zuerst eine Bibliothek öffnen"}</div>}
      <header className="topbar">
        <div className="brand">
          <svg className="brand-mark" viewBox="0 0 64 64" aria-hidden="true">
            <rect width="64" height="64" rx="14" fill="#151b2a" />
            <path d="M23 22c-2-4-2-8 1-10 2 2 3 3 4 2 1-2 3-3 4-3s3 1 4 3c1 1 2 0 4-2 3 2 3 6 1 10z" fill="#e6b456" />
            <path d="M25 24h14c8 4 13 11 13 20 0 8-7 12-20 12S12 52 12 44c0-9 5-16 13-20z" fill="#e6b456" />
            <rect x="22" y="20.5" width="20" height="4.5" rx="2.25" fill="#a8711c" />
            <path d="M40 23c3 1 4 4 3.5 7.5" fill="none" stroke="#a8711c" strokeWidth="2.5" strokeLinecap="round" />
            <circle cx="43.5" cy="31.5" r="2.2" fill="#a8711c" />
            <g fill="#151b2a">
              <rect x="24" y="34" width="7" height="7" rx="1.6" />
              <rect x="33" y="34" width="7" height="7" rx="1.6" />
              <rect x="24" y="43" width="7" height="7" rx="1.6" />
              <rect x="33" y="43" width="7" height="7" rx="1.6" fill="#e6b456" stroke="#151b2a" strokeWidth="2" />
            </g>
          </svg>
          <div>
            <strong>Lootbag</strong>
            <span>Deine Assets. Dein nächstes Spiel.</span>
          </div>
        </div>
        {library ? (
          <div className="topbar-actions">
            {mac ? (
              <button
                className="button primary"
                onClick={() => importPackage()}
                disabled={actionsDisabled}
                title="Ordner, ZIP-Datei oder Unity-Paket importieren"
              >
                <Icon name="plus" />
                Paket importieren
              </button>
            ) : (
              <>
                <button className="button primary" onClick={() => importPackage(false)} disabled={actionsDisabled}>
                  <Icon name="plus" />
                  Ordner importieren
                </button>
                <button className="button primary" onClick={() => importPackage(true)} disabled={actionsDisabled}>
                  <Icon name="plus" />
                  ZIP/Unity importieren
                </button>
              </>
            )}
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
